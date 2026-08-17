use nextjson::{DecodeSlot, Error, NsonDeserialize, Value, Write};

#[test]
fn top_level_rejects_trailing_values_and_garbage() {
    for input in ["null true", "1 2", "{}[]", r#""ok" trailing"#] {
        let result = nextjson::from_str::<Value>(input);
        assert!(result.is_err(), "accepted trailing input: {input:?}");
    }

    assert_eq!(nextjson::from_str::<u32>("42 \n\t").unwrap(), 42);
}

#[test]
fn rejects_non_standard_trailing_commas() {
    for input in ["[1,]", "[1, ]", r#"{"x":1,}"#, r#"{"x":1, }"#] {
        assert!(
            nextjson::from_str::<Value>(input).is_err(),
            "accepted trailing comma: {input:?}"
        );
    }
}

#[derive(Debug)]
struct BrokenDecode;

impl<'de> NsonDeserialize<'de> for BrokenDecode {
    fn nextdecode_into<D: nextjson::FormatDecoder<'de>>(
        decoder: &mut D,
        _out: &mut DecodeSlot<Self>,
    ) -> core::result::Result<(), D::Error> {
        decoder.unit()
    }
}

#[test]
fn invalid_safe_decode_impl_returns_error_instead_of_causing_ub() {
    let error = nextjson::from_str::<BrokenDecode>("null").unwrap_err();
    assert!(error
        .to_string()
        .contains("returned success without writing a value"));
}

#[test]
fn direct_decoder_can_validate_its_end() {
    let mut decoder = nextjson::Decoder::new(b"7 false");
    assert_eq!(u8::nextdecode(&mut decoder).unwrap(), 7);
    assert!(decoder.end().is_err());
}

#[test]
fn unescaped_strings_borrow_the_original_input() {
    let input = r#""zero-copy""#;
    let value: &str = nextjson::from_str(input).unwrap();

    assert_eq!(value, "zero-copy");
    assert_eq!(value.as_ptr(), input.as_ptr().wrapping_add(1));
}

#[test]
fn string_escaping_agrees_across_swar_chunk_boundaries() {
    // The raw-copy fast path scans in 8-byte SWAR chunks; every length around
    // the chunk boundary plus every escapable character must round-trip.
    let probe = [
        '"', '\\', '\u{0}', '\u{1f}', ' ', 'a', '\u{7f}', '\u{80}', 'é', '中', '\u{2028}',
    ];
    for len in 0..20usize {
        let mut base = String::new();
        for _ in 0..len {
            base.push('a');
        }
        for &c in &probe {
            let mut s = base.clone();
            s.push(c);
            let encoded = nextjson::nextencode(&s).unwrap();
            let back: String = nextjson::nextdecode(&encoded).unwrap();
            assert_eq!(back, s, "length {len} char {c:?}");
        }
    }
}

struct FailingSink {
    writes: usize,
    flushes: usize,
}

impl Write for FailingSink {
    fn write_all(&mut self, _buf: &[u8]) -> nextjson::Result<()> {
        self.writes += 1;
        Err(Error::custom("injected write failure"))
    }

    fn flush(&mut self) -> nextjson::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn automatic_buffer_flush_propagates_write_failure() {
    let sink = FailingSink {
        writes: 0,
        flushes: 0,
    };
    let payload = "x".repeat(9_000);
    let error = nextjson::to_writer(sink, &payload).unwrap_err();

    assert!(error.is_custom());
    assert!(error.to_string().contains("injected write failure"));
}

#[derive(Default)]
struct FlushProbe {
    bytes: Vec<u8>,
    flushed: bool,
}

impl Write for &mut FlushProbe {
    fn write_all(&mut self, buf: &[u8]) -> nextjson::Result<()> {
        self.bytes.extend_from_slice(buf);
        Ok(())
    }

    fn flush(&mut self) -> nextjson::Result<()> {
        self.flushed = true;
        Ok(())
    }
}

#[test]
fn finish_flushes_the_underlying_sink() {
    let mut sink = FlushProbe::default();
    nextjson::to_writer(&mut sink, &123_u32).unwrap();

    assert_eq!(sink.bytes, b"123");
    assert!(sink.flushed);
}

#[test]
fn all_rust_integer_extremes_round_trip_losslessly() {
    for value in [i128::MIN, -1, 0, i64::MAX as i128, i128::MAX] {
        let json = nextjson::to_string(&value).unwrap();
        assert_eq!(nextjson::from_str::<i128>(&json).unwrap(), value);
    }

    for value in [0, u64::MAX as u128, u128::MAX] {
        let json = nextjson::to_string(&value).unwrap();
        assert_eq!(nextjson::from_str::<u128>(&json).unwrap(), value);
        assert_eq!(
            nextjson::from_str::<Value>(&json).unwrap().as_u128(),
            Some(value)
        );
    }
}

#[test]
fn f32_decode_rejects_finite_f64_that_overflows() {
    assert!(nextjson::from_str::<f32>("1e100").is_err());
    assert_eq!(nextjson::from_str::<f32>("3.5").unwrap(), 3.5);
}

thread_local! {
    static DROPS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

struct DropProbe;

impl Drop for DropProbe {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + 1));
    }
}

impl<'de> NsonDeserialize<'de> for DropProbe {
    fn nextdecode_into<D: nextjson::FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> core::result::Result<(), D::Error> {
        let _: &str = NsonDeserialize::nextdecode(decoder)?;
        out.write(DropProbe);
        Ok(())
    }
}

#[derive(NsonDeserialize)]
#[allow(dead_code)]
struct ResourceOwner {
    resource: DropProbe,
    count: u32,
}

fn drops() -> usize {
    DROPS.with(|d| d.get())
}
fn reset_drops() {
    DROPS.with(|d| d.set(0));
}

#[test]
fn derived_struct_drops_initialized_fields_after_later_error() {
    reset_drops();
    let result = nextjson::from_str::<ResourceOwner>(r#"{"resource":"open","count":"bad"}"#);

    assert!(result.is_err());
    assert_eq!(drops(), 1);
}

#[test]
fn duplicate_field_error_drops_the_initialized_value() {
    reset_drops();
    let error = match nextjson::from_str::<ResourceOwner>(
        r#"{"resource":"first","resource":"second","count":1}"#,
    ) {
        Ok(_) => panic!("duplicate field must fail"),
        Err(error) => error,
    };

    assert_eq!(drops(), 1);
    assert_eq!(error.classification(), "duplicate field");
}

#[test]
fn map_key_replay_handles_escapes_and_rejects_prefix_parses() {
    let input = r#"{"quote\"slash\\":7}"#;
    let map: std::collections::BTreeMap<String, u32> = nextjson::from_str(input).unwrap();
    assert_eq!(map.get("quote\"slash\\"), Some(&7));

    let malformed_numeric_key =
        nextjson::from_str::<std::collections::BTreeMap<u32, u32>>(r#"{"1x":7}"#);
    assert!(malformed_numeric_key.is_err());
}

#[test]
fn canonical_nextencode_nextdecode_entry_points_round_trip() {
    let expected = nextjson::json!({
        "name": "next-api",
        "values": [1, 2, 3],
        "enabled": true
    });
    let bytes = nextjson::nextencode(&expected).unwrap();
    let actual: nextjson::Value = nextjson::nextdecode(&bytes).unwrap();
    assert_eq!(actual, expected);
}

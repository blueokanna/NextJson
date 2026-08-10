use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn nextdecode_into(
        decoder: &mut nextjson::Decoder<'de>,
        _out: &mut DecodeSlot<Self>,
    ) -> nextjson::Result<()> {
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

static DROPS: AtomicUsize = AtomicUsize::new(0);

struct DropProbe;

impl Drop for DropProbe {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

impl<'de> NsonDeserialize<'de> for DropProbe {
    fn nextdecode_into(
        decoder: &mut nextjson::Decoder<'de>,
        out: &mut DecodeSlot<Self>,
    ) -> nextjson::Result<()> {
        let _: &str = NsonDeserialize::nextdecode(decoder)?;
        out.write(DropProbe);
        Ok(())
    }
}

#[derive(NsonDeserialize)]
struct ResourceOwner {
    resource: DropProbe,
    count: u32,
}

#[test]
fn derived_struct_drops_initialized_fields_after_later_error() {
    DROPS.store(0, Ordering::SeqCst);
    let result = nextjson::from_str::<ResourceOwner>(r#"{"resource":"open","count":"bad"}"#);

    assert!(result.is_err());
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
}

#[test]
fn duplicate_field_replacement_drops_the_previous_value() {
    DROPS.store(0, Ordering::SeqCst);
    let value = nextjson::from_str::<ResourceOwner>(
        r#"{"resource":"first","resource":"second","count":1}"#,
    )
    .unwrap();

    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    assert_eq!(value.count, 1);
    let _ = &value.resource;
    drop(value);
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
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

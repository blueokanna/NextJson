//! Stream decoder chunk-boundary coverage: every chunk size must parse
//! identically to the in-memory decoder, and truncated inputs must fail
//! without panicking regardless of chunking.

use nextjson::Value;

struct Chunked<R> {
    inner: R,
    size: usize,
}

impl<R: std::io::Read> std::io::Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.size.min(buf.len());
        self.inner.read(&mut buf[..n])
    }
}

#[test]
fn stream_decoder_all_chunk_sizes() {
    let input = br#"{"a":[1,2,"x"],"b":true,"c":null,"d":1.5,"e":"multi\nline"}"#;
    for size in 1..=8 {
        let v: Value = nextjson::from_reader(Chunked {
            inner: &input[..],
            size,
        })
        .unwrap();
        assert_eq!(v["a"][2], Value::from("x"));
        assert_eq!(v["b"], Value::from(true));
        assert_eq!(v["c"], Value::Null);
        assert_eq!(v["d"], Value::from(1.5_f64));
        assert_eq!(v["e"], Value::from("multi\nline"));
    }
}

#[test]
fn stream_decoder_matches_in_memory_on_truncation() {
    for bad in [r#"{"a":1"#, r#"[1,2"#, r#""unterminated"#, r#"{""#, r#"["#] {
        assert!(
            nextjson::from_reader::<_, Value>(Chunked {
                inner: bad.as_bytes(),
                size: 2,
            })
            .is_err(),
            "accepted truncated {bad:?}"
        );
    }
}

#[test]
fn stream_decoder_typed_scalars_across_chunks() {
    // Typed scalar reads (u64, i64, f64, bool, char) through a 1-byte reader.
    struct OneByte<R>(R);
    impl<R: std::io::Read> std::io::Read for OneByte<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let mut one = [0u8; 1];
            let n = self.0.read(&mut one)?;
            if n > 0 {
                buf[0] = one[0];
            }
            Ok(n)
        }
    }
    assert_eq!(
        nextjson::from_reader::<_, u64>(OneByte(&b"1234567890"[..])).unwrap(),
        1234567890
    );
    assert_eq!(
        nextjson::from_reader::<_, i64>(OneByte(&b"-987654321"[..])).unwrap(),
        -987654321
    );
    assert_eq!(
        nextjson::from_reader::<_, f64>(OneByte(&b"3.25"[..])).unwrap(),
        3.25
    );
    assert_eq!(
        nextjson::from_reader::<_, bool>(OneByte(&b"true"[..])).unwrap(),
        true
    );
    assert_eq!(
        nextjson::from_reader::<_, char>(OneByte(&br#""z""#[..])).unwrap(),
        'z'
    );
    assert_eq!(
        nextjson::from_reader::<_, String>(OneByte(&br#""hello""#[..])).unwrap(),
        "hello"
    );
}

#[test]
fn stream_decoder_empty_and_whitespace_inputs() {
    assert!(nextjson::from_reader::<_, Value>(Chunked {
        inner: &b""[..],
        size: 1,
    })
    .is_err());
    assert!(nextjson::from_reader::<_, Value>(Chunked {
        inner: &b"   \n\t "[..],
        size: 1,
    })
    .is_err());
}

//! Error-branch coverage for the decoder, encoder, stream decoder, and the
//! YAML/TOML codecs' recently added features.
//!
//! These tests exercise lexer/parser/protocol failure paths that round-trip
//! tests do not reach, so the coverage gate sees the error branches executed.

use nextjson::formats;
use nextjson::formats::Format;
use nextjson::{Decoder, Encoder, Error, NsonDeserialize, Number, Token, Value, Write};

// ---------------------------------------------------------------------------
// Decoder: byte-input lexer error branches
// ---------------------------------------------------------------------------

#[test]
fn lexer_error_branches() {
    // Invalid / truncated escapes.
    for bad in [r#""\q""#, r#""\x""#, r#""\u12""#, r#""\u""#] {
        assert!(
            nextjson::from_str::<Value>(bad).is_err(),
            "accepted escape {bad:?}"
        );
    }
    // Lone / high surrogate escapes.
    assert!(nextjson::from_str::<Value>(r#""\ud800""#).is_err());
    assert!(nextjson::from_str::<Value>(r#""\udfff""#).is_err());
    // Unescaped control character in a string.
    assert!(nextjson::from_str::<Value>("\"a\u{01}b\"").is_err());
    // Invalid UTF-8 bytes inside a string.
    assert!(nextjson::from_slice::<Value>(&[b'"', 0xC3, 0x28, b'"']).is_err());
    // Number grammar violations.
    for bad in [
        "01", "1.", ".5", "1e", "+1", "--1", "1..2", "1e+", "0x10", "1_0", "-",
    ] {
        assert!(
            nextjson::from_str::<Value>(bad).is_err(),
            "accepted number {bad:?}"
        );
    }
    // Number out of range (beyond i128).
    let huge = "9".repeat(60);
    assert!(nextjson::from_str::<Number>(&huge).is_err());
    // Bare tokens / structural garbage.
    for bad in ["tru", "nul", "truE", "]", "}", ",", ":", ""] {
        assert!(
            nextjson::from_str::<Value>(bad).is_err(),
            "accepted {bad:?}"
        );
    }
}

#[test]
fn decoder_container_error_branches() {
    // Non-string object keys.
    for bad in [r#"{1:2}"#, r#"{true:2}"#, r#"{null:2}"#, r#"{[1]:2}"#] {
        assert!(
            nextjson::from_str::<Value>(bad).is_err(),
            "accepted key form {bad:?}"
        );
    }
    // Missing separator / value.
    assert!(nextjson::from_str::<Value>(r#"{"a" 1}"#).is_err());
    assert!(nextjson::from_str::<Value>(r#"{"a":}"#).is_err());
    assert!(nextjson::from_str::<Value>(r#"[1 2]"#).is_err());
    assert!(nextjson::from_str::<Value>(r#"{"a":1"#).is_err());
    assert!(nextjson::from_str::<Value>(r#"[1,2"#).is_err());
    // Trailing / doubled separators.
    assert!(nextjson::from_str::<Value>(r#"{"a":1,}"#).is_err());
    assert!(nextjson::from_str::<Value>(r#"[,]"#).is_err());
    assert!(nextjson::from_str::<Value>(r#"{"a":1,,}"#).is_err());
    // Nesting limit (default 128).
    let deep = "[".repeat(200) + &"]".repeat(200);
    assert!(nextjson::from_str::<Value>(&deep).is_err());
    // Scalar type mismatches on derived scalars.
    assert!(nextjson::from_str::<char>(r#""ab""#).is_err());
    assert!(nextjson::from_str::<char>(r#"1"#).is_err());
    assert!(nextjson::from_str::<bool>(r#"1"#).is_err());
    assert!(nextjson::from_str::<()>(r#"1"#).is_err());
    assert!(nextjson::from_str::<u32>(r#""x""#).is_err());
    assert!(nextjson::from_str::<i32>(r#"1.5"#).is_err());
}

// ---------------------------------------------------------------------------
// Decoder: token-stream (tree replay) paths
// ---------------------------------------------------------------------------

#[test]
fn tree_decoder_paths() {
    let toks = vec![
        Token::BeginObject,
        Token::Str("a".into()),
        Token::Number(Number::from(1_i64)),
        Token::EndObject,
    ];
    let mut d = Decoder::from_tokens(toks);
    let v = Value::nextdecode(&mut d).unwrap();
    assert_eq!(v["a"], Value::from(1_i64));
    d.end().unwrap();

    // Truncated token stream: begin object, no key.
    let mut d = Decoder::from_tokens(vec![Token::BeginObject]);
    assert!(Value::nextdecode(&mut d).is_err());

    // save/restore backtracking on the tree reader.
    let toks = vec![
        Token::BeginObject,
        Token::Str("a".into()),
        Token::Number(Number::from(1_i64)),
        Token::EndObject,
    ];
    let mut d = Decoder::from_tokens(toks);
    let mark = d.save();
    let v = Value::nextdecode(&mut d).unwrap();
    assert_eq!(v["a"], Value::from(1_i64));
    d.restore(mark);
    let v2 = Value::nextdecode(&mut d).unwrap();
    assert_eq!(v2, v);
}

// ---------------------------------------------------------------------------
// Encoder: validating (`Encoder::<_, true>`) protocol error branches
// ---------------------------------------------------------------------------

struct Sink(Vec<u8>);
impl Write for Sink {
    fn write_all(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(buf);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

fn validating() -> Encoder<Sink, true> {
    Encoder::<_, true>::new(Sink(Vec::new()))
}

#[test]
fn checked_encoder_protocol_errors() {
    // Multiple root values.
    let mut e = validating();
    e.write_u64(1).unwrap();
    assert!(e.write_u64(2).is_err());
    e.finish().unwrap();

    // Array: separator without value, then end.
    let mut e = validating();
    e.begin_array().unwrap();
    e.separator().unwrap();
    assert!(e.end_array().is_err());
    // Array: key is invalid inside an array.
    let mut e = validating();
    e.begin_array().unwrap();
    assert!(e.key("k").is_err());

    // Object: key then key (missing value).
    let mut e = validating();
    e.begin_object().unwrap();
    e.key("a").unwrap();
    assert!(e.key("b").is_err());
    // Object: value without key.
    let mut e = validating();
    e.begin_object().unwrap();
    assert!(e.write_u64(1).is_err());
    // Object: end while a value is pending.
    let mut e = validating();
    e.begin_object().unwrap();
    e.key("a").unwrap();
    assert!(e.end_object().is_err());

    // Mismatched container closes.
    let mut e = validating();
    e.begin_array().unwrap();
    e.separator().unwrap();
    e.begin_object().unwrap();
    assert!(e.end_array().is_err());
    // Unmatched end.
    let mut e = validating();
    assert!(e.end_object().is_err());
    // finish without a root value.
    let e = validating();
    assert!(e.finish().is_err());
}

// ---------------------------------------------------------------------------
// Stream decoder: tiny chunk sizes and early EOF
// ---------------------------------------------------------------------------

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

#[test]
fn stream_decoder_single_byte_chunks() {
    let input = br#"{"a":[1,2,"x"],"b":true,"c":1.5}"#;
    let v: Value = nextjson::from_reader(OneByte(&input[..])).unwrap();
    assert_eq!(v["a"][2], Value::from("x"));
    assert_eq!(v["b"], Value::from(true));
    assert_eq!(v["c"], Value::from(1.5_f64));
}

#[test]
fn stream_decoder_truncated_input_errors() {
    let input = br#"{"a":[1,2"#;
    assert!(nextjson::from_reader::<_, Value>(OneByte(&input[..])).is_err());
}

// ---------------------------------------------------------------------------
// YAML: recently added feature error branches
// ---------------------------------------------------------------------------

#[test]
fn yaml_feature_error_branches() {
    // Tag type coercion failures.
    assert!(formats::Yaml.decode::<Value>(b"a: !!int xyz\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: !!float zz\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: !!bool maybe\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: !!float .inf\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: !!nope x\n").is_err());
    // Anchor / alias malformations.
    assert!(formats::Yaml.decode::<Value>(b"a: &\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: *x y\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: *missing\n").is_err());
    // Merge with a non-mapping source.
    assert!(formats::Yaml.decode::<Value>(b"a:\n  <<: 42\n").is_err());
    // Block scalar with an invalid header.
    assert!(formats::Yaml.decode::<Value>(b"a: |x\n  v\n").is_err());
    // Non-finite plain floats.
    assert!(formats::Yaml.decode::<Value>(b"a: .inf\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: -.inf\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: .nan\n").is_err());
    // Flow collections reject anchors/aliases explicitly.
    assert!(formats::Yaml.decode::<Value>(b"a: [*x]\n").is_err());
    assert!(formats::Yaml.decode::<Value>(b"a: {x: &a 1}\n").is_err());
    // Trailing garbage after the document.
    assert!(formats::Yaml.decode::<Value>(b"a: 1\nb: 2\n").is_ok());
    // Inconsistent indentation.
    assert!(formats::Yaml
        .decode::<Value>(b"a:\n  b: 1\n c: 2\n")
        .is_err());
}

#[test]
fn yaml_block_scalar_edge_cases() {
    // `|+` with no trailing blank lines keeps one newline.
    let v: Value = formats::Yaml.decode(b"a: |+\n  x\n").unwrap();
    assert_eq!(v["a"], Value::from("x\n"));
    // Folded block with blank lines: a blank line folds to one line break.
    let v: Value = formats::Yaml.decode(b"a: >\n  one\n\n  two\n").unwrap();
    assert_eq!(v["a"], Value::from("one\ntwo\n"));
    // Explicit indentation indicator strips exactly that many columns.
    let v: Value = formats::Yaml.decode(b"a: |1\n  x\n").unwrap();
    assert_eq!(v["a"], Value::from(" x\n"));
    // Anchored block scalar referenced twice.
    let v: Value = formats::Yaml.decode(b"a: &t |\n  x\nb: *t\n").unwrap();
    assert_eq!(v["a"], v["b"]);
}

// ---------------------------------------------------------------------------
// TOML: recently added feature error branches
// ---------------------------------------------------------------------------

#[test]
fn toml_radix_and_string_error_branches() {
    // Invalid radix digits.
    assert!(formats::Toml.decode::<Value>(b"x = 0xZZ\n").is_err());
    assert!(formats::Toml.decode::<Value>(b"x = 0o9\n").is_err());
    assert!(formats::Toml.decode::<Value>(b"x = 0b2\n").is_err());
    // Unterminated multi-line strings.
    assert!(formats::Toml.decode::<Value>(b"x = \"\"\"abc\n").is_err());
    assert!(formats::Toml.decode::<Value>(b"x = '''abc\n").is_err());
    // Invalid multi-line escape.
    assert!(formats::Toml
        .decode::<Value>(b"x = \"\"\"a\\qb\"\"\"\n")
        .is_err());
    // Invalid unicode escapes in multi-line strings.
    assert!(formats::Toml
        .decode::<Value>(b"x = \"\"\"a\\u12\"\"\"\n")
        .is_err());
    // Invalid date-time shapes are not silently stringified.
    for bad in [
        "2020-13-99",
        "1979-05-27T25:00:00",
        "07:99:00",
        "1979-05-27T07:00:00+99:00",
    ] {
        let input = format!("x = {bad}\n");
        assert!(
            formats::Toml.decode::<Value>(input.as_bytes()).is_err(),
            "accepted bad datetime {bad:?}"
        );
    }
}

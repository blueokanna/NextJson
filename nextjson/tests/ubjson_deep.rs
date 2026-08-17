use nextjson::formats::{self, Format};
use nextjson::map::Map;
use nextjson::Value;

fn deep_value(depth: usize) -> Value {
    let mut v = Value::from(1_u64);
    for _ in 0..depth {
        v = Value::Array(vec![v]);
    }
    v
}

fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut map: Map = Map::new();
    for (k, v) in pairs {
        map.insert((*k).to_string(), v.clone());
    }
    Value::Object(map)
}

#[test]
fn ubjson_deep_roundtrip() {
    for depth in [1usize, 2, 4, 8, 16, 23, 24, 25, 40, 60] {
        let value = deep_value(depth);
        let bytes = nextjson::formats::Ubjson.encode(&value).unwrap();
        let back: Value = nextjson::formats::Ubjson.decode(&bytes).unwrap();
        assert_eq!(back, value, "depth={depth}");
    }
}

#[test]
fn ubjson_deep_depth_limit() {
    // The encoder is trusted-path (recursion is data-driven, not attacker
    // controlled); the *decoder* must bound nesting so hostile input cannot
    // overflow the stack. 129+ nested arrays are rejected.
    let bytes = [&[b'['; 200][..], &[b'U'; 200][..], &[b']'; 200][..]].concat();
    let res: Result<Value, _> = nextjson::formats::Ubjson.decode(&bytes);
    assert!(res.is_err(), "deeply nested input must be rejected");
    // ...but just below the limit round-trips.
    let value = deep_value(120);
    let bytes = nextjson::formats::Ubjson.encode(&value).unwrap();
    let back: Value = nextjson::formats::Ubjson.decode(&bytes).unwrap();
    assert_eq!(back, value);
}

fn roundtrip<F: Format>(name: &str, value: &Value, format: F) {
    let bytes = format.encode(value).unwrap();
    let back: Value = format.decode(&bytes).unwrap();
    assert_eq!(back, *value, "format={name}");
}

#[test]
fn all_binary_deep_roundtrip() {
    let value = deep_value(24);
    roundtrip("msgpack", &value, formats::MsgPack);
    roundtrip("cbor", &value, formats::Cbor);
    roundtrip("ubjson", &value, formats::Ubjson);
    roundtrip("smile", &value, formats::Smile);
    roundtrip("bson", &value, formats::Bson);
    roundtrip("pickle", &value, formats::Pickle);
    roundtrip("bencode", &value, formats::Bencode);
}

#[test]
fn all_text_deep_roundtrip() {
    let value = deep_value(24);
    roundtrip("json", &value, formats::Json);
    roundtrip("json5", &value, formats::Json5);
    roundtrip("hjson", &value, formats::Hjson);
    roundtrip("yaml", &value, formats::Yaml);
    roundtrip("ron", &value, formats::Ron);
    roundtrip("sexpr", &value, formats::Sexpr);
    roundtrip("edn", &value, formats::Edn);
}

/// NDJSON deliberately cannot round-trip a top-level array: the top-level
/// array IS the record stream (one element per line, no enclosing brackets),
/// so a `Value::Array` root is spread across lines. A nested (non-root)
/// array round-trips fine.
#[test]
fn ndjson_deep_nested_roundtrip() {
    // 24 layers, but wrapped in an object so the root is not an array.
    let mut inner = Value::from(1_u64);
    for _ in 0..24 {
        inner = Value::Array(vec![inner]);
    }
    let value = obj(&[("deep", inner)]);
    let bytes = formats::Ndjson.encode(&value).unwrap();
    let text = core::str::from_utf8(&bytes).unwrap();
    assert_eq!(text.lines().count(), 1, "single root value = one line");
    let back: Value = formats::Ndjson.decode(&bytes).unwrap();
    assert_eq!(back, value);
}

#[test]
fn ndjson_root_array_is_stream() {
    // A root array is a stream: encode spreads elements across lines and
    // decoding as a Vec collects them back. A single `Value` decode reads
    // the first line and rejects any further non-empty lines (documented
    // NDJSON semantics: a top-level array has no "array value" form).
    let value = Value::Array(vec![
        obj(&[("a", Value::from(1_u64))]),
        obj(&[("b", Value::from(2_u64))]),
    ]);
    let bytes = formats::Ndjson.encode(&value).unwrap();
    let text = core::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with('{'));
    let back: Vec<Value> = formats::Ndjson.decode(&bytes).unwrap();
    assert_eq!(back.len(), 2);
    assert!(formats::Ndjson.decode::<Value>(&bytes).is_err());
}

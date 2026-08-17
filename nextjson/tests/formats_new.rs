//! Tests for the Phase 22 format additions: UBJSON, SMILE, NDJSON, INI, EDN.

use nextjson::formats::{self, Format};
use nextjson::Value;
use nextjson::{NsonDeserialize, NsonSerialize};

#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Record {
    id: u64,
    active: bool,
    score: f64,
    name: String,
    tags: Vec<String>,
    samples: Vec<i64>,
}

fn record() -> Record {
    Record {
        id: 7,
        active: true,
        score: 3.25,
        name: "héllo 世界".into(),
        tags: vec!["a".into(), "b".into()],
        samples: vec![1, -2, 3],
    }
}

/// Build a `Value::Object` from `(key, value)` pairs.
fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut map = nextjson::Map::new();
    for (key, value) in pairs {
        map.insert(key.to_string(), value.clone());
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// UBJSON
// ---------------------------------------------------------------------------

#[test]
fn ubjson_typed_roundtrip() {
    let r = record();
    let bytes = formats::Ubjson.encode(&r).unwrap();
    let back: Record = formats::Ubjson.decode(&bytes).unwrap();
    assert_eq!(back, r);
}

#[test]
fn ubjson_value_roundtrip() {
    let value = obj(&[
        ("null", Value::Null),
        ("bool", Value::Bool(true)),
        ("int", Value::from(12345_i64)),
        ("neg", Value::from(-64_i64)),
        ("float", Value::from(1.5_f64)),
        ("str", Value::String("hi".into())),
        (
            "arr",
            Value::Array(vec![Value::from(1_u64), Value::from(2_u64)]),
        ),
    ]);
    let bytes = formats::Ubjson.encode(&value).unwrap();
    let back: Value = formats::Ubjson.decode(&bytes).unwrap();
    assert_eq!(back, value);
}

#[test]
fn ubjson_interop_with_serde_ubjson_shapes() {
    // A counted array `[#U2 U1 U2` and a typed-counted `[$U#U2 1 2` must
    // decode. Both are legal UBJSON; counted containers have no end marker
    // and untyped elements keep their markers.
    let counted = b"[#U\x02U\x01U\x02";
    let v: Vec<u64> = formats::Ubjson.decode(counted).unwrap();
    assert_eq!(v, vec![1, 2]);

    let typed = b"[$U#U\x02\x01\x02";
    let v: Vec<u64> = formats::Ubjson.decode(typed).unwrap();
    assert_eq!(v, vec![1, 2]);

    // Counted object `{#U1 U1 a U2` -> {"a": 2}
    let obj_bytes = b"{#U\x01U\x01aU\x02";
    let v: Value = formats::Ubjson.decode(obj_bytes).unwrap();
    assert_eq!(v, obj(&[("a", Value::from(2_u64))]));
}

#[test]
fn ubjson_high_precision_u64() {
    // 2^63 (beyond i64::MAX) must round-trip exactly through the `H`
    // high-precision decimal form.
    let value = Value::from(1_u64 << 63);
    let bytes = formats::Ubjson.encode(&value).unwrap();
    assert_eq!(bytes[0], b'H', "expected high-precision marker");
    let back: Value = formats::Ubjson.decode(&bytes).unwrap();
    assert_eq!(back, value);
}

#[test]
fn ubjson_bytes_typed_array() {
    let bytes_in = vec![0u8, 1, 2, 250];
    let encoded = formats::Ubjson.encode(&nextjson::Bytes(&bytes_in)).unwrap();
    // `[ $ U # ...` with no closing `]`.
    assert!(encoded.starts_with(b"[$U#"), "expected typed uint8 array");
    let back: Vec<u8> = formats::Ubjson.decode(&encoded).unwrap();
    assert_eq!(back, bytes_in);
}

#[test]
fn ubjson_deep_nesting_limited() {
    // 200 nested arrays must be rejected (depth cap), not stack-overflow.
    let mut ub = vec![b'['; 200];
    ub.push(b']');
    assert!(formats::Ubjson.decode::<Value>(&ub).is_err());
}

// ---------------------------------------------------------------------------
// SMILE
// ---------------------------------------------------------------------------

#[test]
fn smile_typed_roundtrip() {
    let r = record();
    let bytes = formats::Smile.encode(&r).unwrap();
    assert!(bytes.starts_with(&[0x3A, 0x29, 0x0A]), "smile header");
    let back: Record = formats::Smile.decode(&bytes).unwrap();
    assert_eq!(back, r);
}

#[test]
fn smile_value_roundtrip() {
    let value = obj(&[
        ("null", Value::Null),
        ("bool", Value::Bool(false)),
        ("int", Value::from(100_000_i64)),
        ("small", Value::from(-16_i64)),
        ("float", Value::from(-29.951_f64)),
        ("long", Value::String("x".repeat(70))),
        ("uni", Value::String("世界".into())),
    ]);
    let bytes = formats::Smile.encode(&value).unwrap();
    let back: Value = formats::Smile.decode(&bytes).unwrap();
    assert_eq!(back, value);
}

#[test]
fn smile_end_marker() {
    // Explicit `0xFF` end-of-content marker is accepted.
    let bytes = formats::Smile.encode(&record()).unwrap();
    let mut with_end = bytes;
    with_end.push(0xFF);
    let back: Record = formats::Smile.decode(&with_end).unwrap();
    assert_eq!(back, record());
}

#[test]
fn smile_decodes_shared_name_references() {
    // Hand-built SMILE: header with shared-name flag (0x01), object with
    // `a` then a short shared reference to it, both values strings.
    let bytes = [
        0x3A, 0x29, 0x0A, 0x01, // header, shared names enabled
        0xFA, // START_OBJECT
        0x80, b'a', // key "a"
        0x40, b'1', // value "1" (tiny ascii len 1)
        0x40, // short shared key ref index 0
        0x40, b'2', // value "2"
        0xFB, // END_OBJECT
    ];
    let value: Value = formats::Smile.decode(&bytes).unwrap();
    assert_eq!(
        value,
        obj(&[
            ("a", Value::String("1".into())),
            ("a", Value::String("2".into())),
        ])
    );
}

#[test]
fn smile_deep_nesting_limited() {
    let mut sm = vec![0x3A, 0x29, 0x0A, 0x00];
    sm.extend(vec![0xF8; 200]);
    sm.push(0xF9);
    assert!(formats::Smile.decode::<Value>(&sm).is_err());
}

#[test]
fn smile_small_int_normalized_to_u64() {
    // Small-int tokens must decode to `Number::U64` (not `I64`) so the
    // value equals `Value::from(n)` — the library's equality invariant.
    // Regression: `Value::from(1_u64)` written via `write_u64` came back as
    // `I64(1)` and failed round-trip equality (incl. deep nesting).
    for n in [0_u64, 1, 15, 16, 100, 1_000_000, i64::MAX as u64] {
        let v = Value::from(n);
        let bytes = formats::Smile.encode(&v).unwrap();
        let back: Value = formats::Smile.decode(&bytes).unwrap();
        assert_eq!(back, v, "n={n}");
    }
    // Negative small ints stay signed.
    for n in [-1_i64, -16, -100] {
        let v = Value::from(n);
        let bytes = formats::Smile.encode(&v).unwrap();
        let back: Value = formats::Smile.decode(&bytes).unwrap();
        assert_eq!(back, v, "n={n}");
    }
}

#[test]
fn smile_vint_64bit_boundaries() {
    // The 10-byte VInt path (64 data bits: 9×7 + 1 final bit) must round-trip
    // both ends of the signed range exactly. Regression: `i64::MAX` decoded
    // with a 6-bit final shift and overflowed ("smile: vint overflow").
    let values = [
        i64::MIN,
        i64::MIN + 1,
        -2_i64.pow(62),
        -2_i64.pow(31),
        -1,
        0,
        1,
        2_i64.pow(31),
        2_i64.pow(62),
        i64::MAX - 1,
        i64::MAX,
    ];
    for n in values {
        let v = Value::from(n);
        let bytes = formats::Smile.encode(&v).unwrap();
        let back: Value = formats::Smile.decode(&bytes).unwrap();
        assert_eq!(back, v, "n={n}");
    }
}

// ---------------------------------------------------------------------------
// NDJSON
// ---------------------------------------------------------------------------

#[test]
fn ndjson_array_lines() {
    let rows = vec![record(), record()];
    let bytes = formats::Ndjson.encode(&rows).unwrap();
    let text = core::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with('{') && lines[1].starts_with('{'));
    let back: Vec<Record> = formats::Ndjson.decode(&bytes).unwrap();
    assert_eq!(back, rows);
}

#[test]
fn ndjson_single_value() {
    let bytes = formats::Ndjson.encode(&record()).unwrap();
    let back: Record = formats::Ndjson.decode(&bytes).unwrap();
    assert_eq!(back, record());
}

#[test]
fn ndjson_skips_blank_lines_and_cr() {
    let input = b"{\"a\":1}\r\n\n{\"b\":2}\n";
    let value: Vec<Value> = formats::Ndjson.decode(input).unwrap();
    assert_eq!(
        value,
        vec![
            obj(&[("a", Value::from(1_u64))]),
            obj(&[("b", Value::from(2_u64))]),
        ]
    );
}

#[test]
fn ndjson_rejects_trailing_data_in_single_mode() {
    let input = b"{\"a\":1}\n{\"b\":2}\n";
    assert!(formats::Ndjson.decode::<Value>(input).is_err());
}

// ---------------------------------------------------------------------------
// INI
// ---------------------------------------------------------------------------

#[test]
fn ini_roundtrip_config() {
    #[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
    struct Config {
        title: String,
        retries: String,
        owner: Owner,
    }
    #[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
    struct Owner {
        name: String,
        id: String,
    }
    let config = Config {
        title: "NextJson".into(),
        retries: "3".into(),
        owner: Owner {
            name: "blue".into(),
            id: "42".into(),
        },
    };
    let bytes = formats::Ini.encode(&config).unwrap();
    let text = core::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("[owner]"));
    assert!(text.contains("title = NextJson"));
    let back: Config = formats::Ini.decode(&bytes).unwrap();
    assert_eq!(back, config);
}

#[test]
fn ini_parses_comments_and_quotes() {
    let input = b"; comment\n# another\nname = \"a;b\"\n[sec]\nkey = v\r\n";
    let value: Value = formats::Ini.decode(input).unwrap();
    assert_eq!(
        value,
        obj(&[
            ("name", Value::String("a;b".into())),
            ("sec", obj(&[("key", Value::String("v".into()))])),
        ])
    );
}

#[test]
fn ini_rejects_arrays_and_nested_sections() {
    let value = Value::Array(vec![Value::from(1u64)]);
    assert!(formats::Ini.encode(&value).is_err());
    let nested = obj(&[("a", obj(&[("b", Value::Object(Default::default()))]))]);
    assert!(formats::Ini.encode(&nested).is_err());
}

#[test]
fn ini_type_guessing_roundtrip() {
    // Unquoted values are type-guessed on decode; numeric/boolean-looking
    // strings are quoted on encode so the round-trip is unambiguous.
    let value = obj(&[
        ("flag", Value::Bool(true)),
        ("count", Value::from(42_u64)),
        ("ratio", Value::from(3.5_f64)),
        ("negative", Value::from(-7_i64)),
        ("plain", Value::String("hello".into())),
        ("numstr", Value::String("3".into())),
        ("boolstr", Value::String("true".into())),
        ("floatstr", Value::String("2.5".into())),
    ]);
    let bytes = formats::Ini.encode(&value).unwrap();
    let text = core::str::from_utf8(&bytes).unwrap();
    // Numeric-looking strings must be quoted to stay strings.
    assert!(text.contains("numstr = \"3\""));
    assert!(text.contains("boolstr = \"true\""));
    assert!(text.contains("floatstr = \"2.5\""));
    let back: Value = formats::Ini.decode(&bytes).unwrap();
    assert_eq!(back, value);
}

#[test]
fn ini_typed_decode() {
    // Decoding directly into a typed struct: numbers/booleans come back typed.
    #[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
    struct Typed {
        retries: u32,
        flag: bool,
        ratio: f64,
        title: String,
    }
    let input = b"title = NextJson\nretries = 3\nflag = true\nratio = 2.5\n";
    let back: Typed = formats::Ini.decode(input).unwrap();
    assert_eq!(
        back,
        Typed {
            retries: 3,
            flag: true,
            ratio: 2.5,
            title: "NextJson".into(),
        }
    );
}

// ---------------------------------------------------------------------------
// EDN
// ---------------------------------------------------------------------------

#[test]
fn edn_roundtrip() {
    let value = obj(&[
        ("nil", Value::Null),
        ("bool", Value::Bool(true)),
        ("int", Value::from(-5_i64)),
        ("float", Value::from(1.5_f64)),
        ("str", Value::String("hi \"quoted\"\n".into())),
        (
            "list",
            Value::Array(vec![Value::from(1_u64), Value::from(2_u64)]),
        ),
    ]);
    let bytes = formats::Edn.encode(&value).unwrap();
    let back: Value = formats::Edn.decode(&bytes).unwrap();
    assert_eq!(back, value);
}

#[test]
fn edn_parses_vectors_lists_keywords_and_discard() {
    // `#_` discard skips a value; keyword keys decode to their name.
    let input = br#"{:a 1, "b" #_ 99 2, :c [1 2]}"#;
    let value: Value = formats::Edn.decode(input).unwrap();
    assert_eq!(
        value,
        obj(&[
            ("a", Value::from(1_u64)),
            ("b", Value::from(2_u64)),
            (
                "c",
                Value::Array(vec![Value::from(1_u64), Value::from(2_u64)])
            ),
        ])
    );
}

#[test]
fn edn_rejects_symbols_sets_and_tagged() {
    assert!(formats::Edn.decode::<Value>(b"foo").is_err());
    assert!(formats::Edn.decode::<Value>(b"#{1 2}").is_err());
    assert!(formats::Edn.decode::<Value>(b"#inst \"2020\"").is_err());
    assert!(formats::Edn.decode::<Value>(b"\\a").is_err());
}

// ---------------------------------------------------------------------------
// Registry / detection
// ---------------------------------------------------------------------------

#[test]
fn new_formats_registered() {
    for name in ["ubjson", "smile", "ndjson", "ini", "edn"] {
        assert!(
            formats::by_name(name).is_some(),
            "{name} missing from registry"
        );
    }
}

#[test]
fn detect_smile_and_ubjson() {
    assert_eq!(
        formats::detect(b":)\n\x00"),
        Some(formats::FormatKind::Smile)
    );
    assert_eq!(
        formats::detect(b"{\x53\x55\x01a"), // `{ S U 1 a`
        Some(formats::FormatKind::Ubjson)
    );
    assert_eq!(
        formats::detect(b"[\x24\x55\x23"), // `[ $ U #`
        Some(formats::FormatKind::Ubjson)
    );
}

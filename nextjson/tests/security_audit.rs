//! Regression tests for issues found by the security audit.
//!
//! Each test locks a fixed vulnerability so it cannot regress:
//!
//! - **pickle `TUPLE1/2/3` depth bypass** (Critical): `NONE` + N×`TUPLE1`
//!   builds N nested arrays with one byte per level while bypassing the
//!   `MARK`-based depth counter; the tree→token replay used to recurse
//!   without bound and overflow the stack. The shared replay now enforces
//!   the same 128-depth ceiling as every decoder.
//! - **hjson unquoted-key UTF-8 corruption** (Major): multi-byte keys were
//!   assembled one byte per `char`, silently garbling non-ASCII keys.
//! - **non-finite float acceptance** (Major): `1e999` parses to `inf` via
//!   `str::parse::<f64>()`; text decoders silently accepted it, contradicting
//!   the "non-finite floats are rejected" invariant.
//! - **text encoders emitting `NaN`/`inf`** (Major): `Value::from(f64::NAN)`
//!   serialized to invalid scalar text instead of erroring.
//! - **validation recursion / arithmetic**: `max_str_len = u64::MAX` must not
//!   overflow; a hand-built deep tree must be bounded, not crash.

use nextjson::formats;
use nextjson::Value;

// ---------------------------------------------------------------------------
// Critical: pickle depth bypass via TUPLE1/2/3
// ---------------------------------------------------------------------------

fn pickle_payload(n_tuples: usize) -> Vec<u8> {
    // Protocol 2 header + NONE + n × TUPLE1 (0x85) + STOP.
    let mut payload = vec![0x80, 0x02, 0x4E];
    payload.extend(vec![0x85u8; n_tuples]);
    payload.push(0x2E);
    payload
}

#[test]
fn pickle_tuple_chain_below_limit_roundtrips() {
    // 100 nested arrays are within the 128 ceiling and must decode.
    let v: Value = formats::decode_with(&pickle_payload(100), formats::Pickle).unwrap();
    let mut depth = 0u32;
    let mut cur = &v;
    while let Value::Array(a) = cur {
        depth += 1;
        assert_eq!(a.len(), 1);
        cur = &a[0];
    }
    assert_eq!(cur, &Value::Null);
    assert_eq!(depth, 100);
}

#[test]
fn pickle_tuple_chain_over_limit_is_an_error_not_a_crash() {
    // 200_000 TUPLE1 ops would previously overflow the stack in the
    // tree→token replay (1 byte per nesting level, `MARK` counter never
    // incremented). It must now be a clean error.
    let err = formats::decode_with::<Value, _>(&pickle_payload(200_000), formats::Pickle)
        .expect_err("deep pickle chain must be rejected, not crash the process");
    assert!(
        err.to_string().contains("recursion"),
        "unexpected error: {err}"
    );
}

#[test]
fn pickle_mark_chain_is_still_bounded() {
    // 200 unclosed MARKs drive `mark_depth` past 128; the MARK-counted path
    // must still reject before any deep value exists.
    let mut payload = vec![0x80, 0x02];
    payload.extend(vec![0x28; 200]); // MARK × 200
    payload.push(0x2E); // STOP
    let err = formats::decode_with::<Value, _>(&payload, formats::Pickle)
        .expect_err("MARK-chain beyond 128 must be rejected");
    assert!(
        err.to_string().contains("recursion"),
        "unexpected error: {err}"
    );
}

#[test]
fn from_value_bounds_hand_built_deep_trees() {
    // `from_value` replays a Value through the same token path; a hand-built
    // 5000-deep tree must be an error, not a stack overflow.
    let mut v = Value::Null;
    for _ in 0..5000 {
        v = Value::Array(vec![v]);
    }
    let err = nextjson::from_value::<Value>(v).expect_err("deep hand-built tree must be rejected");
    assert!(err.to_string().contains("depth"), "unexpected error: {err}");
}

// ---------------------------------------------------------------------------
// Major: hjson unquoted-key UTF-8 corruption
// ---------------------------------------------------------------------------

#[test]
fn hjson_unquoted_unicode_key_is_preserved() {
    // `ключ` = \u{43A}\u{43B}\u{44E}\u{447}; written with escapes so the
    // literal stays ASCII and cannot be mangled by any toolchain encoding.
    let key = "\u{43A}\u{43B}\u{44E}\u{447}"; // ключ
    let input = format!("{{{key}: 1}}");
    let v: Value = formats::decode_with(input.as_bytes(), formats::Hjson).unwrap();
    assert_eq!(v.get(key), Some(&Value::from(1_i64)));
    // Round-trips through encode + decode.
    let bytes = formats::encode_with(&v, formats::Hjson).unwrap();
    let back: Value = formats::decode_with(&bytes, formats::Hjson).unwrap();
    assert_eq!(back, v);
}

// ---------------------------------------------------------------------------
// Major: non-finite floats must be rejected on decode
// ---------------------------------------------------------------------------

#[test]
fn text_decoders_reject_overflowing_floats() {
    // `str::parse::<f64>()` turns `1e999` into `inf`; every text decoder must
    // reject it instead of accepting a value the JSON data model cannot hold.
    assert!(formats::decode_with::<Value, _>(b"x = 1e999", formats::Toml).is_err());
    assert!(formats::decode_with::<Value, _>(b"a: 1e999", formats::Yaml).is_err());
    assert!(formats::decode_with::<Value, _>(b"{a: 1e999}", formats::Hjson).is_err());
    assert!(formats::decode_with::<Value, _>(b"(a: 1e999)", formats::Ron).is_err());
    assert!(formats::decode_with::<Value, _>(b"1e999", formats::Sexpr).is_err());
    // urlform `Value` decoding keeps every value as a string (it is a flat
    // key/value text format); the non-finite rejection applies on the typed
    // number path.
    assert!(formats::decode_with::<f64, _>(b"a=1e999", formats::UrlForm).is_err());
    let url: Value = formats::decode_with(b"a=1e999", formats::UrlForm).unwrap();
    assert_eq!(url.get("a"), Some(&Value::from("1e999")));
    // CSV is a lossy, untyped cell format: a non-finite cell is classified as
    // a string rather than a fabricated number (never NaN on the wire).
    let csv: Value = formats::decode_with(b"1e999", formats::Csv).unwrap();
    assert!(
        csv.as_str().is_some(),
        "expected a string cell, got {csv:?}"
    );
    // The spelled spellings were already rejected; keep that locked.
    assert!(formats::decode_with::<Value, _>(b"a: .inf", formats::Yaml).is_err());
    assert!(formats::decode_with::<Value, _>(b"a: .nan", formats::Yaml).is_err());
}

#[test]
fn json5_keeps_its_documented_infinity_nan_support() {
    // JSON5 explicitly allows Infinity/NaN literals (documented in the
    // capability matrix); this is a deliberate exemption from the
    // non-finite rejection, not a bug.
    let v: Value = formats::decode_with(b"Infinity", formats::Json5).unwrap();
    assert!(v.as_number().is_some());
    // The JSON5 encoder emits strict JSON, so re-encoding errors honestly.
    assert!(formats::encode_with(&v, formats::Json5).is_err());
}

#[test]
fn typed_f64_decode_rejects_non_finite_from_binary_wire() {
    // msgpack can carry NaN bytes; the typed f64 target (the JSON data model)
    // rejects it instead of propagating NaN into business logic.
    let err = formats::decode_with::<f64, _>(
        &[0xcb, 0x7f, 0xf0, 0, 0, 0, 0, 0, 0], // f64 +inf
        formats::MsgPack,
    )
    .expect_err("typed f64 must reject +inf from the wire");
    assert!(
        err.to_string().contains("out of range"),
        "unexpected: {err}"
    );
}

// ---------------------------------------------------------------------------
// Major: text encoders must reject non-finite instead of emitting NaN text
// ---------------------------------------------------------------------------

#[test]
fn text_encoders_reject_non_finite_values() {
    let mut map = nextjson::Map::new();
    map.insert("value".to_string(), Value::from(f64::NAN));
    assert!(formats::encode_with(&map, formats::Toml).is_err());
    assert!(formats::encode_with(&map, formats::Yaml).is_err());
    assert!(formats::encode_with(&map, formats::Ron).is_err());
    assert!(formats::encode_with(&map, formats::Sexpr).is_err());
    assert!(formats::encode_with(&map, formats::UrlForm).is_err());
    assert!(formats::encode_with(&map, formats::Csv).is_err());
    // Binary formats that cannot represent NaN already reject.
    assert!(formats::encode_with(&map, formats::Bson).is_err());
    assert!(formats::encode_with(&map, formats::Pickle).is_err());
    // JSON (strict) rejects too.
    assert!(formats::encode_with(&map, formats::Json).is_err());
}

// ---------------------------------------------------------------------------
// Minor: validation arithmetic and recursion bounds
// ---------------------------------------------------------------------------

#[test]
fn max_str_len_u64_max_does_not_overflow() {
    use nextjson::{FieldSchema, Policy, StructSchema, TypeSchema};
    const S: TypeSchema = TypeSchema::Struct(&StructSchema {
        name: "S",
        transparent: false,
        max_depth: None,
        deny_unknown_fields: false,
        fields: &[FieldSchema {
            name: "s",
            orig: "s",
            required: true,
            flattened: false,
            policy: Policy {
                max_str_len: Some(u64::MAX),
                max_items: None,
                min: None,
                max: None,
                sensitive: false,
            },
            ty: TypeSchema::Str,
        }],
    });
    // Any string is shorter than u64::MAX; must validate cleanly, no panic.
    let v = nextjson::json!({ "s": "hello" });
    assert!(nextjson::validate(S, &v).is_ok());
    let _ = S;
}

#[test]
fn validate_bounds_hand_built_deep_trees() {
    // The validation walk is schema-driven, so a deep hand-built tree with a
    // shallow schema terminates (no crash) and reports no depth violation;
    // the hard depth cap is the backstop for hand-built deep schemas.
    use nextjson::TypeSchema;
    let mut v = Value::Null;
    for _ in 0..5000 {
        v = Value::Array(vec![v]);
    }
    let schema = TypeSchema::Seq(&TypeSchema::Opaque);
    let report = nextjson::validate(schema, &v);
    assert!(
        report.is_ok(),
        "walk must terminate and accept a shallow schema: {report:?}"
    );
}

#[test]
fn container_max_depth_attribute_overflow_is_safe() {
    // `max_depth = u64::MAX` on a container must not overflow `depth + m`.
    use nextjson::{StructSchema, TypeSchema};
    const S: TypeSchema = TypeSchema::Struct(&StructSchema {
        name: "S",
        transparent: false,
        max_depth: Some(u64::MAX),
        deny_unknown_fields: false,
        fields: &[],
    });
    let v = nextjson::json!({});
    assert!(nextjson::validate(S, &v).is_ok());
}

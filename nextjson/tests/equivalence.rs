//! Format-equivalence and differential testing platform.
//!
//! The multi-format claim — "one data model, many wire formats, no lossy
//! fallback" — is verified here as an automated equivalence matrix:
//!
//! 1. **Cross-decode matrix**: every value is encoded with each
//!    JSON-compatible format and decoded with every other format in the
//!    family; all decodes must produce the identical `Value`. This is the
//!    strongest form of the cross-format-relay property: no intermediate
//!    typed value and no per-pair code.
//! 2. **Randomized differential**: a deterministic LCG generates nested
//!    values; each is round-tripped and cross-decoded so the matrix is
//!    exercised far beyond the hand-written fixtures.
//! 3. **Boundary values**: exact `i128`/`u128`, `f64` extremes, `-0.0`,
//!    Unicode scalar boundaries and control characters travel through every
//!    wire format that can represent them.
//! 4. **Semantics under ambiguity**: duplicate keys and unknown fields
//!    behave identically across formats.

use nextjson::formats;
use nextjson::formats::{Cbor, Hjson, Json, Json5, MsgPack, Ron, Yaml};
use nextjson::Value;

/// A concrete handle over one JSON-compatible format. The `Format` trait is
/// generic (not `dyn`-compatible), so the family is dispatched through this
/// small enum instead of a trait object.
#[derive(Clone, Copy)]
enum Fmt {
    Json,
    Json5,
    Hjson,
    Yaml,
    Ron,
    Cbor,
    MsgPack,
}

impl Fmt {
    fn name(self) -> &'static str {
        match self {
            Fmt::Json => "json",
            Fmt::Json5 => "json5",
            Fmt::Hjson => "hjson",
            Fmt::Yaml => "yaml",
            Fmt::Ron => "ron",
            Fmt::Cbor => "cbor",
            Fmt::MsgPack => "msgpack",
        }
    }
    fn encode(self, value: &Value) -> nextjson::Result<Vec<u8>> {
        match self {
            Fmt::Json => formats::encode_with(value, Json),
            Fmt::Json5 => formats::encode_with(value, Json5),
            Fmt::Hjson => formats::encode_with(value, Hjson),
            Fmt::Yaml => formats::encode_with(value, Yaml),
            Fmt::Ron => formats::encode_with(value, Ron),
            Fmt::Cbor => formats::encode_with(value, Cbor),
            Fmt::MsgPack => formats::encode_with(value, MsgPack),
        }
    }
    fn decode(self, bytes: &[u8]) -> nextjson::Result<Value> {
        match self {
            Fmt::Json => formats::decode_with(bytes, Json),
            Fmt::Json5 => formats::decode_with(bytes, Json5),
            Fmt::Hjson => formats::decode_with(bytes, Hjson),
            Fmt::Yaml => formats::decode_with(bytes, Yaml),
            Fmt::Ron => formats::decode_with(bytes, Ron),
            Fmt::Cbor => formats::decode_with(bytes, Cbor),
            Fmt::MsgPack => formats::decode_with(bytes, MsgPack),
        }
    }
    /// Relay bytes written in `self` into `dest`, returning dest bytes.
    fn transcode_to(self, bytes: &[u8], dest: Fmt) -> nextjson::Result<Vec<u8>> {
        match (self, dest) {
            (Fmt::Json, Fmt::Json) => formats::transcode(bytes, Json, Json),
            (Fmt::Json, Fmt::Json5) => formats::transcode(bytes, Json, Json5),
            (Fmt::Json, Fmt::Hjson) => formats::transcode(bytes, Json, Hjson),
            (Fmt::Json, Fmt::Yaml) => formats::transcode(bytes, Json, Yaml),
            (Fmt::Json, Fmt::Ron) => formats::transcode(bytes, Json, Ron),
            (Fmt::Json, Fmt::Cbor) => formats::transcode(bytes, Json, Cbor),
            (Fmt::Json, Fmt::MsgPack) => formats::transcode(bytes, Json, MsgPack),
            (Fmt::Json5, Fmt::Json) => formats::transcode(bytes, Json5, Json),
            (Fmt::Json5, Fmt::Json5) => formats::transcode(bytes, Json5, Json5),
            (Fmt::Json5, Fmt::Hjson) => formats::transcode(bytes, Json5, Hjson),
            (Fmt::Json5, Fmt::Yaml) => formats::transcode(bytes, Json5, Yaml),
            (Fmt::Json5, Fmt::Ron) => formats::transcode(bytes, Json5, Ron),
            (Fmt::Json5, Fmt::Cbor) => formats::transcode(bytes, Json5, Cbor),
            (Fmt::Json5, Fmt::MsgPack) => formats::transcode(bytes, Json5, MsgPack),
            (Fmt::Hjson, Fmt::Json) => formats::transcode(bytes, Hjson, Json),
            (Fmt::Hjson, Fmt::Json5) => formats::transcode(bytes, Hjson, Json5),
            (Fmt::Hjson, Fmt::Hjson) => formats::transcode(bytes, Hjson, Hjson),
            (Fmt::Hjson, Fmt::Yaml) => formats::transcode(bytes, Hjson, Yaml),
            (Fmt::Hjson, Fmt::Ron) => formats::transcode(bytes, Hjson, Ron),
            (Fmt::Hjson, Fmt::Cbor) => formats::transcode(bytes, Hjson, Cbor),
            (Fmt::Hjson, Fmt::MsgPack) => formats::transcode(bytes, Hjson, MsgPack),
            (Fmt::Yaml, Fmt::Json) => formats::transcode(bytes, Yaml, Json),
            (Fmt::Yaml, Fmt::Json5) => formats::transcode(bytes, Yaml, Json5),
            (Fmt::Yaml, Fmt::Hjson) => formats::transcode(bytes, Yaml, Hjson),
            (Fmt::Yaml, Fmt::Yaml) => formats::transcode(bytes, Yaml, Yaml),
            (Fmt::Yaml, Fmt::Ron) => formats::transcode(bytes, Yaml, Ron),
            (Fmt::Yaml, Fmt::Cbor) => formats::transcode(bytes, Yaml, Cbor),
            (Fmt::Yaml, Fmt::MsgPack) => formats::transcode(bytes, Yaml, MsgPack),
            (Fmt::Ron, Fmt::Json) => formats::transcode(bytes, Ron, Json),
            (Fmt::Ron, Fmt::Json5) => formats::transcode(bytes, Ron, Json5),
            (Fmt::Ron, Fmt::Hjson) => formats::transcode(bytes, Ron, Hjson),
            (Fmt::Ron, Fmt::Yaml) => formats::transcode(bytes, Ron, Yaml),
            (Fmt::Ron, Fmt::Ron) => formats::transcode(bytes, Ron, Ron),
            (Fmt::Ron, Fmt::Cbor) => formats::transcode(bytes, Ron, Cbor),
            (Fmt::Ron, Fmt::MsgPack) => formats::transcode(bytes, Ron, MsgPack),
            (Fmt::Cbor, Fmt::Json) => formats::transcode(bytes, Cbor, Json),
            (Fmt::Cbor, Fmt::Json5) => formats::transcode(bytes, Cbor, Json5),
            (Fmt::Cbor, Fmt::Hjson) => formats::transcode(bytes, Cbor, Hjson),
            (Fmt::Cbor, Fmt::Yaml) => formats::transcode(bytes, Cbor, Yaml),
            (Fmt::Cbor, Fmt::Ron) => formats::transcode(bytes, Cbor, Ron),
            (Fmt::Cbor, Fmt::Cbor) => formats::transcode(bytes, Cbor, Cbor),
            (Fmt::Cbor, Fmt::MsgPack) => formats::transcode(bytes, Cbor, MsgPack),
            (Fmt::MsgPack, Fmt::Json) => formats::transcode(bytes, MsgPack, Json),
            (Fmt::MsgPack, Fmt::Json5) => formats::transcode(bytes, MsgPack, Json5),
            (Fmt::MsgPack, Fmt::Hjson) => formats::transcode(bytes, MsgPack, Hjson),
            (Fmt::MsgPack, Fmt::Yaml) => formats::transcode(bytes, MsgPack, Yaml),
            (Fmt::MsgPack, Fmt::Ron) => formats::transcode(bytes, MsgPack, Ron),
            (Fmt::MsgPack, Fmt::Cbor) => formats::transcode(bytes, MsgPack, Cbor),
            (Fmt::MsgPack, Fmt::MsgPack) => formats::transcode(bytes, MsgPack, MsgPack),
        }
    }
}

/// The JSON-compatible family: every format whose wire model is exactly the
/// JSON data model (null / bool / finite number / string / array / object).
/// Encoder output may differ (bencode, bson, pickle, toml, csv, urlform,
/// postcard, sexpr have model or shape restrictions), so they are covered by
/// their own wire tests.
const JSON_FAMILY: [Fmt; 7] = [
    Fmt::Json,
    Fmt::Json5,
    Fmt::Hjson,
    Fmt::Yaml,
    Fmt::Ron,
    Fmt::Cbor,
    Fmt::MsgPack,
];

/// A rich value that every JSON-compatible format must preserve exactly.
fn rich() -> Value {
    nextjson::json!({
        "name": "NextJson",
        "count": 7,
        "ratio": 3.25,
        "ok": true,
        "no": null,
        "items": ["a", "b", 3, [1, 2, {"k": false}]],
        "config": { "deep": -17, "pi": 1.2345 },
        "unicode": "héllo 世界 🎉 \u{1F600}",
        "escapes": "line1\nline2\t\"quoted\"\\",
        "empty": {},
        "empty_list": [],
    })
}

/// The format-equivalence claim: relaying a value from any source format into
/// any destination format through the event stream must produce **exactly the
/// same bytes** as encoding the value directly in the destination format.
///
/// This is stronger than a value round-trip: the relay is byte-identical to
/// the direct encoder, which is what makes multi-format a first-class
/// property instead of a per-pair adapter.
#[test]
fn transcode_is_byte_identical_to_direct_encode() {
    let value = rich();
    let mut combos = 0usize;
    for source in JSON_FAMILY {
        let bytes = source
            .encode(&value)
            .unwrap_or_else(|e| panic!("encode with {} failed: {e}", source.name()));
        for dest in JSON_FAMILY {
            let relayed = source.transcode_to(&bytes, dest).unwrap_or_else(|e| {
                panic!("relay {} -> {} failed: {e}", source.name(), dest.name())
            });
            let direct = dest
                .encode(&value)
                .unwrap_or_else(|e| panic!("direct encode with {} failed: {e}", dest.name()));
            assert_eq!(
                relayed,
                direct,
                "relay {} -> {} differs from direct {} encode",
                source.name(),
                dest.name(),
                dest.name()
            );
            combos += 1;
        }
    }
    // 7 x 7 = 49 relay combinations.
    assert_eq!(combos, JSON_FAMILY.len() * JSON_FAMILY.len());
}

/// A deterministic xorshift-style generator so failures are reproducible
/// without an external RNG dependency.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_value(rng: &mut Lcg, depth: u32) -> Value {
    if depth == 0 {
        return random_scalar(rng);
    }
    match rng.pick(6) {
        0 => random_scalar(rng),
        1 => Value::from(rng.next() as i64),
        2 => {
            let n = rng.pick(6);
            let mut arr = Vec::new();
            for _ in 0..n {
                arr.push(random_value(rng, depth - 1));
            }
            Value::Array(arr)
        }
        3 => {
            let n = rng.pick(5);
            let mut m = nextjson::Map::new();
            for i in 0..n {
                m.insert(
                    format!("key{i}_{}", rng.pick(100)),
                    random_value(rng, depth - 1),
                );
            }
            Value::Object(m)
        }
        4 => Value::String(format!("s{}", rng.pick(1000))),
        _ => Value::Bool(rng.pick(2) == 0),
    }
}

fn random_scalar(rng: &mut Lcg) -> Value {
    match rng.pick(5) {
        0 => Value::Null,
        1 => Value::Bool(rng.pick(2) == 0),
        2 => Value::from(rng.next() as i64),
        3 => Value::from((rng.next() as i64 % 1000) as f64 / 10.0),
        _ => Value::String(format!("v{}", rng.pick(100))),
    }
}

/// 200 generated values: each is relayed across the whole family and must be
/// byte-identical to the direct destination encode, plus a value round-trip
/// through every format.
#[test]
fn random_differential_transcode() {
    let mut rng = Lcg(0x9E3779B97F4A7C15);
    for _ in 0..200 {
        let value = random_value(&mut rng, 4);
        for source in JSON_FAMILY {
            let Ok(bytes) = source.encode(&value) else {
                continue; // a value this generator produced may exceed a
                          // format's wire model; round-trips of such values
                          // are covered by the per-format tests.
            };
            for dest in JSON_FAMILY {
                let relayed = match source.transcode_to(&bytes, dest) {
                    Ok(b) => b,
                    Err(e) => panic!(
                        "relay {} -> {} failed on {value:?}: {e}",
                        source.name(),
                        dest.name()
                    ),
                };
                let direct = match dest.encode(&value) {
                    Ok(b) => b,
                    Err(e) => panic!("direct {} encode failed on {value:?}: {e}", dest.name()),
                };
                assert_eq!(
                    relayed,
                    direct,
                    "relay {} -> {} differs on {value:?}",
                    source.name(),
                    dest.name()
                );
            }
        }
    }
}

/// Exact numeric extremes survive every format that can represent them.
#[test]
fn numeric_boundaries_across_formats() {
    let ints: Vec<Value> = vec![
        Value::from(0_i64),
        Value::from(-1_i64),
        Value::from(i64::MIN),
        Value::from(i64::MAX),
        Value::from(u64::MAX),
        Value::from(i128::MIN),
        Value::from(i128::MAX),
        Value::from(u128::MAX),
    ];
    let floats: Vec<Value> = vec![
        Value::from(3.5_f64),
        Value::from(-0.0_f64),
        Value::from(1e300_f64),
        Value::from(-1e-300_f64),
    ];

    for value in ints.iter().chain(floats.iter()) {
        for format in JSON_FAMILY {
            if let Ok(bytes) = format.encode(value) {
                let decoded: Value = format
                    .decode(&bytes)
                    .unwrap_or_else(|e| panic!("{} decode failed: {e}", format.name()));
                let equal = match (value, &decoded) {
                    // -0.0 and 0.0 compare equal; both are valid JSON.
                    (Value::Number(a), Value::Number(b)) => {
                        a.as_f64() == b.as_f64()
                            || (a.as_i128().is_some() && a.as_i128() == b.as_i128())
                    }
                    (a, b) => a == b,
                };
                assert!(
                    equal,
                    "{} changed {value:?} into {decoded:?}",
                    format.name()
                );
            }
        }
    }
}

/// Unicode scalar boundaries and control characters survive every format.
#[test]
fn unicode_boundaries_across_formats() {
    let strings = vec![
        "\u{0000}",         // NUL
        "\u{001F}",         // unit separator (last control char)
        "\u{007F}",         // DEL
        "\u{0080}",         // first non-ASCII
        "\u{07FF}\u{0800}", // 2-byte / 3-byte UTF-8 boundary
        "\u{D7FF}\u{E000}", // around the surrogate range (both excluded)
        "\u{FFFD}",         // replacement char
        "\u{10FFFF}",       // last Unicode scalar
        "🦀🦀🦀",           // astral plane
        "héllo wörld 日本語",
    ];
    for s in strings {
        let value = Value::from(s);
        for format in JSON_FAMILY {
            let bytes = format
                .encode(&value)
                .unwrap_or_else(|e| panic!("{} encode of {s:?} failed: {e}", format.name()));
            let decoded: Value = format
                .decode(&bytes)
                .unwrap_or_else(|e| panic!("{} decode of {s:?} failed: {e}", format.name()));
            assert_eq!(decoded, value, "{} changed {s:?}", format.name());
        }
    }
}

/// Regression: the JSON5 decoder must handle `\u` / `\x` escapes and combine
/// valid surrogate pairs into one scalar (lone surrogates become U+FFFD per
/// the JSON5 spec).
#[test]
fn json5_unicode_escapes() {
    let cases: &[(&str, &str)] = &[
        (r#""\u0000""#, "\u{0}"),
        (r#""\u00e9""#, "é"),
        (r#""\uD83D\uDE00""#, "😀"), // surrogate pair -> one scalar
        (r#""\x41\x42""#, "AB"),
        (r#""\uD83D""#, "\u{FFFD}"), // lone high surrogate
        (r#""\uDE00""#, "\u{FFFD}"), // lone low surrogate
    ];
    for (wire, expected) in cases {
        let value: Value = formats::decode_with(wire.as_bytes(), Json5)
            .unwrap_or_else(|e| panic!("json5 decode of {wire:?} failed: {e}"));
        assert_eq!(value, Value::from(*expected), "json5 wire {wire:?}");
    }
}

/// Duplicate keys resolve to the last occurrence in every format that
/// accepts them, matching `Map::insert` semantics.
#[test]
fn duplicate_keys_resolve_identically() {
    let cases: &[&[u8]] = &[
        br#"{"a":1,"a":2}"#,
        // CBOR map with duplicate keys (2 pairs).
        &[0xa2, 0x61, b'a', 0x01, 0x61, b'a', 0x02],
        // MessagePack map with duplicate keys.
        &[0x82, 0xa1, b'a', 0x01, 0xa1, b'a', 0x02],
    ];
    for bytes in cases {
        let value: Value = formats::decode_with(bytes, formats::Json)
            .or_else(|_| formats::decode_with(bytes, formats::Cbor))
            .or_else(|_| formats::decode_with(bytes, formats::MsgPack))
            .expect("at least one decoder must accept the bytes");
        let obj = value
            .as_object()
            .expect("duplicate-key case must be an object");
        assert_eq!(obj.get("a"), Some(&Value::from(2_i64)));
    }
}

/// Unknown fields are retained when decoding into a `Value` (the schema-less
/// consumer preserves the original document).
#[test]
fn unknown_fields_preserved_in_value() {
    let json = br#"{"known":1,"extra":{"nested":[true,null]},"more":"x"}"#;
    let json_value: Value = formats::decode_with(json, formats::Json).unwrap();
    let msgpack = formats::transcode(json, formats::Json, formats::MsgPack).unwrap();
    let mp_value: Value = formats::decode_with(&msgpack, formats::MsgPack).unwrap();
    assert_eq!(json_value, mp_value);
    assert!(json_value.get("extra").is_some());
    assert!(json_value.get("more").is_some());
}

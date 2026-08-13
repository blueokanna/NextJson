//! Cross-format matrix tests: every supported structure × every format.

use nextjson::formats::{self, Format};

struct MissingArraySeparator;

impl nextjson::NsonSchema for MissingArraySeparator {
    const SCHEMA: nextjson::TypeSchema = nextjson::TypeSchema::Opaque;
}

struct CsvReorderedObjectRows;

impl nextjson::NsonSchema for CsvReorderedObjectRows {
    const SCHEMA: nextjson::TypeSchema = nextjson::TypeSchema::Opaque;
}

impl nextjson::NsonSerialize for CsvReorderedObjectRows {
    fn nextencode<E: nextjson::FormatEncoder>(
        &self,
        encoder: &mut E,
    ) -> core::result::Result<(), E::Error> {
        encoder.begin_array()?;
        encoder.separator()?;
        encoder.begin_object()?;
        encoder.key("left")?;
        encoder.write_i64(1)?;
        encoder.key("right")?;
        encoder.write_i64(2)?;
        encoder.end_object()?;
        encoder.separator()?;
        encoder.begin_object()?;
        encoder.key("right")?;
        encoder.write_i64(4)?;
        encoder.key("left")?;
        encoder.write_i64(3)?;
        encoder.end_object()?;
        encoder.end_array()
    }
}

impl nextjson::NsonSerialize for MissingArraySeparator {
    fn nextencode<E: nextjson::FormatEncoder>(
        &self,
        encoder: &mut E,
    ) -> core::result::Result<(), E::Error> {
        encoder.begin_array()?;
        encoder.write_null()?;
        encoder.end_array()
    }
}

#[test]
fn format_entry_points_reject_invalid_serialization_events() {
    assert!(formats::MsgPack.encode(&MissingArraySeparator).is_err());
    assert!(formats::Ron.encode(&MissingArraySeparator).is_err());
    assert!(formats::Yaml.encode(&MissingArraySeparator).is_err());
    assert!(formats::Bencode.encode(&MissingArraySeparator).is_err());
}

// ---------------------------------------------------------------------------
// Text formats
// ---------------------------------------------------------------------------

#[test]
fn ron_roundtrips() {
    roundtrip(&vec![1_i64, 2, 3], formats::Ron);
    roundtrip(&(1_i64, "two".to_string(), 3.5_f64), formats::Ron);
    roundtrip(&"hello".to_string(), formats::Ron);
    roundtrip(&true, formats::Ron);
    roundtrip(&Option::<i64>::None, formats::Ron);
    roundtrip(&Some(7_i64), formats::Ron);
    let mut m = nextjson::Map::new();
    m.insert("name".to_string(), nextjson::Value::from("NextJson"));
    m.insert("n".to_string(), nextjson::Value::from(42_i64));
    roundtrip(&m, formats::Ron);
}

#[test]
fn ron_wire() {
    assert_eq!(formats::Ron.encode(&42_i64).unwrap(), b"42");
    assert_eq!(formats::Ron.encode(&true).unwrap(), b"true");
    assert_eq!(formats::Ron.encode(&"hi".to_string()).unwrap(), b"\"hi\"");
    assert_eq!(formats::Ron.encode(&vec![1_i64, 2]).unwrap(), b"[1, 2]");
    assert_eq!(formats::Ron.encode(&Option::<i64>::None).unwrap(), b"None");
    assert_eq!(formats::Ron.encode(&Some(5_i64)).unwrap(), b"5");
}

#[test]
fn ron_decodes_foreign_syntax() {
    // Struct form and Some() unwrapping from hand-written RON.
    let value: nextjson::Value = formats::Ron.decode(b"(name: \"x\", count: 3)").unwrap();
    assert_eq!(value["name"], nextjson::Value::from("x"));
    assert_eq!(value["count"], nextjson::Value::from(3_i64));
    let value: nextjson::Value = formats::Ron.decode(b"Some([1, 2])").unwrap();
    assert_eq!(value[1], nextjson::Value::from(2_i64));
}

#[test]
fn json5_roundtrips_and_lenient() {
    roundtrip(&vec![1_i64, 2, 3], formats::Json5);
    roundtrip(&"héllo ✓".to_string(), formats::Json5);
    let mut m = nextjson::Map::new();
    m.insert("a".to_string(), nextjson::Value::from(1_i64));
    roundtrip(&m, formats::Json5);
    // JSON5 lenient syntax: comments, unquoted keys, single quotes, trailing comma.
    let value: nextjson::Value = formats::Json5
        .decode(
            br#"{ // comment
            unquotedKey: 'value',
            hex: 0x1F,
            trailing: [1, 2, 3,],
        }"#,
        )
        .unwrap();
    assert_eq!(value["unquotedKey"], nextjson::Value::from("value"));
    assert_eq!(value["hex"], nextjson::Value::from(31_i64));
    assert_eq!(value["trailing"][2], nextjson::Value::from(3_i64));
}

#[test]
fn hjson_roundtrips_and_lenient() {
    roundtrip(&vec![1_i64, 2, 3], formats::Hjson);
    roundtrip(&"hi".to_string(), formats::Hjson);
    let mut m = nextjson::Map::new();
    m.insert("a".to_string(), nextjson::Value::from(1_i64));
    roundtrip(&m, formats::Hjson);
    // Hjson: comments, unquoted keys/strings.
    let value: nextjson::Value = formats::Hjson
        .decode(
            br#"{
            # a comment
            name: NextJson
            nums: [1, 2, 3,]
        }"#,
        )
        .unwrap();
    assert_eq!(value["name"], nextjson::Value::from("NextJson"));
    assert_eq!(value["nums"][0], nextjson::Value::from(1_i64));
}

#[test]
fn sexpr_roundtrips() {
    roundtrip(&vec![1_i64, 2, 3], formats::Sexpr);
    roundtrip(&"hello".to_string(), formats::Sexpr);
    roundtrip(&true, formats::Sexpr);
    roundtrip(&Option::<i64>::None, formats::Sexpr);
    roundtrip(&vec!["a".to_string(), "b".to_string()], formats::Sexpr);
    roundtrip(&vec![vec![1_i64], vec![2_i64]], formats::Sexpr);
    let mut m = nextjson::Map::new();
    m.insert("a".to_string(), nextjson::Value::from(1_i64));
    m.insert("b".to_string(), nextjson::Value::from("two"));
    roundtrip(&m, formats::Sexpr);
}

#[test]
fn sexpr_wire() {
    assert_eq!(formats::Sexpr.encode(&vec![1_i64, 2]).unwrap(), b"(1 2)");
    assert_eq!(formats::Sexpr.encode(&true).unwrap(), b"#t");
    assert_eq!(
        formats::Sexpr.encode(&"a b".to_string()).unwrap(),
        b"\"a b\""
    );
    assert_eq!(formats::Sexpr.encode(&42_i64).unwrap(), b"42");
}

#[test]
fn urlform_roundtrips() {
    let mut m = std::collections::BTreeMap::new();
    m.insert("name".to_string(), "Next Json".to_string());
    m.insert("count".to_string(), "42".to_string());
    roundtrip(&m, formats::UrlForm);
    // Percent-encoding round-trip with special characters.
    let mut m = std::collections::BTreeMap::new();
    m.insert("q".to_string(), "a+b/c d%".to_string());
    let bytes = formats::UrlForm.encode(&m).unwrap();
    assert_eq!(bytes, b"q=a%2Bb%2Fc+d%25");
    let back: std::collections::BTreeMap<String, String> = formats::UrlForm.decode(&bytes).unwrap();
    assert_eq!(back.get("q").unwrap(), "a+b/c d%");
}

#[test]
fn csv_roundtrips() {
    let rows = vec![
        vec!["a".to_string(), "b".to_string()],
        vec!["1".to_string(), "2".to_string()],
    ];
    let bytes = formats::Csv.encode(&rows).unwrap();
    assert_eq!(bytes, b"a,b\n1,2\n");
    let back: Vec<Vec<String>> = formats::Csv.decode(&bytes).unwrap();
    assert_eq!(back, rows);
    // Quoted fields with commas/newlines.
    let rows = vec![vec!["x,y".to_string(), "line\nbreak".to_string()]];
    let bytes = formats::Csv.encode(&rows).unwrap();
    assert_eq!(bytes, b"\"x,y\",\"line\nbreak\"\n");
    let back: Vec<Vec<String>> = formats::Csv.decode(&bytes).unwrap();
    assert_eq!(back, rows);

    let spaced: Vec<Vec<String>> = formats::Csv.decode(b"  left,\tright\n").unwrap();
    assert_eq!(
        spaced,
        vec![vec!["  left".to_string(), "\tright".to_string()]]
    );
    assert!(formats::Csv
        .decode::<Vec<Vec<String>>>(b"a\"b,c\n")
        .is_err());
    assert!(formats::Csv
        .decode::<Vec<Vec<String>>>(b"\"a\"x,c\n")
        .is_err());
}

#[test]
fn csv_object_rows_with_header() {
    let mut r1 = nextjson::Map::new();
    r1.insert("name".to_string(), nextjson::Value::from("a"));
    r1.insert("n".to_string(), nextjson::Value::from(1_i64));
    let mut r2 = nextjson::Map::new();
    r2.insert("name".to_string(), nextjson::Value::from("b"));
    r2.insert("n".to_string(), nextjson::Value::from(2_i64));
    let rows = vec![r1, r2];
    let bytes = formats::Csv.encode(&rows).unwrap();
    assert_eq!(bytes, b"name,n\na,1\nb,2\n");
    let back: Vec<nextjson::Map> = formats::Csv.decode(&bytes).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0]["name"], nextjson::Value::from("a"));
    assert_eq!(back[1]["n"], nextjson::Value::from(2_i64));

    let reordered = formats::Csv.encode(&CsvReorderedObjectRows).unwrap();
    assert_eq!(reordered, b"left,right\n1,2\n3,4\n");

    assert!(formats::Csv
        .decode::<Vec<nextjson::Map>>(b"a,a\n1,2\n")
        .is_err());
    assert!(formats::Csv
        .decode::<Vec<nextjson::Map>>(b"a,b\n1\n")
        .is_err());
    assert!(formats::Csv
        .decode::<Vec<nextjson::Map>>(b"a\n1,2\n")
        .is_err());
    assert!(formats::Csv.encode(&42_i64).is_err());
}

#[test]
fn toml_roundtrips() {
    let mut m = nextjson::Map::new();
    m.insert("title".to_string(), nextjson::Value::from("NextJson"));
    m.insert("version".to_string(), nextjson::Value::from(1_i64));
    m.insert("enabled".to_string(), nextjson::Value::from(true));
    m.insert(
        "list".to_string(),
        nextjson::Value::from(vec![
            nextjson::Value::from(1_i64),
            nextjson::Value::from(2_i64),
        ]),
    );
    let mut nested = nextjson::Map::new();
    nested.insert("key".to_string(), nextjson::Value::from("val"));
    m.insert("table".to_string(), nextjson::Value::Object(nested));
    roundtrip(&m, formats::Toml);

    let mut encoder = formats::TomlEncoder::new(Vec::new());
    nextjson::NsonSerialize::nextencode(&m, &mut encoder).unwrap();
    let encoded = encoder.finish().unwrap();
    assert!(!encoded.is_empty());
    let direct: nextjson::Map = formats::Toml.decode(&encoded).unwrap();
    assert_eq!(direct, m);
}

#[test]
fn toml_decodes_foreign() {
    let input = br#"
        title = "NextJson"
        [owner]
        name = "blueokanna"
        [[products]]
        name = "Hammer"
        [products.details]
        color = "red"
    "#;
    let value: nextjson::Value = formats::Toml.decode(input).unwrap();
    assert_eq!(value["title"], nextjson::Value::from("NextJson"));
    assert_eq!(value["owner"]["name"], nextjson::Value::from("blueokanna"));
    assert_eq!(
        value["products"][0]["name"],
        nextjson::Value::from("Hammer")
    );
    assert_eq!(
        value["products"][0]["details"]["color"],
        nextjson::Value::from("red")
    );
}

#[test]
fn toml_rejects_excessive_nesting() {
    let mut input = String::from("value = ");
    input.push_str(&"[".repeat(129));
    input.push('0');
    input.push_str(&"]".repeat(129));
    assert!(formats::Toml
        .decode::<nextjson::Value>(input.as_bytes())
        .is_err());
}

#[test]
fn toml_multi_line_basic_string() {
    let input = br#"
s = """
Roses are red
Violets are blue"""
t = "simple"
"#;
    let value: nextjson::Value = formats::Toml.decode(input).unwrap();
    assert_eq!(
        value["s"],
        nextjson::Value::from("Roses are red\nViolets are blue")
    );
    assert_eq!(value["t"], nextjson::Value::from("simple"));
}

#[test]
fn toml_multi_line_string_continuation_and_trim() {
    // Line-ending backslash trims the newline plus following whitespace;
    // trailing whitespace before the closing delimiter is trimmed.
    let input = br#"
s = """
first \
    continued
   last   """
t = '''
literal
  text'''
"#;
    let value: nextjson::Value = formats::Toml.decode(input).unwrap();
    assert_eq!(
        value["s"],
        nextjson::Value::from("first continued\n   last")
    );
    assert_eq!(value["t"], nextjson::Value::from("literal\n  text"));
}

#[test]
fn toml_multi_line_string_escapes() {
    let input = br#"
s = """
quote \" here
unicode \u00e9
"""
"#;
    let value: nextjson::Value = formats::Toml.decode(input).unwrap();
    assert_eq!(
        value["s"],
        nextjson::Value::from("quote \" here\nunicode é")
    );
}

#[test]
fn toml_datetime_preserved_as_string() {
    let input = br#"
odt = 1979-05-27T07:32:00Z
ldt = 1979-05-27T07:32:00
ld = 1979-05-27
lt = 07:32:00
odt2 = 1979-05-27 07:32:00.999-07:00
"#;
    let value: nextjson::Value = formats::Toml.decode(input).unwrap();
    assert_eq!(value["odt"], nextjson::Value::from("1979-05-27T07:32:00Z"));
    assert_eq!(value["ldt"], nextjson::Value::from("1979-05-27T07:32:00"));
    assert_eq!(value["ld"], nextjson::Value::from("1979-05-27"));
    assert_eq!(value["lt"], nextjson::Value::from("07:32:00"));
    assert_eq!(
        value["odt2"],
        nextjson::Value::from("1979-05-27 07:32:00.999-07:00")
    );
}

#[test]
fn toml_invalid_datetime_is_not_silently_stringified() {
    // Month 13 is not a plausible date: the value must not be kept as a
    // date string (it fails the number path instead).
    assert!(formats::Toml
        .decode::<nextjson::Value>(b"d = 2020-13-99\n")
        .is_err());
}

#[test]
fn toml_time_shape_with_non_digit_bytes_errors_not_panics() {
    // `+:2:345` has the `??:??:??` shape (len >= 8, `:` at offsets 2 and 5)
    // but a byte below `'0'` at offset 0. The time-range validator used to
    // subtract `b'0'` before checking digits, which underflowed (panic in
    // debug builds) instead of rejecting the value.
    for bad in [
        "d = +:2:345\n",
        "d = -:2:345\n",
        "d = .:2:345\n",
        "d = 1:2:34\n",
        "d = 1 :2:345\n",
    ] {
        let out = formats::Toml.decode::<nextjson::Value>(bad.as_bytes());
        // Must not panic; either a clear error or (never) a value.
        if let Ok(value) = out {
            // If it did parse, it must not be the malformed time kept as a
            // datetime string.
            let s = value["d"].as_str().unwrap_or("");
            assert!(
                !is_malformed_time(s),
                "malformed time survived as a string: {s:?}"
            );
        }
    }
}

/// Whether `s` looks like the rejected `??:??:??` shape with a non-digit in a
/// digit slot (used to assert the bug did not silently stringify it).
fn is_malformed_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 8
        && b[2] == b':'
        && b[5] == b':'
        && b[..8]
            .iter()
            .enumerate()
            .any(|(i, &c)| i != 2 && i != 5 && !c.is_ascii_digit())
}

#[test]
fn toml_hex_octal_binary_integers() {
    let input = br#"
hex = 0xDEADBEEF
hexu = 0Xdead_beef
oct = 0o755
bin = 0b1101_0101
neg = -42
"#;
    let value: nextjson::Value = formats::Toml.decode(input).unwrap();
    assert_eq!(value["hex"], nextjson::Value::from(0xDEADBEEF_u64));
    assert_eq!(value["hexu"], nextjson::Value::from(0xdead_beef_u64));
    assert_eq!(value["oct"], nextjson::Value::from(0o755_u64));
    assert_eq!(value["bin"], nextjson::Value::from(0b1101_0101_u64));
    assert_eq!(value["neg"], nextjson::Value::from(-42_i64));
}

#[test]
fn toml_invalid_radix_integer_is_error() {
    assert!(formats::Toml
        .decode::<nextjson::Value>(b"x = 0xZZ\n")
        .is_err());
}

#[test]
fn yaml_roundtrips() {
    let mut m = nextjson::Map::new();
    m.insert("name".to_string(), nextjson::Value::from("NextJson"));
    m.insert("count".to_string(), nextjson::Value::from(3_i64));
    m.insert("ok".to_string(), nextjson::Value::from(true));
    m.insert(
        "tags".to_string(),
        nextjson::Value::from(vec![
            nextjson::Value::from("fast"),
            nextjson::Value::from("safe"),
        ]),
    );
    let mut nested = nextjson::Map::new();
    nested.insert("deep".to_string(), nextjson::Value::from(1_i64));
    m.insert("config".to_string(), nextjson::Value::Object(nested));
    roundtrip(&m, formats::Yaml);

    let mut encoder = formats::YamlEncoder::new(Vec::new());
    nextjson::NsonSerialize::nextencode(&m, &mut encoder).unwrap();
    let encoded = encoder.finish().unwrap();
    assert!(!encoded.is_empty());
    let direct: nextjson::Map = formats::Yaml.decode(&encoded).unwrap();
    assert_eq!(direct, m);
}

#[test]
fn yaml_decodes_foreign() {
    let input = br#"
name: NextJson
count: 3
ok: true
tags:
  - fast
  - safe
config:
  deep: 1
"#;
    let value: nextjson::Value = formats::Yaml.decode(input).unwrap();
    assert_eq!(value["name"], nextjson::Value::from("NextJson"));
    assert_eq!(value["count"], nextjson::Value::from(3_i64));
    assert_eq!(value["ok"], nextjson::Value::from(true));
    assert_eq!(value["tags"][1], nextjson::Value::from("safe"));
    assert_eq!(value["config"]["deep"], nextjson::Value::from(1_i64));
}

#[test]
fn yaml_flow_style() {
    let value: nextjson::Value = formats::Yaml
        .decode(br#"{a: 1, b: [true, null], c: {x: y}}"#)
        .unwrap();
    assert_eq!(value["a"], nextjson::Value::from(1_i64));
    assert_eq!(value["b"][0], nextjson::Value::from(true));
    assert_eq!(value["b"][1], nextjson::Value::Null);
    assert_eq!(value["c"]["x"], nextjson::Value::from("y"));
}

#[test]
fn yaml_rejects_excessive_flow_nesting() {
    let mut input = "[".repeat(129);
    input.push('0');
    input.push_str(&"]".repeat(129));
    assert!(formats::Yaml
        .decode::<nextjson::Value>(input.as_bytes())
        .is_err());
}

#[test]
fn formats_registry_count() {
    // More formats registered than the serde screenshot list alone.
    let all = formats::all();
    assert!(all.len() >= 14);
    for info in all {
        assert!(!info.name.is_empty());
    }
    // No duplicate entries: every name is unique.
    let mut names: Vec<&str> = all.iter().map(|f| f.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        all.len(),
        "registry contains duplicate formats"
    );
}

// ---------------------------------------------------------------------------
// Cross-format transcoding and full matrix
// ---------------------------------------------------------------------------

#[test]
fn transcode_between_formats() {
    let json = br#"{"name":"NextJson","values":[1,2,3],"ok":true}"#;
    let msgpack = formats::transcode(json, formats::Json, formats::MsgPack).unwrap();
    let back = formats::transcode(&msgpack, formats::MsgPack, formats::Json).unwrap();
    assert_eq!(back, json);
    // JSON -> YAML -> JSON
    let yaml = formats::transcode(json, formats::Json, formats::Yaml).unwrap();
    let back = formats::transcode(&yaml, formats::Yaml, formats::Json).unwrap();
    assert_eq!(back, json);
    // JSON -> CBOR -> JSON
    let cbor = formats::transcode(json, formats::Json, formats::Cbor).unwrap();
    let back = formats::transcode(&cbor, formats::Cbor, formats::Json).unwrap();
    assert_eq!(back, json);
    // JSON -> RON -> JSON
    let ron = formats::transcode(json, formats::Json, formats::Ron).unwrap();
    let back = formats::transcode(&ron, formats::Ron, formats::Json).unwrap();
    assert_eq!(back, json);
}

#[test]
fn full_matrix_roundtrips() {
    // A structure rich enough for every self-describing format.
    let mut m = nextjson::Map::new();
    m.insert("name".to_string(), nextjson::Value::from("NextJson"));
    m.insert("count".to_string(), nextjson::Value::from(7_i64));
    m.insert("ok".to_string(), nextjson::Value::from(true));
    m.insert(
        "items".to_string(),
        nextjson::Value::from(vec![
            nextjson::Value::from("a"),
            nextjson::Value::from("b"),
            nextjson::Value::from(3_i64),
        ]),
    );
    let mut nested = nextjson::Map::new();
    nested.insert("deep".to_string(), nextjson::Value::from(1_i64));
    m.insert("config".to_string(), nextjson::Value::Object(nested));

    roundtrip(&m, formats::Json);
    roundtrip(&m, formats::Cbor);
    roundtrip(&m, formats::Json5);
    roundtrip(&m, formats::Hjson);
    roundtrip(&m, formats::MsgPack);
    roundtrip(&m, formats::Pickle);
    roundtrip(&m, formats::Ron);
    roundtrip(&m, formats::Yaml);
    roundtrip(&m, formats::Toml);
    // BSON requires a top-level document (this map qualifies).
    roundtrip(&m, formats::Bson);
    // S-expressions encode maps as alists; schema-less `Value` decoding of a
    // nested map is ambiguous there (typed round-trips are tested above).
    assert!(formats::Sexpr.encode(&m).is_ok());
    // Bencode lacks bool/null/float types (lossy for `Value`), so its
    // round-trips are covered by format-specific tests above.
    assert!(formats::Bencode.encode(&m).is_ok());

    // Floats round-trip through every float-capable format.
    let f = 3.25_f64;
    roundtrip(&f, formats::Json);
    roundtrip(&f, formats::Cbor);
    roundtrip(&f, formats::MsgPack);
    roundtrip(&f, formats::Pickle);
    roundtrip(&f, formats::Ron);
    roundtrip(&f, formats::Sexpr);
    roundtrip(&f, formats::Yaml);
    // ...but not toml/bson: both are document-shaped, so a bare scalar root is
    // not representable (rejected honestly, like the top-level-document rule).
    assert!(formats::Toml.encode(&f).is_err());
    assert!(formats::Bson.encode(&f).is_err());
    // ...but not bencode/postcard (no float type on the wire).
    assert!(formats::Bencode.encode(&f).is_err());
    assert!(formats::Postcard.encode(&f).is_err());
}

#[test]
fn cross_language_wire_compatibility() {
    // MessagePack bytes produced by a Python msgpack writer.
    let foreign: &[u8] = &[
        0x82, // fixmap(2)
        0xa3, b'f', b'o', b'o', 0x2a, // "foo" -> 42 (positive fixint)
        0xa3, b'b', b'a', b'r', 0xa3, b'b', b'a', b'z', // "bar" -> "baz"
    ];
    let value: nextjson::Value = formats::MsgPack.decode(foreign).unwrap();
    assert_eq!(value["foo"], nextjson::Value::from(42_u8));
    assert_eq!(value["bar"], nextjson::Value::from("baz"));

    // CBOR bytes produced by a cbor2 (Python) writer.
    let cbor_foreign: &[u8] = &[
        0xa2, // map(2)
        0x63, b'f', b'o', b'o', 0x18, 0x2a, // text(3) "foo": uint(42)
        0x63, b'b', b'a', b'r', 0x63, b'b', b'a', b'z', // "bar": "baz"
    ];
    let value: nextjson::Value = formats::Cbor.decode(cbor_foreign).unwrap();
    assert_eq!(value["foo"], nextjson::Value::from(42_u8));

    // BSON bytes produced by MongoDB-style writers.
    let bson_foreign: &[u8] = &[
        0x1a, 0x00, 0x00, 0x00, // len = 26
        0x10, b'n', 0x00, 0x2a, 0x00, 0x00, 0x00, // int32 "n" = 42
        0x08, b'b', 0x00, 0x01, // bool "b" = true
        0x02, b's', 0x00, 0x03, 0x00, 0x00, 0x00, b'h', b'i', 0x00, // "s" = "hi"
        0x00,
    ];
    let value: nextjson::Value = formats::Bson.decode(bson_foreign).unwrap();
    assert_eq!(value["n"], nextjson::Value::from(42_i32));
    assert_eq!(value["b"], nextjson::Value::from(true));
    assert_eq!(value["s"], nextjson::Value::from("hi"));

    // TOML from Cargo.
    let toml_foreign = br#"[package]
name = "nextjson"
version = "0.1.0"
edition = "2021"
"#;
    let value: nextjson::Value = formats::Toml.decode(toml_foreign).unwrap();
    assert_eq!(value["package"]["name"], nextjson::Value::from("nextjson"));

    // YAML from a Kubernetes manifest.
    let yaml_foreign = br#"apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  key: value
"#;
    let value: nextjson::Value = formats::Yaml.decode(yaml_foreign).unwrap();
    assert_eq!(value["kind"], nextjson::Value::from("ConfigMap"));
    assert_eq!(
        value["metadata"]["name"],
        nextjson::Value::from("app-config")
    );
}

fn roundtrip<F: Format, T>(value: &T, format: F) -> T
where
    T: nextjson::NsonSerialize
        + for<'de> nextjson::NsonDeserialize<'de>
        + Clone
        + PartialEq
        + core::fmt::Debug,
{
    let bytes = format.encode(value).expect("encode");
    let back: T = format.decode(&bytes).expect("decode");
    assert_eq!(&back, value, "round-trip failed for {}", F::NAME);
    back
}

#[test]
fn msgpack_scalars() {
    assert_eq!(formats::MsgPack.encode(&true).unwrap(), &[0xc3], "true");
    assert_eq!(formats::MsgPack.encode(&false).unwrap(), &[0xc2]);
    assert_eq!(
        formats::MsgPack.encode(&Option::<u8>::None).unwrap(),
        &[0xc0]
    );
    assert_eq!(formats::MsgPack.encode(&42_u8).unwrap(), &[0x2a]);
    assert_eq!(formats::MsgPack.encode(&-1_i8).unwrap(), &[0xff]);
    assert_eq!(
        formats::MsgPack.encode(&300_u16).unwrap(),
        &[0xcd, 0x01, 0x2c]
    );
    assert_eq!(
        formats::MsgPack.encode(&"hello").unwrap(),
        &[0xa5, b'h', b'e', b'l', b'l', b'o']
    );
    // Wire-exact float64.
    assert_eq!(
        formats::MsgPack.encode(&1.5_f64).unwrap(),
        &[0xcb, 0x3f, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn msgpack_containers_wire() {
    // [1, 2, 3] -> fixarray(3)
    assert_eq!(
        formats::MsgPack.encode(&vec![1_u8, 2, 3]).unwrap(),
        &[0x93, 0x01, 0x02, 0x03]
    );
    // {"a":1} -> fixmap(1) + fixstr(1) + uint
    let mut map = std::collections::BTreeMap::new();
    map.insert("a".to_string(), 1_u8);
    assert_eq!(
        formats::MsgPack.encode(&map).unwrap(),
        &[0x81, 0xa1, b'a', 0x01]
    );
}

#[test]
fn msgpack_roundtrips() {
    roundtrip(
        &(7_u64, "NextJson".to_string(), vec![1_i32, 2, 3]),
        formats::MsgPack,
    );
    roundtrip(
        &[
            ("name".to_string(), "NextJson".to_string()),
            ("kind".to_string(), "serde-free".to_string()),
            ("ok".to_string(), "yes".to_string()),
        ],
        formats::MsgPack,
    );
    roundtrip(&vec![0_u16, 1, 15, 16, 255, 256, 65535], formats::MsgPack);
    roundtrip(&[-128_i16, -1, 0, 127, 128, 32767], formats::MsgPack);
    roundtrip(&3.25_f64, formats::MsgPack);
    roundtrip(&-0.5_f32, formats::MsgPack);
    roundtrip(&"héllo wörld ✓".to_string(), formats::MsgPack);
    roundtrip(&Option::<String>::None, formats::MsgPack);
    roundtrip(&Some("x".to_string()), formats::MsgPack);
    roundtrip(&[true, false, true], formats::MsgPack);
    roundtrip(&Vec::<i64>::new(), formats::MsgPack);
    roundtrip(
        &[
            ["a".to_string(), "b".to_string()],
            ["c".to_string(), "d".to_string()],
        ],
        formats::MsgPack,
    );
}

#[test]
fn msgpack_large_containers_need_wide_headers() {
    // 20 elements forces array16.
    let v: Vec<u8> = (0..20).collect();
    let bytes = formats::MsgPack.encode(&v).unwrap();
    assert_eq!(&bytes[..3], &[0xdc, 0x00, 0x14]);
    let back: Vec<u8> = formats::MsgPack.decode(&bytes).unwrap();
    assert_eq!(back, v);

    // 20 entries forces map16.
    let mut m = std::collections::BTreeMap::new();
    for i in 0..20_u8 {
        m.insert(i.to_string(), i);
    }
    let bytes = formats::MsgPack.encode(&m).unwrap();
    assert_eq!(&bytes[..3], &[0xde, 0x00, 0x14]);
    let back: std::collections::BTreeMap<String, u8> = formats::MsgPack.decode(&bytes).unwrap();
    assert_eq!(back, m);
}

#[test]
fn msgpack_decodes_foreign_wire_bytes() {
    // Hand-crafted MessagePack from a Python msgpack writer:
    // {"list": [1, 2, 3], "s": "x", "n": null, "b": true}
    let foreign: &[u8] = &[
        0x84, 0xa4, b'l', b'i', b's', b't', 0x93, 0x01, 0x02, 0x03, 0xa1, b's', 0xa1, b'x', 0xa1,
        b'n', 0xc0, 0xa1, b'b', 0xc3,
    ];
    let value: nextjson::Value = formats::MsgPack.decode(foreign).unwrap();
    assert_eq!(value["list"][2], nextjson::Value::from(3_u8));
    assert_eq!(value["s"], nextjson::Value::from("x"));
    assert_eq!(value["n"], nextjson::Value::Null);
    assert_eq!(value["b"], nextjson::Value::from(true));
}

#[test]
fn msgpack_rejects_bad_input() {
    // Truncated string length.
    assert!(formats::MsgPack
        .decode::<String>(&[0xda, 0x01, 0x00, b'a'])
        .is_err());
    // Invalid utf-8 in a string.
    assert!(formats::MsgPack.decode::<String>(&[0xa1, 0xff]).is_err());
    // Trailing bytes.
    assert!(formats::MsgPack.decode::<u8>(&[0x2a, 0x2b]).is_err());
    // Out-of-64-bit u128 must error on encode.
    assert!(formats::MsgPack.encode(&u128::MAX).is_err());
}

#[test]
fn registry_and_detection() {
    assert!(formats::all().iter().any(|f| f.name == "msgpack"));
    assert_eq!(formats::by_name("JSON"), Some(formats::FormatKind::Json));
    assert_eq!(
        formats::by_extension("yml"),
        Some(formats::FormatKind::Yaml)
    );
    assert_eq!(
        formats::detect(&[0x93, 0x01, 0x02, 0x03]),
        Some(formats::FormatKind::MsgPack)
    );
    assert_eq!(
        formats::detect(b"{\"a\":1}"),
        Some(formats::FormatKind::Json)
    );
}

// ---------------------------------------------------------------------------
// Postcard
// ---------------------------------------------------------------------------

#[test]
fn postcard_wire() {
    assert_eq!(formats::Postcard.encode(&42_u64).unwrap(), &[0x2a]);
    assert_eq!(formats::Postcard.encode(&0_u64).unwrap(), &[0x00]);
    assert_eq!(
        formats::Postcard.encode(&"abc".to_string()).unwrap(),
        &[0x03, b'a', b'b', b'c']
    );
    assert_eq!(
        formats::Postcard.encode(&vec![1_u8, 2, 3]).unwrap(),
        &[0x03, 0x01, 0x02, 0x03]
    );
    assert_eq!(formats::Postcard.encode(&300_u64).unwrap(), &[0xac, 0x02]);
}

#[test]
fn postcard_typed_roundtrips() {
    roundtrip(&42_u64, formats::Postcard);
    roundtrip(&vec![1_u64, 2, 3], formats::Postcard);
    roundtrip(&"hello".to_string(), formats::Postcard);
    roundtrip(&["a".to_string(), "b".to_string()], formats::Postcard);
    let mut m = std::collections::BTreeMap::new();
    m.insert("x".to_string(), 7_u64);
    m.insert("y".to_string(), 8_u64);
    roundtrip(&m, formats::Postcard);
    roundtrip(&(), formats::Postcard);
}

#[test]
fn postcard_rejects_non_self_describing() {
    // Signed and floats cannot be encoded (postcard is not self-describing).
    assert!(formats::Postcard.encode(&-1_i64).is_err());
    assert!(formats::Postcard.encode(&1.5_f64).is_err());
    // Value decoding requires peeking, which postcard cannot do.
    assert!(formats::Postcard
        .decode::<nextjson::Value>(&[0x2a])
        .is_err());
    // Option requires peeking to distinguish None.
    assert!(formats::Postcard.decode::<Option<u64>>(&[0x2a]).is_err());
    assert!(formats::Postcard.decode::<u64>(&[0x80, 0x00]).is_err());
}

// ---------------------------------------------------------------------------
// Bencode
// ---------------------------------------------------------------------------

#[test]
fn bencode_wire() {
    assert_eq!(formats::Bencode.encode(&42_i64).unwrap(), b"i42e");
    assert_eq!(formats::Bencode.encode(&-5_i64).unwrap(), b"i-5e");
    assert_eq!(
        formats::Bencode.encode(&"spam".to_string()).unwrap(),
        b"4:spam"
    );
    assert_eq!(
        formats::Bencode.encode(&vec![1_i64, 2, 3]).unwrap(),
        b"li1ei2ei3ee"
    );
    let mut m = nextjson::Map::new();
    m.insert("bar".to_string(), nextjson::Value::from("spam"));
    m.insert("foo".to_string(), nextjson::Value::from(42_i64));
    assert_eq!(
        formats::Bencode.encode(&m).unwrap(),
        b"d3:bar4:spam3:fooi42ee"
    );
}

#[test]
fn bencode_roundtrips() {
    roundtrip(&42_i64, formats::Bencode);
    roundtrip(&-1000_i128, formats::Bencode);
    roundtrip(&i128::MIN, formats::Bencode);
    roundtrip(&"spam eggs".to_string(), formats::Bencode);
    roundtrip(&vec![1_i64, 2, 3], formats::Bencode);
    roundtrip(&vec![vec![1_i64], vec![2_i64, 3_i64]], formats::Bencode);
    let mut m = nextjson::Map::new();
    m.insert("a".to_string(), nextjson::Value::from(1_i64));
    m.insert("b".to_string(), nextjson::Value::from("two"));
    m.insert(
        "c".to_string(),
        nextjson::Value::from(vec![
            nextjson::Value::from(10_i64),
            nextjson::Value::from(20_i64),
        ]),
    );
    roundtrip(&m, formats::Bencode);

    let mut unsorted = nextjson::Map::new();
    unsorted.insert("z".to_string(), nextjson::Value::from(1_i64));
    unsorted.insert("a".to_string(), nextjson::Value::from(2_i64));
    assert_eq!(
        formats::Bencode.encode(&unsorted).unwrap(),
        b"d1:ai2e1:zi1ee"
    );
    // Bools encode as integers (bencode has no bool type); a *typed* bool
    // target still decodes correctly.
    let flag = true;
    let bytes = formats::Bencode.encode(&flag).unwrap();
    assert_eq!(bytes, b"i1e");
    assert!(formats::Bencode.decode::<bool>(&bytes).unwrap());
}

#[test]
fn bencode_decodes_foreign_wire_bytes() {
    // Hand-crafted torrent-style bencode.
    let foreign = b"d8:announce13:udp://tracker6:piecesli1ei2ei3eee";
    let value: nextjson::Value = formats::Bencode.decode(foreign).unwrap();
    assert_eq!(value["announce"], nextjson::Value::from("udp://tracker"));
    assert_eq!(value["pieces"][2], nextjson::Value::from(3_i64));
}

#[test]
fn bencode_rejects_unsupported() {
    assert!(formats::Bencode.encode(&1.5_f64).is_err());
    assert!(formats::Bencode.encode(&Option::<u8>::None).is_err());
    assert!(formats::Bencode.encode(&u128::MAX).is_err());
    assert!(formats::Bencode.decode::<i64>(b"i03e").is_err());
    assert!(formats::Bencode.decode::<i64>(b"i-0e").is_err());
    assert!(formats::Bencode.decode::<String>(b"03:abc").is_err());
}

// ---------------------------------------------------------------------------
// Pickle
// ---------------------------------------------------------------------------

#[test]
fn pickle_wire() {
    // 42 -> PROTO(2) BININT1(42) STOP
    assert_eq!(
        formats::Pickle.encode(&42_i64).unwrap(),
        &[0x80, 0x02, 0x4b, 0x2a, 0x2e]
    );
    // "abc" -> PROTO(2) BINUNICODE(len=3) "abc" STOP
    assert_eq!(
        formats::Pickle.encode(&"abc".to_string()).unwrap(),
        &[0x80, 0x02, 0x58, 0x03, 0, 0, 0, b'a', b'b', b'c', 0x2e]
    );
}

#[test]
fn pickle_roundtrips() {
    roundtrip(&42_i64, formats::Pickle);
    roundtrip(&-300_i64, formats::Pickle);
    roundtrip(&1.5_f64, formats::Pickle);
    roundtrip(&true, formats::Pickle);
    roundtrip(&"héllo ✓".to_string(), formats::Pickle);
    roundtrip(&vec![1_i64, 2, 3], formats::Pickle);
    roundtrip(&(1_i64, "two".to_string(), 3.0_f64), formats::Pickle);
    let mut m = nextjson::Map::new();
    m.insert("a".to_string(), nextjson::Value::from(1_i64));
    m.insert(
        "b".to_string(),
        nextjson::Value::from(vec![
            nextjson::Value::from(1_i64),
            nextjson::Value::from(2_i64),
        ]),
    );
    roundtrip(&m, formats::Pickle);
    roundtrip(&Option::<i64>::None, formats::Pickle);
    roundtrip(&Some(7_i64), formats::Pickle);
    // Large integers use LONG1.
    roundtrip(&i128::MAX, formats::Pickle);
    roundtrip(&i128::MIN, formats::Pickle);
}

#[test]
fn pickle_decodes_real_python_bytes() {
    // CPython: pickle.dumps({"a": [1, 2, 3], "sp": None, "b": True}, 2)
    let foreign: &[u8] = &[
        0x80, 0x02, 0x7d, 0x28, // PROTO2 EMPTY_DICT MARK
        0x58, 0x01, 0x00, 0x00, 0x00, b'a', // BINUNICODE "a"
        0x5d, 0x28, // EMPTY_LIST MARK
        0x4b, 0x01, 0x4b, 0x02, 0x4b, 0x03, // BININT1 1 2 3
        0x65, // APPENDS
        0x58, 0x02, 0x00, 0x00, 0x00, b's', b'p', // BINUNICODE "sp"
        0x4e, // NONE
        0x58, 0x01, 0x00, 0x00, 0x00, b'b', // BINUNICODE "b"
        0x88, // NEWTRUE
        0x75, // SETITEMS
        0x2e, // STOP
    ];
    let value: nextjson::Value = formats::Pickle.decode(foreign).unwrap();
    assert_eq!(value["a"][0], nextjson::Value::from(1_i64));
    assert_eq!(value["a"][2], nextjson::Value::from(3_i64));
    assert_eq!(value["sp"], nextjson::Value::Null);
    assert_eq!(value["b"], nextjson::Value::from(true));
}

#[test]
fn pickle_rejects_bad_input() {
    assert!(formats::Pickle.decode::<i64>(&[0x80, 0x02, 0xff]).is_err());
    assert!(formats::Pickle.decode::<i64>(&[0x80, 0x02]).is_err());
    assert!(formats::Pickle
        .decode::<i64>(&[0x80, 0x05, 0x4b, 0x01, 0x2e])
        .is_err());
}

#[test]
fn pickle_rejects_non_finite_wire_floats() {
    for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
        let mut wire = vec![0x80, 0x02, 0x47];
        wire.extend_from_slice(&value.to_be_bytes());
        wire.push(0x2e);
        assert!(formats::Pickle.decode::<nextjson::Value>(&wire).is_err());
    }
}

// ---------------------------------------------------------------------------
// BSON
// ---------------------------------------------------------------------------

#[test]
fn bson_wire() {
    // {"a": 1} -> doc(int32 len) + [int32 "a" 1] + 0x00
    let mut m = nextjson::Map::new();
    m.insert("a".to_string(), nextjson::Value::from(1_i32));
    let bytes = formats::Bson.encode(&m).unwrap();
    assert_eq!(
        bytes,
        &[0x0c, 0x00, 0x00, 0x00, 0x10, b'a', 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]
    );
    // [1, 2, 3] -> array document with numeric keys
    let arr = vec![1_i64, 2_i64, 3_i64];
    let bytes = formats::Bson.encode(&arr).unwrap();
    assert_eq!(
        bytes,
        &[
            0x1a, 0x00, 0x00, 0x00, // len = 26
            0x10, b'0', 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, b'1', 0x00, 0x02, 0x00, 0x00, 0x00,
            0x10, b'2', 0x00, 0x03, 0x00, 0x00, 0x00, 0x00,
        ]
    );
    // {"a": [true, false], "b": "hi", "c": null}
    let mut m = nextjson::Map::new();
    m.insert(
        "a".to_string(),
        nextjson::Value::from(vec![
            nextjson::Value::from(true),
            nextjson::Value::from(false),
        ]),
    );
    m.insert("b".to_string(), nextjson::Value::from("hi"));
    m.insert("c".to_string(), nextjson::Value::Null);
    let bytes = formats::Bson.encode(&m).unwrap();
    let back: nextjson::Value = formats::Bson.decode(&bytes).unwrap();
    assert_eq!(back["a"][0], nextjson::Value::from(true));
    assert_eq!(back["a"][1], nextjson::Value::from(false));
    assert_eq!(back["b"], nextjson::Value::from("hi"));
    assert_eq!(back["c"], nextjson::Value::Null);
}

#[test]
fn bson_roundtrips() {
    roundtrip(&vec![1_i64, 2, 3], formats::Bson);
    roundtrip(&vec!["a".to_string(), "b".to_string()], formats::Bson);
    roundtrip(&vec![vec![1_i64], vec![2_i64, 3_i64]], formats::Bson);
    roundtrip(&(1_i64, "two".to_string(), 3.0_f64), formats::Bson);
    let mut m = nextjson::Map::new();
    m.insert("name".to_string(), nextjson::Value::from("NextJson"));
    m.insert("count".to_string(), nextjson::Value::from(7_i64));
    m.insert("ok".to_string(), nextjson::Value::from(true));
    m.insert(
        "tags".to_string(),
        nextjson::Value::from(vec![
            nextjson::Value::from("fast"),
            nextjson::Value::from("safe"),
        ]),
    );
    roundtrip(&m, formats::Bson);
}

#[test]
fn bson_rejects_unsupported() {
    // Root scalars are invalid BSON (document-oriented).
    assert!(formats::Bson.encode(&42_i64).is_err());
    assert!(formats::Bson.encode(&"hi".to_string()).is_err());
    assert!(formats::Bson.encode(&true).is_err());
    assert!(formats::Bson.encode(&Option::<u8>::None).is_err());
    assert!(formats::Bson.encode(&u128::MAX).is_err());
    assert!(formats::Bson.encode(&i128::MIN).is_err());

    let mut nul_key = nextjson::Map::new();
    nul_key.insert("bad\0key".to_string(), nextjson::Value::Null);
    assert!(formats::Bson.encode(&nul_key).is_err());

    let mut non_finite = nextjson::Map::new();
    non_finite.insert("value".to_string(), nextjson::Value::from(f64::INFINITY));
    assert!(formats::Bson.encode(&non_finite).is_err());

    let invalid_bool = [9, 0, 0, 0, 0x08, b'a', 0, 2, 0];
    assert!(formats::Bson
        .decode::<nextjson::Value>(&invalid_bool)
        .is_err());

    let invalid_string_terminator = [14, 0, 0, 0, 0x02, b's', 0, 2, 0, 0, 0, b'a', 1, 0];
    assert!(formats::Bson
        .decode::<nextjson::Value>(&invalid_string_terminator)
        .is_err());

    let negative_document_length = [0xff, 0xff, 0xff, 0xff, 0];
    assert!(formats::Bson
        .decode::<nextjson::Value>(&negative_document_length)
        .is_err());
}

#[test]
fn bson_rejects_truncated_documents() {
    // Length claims 100 bytes but only 12 are present.
    let mut m = nextjson::Map::new();
    m.insert("a".to_string(), nextjson::Value::from(1_i32));
    let bytes = formats::Bson.encode(&m).unwrap();
    let mut bad = bytes.clone();
    bad[0] = 100;
    assert!(formats::Bson.decode::<nextjson::Value>(&bad).is_err());
}

// ---------------------------------------------------------------------------
// Regression tests: adversarial / boundary inputs (no panics, no corruption)
// ---------------------------------------------------------------------------

#[test]
fn urlform_truncated_percent_escape_is_error_not_panic() {
    // A trailing `%X` used to index out of bounds and abort the process.
    for input in [
        &b"x=%1"[..],
        &b"x=%F"[..],
        &b"a=%a&b=2"[..],
        &b"%"[..],
        &b"x=%"[..],
    ] {
        assert!(
            formats::UrlForm.decode::<nextjson::Value>(input).is_err(),
            "input {input:?} must error, not panic"
        );
    }
}

#[test]
fn urlform_percent_utf8_roundtrip() {
    // `%C3%A9` must decode as `é`, not two Latin-1 characters.
    let bytes = b"q=%C3%A9";
    let value: nextjson::Value = formats::UrlForm.decode(bytes).unwrap();
    assert_eq!(value["q"], nextjson::Value::from("é"));
    let mut m = std::collections::BTreeMap::new();
    m.insert("q".to_string(), "é✓".to_string());
    roundtrip(&m, formats::UrlForm);
}

#[test]
fn urlform_option_and_value_decode() {
    // `Option`/`Value` peek before reading: the value must not be lost or
    // replaced by the next pair.
    let bytes = b"a=1&b=2";
    let value: nextjson::Value = formats::UrlForm.decode(bytes).unwrap();
    assert_eq!(value["a"], nextjson::Value::from("1"));
    assert_eq!(value["b"], nextjson::Value::from("2"));
    let opt: std::collections::BTreeMap<String, Option<i64>> =
        formats::UrlForm.decode(bytes).unwrap();
    assert_eq!(opt.get("a"), Some(&Some(1)));
    assert_eq!(opt.get("b"), Some(&Some(2)));
}

#[test]
fn pickle_large_unsigned_roundtrips() {
    // Values in [2^31, 2^32) and [2^63, 2^64) used to flip sign on the wire.
    roundtrip(&0x8000_0000_u64, formats::Pickle); // 2^31
    roundtrip(&0xFFFF_FFFF_u64, formats::Pickle); // 2^32-1
    roundtrip(&0x8000_0000_0000_0000_u64, formats::Pickle); // 2^63
    roundtrip(&u64::MAX, formats::Pickle);
    roundtrip(&i128::MAX, formats::Pickle);
    roundtrip(&(1_i128 << 63), formats::Pickle);
    roundtrip(&(-(1_i128 << 63) - 1), formats::Pickle);
}

#[test]
fn pickle_rejects_deep_nesting() {
    // 3 bytes per nesting level used to build a 200k-deep tree and overflow
    // the stack when replayed; must now error instead.
    let mut bytes = vec![0x80, 0x02];
    for _ in 0..400 {
        bytes.extend_from_slice(&[0x5d, 0x28]); // EMPTY_LIST, MARK
    }
    bytes.extend_from_slice(&[0x65, 0x2e]); // APPENDS, STOP
    assert!(
        formats::Pickle.decode::<nextjson::Value>(&bytes).is_err(),
        "deeply nested pickle must be rejected"
    );
}

#[test]
fn ron_rejects_deep_some_nesting() {
    // `Some(Some(...))` recursion used to be unbounded at the token layer.
    let mut input = String::new();
    for _ in 0..400 {
        input.push_str("Some(");
    }
    input.push('1');
    for _ in 0..400 {
        input.push(')');
    }
    assert!(
        formats::Ron
            .decode::<nextjson::Value>(input.as_bytes())
            .is_err(),
        "deeply nested Some must be rejected"
    );
}

#[test]
fn json5_negative_hex_preserves_sign() {
    let value: nextjson::Value = formats::Json5.decode(b"{a: -0x1F, b: +0x10}").unwrap();
    assert_eq!(value["a"], nextjson::Value::from(-31_i64));
    assert_eq!(value["b"], nextjson::Value::from(16_i64));
}

#[test]
fn csv_scalar_and_utf8_decode() {
    // A bare scalar used to fail with "unconsumed rows" even after reading it.
    let v: i32 = formats::Csv.decode(b"42").unwrap();
    assert_eq!(v, 42);
    let s: String = formats::Csv.decode("héllo".as_bytes()).unwrap();
    assert_eq!(s, "héllo");
    // Multi-byte fields round-trip byte-for-byte.
    let rows = vec![vec!["café".to_string(), "✓".to_string()]];
    let bytes = formats::Csv.encode(&rows).unwrap();
    let back: Vec<Vec<String>> = formats::Csv.decode(&bytes).unwrap();
    assert_eq!(back, rows);
}

#[test]
fn hjson_inline_comment_in_unquoted_value() {
    // `#` starts a comment to the end of the line, so the closing `}` must
    // live on its own line.
    let value: nextjson::Value = formats::Hjson.decode(b"{ a: hello # comment\n}\n").unwrap();
    assert_eq!(value["a"], nextjson::Value::from("hello"));
}

#[test]
fn yaml_quoted_scalar_keeps_hash() {
    // `#` inside quotes must not be stripped as a comment.
    let value: nextjson::Value = formats::Yaml.decode(br#"a: "hello # world""#).unwrap();
    assert_eq!(value["a"], nextjson::Value::from("hello # world"));
}

#[test]
fn yaml_dash_mapping_key_not_swallowed() {
    // An indented `---: x` is a legal key; it must not be skipped.
    let value: nextjson::Value = formats::Yaml.decode(b"a:\n  ---: x\n  b: y\n").unwrap();
    assert_eq!(value["a"]["---"], nextjson::Value::from("x"));
    assert_eq!(value["a"]["b"], nextjson::Value::from("y"));
}

#[test]
fn yaml_sequence_item_nested_block() {
    // `- name: x` followed by an indented `details:` block must not flatten.
    let input = b"- name: x\n  details:\n    a: 1\n- name: y\n";
    let value: nextjson::Value = formats::Yaml.decode(&input[..]).unwrap();
    assert_eq!(value[0]["name"], nextjson::Value::from("x"));
    assert_eq!(value[0]["details"]["a"], nextjson::Value::from(1_i64));
    assert_eq!(value[1]["name"], nextjson::Value::from("y"));
}

#[test]
fn yaml_block_scalar_literal() {
    let input = b"text: |\n  line one\n  line two\nnext: 1\n";
    let value: nextjson::Value = formats::Yaml.decode(&input[..]).unwrap();
    assert_eq!(value["text"], nextjson::Value::from("line one\nline two\n"));
    assert_eq!(value["next"], nextjson::Value::from(1_i64));
}

#[test]
fn yaml_block_scalar_chomping() {
    let strip: nextjson::Value = formats::Yaml.decode(b"t: |-\n  a\n  b\n").unwrap();
    assert_eq!(strip["t"], nextjson::Value::from("a\nb"));
    let keep: nextjson::Value = formats::Yaml.decode(b"t: |+\n  a\n\n\n").unwrap();
    assert_eq!(keep["t"], nextjson::Value::from("a\n\n\n"));
    let clip: nextjson::Value = formats::Yaml.decode(b"t: |\n  a\n\n\n").unwrap();
    assert_eq!(clip["t"], nextjson::Value::from("a\n"));
}

#[test]
fn yaml_block_scalar_folded() {
    let value: nextjson::Value = formats::Yaml
        .decode(b"t: >\n  folded\n  text\n  next\n")
        .unwrap();
    assert_eq!(value["t"], nextjson::Value::from("folded text next\n"));
}

#[test]
fn yaml_block_scalar_in_sequence() {
    let value: nextjson::Value = formats::Yaml.decode(b"- |\n  first\n- second\n").unwrap();
    assert_eq!(value[0], nextjson::Value::from("first\n"));
    assert_eq!(value[1], nextjson::Value::from("second"));
}

#[test]
fn yaml_block_scalar_indent_indicator() {
    // `|2` strips exactly two spaces of indentation from every content line.
    let input = b"t: |2\n    four spaces\n  two\n";
    let value: nextjson::Value = formats::Yaml.decode(input).unwrap();
    assert_eq!(value["t"], nextjson::Value::from("  four spaces\ntwo\n"));
}

#[test]
fn yaml_block_scalar_header_comment() {
    let input = b"t: | # comment\n  content\n";
    let value: nextjson::Value = formats::Yaml.decode(input).unwrap();
    assert_eq!(value["t"], nextjson::Value::from("content\n"));
}

#[test]
fn yaml_anchors_and_aliases() {
    let input = b"base: &b\n  x: 1\n  y: two\ncopy: *b\n";
    let value: nextjson::Value = formats::Yaml.decode(&input[..]).unwrap();
    assert_eq!(value["base"], value["copy"]);
    assert_eq!(value["copy"]["x"], nextjson::Value::from(1_i64));
    assert_eq!(value["copy"]["y"], nextjson::Value::from("two"));
}

#[test]
fn yaml_alias_scalar_and_sequence() {
    let input = b"a: &n 42\nb: *n\nlist:\n  - &s hello\n  - *s\n";
    let value: nextjson::Value = formats::Yaml.decode(input).unwrap();
    assert_eq!(value["a"], nextjson::Value::from(42_i64));
    assert_eq!(value["b"], nextjson::Value::from(42_i64));
    assert_eq!(value["list"][0], nextjson::Value::from("hello"));
    assert_eq!(value["list"][1], nextjson::Value::from("hello"));
}

#[test]
fn yaml_anchor_on_block_scalar() {
    let input = b"a: &t |\n  line\nb: *t\n";
    let value: nextjson::Value = formats::Yaml.decode(input).unwrap();
    assert_eq!(value["a"], nextjson::Value::from("line\n"));
    assert_eq!(value["b"], nextjson::Value::from("line\n"));
}

#[test]
fn yaml_unknown_alias_errors() {
    assert!(formats::Yaml
        .decode::<nextjson::Value>(b"a: *missing\n")
        .is_err());
}

#[test]
fn yaml_multi_document_rejected() {
    let input = b"a: 1\n---\nb: 2\n";
    assert!(formats::Yaml.decode::<nextjson::Value>(&input[..]).is_err());
}

#[test]
fn yaml_document_end_marker_accepted() {
    let input = b"a: 1\n...\n";
    let value: nextjson::Value = formats::Yaml.decode(&input[..]).unwrap();
    assert_eq!(value["a"], nextjson::Value::from(1_i64));
}

#[test]
fn yaml_standard_tags_force_types() {
    let input = b"
s: !!str 123
i: !!int \"42\"
f: !!float 2.5
t: !!bool true
n: !!null anything
";
    let value: nextjson::Value = formats::Yaml.decode(input).unwrap();
    assert_eq!(value["s"], nextjson::Value::from("123"));
    assert_eq!(value["i"], nextjson::Value::from(42_i64));
    assert_eq!(value["f"], nextjson::Value::from(2.5_f64));
    assert_eq!(value["t"], nextjson::Value::from(true));
    assert_eq!(value["n"], nextjson::Value::Null);
}

#[test]
fn yaml_unsupported_tag_is_error() {
    assert!(formats::Yaml
        .decode::<nextjson::Value>(b"x: !custom value\n")
        .is_err());
}

#[test]
fn yaml_merge_key() {
    let input = b"
defaults: &defaults
  host: localhost
  port: 8080
server:
  <<: *defaults
  port: 9000
";
    let value: nextjson::Value = formats::Yaml.decode(input).unwrap();
    // `<<` merges the anchor mapping; explicit keys win.
    assert_eq!(value["server"]["host"], nextjson::Value::from("localhost"));
    assert_eq!(value["server"]["port"], nextjson::Value::from(9000_i64));
}

#[test]
fn yaml_merge_must_be_mapping() {
    assert!(formats::Yaml
        .decode::<nextjson::Value>(b"a:\n  <<: 42\n")
        .is_err());
}

#[test]
fn yaml_non_finite_floats_rejected() {
    for bad in [".inf", "-.inf", ".nan"] {
        let input = format!("x: {bad}\n");
        assert!(
            formats::Yaml
                .decode::<nextjson::Value>(input.as_bytes())
                .is_err(),
            "accepted non-finite {bad}"
        );
    }
}

//! Round trip for fully scalar type structures: covers the width-based methods of various format encoders.
//!
//! `Value` only accesses `write_u64/i64/f64/str/bool/null`, and cannot reach `write_u8`,
//! `write_i16`, `write_char`, `write_f32`, etc., width branches; structures with fully scalar type
//! Round trip can cover these branches for all formats in one go.

use nextjson::formats::Format;
use nextjson::{NsonDeserialize, NsonSerialize};

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct IntBag {
    u8_: u8,
    u16_: u16,
    u32_: u32,
    u64_: u64,
    u128_: u128,
    i8_: i8,
    i16_: i16,
    i32_: i32,
    i64_: i64,
    i128_: i128,
    s: String,
    seq: Vec<i32>,
    map: std::collections::BTreeMap<String, u16>,
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct RichBag {
    b: bool,
    f32_: f32,
    f64_: f64,
    c: char,
    o: Option<i64>,
    inner: IntBag,
}

fn int_bag() -> IntBag {
    IntBag {
        u8_: 1,
        u16_: 2,
        u32_: 3,
        u64_: 4,
        u128_: 5,
        i8_: -1,
        i16_: -2,
        i32_: -3,
        i64_: -4,
        i128_: -5,
        s: "text".into(),
        seq: vec![1, 2, 3],
        map: std::collections::BTreeMap::from([("k".to_string(), 7)]),
    }
}

fn rich_bag() -> RichBag {
    RichBag {
        b: true,
        f32_: 1.5,
        f64_: -2.25,
        c: 'x',
        o: Some(9),
        inner: int_bag(),
    }
}

/// Full data model formats: Integer width, bool, floating point, char, Option,
/// And nested structures are all supported
#[test]
fn full_model_typed_roundtrip() {
    use nextjson::formats::{Cbor, Hjson, Json, Json5, MsgPack, Pickle, Ron, Sexpr, Yaml};
    let v = rich_bag();
    type Codec = (&'static str, fn(&RichBag) -> Vec<u8>, fn(&[u8]) -> RichBag);
    let fns: &[Codec] = &[
        (
            "json",
            |v| Json.encode(v).unwrap(),
            |b| Json.decode(b).unwrap(),
        ),
        (
            "json5",
            |v| Json5.encode(v).unwrap(),
            |b| Json5.decode(b).unwrap(),
        ),
        (
            "hjson",
            |v| Hjson.encode(v).unwrap(),
            |b| Hjson.decode(b).unwrap(),
        ),
        (
            "yaml",
            |v| Yaml.encode(v).unwrap(),
            |b| Yaml.decode(b).unwrap(),
        ),
        (
            "ron",
            |v| Ron.encode(v).unwrap(),
            |b| Ron.decode(b).unwrap(),
        ),
        (
            "sexpr",
            |v| Sexpr.encode(v).unwrap(),
            |b| Sexpr.decode(b).unwrap(),
        ),
        (
            "cbor",
            |v| Cbor.encode(v).unwrap(),
            |b| Cbor.decode(b).unwrap(),
        ),
        (
            "msgpack",
            |v| MsgPack.encode(v).unwrap(),
            |b| MsgPack.decode(b).unwrap(),
        ),
        (
            "pickle",
            |v| Pickle.encode(v).unwrap(),
            |b| Pickle.decode(b).unwrap(),
        ),
    ];
    for (name, enc, dec) in fns {
        let bytes = enc(&v);
        let back = dec(&bytes);
        assert_eq!(back, v, "roundtrip failed for {name}");
    }
}

/// Document format (toml/bson): The root of the structure is the document, and the scalar width is also traversed once
#[test]
fn document_shaped_typed_roundtrip() {
    use nextjson::formats::{Bson, Toml};
    let v = rich_bag();
    let t = Toml.encode(&v).unwrap();
    assert_eq!(Toml.decode::<RichBag>(&t).unwrap(), v);
    let b = Bson.encode(&v).unwrap();
    assert_eq!(Bson.decode::<RichBag>(&b).unwrap(), v);
}

/// bencode: None (bool/float/char/null); uses a set of integers to validate the width encoding
#[test]
fn bencode_typed_roundtrip() {
    use nextjson::formats::Bencode;
    let v = int_bag();
    let bytes = Bencode.encode(&v).unwrap();
    assert_eq!(Bencode.decode::<IntBag>(&bytes).unwrap(), v);
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct UnsignedBag {
    u8_: u8,
    u16_: u16,
    u32_: u32,
    u64_: u64,
    u128_: u128,
    s: String,
    seq: Vec<u32>,
    map: std::collections::BTreeMap<String, u16>,
}

/// Postcard: Not self-describing, rejects signed scalars; uses unsigned sets for wire routing
#[test]
fn postcard_typed_roundtrip() {
    use nextjson::formats::Postcard;
    let v = UnsignedBag {
        u8_: 1,
        u16_: 2,
        u32_: 3,
        u64_: 4,
        u128_: 5,
        s: "text".into(),
        seq: vec![1, 2, 3],
        map: std::collections::BTreeMap::from([("k".to_string(), 7)]),
    };
    let bytes = Postcard.encode(&v).unwrap();
    assert_eq!(Postcard.decode::<UnsignedBag>(&bytes).unwrap(), v);
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq, Clone)]
struct FlatRow {
    a: u32,
    b: i64,
    c: f64,
    d: bool,
    e: String,
}

/// csv / urlform: A row-oriented text format that can only contain scalars (nested containers cannot be represented)
#[test]
fn row_formats_typed_roundtrip() {
    use nextjson::formats::{Csv, UrlForm};
    let rows = vec![
        FlatRow {
            a: 1,
            b: -2,
            c: 3.5,
            d: true,
            e: "x".into(),
        },
        FlatRow {
            a: 2,
            b: 0,
            c: -1.25,
            d: false,
            e: "y".into(),
        },
    ];
    let c = Csv.encode(&rows).unwrap();
    assert_eq!(Csv.decode::<Vec<FlatRow>>(&c).unwrap(), rows);

    let row = rows[0].clone();
    let u = UrlForm.encode(&row).unwrap();
    assert_eq!(UrlForm.decode::<FlatRow>(&u).unwrap(), row);
}

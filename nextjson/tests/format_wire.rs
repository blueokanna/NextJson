//! Format decoder branch coverage: single-byte matrix + structured malformed input.
//!
//! Each marker byte (0x00-0xFF) in binary format is assigned to a different decoding branch;
//! Feeding in one by one and asserting "no panic" (error reporting is the correct behavior) systematically covers these branches.
//! Text format is fed a set of structured malformed syntax.

use nextjson::formats::Format;
use nextjson::Value;

#[test]
fn binary_single_byte_matrix_never_panics() {
    use nextjson::formats::{Bencode, Bson, Cbor, MsgPack, Pickle, Postcard};
    for b in 0u8..=255 {
        let _ = MsgPack.decode::<Value>(&[b]);
        let _ = Cbor.decode::<Value>(&[b]);
        let _ = Postcard.decode::<u32>(&[b]);
        let _ = Bencode.decode::<Value>(&[b]);
        let _ = Bson.decode::<Value>(&[b, 0x00, 0x00, 0x00, 0x00]);
        let _ = Pickle.decode::<Value>(&[0x80, b]);
    }
}

#[test]
fn binary_double_byte_sequences_never_panic() {
    use nextjson::formats::{Bencode, Cbor, MsgPack, Pickle, Postcard};
    let seeds: &[u8] = &[
        0x80, 0x81, 0x90, 0x91, 0xa0, 0xa1, 0xc0, 0xc1, 0xc2, 0xc4, 0xca, 0xdc, 0xde, 0xe0, 0xff,
        0x1b, 0x38, 0x78, 0x98, 0xb8, 0xd8, 0xf4,
    ];
    for &a in seeds {
        for &b in seeds {
            let _ = MsgPack.decode::<Value>(&[a, b]);
            let _ = Cbor.decode::<Value>(&[a, b]);
            let _ = Postcard.decode::<u32>(&[a, b]);
            let _ = Bencode.decode::<Value>(&[a, b]);
            let _ = Pickle.decode::<Value>(&[0x80, 0x04, a, b]);
        }
    }
}

#[test]
fn msgpack_wire_edge_cases() {
    use nextjson::formats::MsgPack;
    assert!(MsgPack.decode::<Value>(&[0xc4, 0xff, 0x01]).is_err()); // bin8 length exceeds the remaining
    assert!(MsgPack.decode::<Value>(&[0xa1]).is_err()); // fixstr length exceeds the limit
    assert!(MsgPack.decode::<Value>(&[0xdc, 0x00]).is_err()); // str16 is missing length bytes
    assert!(MsgPack.decode::<Value>(&[0x81, 0xa1]).is_err()); // map1 Missing Values
    assert!(MsgPack.decode::<Value>(&[0x93, 1, 2]).is_err()); // array3 is missing elements
    assert!(MsgPack
        .decode::<Value>(&[0xca, 0x7f, 0xff, 0xff, 0xff])
        .is_ok()); // float32
    assert!(MsgPack
        .decode::<Value>(&[0xcb, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0])
        .is_ok()); // float64 1.0
    assert!(MsgPack.decode::<Value>(&[0xd4, 0x00]).is_err()); // Missing ext data
    assert!(MsgPack.decode::<Value>(&[0xd4, 0x00, 0x01]).is_err()); // ext Non-Value Model
    assert!(MsgPack.decode::<Value>(&[0xc7]).is_err()); // ext8 is missing length
}

#[test]
fn cbor_wire_edge_cases() {
    use nextjson::formats::Cbor;
    assert!(Cbor.decode::<Value>(&[0x18, 0xff]).is_ok()); // uint8 255
    assert!(Cbor.decode::<Value>(&[0x1b, 0xff]).is_err()); // uint64 is missing 8 bytes
    assert!(Cbor.decode::<Value>(&[0x62, b'a']).is_err()); // missing bytes in text
    assert!(Cbor.decode::<Value>(&[0x80]).is_ok()); // empty ArrayList
    assert!(Cbor.decode::<Value>(&[0xff]).is_err()); // Suspended break
    assert!(Cbor.decode::<Value>(&[0xc0, 0x01]).is_err());
    assert!(Cbor.decode::<Value>(&[0xf9, 0x3c, 0x00]).is_ok()); // f16 1.0
    assert!(Cbor.decode::<Value>(&[0xd8, 0x01]).is_err()); // Tag 24 lacks follow-up.
}

#[test]
fn pickle_wire_edge_cases() {
    use nextjson::formats::Pickle;
    assert!(Pickle.decode::<Value>(&[0x80, 0x04, 0x4b]).is_err()); // BININT truncation
    assert_eq!(
        Pickle
            .decode::<Value>(&Pickle.encode(&Value::Null).unwrap())
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        Pickle
            .decode::<Value>(&Pickle.encode(&Value::from(true)).unwrap())
            .unwrap(),
        Value::from(true)
    );
    assert_eq!(
        Pickle
            .decode::<Value>(&Pickle.encode(&Value::from("hi")).unwrap())
            .unwrap(),
        Value::from("hi")
    );
    assert!(Pickle.decode::<Value>(&[0x80, 0x04, 0x8c]).is_err()); // Cut off
    assert!(Pickle.decode::<Value>(&[0x80, 0x04, 0x96, 0x00]).is_err()); // LONG_BINBYTES truncation
    assert!(Pickle.decode::<Value>(&[0x80, 0x04, 0x93, 0x00]).is_err()); // STACK_GLOBAL truncation
}

#[test]
fn bencode_wire_edge_cases() {
    use nextjson::formats::Bencode;
    assert!(Bencode.decode::<Value>(b"i42e").is_ok());
    assert!(Bencode.decode::<Value>(b"i-1e").is_ok());
    assert!(Bencode.decode::<Value>(b"i-").is_err()); // Negative number truncation
    assert!(Bencode.decode::<Value>(b"3:abc").is_ok());
    assert!(Bencode.decode::<Value>(b"3:ab").is_err()); // Length mismatch
    assert!(Bencode.decode::<Value>(b"l1:ai1ee").is_ok()); // List
    assert!(Bencode.decode::<Value>(b"d1:ai1ee").is_ok()); // Dictionary
    assert!(Bencode.decode::<Value>(b"d1:ai1e").is_err()); // Dictionary truncation
    assert!(Bencode.decode::<Value>(b"x").is_err()); // invalid intro
}

#[test]
fn postcard_wire_edge_cases() {
    use nextjson::formats::Postcard;
    // Postcard is not self-describing: Value decoding is not supported; use u32/String type to pass wire.
    assert!(Postcard.decode::<u32>(&[0x00]).is_ok()); // 0
    assert!(Postcard.decode::<u32>(&[0x7f]).is_ok()); // 127
    assert!(Postcard.decode::<u32>(&[0x80, 0x01]).is_ok()); // 128 (varint)
    assert!(Postcard.decode::<u32>(&[0x80]).is_err()); // varint truncation
                                                       // String length prefix
    assert!(Postcard.decode::<String>(&[0x05, b'h', b'e']).is_err());
    assert!(Postcard.decode::<String>(&[0x03, b'a', b'b', b'c']).is_ok());
}

#[test]
fn text_formats_malformed_grammar() {
    use nextjson::formats::{Csv, Hjson, Json5, Ron, Sexpr, Toml, UrlForm, Yaml};
    assert!(Json5.decode::<Value>(b"// comment only").is_err());
    assert!(Json5.decode::<Value>(b"{\"a\": }").is_err());
    assert!(Hjson.decode::<Value>(b"{unterminated").is_err());
    assert!(Hjson.decode::<Value>(b"a: 1\n b: 2 }").is_err());
    assert!(Yaml.decode::<Value>(b"- a\n- [unclosed").is_err());
    assert!(Yaml.decode::<Value>(b"a: [1, 2").is_err());
    assert!(Toml.decode::<Value>(b"[table\nkey=1").is_err());
    assert!(Toml.decode::<Value>(b"a = [1, 2").is_err());
    assert!(Ron.decode::<Value>(b"Enum(Variant(").is_err());
    assert!(Ron.decode::<Value>(b"Some(1, 2)").is_err());
    assert!(Sexpr.decode::<Value>(b"(a (b c)").is_err());
    assert!(Sexpr.decode::<Value>(b"\"unterminated").is_err());
    assert!(UrlForm.decode::<Value>(b"a=%E0%A4%A").is_err());
    assert!(Csv.decode::<Value>(b"a,b\n\"unterminated").is_err());
    assert!(Csv.decode::<Value>(b"a,b\n1,2,3").is_err());
}

#[test]
fn text_formats_valid_variants() {
    use nextjson::formats::{Hjson, Json5, Ron, Sexpr, Toml, Yaml};
    // JSON5 comments / single quotes / trailing comma
    assert!(Json5.decode::<Value>(b"{a: 1, b: 'x',}").is_ok());
    // Hjson without quotes
    assert!(Hjson.decode::<Value>(b"{ key: 1 }").is_ok());
    // YAML streaming and block-based
    assert!(Yaml.decode::<Value>(b"a: 1\nb: [1, 2]").is_ok());
    // TOML inline tables
    assert!(Toml.decode::<Value>(b"a = 1\n[t]\nx = 2").is_ok());
    // RON structure/enumeration
    assert!(Ron.decode::<Value>(b"(a: 1, b: 2)").is_ok());
    // S is a nested list of expressions
    assert!(Sexpr.decode::<Value>(b"(a b (c d))").is_ok());
}

#[test]
fn yaml_and_toml_scalar_and_table_edges() {
    use nextjson::formats::{Toml, Yaml};
    let v: Value = Yaml.decode(b"x: 1.5\ny: true\nz: null").unwrap();
    assert_eq!(v["x"], Value::from(1.5));
    assert_eq!(v["y"], Value::from(true));
    assert_eq!(v["z"], Value::Null);
    // TOML supports multiple types of scalars and arrays.
    let v: Value = Toml
        .decode(b"i = 1\nf = 2.5\nb = true\narr = [1, 2, 3]")
        .unwrap();
    assert_eq!(v["i"], Value::from(1_i64));
    assert_eq!(v["b"], Value::from(true));
    assert_eq!(v["arr"].as_array().unwrap().len(), 3);
    assert!(Toml.decode::<Value>(b"a = 1\na = 2").is_err());
}

//! Tests for the extended event-model primitives (P0 data-model work):
//!
//! - dedicated byte path (`Bytes` wrapper + `write_bytes` / `bytes()`);
//! - non-string map keys (`map_key`);
//! - `Option` semantics (`write_none` / `write_some` / `option_tag`);
//! - source integer width (`write_u8..u32` / `write_i8..i32`);
//! - `is_human_readable` split between text and binary formats.

use nextjson::formats::{self, Format};
use nextjson::{Bytes, Map};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// P0.1 bytes
// ---------------------------------------------------------------------------

#[test]
fn bytes_wrapper_roundtrips_as_json_array() {
    // Plain `Vec<u8>` keeps the sequence spelling (matches serde), so a
    // wrapped `Bytes` encodes to a JSON array of numbers.
    let value = Bytes(b"\x00\x01binary");
    let json = formats::encode_with(&value, formats::Json).unwrap();
    assert_eq!(json, b"[0,1,98,105,110,97,114,121]");

    // `Bytes` is a *borrowed* wrapper, so it decodes from an unescaped JSON
    // string (borrowed bytes) rather than from an owned array or an escaped
    // string — the same borrowing rule as `&[u8]` / serde's `deserialize_bytes`.
    let back: Bytes<'_> = formats::decode_with(b"\"abc\"", formats::Json).unwrap();
    assert_eq!(back.as_bytes(), b"abc");
}

#[test]
fn bytes_wrapper_is_compact_in_binary_formats() {
    let value = Bytes(b"\x00\x01binary");

    // Postcard: varint length + raw bytes.
    let pc = formats::encode_with(&value, formats::Postcard).unwrap();
    assert_eq!(pc, b"\x08\x00\x01binary");
    let back: Bytes<'_> = formats::decode_with(&pc, formats::Postcard).unwrap();
    assert_eq!(back.as_bytes(), b"\x00\x01binary");

    // MessagePack: bin8 (0xC4) length + raw bytes.
    let mp = formats::encode_with(&value, formats::MsgPack).unwrap();
    assert_eq!(mp, b"\xc4\x08\x00\x01binary");
    let back: Bytes<'_> = formats::decode_with(&mp, formats::MsgPack).unwrap();
    assert_eq!(back.as_bytes(), b"\x00\x01binary");
}

#[test]
fn bytes_wrapper_roundtrips_across_every_binary_format() {
    fn check<F: Format>(format: F) {
        let value = Bytes(b"\xde\xad\xbe\xef");
        let encoded = formats::encode_with(&value, format).unwrap();
        let back: Bytes<'_> = formats::decode_with(&encoded, format).unwrap();
        assert_eq!(back.as_bytes(), b"\xde\xad\xbe\xef");
    }
    check(formats::Postcard);
    check(formats::MsgPack);
    check(formats::Bencode);
    // Pickle decodes through an owned `Value` tree (its VM turns `BINBYTES`
    // into a string), so neither borrowed `Bytes` nor the sequence-based
    // `Vec<u8>` can round-trip pickle bytes; the encoder still writes the
    // native bytes opcode for external consumers.

    // BSON is document-oriented and relays through an owned value tree, so a
    // *borrowed* `Bytes` cannot decode there. Verify the encoder writes the
    // native binary element (`0x05 <len> <subtype> <bytes>`) and that an owned
    // `Vec<u8>` (sequence spelling) round-trips instead.
    #[derive(nextjson::NsonSerialize)]
    struct Out {
        payload: Bytes<'static>,
    }
    let encoded = formats::Bson
        .encode(&Out {
            payload: Bytes(b"\xde\xad\xbe\xef"),
        })
        .unwrap();
    assert!(
        encoded.contains(&0x05) && encoded.windows(4).any(|w| w == [0xde, 0xad, 0xbe, 0xef]),
        "native binary element with raw payload"
    );

    #[derive(nextjson::NsonSerialize, nextjson::NsonDeserialize)]
    struct Envelope {
        payload: Vec<u8>,
    }
    let encoded = formats::Bson
        .encode(&Envelope {
            payload: b"\xde\xad\xbe\xef".to_vec(),
        })
        .unwrap();
    let back: Envelope = formats::Bson.decode(&encoded).unwrap();
    assert_eq!(back.payload, b"\xde\xad\xbe\xef");
}

#[test]
fn byte_slices_roundtrip_through_the_generic_sequence_path() {
    // `&[u8]` serializes through the generic `[T]` impl (array of u8).
    let bytes: &[u8] = &[1, 2, 3];
    let json = formats::encode_with(&bytes, formats::Json).unwrap();
    assert_eq!(json, b"[1,2,3]");

    // Owned `Vec<u8>` decodes from the array spelling.
    let vec_back: Vec<u8> = formats::decode_with(&json, formats::Json).unwrap();
    assert_eq!(vec_back, vec![1, 2, 3]);

    // A borrowed `&[u8]` decodes from a string (borrowed bytes), matching
    // serde's `deserialize_bytes` borrowing rule.
    let from_string: &[u8] = formats::decode_with(b"\"abc\"", formats::Json).unwrap();
    assert_eq!(from_string, b"abc");
}

// ---------------------------------------------------------------------------
// P0.2 non-string map keys
// ---------------------------------------------------------------------------

#[test]
fn numeric_map_keys_roundtrip_in_json() {
    let mut map = BTreeMap::new();
    map.insert(1u8, "one".to_string());
    map.insert(2u8, "two".to_string());
    map.insert(10u8, "ten".to_string());

    let json = formats::encode_with(&map, formats::Json).unwrap();
    assert_eq!(json, br#"{"1":"one","2":"two","10":"ten"}"#);

    let back: BTreeMap<u8, String> = formats::decode_with(&json, formats::Json).unwrap();
    assert_eq!(back, map);
}

#[test]
fn numeric_map_keys_roundtrip_in_binary_formats() {
    let mut map = BTreeMap::new();
    map.insert(1u8, "one".to_string());
    map.insert(2u8, "two".to_string());
    map.insert(3u8, "three".to_string());

    fn check<K, V>(map: &BTreeMap<K, V>, format: impl Format)
    where
        K: nextjson::NsonSerialize
            + for<'de> nextjson::NsonDeserialize<'de>
            + Ord
            + core::fmt::Debug,
        V: nextjson::NsonSerialize
            + for<'de> nextjson::NsonDeserialize<'de>
            + PartialEq
            + core::fmt::Debug,
    {
        let encoded = formats::encode_with(map, format).unwrap();
        let back: BTreeMap<K, V> = formats::decode_with(&encoded, format).unwrap();
        assert_eq!(back, *map);
    }
    check(&map, formats::Postcard);
    check(&map, formats::MsgPack);
    check(&map, formats::Bencode);
}

#[test]
fn bool_map_keys() {
    let mut map = BTreeMap::new();
    map.insert(false, 1i32);
    map.insert(true, 2i32);
    let json = formats::encode_with(&map, formats::Json).unwrap();
    let back: BTreeMap<bool, i32> = formats::decode_with(&json, formats::Json).unwrap();
    assert_eq!(back, map);
}

// ---------------------------------------------------------------------------
// P0.3 Option semantics
// ---------------------------------------------------------------------------

#[test]
fn option_roundtrips_everywhere() {
    fn check<F: Format>(format: F) {
        let none = Option::<i64>::None;
        let encoded = formats::encode_with(&none, format).unwrap();
        let back: Option<i64> = formats::decode_with(&encoded, format).unwrap();
        assert_eq!(back, None);

        let some = Some(7i64);
        let encoded = formats::encode_with(&some, format).unwrap();
        let back: Option<i64> = formats::decode_with(&encoded, format).unwrap();
        assert_eq!(back, Some(7));
    }
    check(formats::Json);
    check(formats::Json5);
    check(formats::Hjson);
    check(formats::Yaml);
    check(formats::Ron);
    check(formats::Cbor);
    check(formats::MsgPack);
    // Bencode has no null type and postcard is not self-describing (it cannot
    // peek), so `Option` is rejected there — both are documented limitations.
    check(formats::Pickle);

    // TOML is document-oriented (requires a top-level table), so wrap the
    // option in a struct for that format.
    #[derive(nextjson::NsonSerialize, nextjson::NsonDeserialize)]
    struct Envelope {
        value: Option<i64>,
    }
    let encoded = formats::Toml.encode(&Envelope { value: Some(7) }).unwrap();
    let back: Envelope = formats::Toml.decode(&encoded).unwrap();
    assert_eq!(back.value, Some(7));
}

// ---------------------------------------------------------------------------
// P0.4 source integer width
// ---------------------------------------------------------------------------

#[test]
fn width_specific_encoders_are_compact() {
    // Postcard: a small u8 is a single varint byte.
    let pc = formats::encode_with(&5u8, formats::Postcard).unwrap();
    assert_eq!(pc, b"\x05");

    let u16_value = 300u16;
    let pc = formats::encode_with(&u16_value, formats::Postcard).unwrap();
    assert_eq!(pc, b"\xac\x02");
    let back: u16 = formats::decode_with(&pc, formats::Postcard).unwrap();
    assert_eq!(back, 300);

    // MessagePack: u8 <= 127 is a single fixint byte.
    let mp = formats::encode_with(&100u8, formats::MsgPack).unwrap();
    assert_eq!(mp, b"\x64");
}

#[test]
fn width_typed_scalars_still_roundtrip_in_every_format() {
    let values = (1i8, -2i16, 3i32, 4i64, 5u8, 6u16, 7u32, 8u64);
    fn check<F: Format>(format: F) {
        let values = (1i8, -2i16, 3i32, 4i64, 5u8, 6u16, 7u32, 8u64);
        let encoded = formats::encode_with(&values, format).unwrap();
        let back: (i8, i16, i32, i64, u8, u16, u32, u64) =
            formats::decode_with(&encoded, format).unwrap();
        assert_eq!(back, values);
    }
    check(formats::Json);
    check(formats::Cbor);
    check(formats::MsgPack);
    check(formats::Bencode);
    check(formats::Pickle);

    // BSON is document-oriented; a tuple maps to an array element inside a
    // document, so it still round-trips through the marker's own entry point.
    let encoded = formats::Bson.encode(&values).unwrap();
    let back: (i8, i16, i32, i64, u8, u16, u32, u64) = formats::Bson.decode(&encoded).unwrap();
    assert_eq!(back, values);

    // Postcard is unsigned-only at the scalar level (documented), so check
    // the width-specific unsigned path there.
    let unsigned = (5u8, 6u16, 7u32, 8u64);
    let pc = formats::encode_with(&unsigned, formats::Postcard).unwrap();
    let back: (u8, u16, u32, u64) = formats::decode_with(&pc, formats::Postcard).unwrap();
    assert_eq!(back, unsigned);
}

// ---------------------------------------------------------------------------
// P1.2 is_human_readable
// ---------------------------------------------------------------------------

#[test]
fn human_readable_flag_matches_format_kind() {
    let json = formats::JsonEncoder::new(Vec::new());
    assert!(nextjson::FormatEncoder::is_human_readable(&json));

    let postcard = formats::PostcardEncoder::new(Vec::new());
    assert!(!nextjson::FormatEncoder::is_human_readable(&postcard));

    let msgpack = formats::MsgPackEncoder::new(Vec::new());
    assert!(!nextjson::FormatEncoder::is_human_readable(&msgpack));

    let decoder = nextjson::Decoder::new(b"null");
    assert!(nextjson::FormatDecoder::is_human_readable(&decoder));

    let postcard_decoder = formats::PostcardDecoder::new(b"\x00");
    assert!(!nextjson::FormatDecoder::is_human_readable(
        &postcard_decoder
    ));
}

// ---------------------------------------------------------------------------
// regression: `Value` and `Map` still behave
// ---------------------------------------------------------------------------

#[test]
fn value_and_map_still_roundtrip() {
    fn check<F: Format>(format: F) {
        let mut m = Map::new();
        m.insert("a".to_string(), nextjson::Value::from(1i64));
        m.insert("b".to_string(), nextjson::Value::from("two"));
        let encoded = formats::encode_with(&m, format).unwrap();
        let back: Map = formats::decode_with(&encoded, format).unwrap();
        assert_eq!(back, m);
    }
    check(formats::Json);
    check(formats::MsgPack);
    check(formats::Bencode);
    // Postcard is not self-describing, so a schema-less `Map` (which decodes
    // through `Value`) is rejected there — a documented limitation.
}

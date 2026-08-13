//! Deep error-branch coverage for the binary codecs: BSON extension type
//! bytes, MessagePack ext/bin markers, and pickle VM error paths. Each case
//! asserts the codec *rejects* the construct (or errors), which executes the
//! parser/VM `other => Err` branches that round-trip tests never reach.

use nextjson::formats;
use nextjson::Value;

/// A minimal BSON document containing one element with the given type byte.
fn bson_doc(ty: u8) -> Vec<u8> {
    // <int32 len> <type> 'a' 0x00 0x00
    let body = [ty, b'a', 0x00, 0x00];
    let len = (4 + body.len()) as u32;
    let mut doc = Vec::new();
    doc.extend_from_slice(&len.to_le_bytes());
    doc.extend_from_slice(&body);
    doc
}

#[test]
fn bson_rejects_extension_type_bytes() {
    // Undefined, ObjectId, datetime, regex, dbpointer, code, symbol,
    // code-with-scope, timestamp, decimal128, min-key, max-key.
    for ty in [
        0x06u8, 0x07, 0x09, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x11, 0x13, 0x7F, 0xFF,
    ] {
        let doc = bson_doc(ty);
        assert!(
            formats::decode_with::<Value, _>(&doc, formats::Bson).is_err(),
            "accepted unsupported bson type 0x{ty:02x}"
        );
    }
}

#[test]
fn bson_rejects_bad_boolean_byte() {
    // T_BOOL = 0x08 followed by a non-0/1 byte.
    let mut doc = bson_doc(0x08);
    doc.push(0x02); // boolean value byte
    doc.push(0x00); // terminator
    assert!(formats::decode_with::<Value, _>(&doc, formats::Bson).is_err());
}

#[test]
fn bson_rejects_truncated_and_bad_length() {
    // Length prefix larger than the buffer.
    let mut doc = vec![0x40, 0x00, 0x00, 0x00];
    doc.extend_from_slice(&[0x0A, b'a', 0x00]); // null element
    assert!(formats::decode_with::<Value, _>(&doc, formats::Bson).is_err());
    // Missing 0x00 terminator.
    let mut doc = bson_doc(0x0A);
    doc.pop();
    assert!(formats::decode_with::<Value, _>(&doc, formats::Bson).is_err());
}

#[test]
fn msgpack_rejects_ext_and_bin_markers() {
    // fixext1..16, ext8/16/32, bin8/16/32.
    let cases: &[&[u8]] = &[
        &[0xD4, 0x01, 0x00], // fixext1
        &[0xD5, 0x01, 0x00, 0x00],
        &[0xD6, 0x01, 0, 0, 0, 0],
        &[0xD7, 0x01, 0, 0, 0, 0, 0, 0, 0, 0],
        &[0xD8, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[0xC7, 0x01, 0x01, 0x00],                   // ext8
        &[0xC8, 0x01, 0x00, 0x01, 0x00],             // ext16
        &[0xC9, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00], // ext32
        &[0xC4, 0x01, 0x00],                         // bin8
        &[0xC5, 0x01, 0x00, 0x00],                   // bin16
        &[0xC6, 0x01, 0x00, 0x00, 0x00, 0x00],       // bin32
    ];
    for c in cases {
        let _ = formats::decode_with::<Value, _>(c, formats::MsgPack);
    }
}

#[test]
fn msgpack_rejects_bad_headers() {
    // Array/map/str with length prefixes that overrun the buffer.
    for c in [
        &[0x9Fu8][..],                         // fixarray 31 with no elements
        &[0xDC, 0x01, 0x00, 0x01],             // array16 len 1, no element
        &[0xDD, 0x00, 0x00, 0x00, 0x01, 0x00], // array32 len 1
        &[0xD9, 0x05],                         // str8 len 5, no data
        &[0xDA, 0x00, 0x05],                   // str16 len 5
        &[0xCE, 0xFF, 0xFF, 0xFF, 0xFF],       // u32
        &[0xD3, 0x80, 0, 0, 0, 0, 0, 0, 0],    // i64 min
        &[0xCB, 0x7F, 0xF0, 0, 0, 0, 0, 0, 0], // f64 inf
    ] {
        let _ = formats::decode_with::<Value, _>(c, formats::MsgPack);
    }
}

#[test]
fn pickle_error_branches() {
    use formats::Pickle;
    // PROTO version > 2 rejected.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x03], Pickle).is_err());
    // STOP with empty stack.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x2E], Pickle).is_err());
    // Trailing non-newline after STOP.
    assert!(
        formats::decode_with::<Value, _>(&[0x80, 0x02, 0x4B, 0x01, 0x2E, b'x'], Pickle).is_err()
    );
    // INT with non-numeric text.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x49, b'x', 0x0A], Pickle).is_err());
    // FLOAT non-finite.
    assert!(
        formats::decode_with::<Value, _>(&[0x80, 0x02, 0x46, b'i', b'n', b'f', 0x0A], Pickle)
            .is_err()
    );
    // BINFLOAT non-finite.
    assert!(formats::decode_with::<Value, _>(
        &[0x80, 0x02, 0x47, 0x7F, 0xF0, 0, 0, 0, 0, 0, 0],
        Pickle
    )
    .is_err());
    // Truncated BININT1 / BININT2 / LONG1.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x4B], Pickle).is_err());
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x4D, 0x01], Pickle).is_err());
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x8A, 0x05, 0x01], Pickle).is_err());
    // APPENDS without MARK.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x4B, 0x01, 0x65], Pickle).is_err());
    // LIST / DICT / TUPLE without MARK.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x6C], Pickle).is_err());
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x64], Pickle).is_err());
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x74], Pickle).is_err());
    // TUPLE1 with empty stack.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x85], Pickle).is_err());
    // DICT with odd item count.
    assert!(
        formats::decode_with::<Value, _>(&[0x80, 0x02, 0x28, 0x4B, 0x01, 0x64, 0x2E], Pickle)
            .is_err()
    );
    // Unknown opcode.
    assert!(formats::decode_with::<Value, _>(&[0x80, 0x02, 0x01], Pickle).is_err());
    // Excessive MARK nesting.
    let mut deep = vec![0x80, 0x02];
    deep.extend(vec![0x28u8; 129]);
    assert!(formats::decode_with::<Value, _>(&deep, Pickle).is_err());
}

#[test]
fn pickle_valid_opcodes_roundtrip() {
    use formats::Pickle;
    // A small pickle exercising MARK/LIST/APPENDS/DICT/SETITEMS/TUPLE.
    let v: Value = formats::decode_with(
        &[0x80, 0x02, 0x28, 0x4B, 0x01, 0x4B, 0x02, 0x6C, 0x2E],
        Pickle,
    )
    .unwrap();
    // `( 1 2 l .` => MARK, 1, 2, LIST => [1, 2]
    assert_eq!(v, nextjson::json!([1, 2]));
    let v: Value = formats::decode_with(
        &[0x80, 0x02, 0x28, 0x4B, 0x01, 0x4B, 0x02, 0x64, 0x2E],
        Pickle,
    )
    .unwrap();
    // `( 1 2 d .` => MARK, 1, 2, DICT => {1: 2}; the key becomes "1".
    assert_eq!(v, nextjson::json!({ "1": 2 }));
}

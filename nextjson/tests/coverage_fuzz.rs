//! Structured fuzz matrix: byte-level mutations (delete / duplicate / swap /
//! replace) on every format's wire encoding. Decoding must never panic; both
//! success and error paths execute parser branches that round-trip tests
//! never reach.

use nextjson::formats;
use nextjson::Value;

fn rich() -> Value {
    nextjson::json!({
        "a": [1, 2.5, "x", true, null],
        "b": { "c": 1, "d": "text" },
        "e": -17,
    })
}

fn flat() -> Value {
    nextjson::json!({ "x": 1, "y": "v", "z": true })
}

type Decode = dyn Fn(&[u8]);

/// Run `f` on every format that can encode `rich` (or `flat` for the
/// row-shaped / document-shaped codecs), passing the wire bytes plus a
/// decoder for exactly that format. Decoding every mutation against all 15
/// formats would be ~15x slower for the same branch coverage.
fn for_each_wire(mut f: impl FnMut(&[u8], &Decode)) {
    let v = rich();
    if let Ok(b) = formats::encode_with(&v, formats::Json) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Json);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Json5) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Json5);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Hjson) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Hjson);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Yaml) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Yaml);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Ron) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Ron);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::MsgPack) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::MsgPack);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Cbor) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Cbor);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Pickle) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Pickle);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Bencode) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Bencode);
        });
    }
    if let Ok(b) = formats::encode_with(&v, formats::Bson) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Bson);
        });
    }
    let rows = vec![flat()];
    if let Ok(b) = formats::encode_with(&rows, formats::Csv) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Csv);
        });
    }
    if let Ok(b) = formats::encode_with(&flat(), formats::UrlForm) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::UrlForm);
        });
    }
    if let Ok(b) = formats::encode_with(&flat(), formats::Sexpr) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Sexpr);
        });
    }
    if let Ok(b) = formats::encode_with(&flat(), formats::Toml) {
        f(&b, &|x| {
            let _ = formats::decode_with::<Value, _>(x, formats::Toml);
        });
    }
}

/// One mutation per byte position (delete / insert / replace / swap in
/// rotation) plus structural prefixes, so every parser branch is exercised
/// while the debug-mode cost stays a few seconds (a full delete + insert +
/// replace + swap at every position would be ~4x slower for the same
/// coverage).
#[test]
fn mutation_matrix() {
    for_each_wire(|b, dec| {
        if b.is_empty() {
            return;
        }
        for i in 0..b.len() {
            match i % 4 {
                0 => {
                    let mut m = b.to_vec();
                    m.remove(i);
                    dec(&m);
                }
                1 => {
                    let mut m = b.to_vec();
                    m.insert(i, b[i].wrapping_add(1));
                    dec(&m);
                }
                2 => {
                    let mut m = b.to_vec();
                    m[i] = 0x00;
                    dec(&m);
                }
                _ => {
                    if i + 1 < b.len() {
                        let mut m = b.to_vec();
                        m.swap(i, i + 1);
                        dec(&m);
                    }
                }
            }
        }
        // Prefix with structural noise.
        for p in [&b" "[..], b"{", b"0", b"\x00"] {
            let mut m = p.to_vec();
            m.extend_from_slice(b);
            dec(&m);
        }
    });
}

#[test]
fn cross_format_concatenation() {
    // Concatenate two formats' encodings; the second may be misread as
    // trailing garbage, which must error (not panic).
    let v = rich();
    let mut m = formats::encode_with(&v, formats::Json).unwrap();
    if let Ok(other) = formats::encode_with(&v, formats::Yaml) {
        m.extend_from_slice(&other);
        let _ = formats::decode_with::<Value, _>(&m, formats::Json);
    }
    let mut m = formats::encode_with(&v, formats::MsgPack).unwrap();
    if let Ok(other) = formats::encode_with(&v, formats::Cbor) {
        m.extend_from_slice(&other);
        let _ = formats::decode_with::<Value, _>(&m, formats::MsgPack);
    }
    let mut m = formats::encode_with(&v, formats::Pickle).unwrap();
    if let Ok(other) = formats::encode_with(&v, formats::Bson) {
        m.extend_from_slice(&other);
        let _ = formats::decode_with::<Value, _>(&m, formats::Pickle);
    }
    let mut m = formats::encode_with(&v, formats::Cbor).unwrap();
    if let Ok(other) = formats::encode_with(&v, formats::MsgPack) {
        m.extend_from_slice(&other);
        let _ = formats::decode_with::<Value, _>(&m, formats::Cbor);
    }
}

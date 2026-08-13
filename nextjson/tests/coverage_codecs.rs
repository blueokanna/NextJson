//! Truncation matrix: for every format, decoding every prefix of a valid
//! encoding must never panic (it returns an error, or the full value when the
//! cut lands exactly on a complete document). This exercises the parser EOF /
//! boundary branches that round-trip tests never reach.

use nextjson::formats::{self, Format};
use nextjson::{json, Value};

fn all_prefixes<F: Format>(fmt: F, bytes: &[u8]) {
    for cut in 0..=bytes.len() {
        // Never panic, whatever the result is.
        let _ = formats::decode_with::<Value, _>(&bytes[..cut], fmt);
    }
}

fn rich() -> Value {
    json!({
        "a": [1, 2.5, "x", true, null],
        "b": { "c": 1, "d": "text" },
        "e": -17,
    })
}

fn flat() -> Value {
    json!({ "x": 1, "y": "v", "z": true })
}

#[test]
fn truncation_matrix_text_formats() {
    let v = rich();
    let bytes = formats::encode_with(&v, formats::Json).unwrap();
    all_prefixes(formats::Json, &bytes);
    let bytes = formats::encode_with(&v, formats::Json5).unwrap();
    all_prefixes(formats::Json5, &bytes);
    let bytes = formats::encode_with(&v, formats::Hjson).unwrap();
    all_prefixes(formats::Hjson, &bytes);
    let bytes = formats::encode_with(&v, formats::Yaml).unwrap();
    all_prefixes(formats::Yaml, &bytes);
    let bytes = formats::encode_with(&v, formats::Ron).unwrap();
    all_prefixes(formats::Ron, &bytes);
    // row-shaped text formats need a flat map (csv wants an array of rows).
    let rows = vec![flat(), flat()];
    let bytes = formats::encode_with(&rows, formats::Csv).unwrap();
    all_prefixes(formats::Csv, &bytes);
    let bytes = formats::encode_with(&flat(), formats::UrlForm).unwrap();
    all_prefixes(formats::UrlForm, &bytes);
    // sexpr maps are emitted as alists; a flat map also round-trips.
    let bytes = formats::encode_with(&flat(), formats::Sexpr).unwrap();
    all_prefixes(formats::Sexpr, &bytes);
}

#[test]
fn truncation_matrix_binary_formats() {
    let v = rich();
    if let Ok(bytes) = formats::encode_with(&v, formats::MsgPack) {
        all_prefixes(formats::MsgPack, &bytes);
    }
    if let Ok(bytes) = formats::encode_with(&v, formats::Cbor) {
        all_prefixes(formats::Cbor, &bytes);
    }
    if let Ok(bytes) = formats::encode_with(&v, formats::Pickle) {
        all_prefixes(formats::Pickle, &bytes);
    }
    if let Ok(bytes) = formats::encode_with(&v, formats::Bencode) {
        all_prefixes(formats::Bencode, &bytes);
    }
    if let Ok(bytes) = formats::encode_with(&v, formats::Bson) {
        all_prefixes(formats::Bson, &bytes);
    }
}

#[test]
fn truncation_matrix_document_shaped() {
    let bytes = formats::encode_with(&flat(), formats::Toml).unwrap();
    all_prefixes(formats::Toml, &bytes);
}

// ---------------------------------------------------------------------------
// Byte-flip robustness: single-byte mutations must not panic.
// ---------------------------------------------------------------------------

/// Flip bytes at every position with three deltas; decoding must never panic.
macro_rules! flip_matrix {
    ($value:expr, $fmt:expr) => {{
        if let Ok(bytes) = formats::encode_with($value, $fmt) {
            for i in 0..bytes.len() {
                for &delta in &[1u8, 0x7F, 0xFF] {
                    let mut mutated = bytes.clone();
                    mutated[i] ^= delta;
                    let _ = formats::decode_with::<Value, _>(&mutated, $fmt);
                }
            }
        }
    }};
}

#[test]
fn single_byte_flips_do_not_panic() {
    let v = rich();
    flip_matrix!(&v, formats::Json);
    flip_matrix!(&v, formats::Yaml);
    flip_matrix!(&v, formats::Ron);
    flip_matrix!(&v, formats::MsgPack);
    flip_matrix!(&v, formats::Cbor);
    flip_matrix!(&v, formats::Pickle);
    flip_matrix!(&v, formats::Bson);
}

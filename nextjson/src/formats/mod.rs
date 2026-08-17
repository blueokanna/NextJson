//! Dependency-free multi-format codec engine.
//!
//! NextJson serializes and deserializes values through the format-neutral
//! [`crate::ser::FormatEncoder`] and [`crate::de::FormatDecoder`]
//! contracts. The same `NsonSerialize` / `NsonDeserialize` implementation can
//! be used with each codec whose documented wire model can represent the
//! value. Most encoders emit events directly; document-oriented codecs may
//! collect a [`Value`] before producing bytes.
//!
//! # Registry
//!
//! - Text, self-describing: `json`, `json5`, `hjson`, `yaml`, `toml`, `ron`,
//!   `sexpr`, `csv`, `urlform`, `ndjson`, `ini`, `edn`.
//! - Binary, self-describing: `cbor`, `msgpack`, `ubjson`, `smile`, `bson`,
//!   `bencode`, `pickle`.
//! - Binary, schema-light: `postcard`.
//! - Environment: `envy` (deserialization from process environment).
//!
//! The [`Format`] trait carries the name, MIME type, file extensions and the
//! generic `encode` / `decode` entry points, so formats are first-class
//! values that can be passed around, stored in the [`all`] registry, or
//! selected by [`detect`].
//!
//! # Unified typed API
//!
//! ```rust
//! use nextjson::formats;
//!
//! let value = ("NextJson", vec![1_u64, 2, 3], true);
//!
//! let json = formats::encode_with(&value, formats::Json)?;
//! let msgpack = formats::encode_with(&value, formats::MsgPack)?;
//! let yaml = formats::encode_with(&value, formats::Yaml)?;
//!
//! let back: (String, Vec<u64>, bool) = formats::decode_with(&json, formats::Json)?;
//! let back_mp: (String, Vec<u64>, bool) =
//!     formats::decode_with(&msgpack, formats::MsgPack)?;
//! let back_yaml: (String, Vec<u64>, bool) =
//!     formats::decode_with(&yaml, formats::Yaml)?;
//! assert_eq!(back, back_mp);
//! assert_eq!(back, back_yaml);
//! # Ok::<(), nextjson::Error>(())
//! ```
//!
//! # Format-to-format transcoding
//!
//! [`transcode`] converts between compatible formats without a typed value:
//!
//! ```rust
//! use nextjson::formats;
//! let json = br#"{"name":"NextJson","values":[1,2,3]}"#;
//! let msgpack = formats::transcode(json, formats::Json, formats::MsgPack)?;
//! let json2 = formats::transcode(&msgpack, formats::MsgPack, formats::Json)?;
//! assert_eq!(json2, json);
//! # Ok::<(), nextjson::Error>(())
//! ```

pub mod bin;

mod bencode;
mod bson;
mod cbor;
mod csv;
mod edn;
mod envy;
mod hjson;
mod ini;
mod json;
mod json5;
mod msgpack;
mod ndjson;
mod pickle;
mod postcard;
mod ron;
mod sexpr;
mod smile;
mod toml;
mod tree;
pub use self::tree::TreeDecoder;
mod ubjson;
mod urlform;
mod yaml;

use alloc::vec::Vec;

pub use self::bencode::Bencode;
pub use self::bencode::{BencodeDecoder, BencodeEncoder};
pub use self::bson::Bson;
pub use self::bson::{BsonDecoder, BsonEncoder};
pub use self::cbor::Cbor;
pub use self::cbor::{CborDecoder, CborEncoder};
pub use self::csv::Csv;
pub use self::csv::{CsvDecoder, CsvEncoder};
pub use self::edn::Edn;
pub use self::edn::{EdnDecoder, EdnEncoder};
pub use self::envy::Envy;
pub use self::envy::EnvyDecoder;
pub use self::hjson::Hjson;
pub use self::hjson::{HjsonDecoder, HjsonEncoder};
pub use self::ini::Ini;
pub use self::ini::{IniDecoder, IniEncoder};
pub use self::json::Json;
pub use self::json::{JsonDecoder, JsonEncoder};
pub use self::json5::Json5;
pub use self::json5::{Json5Decoder, Json5Encoder};
pub use self::msgpack::MsgPack;
pub use self::msgpack::{MsgPackDecoder, MsgPackEncoder};
pub use self::ndjson::Ndjson;
pub use self::ndjson::NdjsonDecoder;
pub use self::pickle::Pickle;
pub use self::pickle::{PickleDecoder, PickleEncoder};
pub use self::postcard::Postcard;
pub use self::postcard::{PostcardDecoder, PostcardEncoder};
pub use self::ron::Ron;
pub use self::ron::{RonDecoder, RonEncoder};
pub use self::sexpr::Sexpr;
pub use self::sexpr::{SexprDecoder, SexprEncoder};
pub use self::smile::Smile;
pub use self::smile::{SmileDecoder, SmileEncoder};
pub use self::toml::Toml;
pub use self::toml::{TomlDecoder, TomlEncoder};
pub use self::ubjson::Ubjson;
pub use self::ubjson::{UbjsonDecoder, UbjsonEncoder};
pub use self::urlform::UrlForm;
pub use self::urlform::{UrlFormDecoder, UrlFormEncoder};
pub use self::yaml::Yaml;
pub use self::yaml::{YamlDecoder, YamlEncoder};

use crate::error::Result;
use crate::value::Value;
use crate::{NsonDeserialize, NsonSerialize};

/// A registered data format with generic typed entry points.
///
/// Implementors are zero-sized marker types; a `Format` value can be passed by
/// copy, compared by name, or looked up in [`all`].
pub trait Format: Copy + 'static {
    /// Canonical lowercase name, e.g. `"msgpack"`.
    const NAME: &'static str;
    /// IANA-style MIME type, e.g. `"application/msgpack"`.
    const MIME: &'static str;
    /// Common file extensions (without the leading dot).
    const EXTENSIONS: &'static [&'static str];
    /// Whether the wire representation is binary (not UTF-8 text).
    const BINARY: bool;
    /// Encode any `NsonSerialize` value into this format's bytes.
    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>>;
    /// Decode any `NsonDeserialize` value from this format's bytes.
    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T>;
}

/// Serialize a value with an explicit format.
///
/// Equivalent to [`Format::encode`]; exists for call sites that prefer the
/// natural reading order.
pub fn encode_with<T: NsonSerialize + ?Sized, F: Format>(value: &T, format: F) -> Result<Vec<u8>> {
    format.encode(value)
}

/// Deserialize a value with an explicit format.
///
/// Equivalent to [`Format::decode`].
pub fn decode_with<'de, T: NsonDeserialize<'de>, F: Format>(
    input: &'de [u8],
    format: F,
) -> Result<T> {
    format.decode(input)
}

/// Decode one format's bytes into a [`Value`].
pub fn to_value<F: Format>(input: &[u8], format: F) -> Result<Value> {
    format.decode(input)
}

/// Convert between any two formats without a typed value.
///
/// The source is decoded into a [`Value`] and re-emitted through the
/// destination codec. Formats that share the relay protocol additionally
/// support streaming conversion through [`crate::cross_format`].
pub fn transcode<F: Format, G: Format>(input: &[u8], from: F, to: G) -> Result<Vec<u8>> {
    let value: Value = from.decode(input)?;
    to.encode(&value)
}

/// Metadata for one registered format.
#[derive(Clone, Copy, Debug)]
pub struct FormatInfo {
    /// The format marker type (zero-sized).
    pub kind: FormatKind,
    /// Canonical name, e.g. `"json"`.
    pub name: &'static str,
    /// MIME type.
    pub mime: &'static str,
    /// File extensions.
    pub extensions: &'static [&'static str],
    /// Whether the wire representation is binary.
    pub binary: bool,
}

/// Discriminant for every format registered in [`all`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum FormatKind {
    /// JSON (RFC 8259).
    Json,
    /// JSON5 (JSON superset with comments, unquoted keys, ...).
    Json5,
    /// Hjson (human-oriented JSON).
    Hjson,
    /// YAML (block and flow style subset).
    Yaml,
    /// TOML (v1.0 core).
    Toml,
    /// RON (Rusty Object Notation).
    Ron,
    /// S-expressions.
    Sexpr,
    /// RFC 4180 CSV.
    Csv,
    /// `application/x-www-form-urlencoded`.
    UrlForm,
    /// RFC 8949 CBOR.
    Cbor,
    /// MessagePack.
    MsgPack,
    /// Universal Binary JSON.
    Ubjson,
    /// Jackson Smile binary JSON.
    Smile,
    /// BSON (MongoDB documents).
    Bson,
    /// Bencode (BitTorrent).
    Bencode,
    /// Postcard (compact `no_std` binary).
    Postcard,
    /// Python Pickle (protocol 2 subset).
    Pickle,
    /// NDJSON / JSONL (newline-delimited JSON).
    Ndjson,
    /// INI configuration text.
    Ini,
    /// EDN (Clojure data).
    Edn,
    /// Environment variables (deserialization only).
    Envy,
}

/// All registered formats.
pub fn all() -> &'static [FormatInfo] {
    &[
        FormatInfo {
            kind: FormatKind::Json,
            name: "json",
            mime: "application/json",
            extensions: &["json"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Json5,
            name: "json5",
            mime: "application/json5",
            extensions: &["json5"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Hjson,
            name: "hjson",
            mime: "application/hjson",
            extensions: &["hjson"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Yaml,
            name: "yaml",
            mime: "application/yaml",
            extensions: &["yaml", "yml"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Toml,
            name: "toml",
            mime: "application/toml",
            extensions: &["toml"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Ron,
            name: "ron",
            mime: "text/ron",
            extensions: &["ron"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Sexpr,
            name: "sexpr",
            mime: "text/x-sexpr",
            extensions: &["sexp", "sx", "scm"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Csv,
            name: "csv",
            mime: "text/csv",
            extensions: &["csv"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::UrlForm,
            name: "urlform",
            mime: "application/x-www-form-urlencoded",
            extensions: &["form"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Cbor,
            name: "cbor",
            mime: "application/cbor",
            extensions: &["cbor"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::MsgPack,
            name: "msgpack",
            mime: "application/msgpack",
            extensions: &["msgpack", "mp"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::Ubjson,
            name: "ubjson",
            mime: "application/ubjson",
            extensions: &["ubj", "ubjson"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::Smile,
            name: "smile",
            mime: "application/x-jackson-smile",
            extensions: &["smile"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::Bson,
            name: "bson",
            mime: "application/bson",
            extensions: &["bson"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::Bencode,
            name: "bencode",
            mime: "application/x-bittorrent",
            extensions: &["torrent", "bencode"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::Postcard,
            name: "postcard",
            mime: "application/postcard",
            extensions: &["postcard"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::Pickle,
            name: "pickle",
            mime: "application/python-pickle",
            extensions: &["pkl", "pickle"],
            binary: true,
        },
        FormatInfo {
            kind: FormatKind::Ndjson,
            name: "ndjson",
            mime: "application/x-ndjson",
            extensions: &["ndjson", "jsonl"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Ini,
            name: "ini",
            mime: "text/plain",
            extensions: &["ini", "cfg", "conf"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Edn,
            name: "edn",
            mime: "application/edn",
            extensions: &["edn"],
            binary: false,
        },
        FormatInfo {
            kind: FormatKind::Envy,
            name: "envy",
            mime: "text/plain",
            extensions: &[],
            binary: false,
        },
    ]
}

/// Look up a format by canonical name (case-insensitive).
pub fn by_name(name: &str) -> Option<FormatKind> {
    all()
        .iter()
        .find(|info| info.name.eq_ignore_ascii_case(name))
        .map(|info| info.kind)
}

/// Look up a format by file extension (with or without a leading dot).
pub fn by_extension(ext: &str) -> Option<FormatKind> {
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    all()
        .iter()
        .find(|info| info.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
        .map(|info| info.kind)
}

/// Sniff the most likely format from a byte prefix.
///
/// Detection is heuristic and intentionally conservative: only formats with
/// strong structural signatures are reported. Ambiguous or unknown input
/// returns `None`; prefer an explicit format when the caller knows it.
pub fn detect(input: &[u8]) -> Option<FormatKind> {
    let first = *input.first()?;
    // Python Pickle: `\x80 <protocol 0..=5>`; the protocol byte disambiguates
    // from MessagePack fixmap/array.
    if (0x80..=0x85).contains(&first) && input.get(1).is_some_and(|b| *b <= 5) {
        return Some(FormatKind::Pickle);
    }
    // Bencode: `d`/`l`/`i` intro or a `digits:` byte-string length.
    if (matches!(first, b'd' | b'l' | b'i') || first.is_ascii_digit()) && bencode_like(input) {
        return Some(FormatKind::Bencode);
    }
    // BSON: little-endian int32 length that equals the whole input length.
    if input.len() >= 5 && plausible_bson_length(input) {
        return Some(FormatKind::Bson);
    }
    // SMILE: fixed 3-byte header `:)\n` (0x3A 0x29 0x0A).
    if input.len() >= 3 && input[..3] == [0x3A, 0x29, 0x0A] {
        return Some(FormatKind::Smile);
    }
    // UBJSON object/array: `{` is shared with JSON, but a UBJSON object
    // starts with an `S` string key marker, and `[` followed by `$` / `#`
    // is a typed/counted array (JSON never has those bytes).
    if (first == b'{' && input.get(1) == Some(&b'S'))
        || (first == b'[' && matches!(input.get(1), Some(&b'$') | Some(&b'#')))
    {
        return Some(FormatKind::Ubjson);
    }
    // Text formats first: their ASCII starts are far more specific than the
    // ambiguous binary fix encodings. A `---` document marker is YAML before
    // a bare `-` (JSON cannot start with a bare `-` followed by `--`).
    match first {
        b'-' if input.starts_with(b"---") => return Some(FormatKind::Yaml),
        b'{' | b'[' | b'"' | b'-' | b'+' | b'.' => return Some(FormatKind::Json),
        b't' | b'f' | b'n' => return Some(FormatKind::Json),
        b'(' => return Some(FormatKind::Sexpr),
        b'#' | b'=' => return Some(FormatKind::Toml),
        b'%' => return Some(FormatKind::UrlForm),
        _ => {}
    }
    // Binary formats. MessagePack fix containers / typed scalars start at
    // 0x80+; the positive fixint range 0x00..=0x7F is indistinguishable from
    // text and is intentionally not claimed.
    if is_msgpack_signature(first) {
        return Some(FormatKind::MsgPack);
    }
    // CBOR extended-length (additional info 24..=27) major types and tags.
    if is_cbor_signature(first) {
        return Some(FormatKind::Cbor);
    }
    None
}

fn plausible_bson_length(input: &[u8]) -> bool {
    if input.len() < 5 {
        return false;
    }
    let Ok(len) = usize::try_from(u32::from_le_bytes([input[0], input[1], input[2], input[3]]))
    else {
        return false;
    };
    len == input.len() && matches!(input[4], 0x01..=0x13 | 0xFF)
}

fn bencode_like(input: &[u8]) -> bool {
    // `digits:` prefix (byte string) or `i`, `d`, `l` intro followed by valid
    // bencode content.
    let mut i = 0;
    if matches!(input[0], b'd' | b'l' | b'i') {
        return true;
    }
    while i < input.len() && input[i].is_ascii_digit() {
        i += 1;
    }
    i < input.len() && input[i] == b':'
}

fn is_cbor_signature(b: u8) -> bool {
    // Extended-length (additional info 24..=27) major-type encodings and the
    // tag major that MessagePack cannot represent as a fix type. Compact
    // (info < 24) prefixes are claimed by MessagePack in [`detect`].
    matches!(
        b,
        0x18..=0x1B
            | 0x38..=0x3B
            | 0x58..=0x5B
            | 0x78..=0x7B
            | 0x98..=0x9B
            | 0xB8..=0xBB
            | 0xC0..=0xDB
            | 0xF4..=0xF7
            | 0xF9..=0xFB
    )
}

fn is_msgpack_signature(b: u8) -> bool {
    // fixmap / fixarray / fixstr / nil / bool / bin8..32 / ext / float32/64 /
    // str8..32 / array16..32 / map16..32 / negative fixint. The positive
    // fixint range 0x00..=0x7F is deliberately excluded: it is byte-identical
    // to ASCII text and would drown out every text-format signature.
    matches!(
        b,
        0x80..=0x8F
            | 0x90..=0x9F
            | 0xA0..=0xBF
            | 0xC0..=0xC1
            | 0xC2..=0xC3
            | 0xC4..=0xC7
            | 0xCA..=0xCB
            | 0xCC..=0xD4
            | 0xD9..=0xDB
            | 0xDC..=0xDD
            | 0xDE..=0xDF
            | 0xE0..=0xFF
    )
}

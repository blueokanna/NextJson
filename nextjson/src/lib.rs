//! # NextJson
//!
//! A dependency-free, `no_std + alloc` JSON and CBOR library for Rust.
//!
//! The public native contracts are [`NsonSerialize::nextencode`],
//! [`NsonDeserialize::nextdecode_into`], [`nextencode`], and [`nextdecode`].
//! JSON and the JSON-compatible CBOR profile can also be relayed through the
//! format-neutral [`cross_format::EventSink`] protocol without constructing an
//! intermediate [`Value`] tree.
//!
//! ## Quick start
//!
//! ```rust
//! let expected = (7_u64, "NextJson", vec![1_i32, 2, 3]);
//! let json = nextjson::nextencode(&expected)?;
//! let actual: (u64, &str, Vec<i32>) = nextjson::nextdecode(&json)?;
//! assert_eq!(actual, expected);
//! # Ok::<(), nextjson::Error>(())
//! ```
//!
//! ## Cross-format relay
//!
//! ```rust
//! use nextjson::cross_format;
//!
//! let source = br#"{"name":"NextJson","values":[1,2,3]}"#;
//! let cbor = cross_format::json_to_cbor(source)?;
//! let json = cross_format::cbor_to_json(&cbor)?;
//! let value: nextjson::Value = nextjson::nextdecode(&json)?;
//! assert_eq!(value["name"], nextjson::Value::from("NextJson"));
//! # Ok::<(), nextjson::Error>(())
//! ```
//!
//! ## Zero-copy boundary
//!
//! Unescaped JSON strings and definite-length CBOR text strings borrow their
//! input ranges. Escaped JSON and indefinite-length CBOR text must materialize
//! decoded UTF-8. Encoding always writes new output bytes. The library does not
//! describe those required copies as zero-copy.
//!
//! ## Features
//!
//! - `std` (default): standard I/O adapters and standard-library integrations.
//! - `derive` (default): repository-owned `NsonSerialize` and
//!   `NsonDeserialize` procedural macros.
//!
//! Disabling default features leaves a `core + alloc` implementation. The
//! complete workspace dependency graph contains only `nextjson` and the local,
//! optional `nextjson-derive` crate.
//!
//! ## Architecture
//!
//! NextJson uses schema-driven derives, a unified token stream, checked decode
//! slots, and a format-neutral [`cross_format::EventSink`] protocol. The whole
//! workspace build graph contains only its two local crates.
//!
//! The following properties are implemented directly in this repository and
//! are enforced by its tests and build configuration:
//!
//! 1. **Direct dual contract** - `NsonSerialize::nextencode` writes bytes
//!    directly; `NsonDeserialize::nextdecode_into` decodes into a caller-provided
//!    checked nextdecode slot, supporting memory reuse without a placeholder value.
//! 2. **Compile-time schema** - every type carries `const SCHEMA: TypeSchema`,
//!    a runtime-introspectable metadata tree (usable for JSON Schema generation,
//!    validation, and tooling).
//! 3. **Unified token stream** - the byte-stream lexer and the content-replay
//!    reader share identical nextdecode primitives, so internally-tagged,
//!    adjacently-tagged, and untagged enums plus `Value` round-trips reuse one
//!    engine.
//! 4. **Lazy single-token lookahead** - the parser lexes one token at a time;
//!    unescaped strings borrow the input with zero allocation; integer parsing
//!    is hand-rolled with overflow detection.
//! 5. **Safety boundary** - the library is `#![deny(unsafe_code)]`, including
//!    nextdecode slots and partial-initialization cleanup. `no_std` is fully
//!    supported: the core uses only `core` + `alloc`, with `std`-only types
//!    behind the `std` feature.
//! 6. **Streaming cross-format relay** - JSON and the JSON-compatible CBOR
//!    profile exchange borrowed structural events without an intermediate
//!    [`Value`].
//!
//! ## Safety and resource limits
//!
//! This crate denies unsafe Rust. Decode slots use checked state, numeric
//! conversions use checked arithmetic, and the default nesting limit is 128.
//! Applications must still enforce total input bytes, collection sizes, CPU
//! time, and output quotas. Reader APIs buffer their complete input.
//!
//! See the repository's [English README], [Chinese README], [safety model], and
//! [benchmark protocol] for the complete supported surface and reproducibility
//! requirements.
//!
//! [English README]: https://github.com/blueokanna/NextJson/blob/main/README.md
//! [Chinese README]: https://github.com/blueokanna/NextJson/blob/main/README_CN.md
//! [safety model]: https://github.com/blueokanna/NextJson/blob/main/docs/SAFETY.md
//! [benchmark protocol]: https://github.com/blueokanna/NextJson/blob/main/docs/BENCHMARKS.md

#![no_std]
#![deny(unsafe_code)]
#![deny(missing_docs)]
#![doc(html_root_url = "https://docs.rs/nextjson")]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "derive")]
pub use nextjson_derive::{NsonDeserialize, NsonSerialize};

pub use crate::de::{DecodeConfig, DecodeSlot, Decoder, NsonDeserialize, Token};
pub use crate::encoding::{EncodeConfig, Encoder};
pub use crate::error::{Error, Result};
pub use crate::map::Map;
pub use crate::number::Number;
pub use crate::schema::{
    EnumSchema, FieldSchema, NsonSchema, StructSchema, TypeSchema, VariantSchema,
};
pub use crate::ser::NsonSerialize;
pub use crate::value::Value;
pub use crate::write::Write;

pub mod cross_format;
pub mod de;
pub mod encoding;
pub mod error;
mod json_schema;
pub mod map;
mod number;
#[doc(hidden)]
pub mod private;
mod schema;
mod ser;
mod value;
mod write;

/// Private re-exports used by macro-generated code.
#[doc(hidden)]
pub mod __private {
    pub use alloc::borrow::Cow;
    pub use alloc::boxed::Box;
    pub use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
    pub use alloc::format;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec;
    pub use alloc::vec::Vec;
}

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Top-level serialization entry points
// ---------------------------------------------------------------------------

/// Encode a value into a compact JSON byte vector using the native NextJson
/// data model.
///
/// This is the canonical native encoding entry point. Unescaped string data is
/// copied directly into the output buffer without an intermediate JSON value.
pub fn nextencode<T: NsonSerialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut encoder = Encoder::for_vec(EncodeConfig::compact());
    NsonSerialize::nextencode(value, &mut encoder)?;
    Ok(encoder.finish_vec())
}

/// Serialize a value into a compact JSON string.
pub fn to_string<T: NsonSerialize + ?Sized>(value: &T) -> Result<String> {
    let bytes = nextencode(value)?;
    String::from_utf8(bytes).map_err(|e| Error::custom(format!("invalid utf-8: {e}")))
}

/// Serialize a value into a compact JSON byte vector.
pub fn to_vec<T: NsonSerialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    nextencode(value)
}

/// Serialize a value into a pretty-printed JSON string.
pub fn to_string_pretty<T: NsonSerialize + ?Sized>(value: &T) -> Result<String> {
    let bytes = to_vec_pretty(value)?;
    String::from_utf8(bytes).map_err(|e| Error::custom(format!("invalid utf-8: {e}")))
}

/// Serialize a value into a pretty-printed JSON byte vector.
pub fn to_vec_pretty<T: NsonSerialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut encoder = Encoder::for_vec(EncodeConfig::pretty());
    NsonSerialize::nextencode(value, &mut encoder)?;
    Ok(encoder.finish_vec())
}

/// Serialize a value to any `Write` sink.
pub fn to_writer<W, T>(writer: W, value: &T) -> Result<()>
where
    W: Write,
    T: NsonSerialize + ?Sized,
{
    let mut encoder = Encoder::new(writer);
    NsonSerialize::nextencode(value, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

/// Serialize a value to any `Write` sink with pretty printing.
pub fn to_writer_pretty<W, T>(writer: W, value: &T) -> Result<()>
where
    W: Write,
    T: NsonSerialize + ?Sized,
{
    let mut encoder = Encoder::with_config(writer, EncodeConfig::pretty());
    NsonSerialize::nextencode(value, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

/// Serialize a value to a `std::io::Write` sink (requires the `std` feature).
#[cfg(feature = "std")]
pub fn to_io_writer<W, T>(writer: W, value: &T) -> Result<()>
where
    W: std::io::Write,
    T: NsonSerialize + ?Sized,
{
    let mut encoder = Encoder::new(crate::write::StdWriter(writer));
    NsonSerialize::nextencode(value, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Top-level deserialization entry points
// ---------------------------------------------------------------------------

/// Decode one complete JSON value using the native NextJson data model.
///
/// The input lifetime is preserved, so implementations may borrow unescaped
/// strings directly from `input` without allocation.
pub fn nextdecode<'de, T: NsonDeserialize<'de>>(input: &'de [u8]) -> Result<T> {
    let mut decoder = Decoder::new(input);
    let value = T::nextdecode(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

/// Deserialize from a `&str`. The `'de` lifetime allows types to borrow input.
pub fn from_str<'de, T: NsonDeserialize<'de>>(s: &'de str) -> Result<T> {
    nextdecode(s.as_bytes())
}

/// Deserialize from a `&[u8]`. The `'de` lifetime allows types to borrow input.
pub fn from_slice<'de, T: NsonDeserialize<'de>>(slice: &'de [u8]) -> Result<T> {
    nextdecode(slice)
}

/// Deserialize from a `std::io::Read` (requires the `std` feature).
///
/// The target type must be deserializable for any lifetime (owned).
#[cfg(feature = "std")]
pub fn from_reader<R, T>(reader: R) -> Result<T>
where
    R: std::io::Read,
    T: for<'de> NsonDeserialize<'de>,
{
    let mut buf = Vec::new();
    let mut reader = reader;
    reader.read_to_end(&mut buf).map_err(Error::io)?;
    from_slice(&buf)
}

// ---------------------------------------------------------------------------
// Value conversion entry points
// ---------------------------------------------------------------------------

/// Convert any serializable value into a [`Value`].
pub fn to_value<T: NsonSerialize + ?Sized>(value: &T) -> Result<Value> {
    from_slice(&to_vec(value)?)
}

/// Convert a [`Value`] into any type (owned, deserializable for any lifetime).
pub fn from_value<T>(value: Value) -> Result<T>
where
    T: for<'de> NsonDeserialize<'de>,
{
    private::nextdecode_value(value)
}

/// Get the compile-time [`TypeSchema`] of a type (runtime-introspectable).
pub fn schema_of<T: NsonSchema>() -> TypeSchema {
    T::SCHEMA
}

/// Generate a JSON Schema (draft-07 style) for any [`NsonSchema`] type.
pub fn to_json_schema<T: NsonSchema>() -> Value {
    json_schema::from_schema(T::SCHEMA)
}

#[macro_export]
/// The `json!` macro: build a [`Value`] with JSON-like syntax.
///
/// Supports nested objects / arrays, `null` / `true` / `false`, literals, bare
/// identifier keys, expression interpolation, and trailing commas:
///
/// ```rust
/// use nextjson::json;
/// let code = 200;
/// let v = json!({
///     "code": code,
///     "ok": (code == 200),
///     "nested": { "a": [1, 2.5, null], "b": true },
///     "list": [1, 2, 3,],
/// });
/// ```
macro_rules! json {
    ($($json:tt)+) => {
        $crate::json_internal!($($json)+)
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! json_internal {
    // ---------------- main rules ----------------
    (null) => {
        $crate::Value::Null
    };
    (true) => {
        $crate::Value::Bool(true)
    };
    (false) => {
        $crate::Value::Bool(false)
    };
    ([]) => {
        $crate::Value::Array($crate::__private::Vec::new())
    };
    ([$($tt:tt)+]) => {
        $crate::Value::Array($crate::json_internal!(@array [] $($tt)+))
    };
    ({}) => {
        $crate::Value::Object($crate::Map::new())
    };
    ({ $($tt:tt)+ }) => {
        $crate::Value::Object({
            let mut __object = $crate::Map::new();
            $crate::json_internal!(@object __object () ($($tt)+));
            __object
        })
    };
    ($other:expr) => {
        $crate::to_value(&$other).expect("json! interpolation: value must be serializable")
    };

    // ---------------- @array: TT muncher ----------------
    (@array [$($elems:expr,)*]) => {
        $crate::__private::vec![$($elems,)*]
    };
    (@array [$($elems:expr),*]) => {
        $crate::__private::vec![$($elems),*]
    };
    (@array [$($elems:expr,)*] null $($rest:tt)*) => {
        $crate::json_internal!(@array [$($elems,)* $crate::json_internal!(null)] $($rest)*)
    };
    (@array [$($elems:expr,)*] true $($rest:tt)*) => {
        $crate::json_internal!(@array [$($elems,)* $crate::json_internal!(true)] $($rest)*)
    };
    (@array [$($elems:expr,)*] false $($rest:tt)*) => {
        $crate::json_internal!(@array [$($elems,)* $crate::json_internal!(false)] $($rest)*)
    };
    (@array [$($elems:expr,)*] [$($sub:tt)*] $($rest:tt)*) => {
        $crate::json_internal!(@array [$($elems,)* $crate::json_internal!([$($sub)*])] $($rest)*)
    };
    (@array [$($elems:expr,)*] {$($sub:tt)*} $($rest:tt)*) => {
        $crate::json_internal!(@array [$($elems,)* $crate::json_internal!({$($sub)*})] $($rest)*)
    };
    (@array [$($elems:expr,)*] $next:expr , $($rest:tt)*) => {
        $crate::json_internal!(@array [$($elems,)* $crate::json_internal!($next),] $($rest)*)
    };
    (@array [$($elems:expr,)*] $last:expr) => {
        $crate::json_internal!(@array [$($elems,)* $crate::json_internal!($last)])
    };
    (@array [$($elems:expr),*] , $($rest:tt)*) => {
        $crate::json_internal!(@array [$($elems,)*] $($rest)*)
    };

    // ---------------- @object: TT muncher ----------------
    (@object $object:ident () ()) => {};
    (@object $object:ident [$($key:tt)+] ($value:expr) , $($rest:tt)*) => {
        let _ = $object.insert($crate::json_key!(($($key)+)), $value);
        $crate::json_internal!(@object $object () ($($rest)*));
    };
    (@object $object:ident [$($key:tt)+] ($value:expr)) => {
        let _ = $object.insert($crate::json_key!(($($key)+)), $value);
    };
    (@object $object:ident ($($key:tt)+) (: null $($rest:tt)*)) => {
        $crate::json_internal!(@object $object [$($key)+] ($crate::json_internal!(null)) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: true $($rest:tt)*)) => {
        $crate::json_internal!(@object $object [$($key)+] ($crate::json_internal!(true)) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: false $($rest:tt)*)) => {
        $crate::json_internal!(@object $object [$($key)+] ($crate::json_internal!(false)) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: [$($sub:tt)*] $($rest:tt)*)) => {
        $crate::json_internal!(@object $object [$($key)+] ($crate::json_internal!([$($sub)*])) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: {$($sub:tt)*} $($rest:tt)*)) => {
        $crate::json_internal!(@object $object [$($key)+] ($crate::json_internal!({$($sub)*})) $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: $value:expr , $($rest:tt)*)) => {
        $crate::json_internal!(@object $object [$($key)+] ($crate::json_internal!($value)) , $($rest)*);
    };
    (@object $object:ident ($($key:tt)+) (: $value:expr)) => {
        $crate::json_internal!(@object $object [$($key)+] ($crate::json_internal!($value)));
    };
    (@object $object:ident ($($key:tt)*) ($tt:tt $($rest:tt)*)) => {
        $crate::json_internal!(@object $object ($($key)* $tt) ($($rest)*));
    };
}

#[macro_export]
#[doc(hidden)]
/// Convert an object key into a String.
macro_rules! json_key {
    (($s:literal)) => {
        $crate::__private::String::from($s)
    };
    (($i:ident)) => {
        $crate::__private::String::from(stringify!($i))
    };
    (($e:expr)) => {
        $crate::__private::String::from($crate::__private::format!("{}", $e))
    };
}

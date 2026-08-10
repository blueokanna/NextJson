//! # NextJson
//!
//! A high-performance, zero-dependency, `no_std`-ready JSON serialization /
//! deserialization library for Rust with an **original architecture**.
//!
//! ## Design philosophy
//!
//! NextJson deliberately does **not** follow serde's `Visitor` pattern. It is
//! built on an original **schema-driven** design:
//!
//! | Aspect | `serde` | `nextjson` |
//! |---|---|---|
//! | Core abstraction | `Serializer` / `Deserializer` + `Visitor` | `Encoder` / `Decoder` + `decode_into` |
//! | Metadata | derive is fully expanded, no runtime shape | `const SCHEMA: TypeSchema` runtime-introspectable tree |
//! | Deserialization | per-field `Visitor::visit_*` callbacks | direct decode into a `MaybeUninit` slot |
//! | Zero-copy | needs `#[serde(borrow)]` + care | parser returns `Cow::Borrowed` for unescaped strings |
//!
//! ### Innovations
//!
//! 1. **Visitor-free dual contract** — `NsonSerialize::encode` writes bytes
//!    directly; `NsonDeserialize::decode_into` decodes into a caller-provided
//!    uninitialized slot, supporting memory reuse with zero initialization cost.
//! 2. **Compile-time schema** — every type carries `const SCHEMA: TypeSchema`,
//!    a runtime-introspectable metadata tree (usable for JSON Schema generation,
//!    validation, tooling), unlike serde's compiled-away derives.
//! 3. **Unified token stream** — the byte-stream lexer and the content-replay
//!    reader share identical decode primitives, so internally-tagged,
//!    adjacently-tagged, and untagged enums plus `Value` round-trips reuse one
//!    engine.
//! 4. **Lazy single-token lookahead** — the parser lexes one token at a time;
//!    unescaped strings borrow the input with zero allocation; integer parsing
//!    is hand-rolled with overflow detection.
//! 5. **Safety boundary** — the library itself is `#![deny(unsafe_code)]`;
//!    narrowly scoped exceptions cover the documented post-success
//!    `assume_init` and the RAII field slot's move/drop operations. `no_std` is
//!    fully supported: the core uses only `core` +
//!    `alloc`, with `std`-only types behind the `std` feature.

#![no_std]
#![deny(unsafe_code)]
#![deny(missing_docs)]
#![doc(html_root_url = "https://docs.rs/nextjson")]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "derive")]
pub use nextjson_derive::{NsonDeserialize, NsonSerialize};

pub use crate::de::{DecodeConfig, Decoder, NsonDeserialize, Token};
pub use crate::encode::{EncodeConfig, Encoder};
pub use crate::error::{Error, Result};
pub use crate::map::Map;
pub use crate::number::Number;
pub use crate::schema::{
    EnumSchema, FieldSchema, NsonSchema, StructSchema, TypeSchema, VariantSchema,
};
pub use crate::ser::NsonSerialize;
pub use crate::value::Value;
pub use crate::write::Write;

pub mod de;
pub mod encode;
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

/// Serialize a value into a compact JSON string.
pub fn to_string<T: NsonSerialize + ?Sized>(value: &T) -> Result<String> {
    let bytes = to_vec(value)?;
    String::from_utf8(bytes).map_err(|e| Error::custom(format!("invalid utf-8: {e}")))
}

/// Serialize a value into a compact JSON byte vector.
pub fn to_vec<T: NsonSerialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut encoder = Encoder::new(Vec::new());
    NsonSerialize::encode(value, &mut encoder)?;
    encoder.finish()
}

/// Serialize a value into a pretty-printed JSON string.
pub fn to_string_pretty<T: NsonSerialize + ?Sized>(value: &T) -> Result<String> {
    let bytes = to_vec_pretty(value)?;
    String::from_utf8(bytes).map_err(|e| Error::custom(format!("invalid utf-8: {e}")))
}

/// Serialize a value into a pretty-printed JSON byte vector.
pub fn to_vec_pretty<T: NsonSerialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut encoder = Encoder::with_config(Vec::new(), EncodeConfig::pretty());
    NsonSerialize::encode(value, &mut encoder)?;
    encoder.finish()
}

/// Serialize a value to any `Write` sink.
pub fn to_writer<W, T>(writer: W, value: &T) -> Result<()>
where
    W: Write,
    T: NsonSerialize + ?Sized,
{
    let mut encoder = Encoder::new(writer);
    NsonSerialize::encode(value, &mut encoder)?;
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
    NsonSerialize::encode(value, &mut encoder)?;
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
    NsonSerialize::encode(value, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Top-level deserialization entry points
// ---------------------------------------------------------------------------

/// Deserialize from a `&str`. The `'de` lifetime allows types to borrow input.
pub fn from_str<'de, T: NsonDeserialize<'de>>(s: &'de str) -> Result<T> {
    from_slice(s.as_bytes())
}

/// Deserialize from a `&[u8]`. The `'de` lifetime allows types to borrow input.
pub fn from_slice<'de, T: NsonDeserialize<'de>>(slice: &'de [u8]) -> Result<T> {
    let mut decoder = Decoder::new(slice);
    let value = T::decode(&mut decoder)?;
    decoder.end()?;
    Ok(value)
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
    private::decode_value(value)
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

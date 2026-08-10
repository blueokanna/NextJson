//! Optional zero-copy interoperability with the Serde data model.
//!
//! Enable the `serde` feature to serialize any [`serde::Serialize`] value with
//! NextJson's encoder or deserialize any [`serde::Deserialize`] value with
//! NextJson's decoder. The adapters operate directly on the token stream and
//! never build an intermediate [`crate::Value`].

mod de;
mod ser;
#[cfg(feature = "transcode")]
pub mod transcode;

pub use self::de::Deserializer;
pub use self::ser::Serializer;

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::encoding::EncodeConfig;
use crate::error::{Error, Result};
use crate::write::Write;

impl serde::ser::Error for Error {
    fn custom<T: core::fmt::Display>(msg: T) -> Self {
        Error::custom(alloc::format!("{msg}"))
    }
}

impl serde::de::Error for Error {
    fn custom<T: core::fmt::Display>(msg: T) -> Self {
        Error::custom(alloc::format!("{msg}"))
    }
}

/// Encode a Serde value into compact JSON bytes using NextJson.
pub fn nextencode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut serializer = Serializer::for_vec(EncodeConfig::compact());
    value.serialize(&mut serializer)?;
    Ok(serializer.finish_vec())
}

/// Decode one complete JSON value through NextJson's Serde adapter.
///
/// Unescaped strings may borrow directly from `input` when the target type
/// requests borrowed data.
pub fn nextdecode<'de, T: Deserialize<'de>>(input: &'de [u8]) -> Result<T> {
    let mut deserializer = Deserializer::new(input);
    let value = T::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

/// Serialize a Serde value into compact JSON bytes using NextJson.
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    nextencode(value)
}

/// Serialize a Serde value into compact JSON text using NextJson.
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    let bytes = to_vec(value)?;
    String::from_utf8(bytes)
        .map_err(|error| Error::custom(alloc::format!("invalid utf-8: {error}")))
}

/// Serialize a Serde value into pretty-printed JSON bytes using NextJson.
pub fn to_vec_pretty<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut serializer = Serializer::for_vec(EncodeConfig::pretty());
    value.serialize(&mut serializer)?;
    Ok(serializer.finish_vec())
}

/// Serialize a Serde value into pretty-printed JSON text using NextJson.
pub fn to_string_pretty<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    let bytes = to_vec_pretty(value)?;
    String::from_utf8(bytes)
        .map_err(|error| Error::custom(alloc::format!("invalid utf-8: {error}")))
}

/// Serialize a Serde value to a NextJson [`Write`] sink.
pub fn to_writer<W, T>(writer: W, value: &T) -> Result<()>
where
    W: Write,
    T: Serialize + ?Sized,
{
    let mut serializer = Serializer::new(writer);
    value.serialize(&mut serializer)?;
    serializer.finish()?;
    Ok(())
}

/// Serialize a Serde value as pretty-printed JSON to a NextJson [`Write`] sink.
pub fn to_writer_pretty<W, T>(writer: W, value: &T) -> Result<()>
where
    W: Write,
    T: Serialize + ?Sized,
{
    let mut serializer = Serializer::with_config(writer, EncodeConfig::pretty());
    value.serialize(&mut serializer)?;
    serializer.finish()?;
    Ok(())
}

/// Serialize a Serde value to a standard IO writer.
#[cfg(feature = "std")]
pub fn to_io_writer<W, T>(writer: W, value: &T) -> Result<()>
where
    W: std::io::Write,
    T: Serialize + ?Sized,
{
    to_writer(crate::write::StdWriter(writer), value)
}

/// Deserialize a Serde value from UTF-8 JSON text.
pub fn from_str<'de, T: Deserialize<'de>>(input: &'de str) -> Result<T> {
    from_slice(input.as_bytes())
}

/// Deserialize a Serde value from JSON bytes.
///
/// Unescaped strings are passed to Serde as borrowed data tied to `input`.
pub fn from_slice<'de, T: Deserialize<'de>>(input: &'de [u8]) -> Result<T> {
    nextdecode(input)
}

/// Deserialize an owned Serde value from an IO reader.
#[cfg(feature = "std")]
pub fn from_reader<R, T>(mut reader: R) -> Result<T>
where
    R: std::io::Read,
    T: serde::de::DeserializeOwned,
{
    let mut input = Vec::new();
    reader.read_to_end(&mut input).map_err(Error::io)?;
    from_slice(&input)
}

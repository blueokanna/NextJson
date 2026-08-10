//! Streaming interoperability between NextJson and other Serde formats.
//!
//! The functions in this module relay Serde events directly between a
//! deserializer and serializer. They do not construct an intermediate
//! [`crate::Value`] or `serde_json::Value`. This bounds memory use to the
//! buffers maintained by the selected source and destination formats.

use alloc::vec::Vec;

use serde::de::Deserializer as SerdeDeserializer;
use serde::ser::{Error as _, Serializer as SerdeSerializer};

use super::{Deserializer, Serializer};
use crate::encoding::EncodeConfig;
use crate::error::Result;
use crate::write::Write;

/// Stream one complete JSON value into another format's serializer.
///
/// Unescaped JSON strings remain borrowed while they are passed to the target
/// serializer. If the source is invalid or contains trailing data, the error
/// is represented by the target serializer's error type. A streaming target
/// may already have received a prefix when a later source error is detected.
pub fn json_to<S>(input: &[u8], serializer: S) -> core::result::Result<S::Ok, S::Error>
where
    S: SerdeSerializer,
{
    let mut deserializer = Deserializer::new(input);
    let output = serde_transcode::transcode(&mut deserializer, serializer)?;
    deserializer.end().map_err(S::Error::custom)?;
    Ok(output)
}

/// Stream one complete JSON string into another format's serializer.
pub fn json_str_to<S>(input: &str, serializer: S) -> core::result::Result<S::Ok, S::Error>
where
    S: SerdeSerializer,
{
    json_to(input.as_bytes(), serializer)
}

/// Stream one value from another format's deserializer into compact JSON bytes.
///
/// A generic Serde deserializer has no common end-of-input operation. Callers
/// that require rejection of trailing source data must validate exhaustion
/// with the source format's own API after this function returns.
pub fn json_from<'de, D>(deserializer: D) -> Result<Vec<u8>>
where
    D: SerdeDeserializer<'de>,
{
    json_from_with_config(deserializer, EncodeConfig::compact())
}

/// Stream one value from another format's deserializer into pretty JSON bytes.
pub fn json_from_pretty<'de, D>(deserializer: D) -> Result<Vec<u8>>
where
    D: SerdeDeserializer<'de>,
{
    json_from_with_config(deserializer, EncodeConfig::pretty())
}

/// Stream a value from another format's deserializer into JSON bytes using an
/// explicit NextJson encoding configuration.
pub fn json_from_with_config<'de, D>(deserializer: D, config: EncodeConfig) -> Result<Vec<u8>>
where
    D: SerdeDeserializer<'de>,
{
    let mut serializer = Serializer::for_vec(config);
    serde_transcode::transcode(deserializer, &mut serializer)?;
    Ok(serializer.finish_vec())
}

/// Stream a value from another format's deserializer into a NextJson writer.
pub fn json_from_into_writer<'de, D, W>(deserializer: D, writer: W) -> Result<()>
where
    D: SerdeDeserializer<'de>,
    W: Write,
{
    let mut serializer = Serializer::new(writer);
    serde_transcode::transcode(deserializer, &mut serializer)?;
    serializer.finish().map(|_| ())
}

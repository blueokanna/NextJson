//! Dependency-free streaming interoperability between structured formats.
//!
//! The [`EventSink`] contract is deliberately smaller than a type-driven
//! serialization framework. It represents the complete JSON data model and
//! allows formats to relay values without constructing an intermediate tree.

mod cbor;
mod json;

pub use self::cbor::CborSink;
pub use self::json::JsonSink;

use alloc::vec::Vec;

use crate::de::{DecodeConfig, Decoder, Token};
use crate::encoding::EncodeConfig;
use crate::error::{Error, Result};
use crate::number::Number;
use crate::write::Write;

/// Receives a validated stream of JSON-compatible structural events.
///
/// Object keys are separate from string values, which prevents a destination
/// format from accidentally accepting a non-string key that JSON cannot
/// represent. Implementations must return an error for invalid event order.
pub trait EventSink {
    /// Receive a null value.
    fn null(&mut self) -> Result<()>;
    /// Receive a boolean value.
    fn boolean(&mut self, value: bool) -> Result<()>;
    /// Receive a finite number.
    fn number(&mut self, value: Number) -> Result<()>;
    /// Receive a UTF-8 string value.
    fn string(&mut self, value: &str) -> Result<()>;
    /// Begin an array.
    fn begin_array(&mut self) -> Result<()>;
    /// End the current array.
    fn end_array(&mut self) -> Result<()>;
    /// Begin an object.
    fn begin_object(&mut self) -> Result<()>;
    /// Receive an object key.
    fn object_key(&mut self, key: &str) -> Result<()>;
    /// End the current object.
    fn end_object(&mut self) -> Result<()>;
}

/// Relay one complete JSON value into an event sink.
///
/// Unescaped input strings are passed to the sink as direct borrows of
/// `input`. Escaped strings are necessarily materialized by JSON unescaping.
pub fn json_into<S: EventSink + ?Sized>(input: &[u8], sink: &mut S) -> Result<()> {
    json_into_with_config(input, DecodeConfig::default(), sink)
}

/// Relay one complete JSON value using an explicit nesting configuration.
pub fn json_into_with_config<S: EventSink + ?Sized>(
    input: &[u8],
    config: DecodeConfig,
    sink: &mut S,
) -> Result<()> {
    let mut decoder = Decoder::with_config(input, config);
    relay_json_value(&mut decoder, sink)?;
    decoder.end()
}

/// Relay one complete CBOR value into an event sink.
///
/// This accepts the JSON-compatible CBOR profile documented by [`CborSink`].
pub fn cbor_into<S: EventSink + ?Sized>(input: &[u8], sink: &mut S) -> Result<()> {
    cbor::cbor_into_with_max_depth(input, 128, sink)
}

/// Relay one complete CBOR value with an explicit maximum nesting depth.
pub fn cbor_into_with_max_depth<S: EventSink + ?Sized>(
    input: &[u8],
    max_depth: u32,
    sink: &mut S,
) -> Result<()> {
    cbor::cbor_into_with_max_depth(input, max_depth, sink)
}

/// Stream one complete JSON value into CBOR bytes without an intermediate
/// value tree.
pub fn json_to_cbor(input: &[u8]) -> Result<Vec<u8>> {
    let mut sink = CborSink::new(Vec::new());
    json_into(input, &mut sink)?;
    sink.finish()
}

/// Stream one complete JSON value into a CBOR writer.
pub fn json_to_cbor_writer<W: Write>(input: &[u8], writer: W) -> Result<()> {
    let mut sink = CborSink::new(writer);
    json_into(input, &mut sink)?;
    sink.finish().map(|_| ())
}

/// Stream one complete CBOR value into compact JSON bytes without an
/// intermediate value tree.
pub fn cbor_to_json(input: &[u8]) -> Result<Vec<u8>> {
    cbor_to_json_with_config(input, EncodeConfig::compact())
}

/// Stream one complete CBOR value into pretty-printed JSON bytes.
pub fn cbor_to_json_pretty(input: &[u8]) -> Result<Vec<u8>> {
    cbor_to_json_with_config(input, EncodeConfig::pretty())
}

/// Stream one complete CBOR value into JSON bytes with an explicit encoding
/// configuration.
pub fn cbor_to_json_with_config(input: &[u8], config: EncodeConfig) -> Result<Vec<u8>> {
    let mut sink = JsonSink::with_config(Vec::new(), config);
    cbor_into(input, &mut sink)?;
    sink.finish()
}

/// Stream one complete CBOR value into a JSON writer.
pub fn cbor_to_json_writer<W: Write>(input: &[u8], writer: W) -> Result<()> {
    let mut sink = JsonSink::new(writer);
    cbor_into(input, &mut sink)?;
    sink.finish().map(|_| ())
}

fn relay_json_value<'de, S: EventSink + ?Sized>(
    decoder: &mut Decoder<'de>,
    sink: &mut S,
) -> Result<()> {
    match decoder.peek_token()? {
        Token::Null => {
            decoder.unit()?;
            sink.null()
        }
        Token::Bool(_) => sink.boolean(decoder.bool()?),
        Token::Number(_) => sink.number(decoder.number()?),
        Token::Str(_) => {
            let value = decoder.string()?;
            sink.string(value.as_ref())
        }
        Token::BeginArray => {
            decoder.begin_array()?;
            sink.begin_array()?;
            while decoder.array_has_more()? {
                relay_json_value(decoder, sink)?;
                if !decoder.array_entry_sep()? {
                    break;
                }
            }
            decoder.end_array()?;
            sink.end_array()
        }
        Token::BeginObject => {
            decoder.begin_object()?;
            sink.begin_object()?;
            while let Some(key) = decoder.object_key()? {
                sink.object_key(key.as_ref())?;
                relay_json_value(decoder, sink)?;
                if !decoder.object_entry_sep()? {
                    break;
                }
            }
            decoder.end_object()?;
            sink.end_object()
        }
        Token::EndArray | Token::EndObject => Err(Error::custom("unexpected container end")),
    }
}

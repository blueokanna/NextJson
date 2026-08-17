//! Native CBOR codec (RFC 8949, JSON-compatible profile).
//!
//! Replaces the historical event-relay implementation (which serialized to
//! JSON text and re-parsed it to produce CBOR, ~6-10x slower). This codec
//! writes CBOR directly through the unified [`FormatEncoder`] contract and
//! reads it directly through [`FormatDecoder`], eliminating the intermediate
//! JSON round-trip while keeping the exact same wire semantics:
//!
//! - **Definite-length** arrays/maps with a count prefix (the compact,
//!   standard form; the historical relay wrote indefinite-length, which is
//!   still read for interoperability).
//! - Integers through `u64` use major types 0/1; larger values use the
//!   standard bignum tags 2 (unsigned) and 3 (negative).
//! - Floats are written as 64-bit (double); half/float/double are read.
//! - Non-finite floats are rejected (the JSON-compatible profile cannot
//!   represent them), as are byte strings, non-text map keys, and semantic
//!   tags other than 2/3 — matching the documented relay behavior.
//!
//! Both the cross-format relay (`crate::cross_format`) and this codec stay
//! wire-compatible with each other and with external CBOR writers/readers;
//! the foreign-wire fixtures in the test suite cover definite-length maps,
//! half-floats, indefinite text chunks, and bignums.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use crate::cross_format::half_to_f32;
use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::bin::{patch_prefix, Cursor, MAX_CONTAINER_PREALLOC};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// CBOR format marker.
#[derive(Clone, Copy, Debug)]
pub struct Cbor;

impl Format for Cbor {
    const NAME: &'static str = "cbor";
    const MIME: &'static str = "application/cbor";
    const EXTENSIONS: &'static [&'static str] = &["cbor"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = CborEncoder::new(Vec::new());
        // Trust model matches the top-level JSON `FastEncoder`: derive code
        // emits a well-formed event stream, and the encoder still counts
        // container entries (required for definite-length headers), so
        // unbalanced containers are rejected rather than producing garbage.
        T::nextencode(value, &mut encoder)?;
        Ok(encoder.finish_vec())
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = CborDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum FrameKind {
    Array,
    Map,
}

struct Frame {
    start: usize,
    kind: FrameKind,
    count: u64,
}

/// Streaming CBOR encoder (definite-length containers).
pub struct CborEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    frames: Vec<Frame>,
}

impl<W: Write> CborEncoder<W> {
    /// Create a CBOR encoder over `writer`.
    pub fn new(writer: W) -> Self {
        CborEncoder {
            writer,
            buf: Vec::with_capacity(1024),
            frames: Vec::new(),
        }
    }

    fn push(&mut self, byte: u8) {
        self.buf.push(byte);
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Emit a major-type header with the shortest argument encoding.
    fn type_and_argument(&mut self, major: u8, argument: u64) {
        let prefix = major << 5;
        if argument < 24 {
            self.push(prefix | argument as u8);
        } else if argument <= u8::MAX as u64 {
            self.extend(&[prefix | 24, argument as u8]);
        } else if argument <= u16::MAX as u64 {
            self.extend(&[prefix | 25]);
            self.extend(&(argument as u16).to_be_bytes());
        } else if argument <= u32::MAX as u64 {
            self.extend(&[prefix | 26]);
            self.extend(&(argument as u32).to_be_bytes());
        } else {
            self.extend(&[prefix | 27]);
            self.extend(&argument.to_be_bytes());
        }
    }

    fn unsigned(&mut self, value: u128) {
        if let Ok(value) = u64::try_from(value) {
            self.type_and_argument(0, value);
        } else {
            self.bignum(2, value);
        }
    }

    fn signed(&mut self, value: i128) {
        if value >= 0 {
            return self.unsigned(value as u128);
        }
        let argument = (-1 - value) as u128;
        if let Ok(argument) = u64::try_from(argument) {
            self.type_and_argument(1, argument);
        } else {
            self.bignum(3, argument);
        }
    }

    /// Bignum tags 2/3: `tag(value)` followed by a byte string of the
    /// minimal big-endian magnitude.
    fn bignum(&mut self, tag: u64, value: u128) {
        self.type_and_argument(6, tag);
        let bytes = value.to_be_bytes();
        let first = bytes
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(bytes.len() - 1);
        let magnitude = &bytes[first..];
        self.type_and_argument(2, magnitude.len() as u64);
        self.extend(magnitude);
    }

    fn write_string(&mut self, value: &str) {
        let len = u64::try_from(value.len()).expect("string length fits u64");
        self.type_and_argument(3, len);
        self.extend(value.as_bytes());
    }

    fn patch_container(&mut self, frame: Frame) {
        let count = frame.count;
        // Placeholder was a single byte (`0x80` for array / `0xA0` for map).
        let mut header = [0u8; 9];
        let header_len = match frame.kind {
            FrameKind::Array => {
                if count < 24 {
                    self.buf[frame.start] = 0x80 | count as u8;
                    return;
                } else if count <= u8::MAX as u64 {
                    header[0] = 0x98;
                    header[1] = count as u8;
                    2
                } else if count <= u16::MAX as u64 {
                    header[0] = 0x99;
                    header[1..3].copy_from_slice(&(count as u16).to_be_bytes());
                    3
                } else if count <= u32::MAX as u64 {
                    header[0] = 0x9A;
                    header[1..5].copy_from_slice(&(count as u32).to_be_bytes());
                    5
                } else {
                    header[0] = 0x9B;
                    header[1..9].copy_from_slice(&count.to_be_bytes());
                    9
                }
            }
            FrameKind::Map => {
                if count < 24 {
                    self.buf[frame.start] = 0xA0 | count as u8;
                    return;
                } else if count <= u8::MAX as u64 {
                    header[0] = 0xB8;
                    header[1] = count as u8;
                    2
                } else if count <= u16::MAX as u64 {
                    header[0] = 0xB9;
                    header[1..3].copy_from_slice(&(count as u16).to_be_bytes());
                    3
                } else if count <= u32::MAX as u64 {
                    header[0] = 0xBA;
                    header[1..5].copy_from_slice(&(count as u32).to_be_bytes());
                    5
                } else {
                    header[0] = 0xBB;
                    header[1..9].copy_from_slice(&count.to_be_bytes());
                    9
                }
            }
        };
        patch_prefix(&mut self.buf, frame.start, &header[..header_len]);
    }

    /// Flush the internal buffer and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn finish_vec(mut self) -> Vec<u8> {
        core::mem::take(&mut self.buf)
    }
}

impl<W: Write> FormatEncoder for CborEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.frames.push(Frame {
            start: self.buf.len(),
            kind: FrameKind::Array,
            count: 0,
        });
        self.push(0x80); // placeholder array(0)
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("cbor: array separator outside a container"))?;
        frame.count = frame
            .count
            .checked_add(1)
            .ok_or_else(|| Error::custom("cbor: array length overflow"))?;
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("cbor: array end without start"))?;
        self.patch_container(frame);
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.frames.push(Frame {
            start: self.buf.len(),
            kind: FrameKind::Map,
            count: 0,
        });
        self.push(0xA0); // placeholder map(0)
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("cbor: object key outside a container"))?;
        frame.count = frame
            .count
            .checked_add(1)
            .ok_or_else(|| Error::custom("cbor: map length overflow"))?;
        self.write_string(key);
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("cbor: object end without start"))?;
        self.patch_container(frame);
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.push(0xF6);
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.push(if value { 0xF5 } else { 0xF4 });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.write_string(value);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        let mut buf = [0u8; 4];
        self.write_string(value.encode_utf8(&mut buf));
        Ok(())
    }

    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error> {
        match *value {
            Number::I64(v) => self.write_i64(v),
            Number::U64(v) => self.write_u64(v),
            Number::I128(v) => self.write_i128(v),
            Number::U128(v) => self.write_u128(v),
            Number::F64(v) => self.write_f64(v),
        }
    }

    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        self.signed(value as i128);
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.unsigned(value as u128);
        Ok(())
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.signed(value);
        Ok(())
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        self.unsigned(value);
        Ok(())
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("CBOR profile rejects non-finite floats"));
        }
        self.push(0xFB);
        self.extend(&value.to_bits().to_be_bytes());
        Ok(())
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("CBOR profile rejects non-finite floats"));
        }
        self.push(0xFA);
        self.extend(&value.to_bits().to_be_bytes());
        Ok(())
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Decoder-side container frame. `remaining: Some(n)` is a definite-length
/// container with `n` entries left; `None` is an indefinite container
/// terminated by a break marker.
#[derive(Clone, Copy)]
struct DFrame {
    kind: FrameKind,
    remaining: Option<u64>,
}

/// Streaming CBOR decoder (definite + indefinite containers).
pub struct CborDecoder<'de> {
    cur: Cursor<'de>,
    lookahead: Option<Token<'de>>,
    pending: Option<DFrame>,
    frames: Vec<DFrame>,
    depth: u32,
    max_depth: u32,
}

impl<'de> CborDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        CborDecoder {
            cur: Cursor::new(input),
            lookahead: None,
            pending: None,
            frames: Vec::new(),
            depth: 0,
            max_depth: 128,
        }
    }

    /// Validate that the whole input was consumed.
    pub fn end(&mut self) -> Result<()> {
        self.expect_end()
    }

    fn expect_end(&mut self) -> Result<()> {
        if self.lookahead.is_none() && self.frames.is_empty() && self.cur.at_end() {
            Ok(())
        } else {
            Err(Error::custom("cbor: trailing bytes after value"))
        }
    }

    fn header(&mut self) -> Result<u8> {
        self.cur.byte()
    }

    fn read_u8(&mut self) -> Result<u8> {
        self.cur.byte()
    }

    fn read_be_u16(&mut self) -> Result<u16> {
        self.cur.be_u16()
    }

    fn read_be_u32(&mut self) -> Result<u32> {
        self.cur.be_u32()
    }

    fn read_be_u64(&mut self) -> Result<u64> {
        self.cur.be_u64()
    }

    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("cbor: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    /// Decode an argument (additional info 0..=27) into its value.
    fn argument(&mut self, additional: u8) -> Result<u64> {
        match additional {
            0..=23 => Ok(additional as u64),
            24 => Ok(self.read_u8()? as u64),
            25 => Ok(self.read_be_u16()? as u64),
            26 => Ok(self.read_be_u32()? as u64),
            27 => self.read_be_u64(),
            28..=30 => Err(Error::custom("cbor: reserved additional information")),
            31 => Err(Error::custom("cbor: unexpected break marker")),
            // `additional` is always `header & 0x1F`; the arm keeps the match
            // exhaustive over `u8` and treats out-of-range input as invalid.
            _ => Err(Error::custom("cbor: invalid additional information")),
        }
    }

    /// Decode a container length; `31` (indefinite) yields `None`.
    fn container_argument(&mut self, additional: u8) -> Result<Option<u64>> {
        match additional {
            0..=23 => Ok(Some(additional as u64)),
            24 => Ok(Some(self.read_u8()? as u64)),
            25 => Ok(Some(self.read_be_u16()? as u64)),
            26 => Ok(Some(self.read_be_u32()? as u64)),
            27 => Ok(Some(self.read_be_u64()?)),
            28..=30 => Err(Error::custom("cbor: reserved additional information")),
            31 => Ok(None),
            _ => Err(Error::custom("cbor: invalid additional information")),
        }
    }

    fn read_text(&mut self, additional: u8) -> Result<Cow<'de, str>> {
        if additional == 31 {
            // Indefinite-length text: concatenate definite chunks to break.
            let mut out = String::new();
            loop {
                let b = self.header()?;
                if b == 0xFF {
                    break;
                }
                if b >> 5 != 3 {
                    return Err(Error::custom(
                        "cbor: indefinite text chunk must be a text string",
                    ));
                }
                let chunk = self.read_text(b & 0x1F)?;
                out.push_str(&chunk);
            }
            return Ok(Cow::Owned(out));
        }
        let len = usize::try_from(self.argument(additional)?)
            .map_err(|_| Error::custom("cbor: text length exceeds platform limit"))?;
        let bytes = self.cur.take(len)?;
        let s = core::str::from_utf8(bytes)
            .map_err(|_| Error::custom("cbor: invalid utf-8 in text"))?;
        Ok(Cow::Borrowed(s))
    }

    fn read_bignum(&mut self, tag: u64) -> Result<Number> {
        let b = self.header()?;
        if b >> 5 != 2 || b & 0x1F == 31 {
            return Err(Error::custom(
                "cbor: bignum tag must be followed by a definite byte string",
            ));
        }
        let len = usize::try_from(self.argument(b & 0x1F)?)
            .map_err(|_| Error::custom("cbor: bignum magnitude too large"))?;
        if len > 16 {
            return Err(Error::custom("cbor: bignum exceeds 128 bits"));
        }
        let magnitude = self.cur.take(len)?;
        let mut value: u128 = 0;
        for &byte in magnitude {
            value = (value << 8) | byte as u128;
        }
        if tag == 2 {
            if value <= u64::MAX as u128 {
                Ok(Number::U64(value as u64))
            } else {
                Ok(Number::U128(value))
            }
        } else if value <= i64::MAX as u128 {
            Ok(Number::I64(-1 - value as i64))
        } else if value <= i128::MAX as u128 {
            Ok(Number::I128(-1 - value as i128))
        } else {
            Err(Error::custom("cbor: negative bignum exceeds i128"))
        }
    }

    fn read_simple(&mut self, additional: u8) -> Result<Token<'de>> {
        match additional {
            20 => Ok(Token::Bool(false)),
            21 => Ok(Token::Bool(true)),
            22 => Ok(Token::Null),
            25 => {
                let value = half_to_f32(self.read_be_u16()?) as f64;
                self.finite_float(value)
            }
            26 => {
                let value = f32::from_bits(self.read_be_u32()?) as f64;
                self.finite_float(value)
            }
            27 => {
                let value = f64::from_bits(self.read_be_u64()?);
                self.finite_float(value)
            }
            31 => Err(Error::custom("cbor: unexpected break marker")),
            _ => Err(Error::custom("cbor: unsupported simple value")),
        }
    }

    fn finite_float(&self, value: f64) -> Result<Token<'de>> {
        if !value.is_finite() {
            return Err(Error::custom(
                "non-finite CBOR float is not representable in the JSON-compatible profile",
            ));
        }
        Ok(Token::Number(Number::F64(value)))
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        let b = self.header()?;
        let major = b >> 5;
        let additional = b & 0x1F;
        match major {
            0 => Ok(Token::Number(Number::U64(self.argument(additional)?))),
            1 => {
                let argument = self.argument(additional)?;
                let value = if argument <= i64::MAX as u64 {
                    Number::I64(-1 - argument as i64)
                } else {
                    Number::I128(-1 - argument as i128)
                };
                Ok(Token::Number(value))
            }
            2 => Err(Error::custom(
                "CBOR byte strings are not representable in the JSON-compatible profile",
            )),
            3 => Ok(Token::Str(self.read_text(additional)?)),
            4 => {
                self.pending = Some(DFrame {
                    kind: FrameKind::Array,
                    remaining: self.container_argument(additional)?,
                });
                Ok(Token::BeginArray)
            }
            5 => {
                self.pending = Some(DFrame {
                    kind: FrameKind::Map,
                    remaining: self.container_argument(additional)?,
                });
                Ok(Token::BeginObject)
            }
            6 => {
                let tag = self.argument(additional)?;
                if tag != 2 && tag != 3 {
                    return Err(Error::custom("cbor: unsupported semantic tag"));
                }
                Ok(Token::Number(self.read_bignum(tag)?))
            }
            7 => self.read_simple(additional),
            _ => Err(Error::custom("cbor: invalid major type")),
        }
    }

    fn take_pending(&mut self, kind: FrameKind) -> Result<DFrame> {
        match self.pending.take() {
            Some(frame) if frame.kind == kind => Ok(frame),
            _ => Err(Error::custom("cbor: container header mismatch")),
        }
    }

    /// Whether the next item in the current indefinite container is the
    /// break marker (0xFF). Definite containers never inspect the wire here.
    fn at_break(&mut self) -> Result<bool> {
        if self.lookahead.is_some() {
            return Ok(false);
        }
        Ok(self.cur.peek()? == 0xFF)
    }
}

impl<'de> FormatDecoder<'de> for CborDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginObject => {
                let frame = self.take_pending(FrameKind::Map)?;
                self.frames.push(frame);
                Ok(())
            }
            other => Err(Error::invalid_type("a map", token_name(&other))),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("cbor: map end without start"))?;
        match frame.remaining {
            Some(0) => {}
            Some(_) => return Err(Error::custom("cbor: map entry count mismatch")),
            None => {
                // Indefinite map: the break marker terminates it.
                let b = self.header()?;
                if b != 0xFF {
                    return Err(Error::custom("cbor: indefinite map missing break"));
                }
            }
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("cbor: object key outside map"))?;
        match frame.remaining {
            Some(0) => return Ok(None),
            Some(n) => frame.remaining = Some(n - 1),
            None => {
                if self.at_break()? {
                    return Ok(None);
                }
            }
        }
        let b = self.header()?;
        if b >> 5 != 3 {
            return Err(Error::custom("CBOR map key must be a text string"));
        }
        self.read_text(b & 0x1F).map(Some)
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        let frame = self
            .frames
            .last()
            .ok_or_else(|| Error::custom("cbor: object separator outside map"))?;
        match frame.remaining {
            Some(n) => Ok(n > 0),
            None => Ok(!self.at_break()?),
        }
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => {
                let frame = self.take_pending(FrameKind::Array)?;
                self.frames.push(frame);
                Ok(())
            }
            other => Err(Error::invalid_type("an array", token_name(&other))),
        }
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("cbor: array end without start"))?;
        match frame.remaining {
            Some(0) => {}
            Some(_) => return Err(Error::custom("cbor: array element count mismatch")),
            None => {
                let b = self.header()?;
                if b != 0xFF {
                    return Err(Error::custom("cbor: indefinite array missing break"));
                }
            }
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        let frame = self
            .frames
            .last()
            .ok_or_else(|| Error::custom("cbor: array check outside array"))?;
        match frame.remaining {
            Some(n) => Ok(n > 0),
            None => Ok(!self.at_break()?),
        }
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("cbor: array separator outside array"))?;
        match frame.remaining {
            Some(n) if n > 0 => {
                frame.remaining = Some(n - 1);
                Ok(n - 1 > 0)
            }
            Some(_) => Ok(false),
            None => Ok(!self.at_break()?),
        }
    }

    fn array_len_hint(&self) -> Option<usize> {
        self.frames.last().and_then(|frame| {
            (frame.kind == FrameKind::Array).then(|| {
                usize::try_from(frame.remaining.unwrap_or(0))
                    .unwrap_or(usize::MAX)
                    .min(self.cur.remaining_len())
                    .min(MAX_CONTAINER_PREALLOC)
            })
        })
    }

    fn object_len_hint(&self) -> Option<usize> {
        self.frames.last().and_then(|frame| {
            (frame.kind == FrameKind::Map).then(|| {
                usize::try_from(frame.remaining.unwrap_or(0))
                    .unwrap_or(usize::MAX)
                    .min(self.cur.remaining_len())
                    .min(MAX_CONTAINER_PREALLOC)
            })
        })
    }

    fn unit(&mut self) -> Result<(), Self::Error> {
        match self.next_token()? {
            Token::Null => Ok(()),
            other => Err(Error::invalid_type("null", token_name(&other))),
        }
    }

    fn bool(&mut self) -> Result<bool, Self::Error> {
        match self.next_token()? {
            Token::Bool(b) => Ok(b),
            other => Err(Error::invalid_type("bool", token_name(&other))),
        }
    }

    fn number(&mut self) -> Result<Number, Self::Error> {
        match self.next_token()? {
            Token::Number(n) => Ok(n),
            other => Err(Error::invalid_type("number", token_name(&other))),
        }
    }

    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error> {
        match self.next_token()? {
            Token::Str(s) => Ok(s),
            other => Err(Error::invalid_type("string", token_name(&other))),
        }
    }

    fn char(&mut self) -> Result<char, Self::Error> {
        match self.next_token()? {
            Token::Str(s) => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(c),
                    _ => Err(Error::invalid_type("a single-character string", "string")),
                }
            }
            other => Err(Error::invalid_type("char", token_name(&other))),
        }
    }

    fn skip_value(&mut self) -> Result<(), Self::Error> {
        match self.peek_token()? {
            Token::BeginObject => {
                self.begin_object()?;
                while self.object_key()?.is_some() {
                    self.skip_value()?;
                    if !self.object_entry_sep()? {
                        break;
                    }
                }
                self.end_object()
            }
            Token::BeginArray => {
                self.begin_array()?;
                while self.array_has_more()? {
                    self.skip_value()?;
                    if !self.array_entry_sep()? {
                        break;
                    }
                }
                self.end_array()
            }
            _ => {
                self.next_token()?;
                Ok(())
            }
        }
    }

    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error> {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.read_token()?);
        }
        self.lookahead
            .clone()
            .ok_or_else(|| Error::custom("cbor: lookahead unavailable"))
    }

    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        if let Some(t) = self.lookahead.take() {
            return Ok(t);
        }
        self.read_token()
    }

    fn save(&self) -> Mark {
        Mark {
            pos: self.cur.pos(),
            depth: self.depth,
            frame_len: self.frames.len(),
        }
    }

    fn restore(&mut self, mark: Mark) {
        self.cur.seek(mark.pos);
        self.lookahead = None;
        self.pending = None;
        self.frames.truncate(mark.frame_len);
        self.depth = mark.depth;
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

//! UBJSON codec (Universal Binary JSON, v5 / Draft 12).
//!
//! Self-describing binary JSON. Every value is `[type marker][payload]`; the
//! "optimized" forms are the container parameters only:
//!
//! | Marker | Meaning |
//! |---|---|
//! | `Z` | null |
//! | `T` / `F` | true / false |
//! | `i` / `U` / `I` / `l` / `L` | int8 / uint8 / int16 / int32 / int64 (big-endian) |
//! | `d` / `D` | float32 / float64 (big-endian) |
//! | `H` | high-precision number (decimal string; used for >64-bit integers) |
//! | `C` | char (deprecated, still read) |
//! | `S` | string (integer length + UTF-8 bytes) |
//! | `[` / `]` | array start / end |
//! | `{` / `}` | object start / end |
//! | `N` | no-op (skipped) |
//! | `$` | typed container (all elements share one type) |
//! | `#` | container count (counted containers have **no** end marker) |
//!
//! Wire rules matched to `serde_ubjson` for byte-level interop:
//! - **Object keys** are `<integer length><utf-8 bytes>` with **no** `S`
//!   marker (UBJSON quirk).
//! - Integers cascade to the smallest type: `u64` emits `U` / `I` / `l` /
//!   `L` and `H` (decimal string) beyond 63 bits, mirroring
//!   `serde_ubjson`.
//! - Byte strings are the typed array `[ $U #n <raw> ` (no end marker).
//! - The encoder emits end-marker containers (`[ ... ]`, `{ ... }`); the
//!   decoder also accepts counted (`[ #n ...`) and typed-counted
//!   (`[ $t #n ...`) forms.
//!
//! The JSON-compatible profile rejects non-finite floats.

use alloc::borrow::Cow;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::bin::{Cursor, MAX_CONTAINER_PREALLOC};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// UBJSON format marker.
#[derive(Clone, Copy, Debug)]
pub struct Ubjson;

impl Format for Ubjson {
    const NAME: &'static str = "ubjson";
    const MIME: &'static str = "application/ubjson";
    const EXTENSIONS: &'static [&'static str] = &["ubj", "ubjson"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = UbjsonEncoder::new(Vec::new());
        // Trusted path (derive emits a well-formed stream); container
        // bookkeeping is structural and still rejects unbalanced output.
        T::nextencode(value, &mut encoder)?;
        Ok(encoder.finish_vec())
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = UbjsonDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Streaming UBJSON encoder (end-marker containers).
pub struct UbjsonEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
}

impl<W: Write> UbjsonEncoder<W> {
    /// Create a UBJSON encoder over `writer`.
    pub fn new(writer: W) -> Self {
        UbjsonEncoder {
            writer,
            buf: Vec::with_capacity(1024),
        }
    }

    fn push(&mut self, byte: u8) {
        self.buf.push(byte);
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Encode a non-negative integer as a length / count (smallest unsigned
    /// type; mirrors `serde_ubjson`'s length cascade).
    fn write_length(&mut self, value: u64) {
        if value <= u8::MAX as u64 {
            self.push(b'U');
            self.push(value as u8);
        } else if value <= i16::MAX as u64 {
            self.push(b'I');
            self.extend(&(value as u16).to_be_bytes());
        } else if value <= u32::MAX as u64 {
            self.push(b'l');
            self.extend(&(value as u32).to_be_bytes());
        } else {
            self.push(b'L');
            self.extend(&value.to_be_bytes());
        }
    }

    /// Encode a signed integer (smallest type; matches `serde_ubjson`).
    fn write_signed(&mut self, value: i64) {
        if (-128..=127).contains(&value) {
            self.push(b'i');
            self.push(value as u8);
        } else if (0..=u8::MAX as i64).contains(&value) {
            self.push(b'U');
            self.push(value as u8);
        } else if i16::try_from(value).is_ok() {
            self.push(b'I');
            self.extend(&(value as i16).to_be_bytes());
        } else if i32::try_from(value).is_ok() {
            self.push(b'l');
            self.extend(&(value as i32).to_be_bytes());
        } else {
            self.push(b'L');
            self.extend(&value.to_be_bytes());
        }
    }

    /// Encode a non-negative integer (smallest type; matches `serde_ubjson`
    /// for `u64`, which uses `H` beyond 63 bits).
    fn write_unsigned(&mut self, value: u64) {
        if value <= u8::MAX as u64 {
            self.push(b'U');
            self.push(value as u8);
        } else if value <= i16::MAX as u64 {
            self.push(b'I');
            self.extend(&(value as u16).to_be_bytes());
        } else if value <= i32::MAX as u64 {
            self.push(b'l');
            self.extend(&(value as u32).to_be_bytes());
        } else if value <= i64::MAX as u64 {
            self.push(b'L');
            self.extend(&value.to_be_bytes());
        } else {
            // Beyond signed 63-bit range: high-precision decimal string.
            self.write_high_precision(&value.to_string());
        }
    }

    /// Emit a high-precision number: `H` + length + decimal digits.
    fn write_high_precision(&mut self, text: &str) {
        self.push(b'H');
        self.write_length(text.len() as u64);
        self.extend(text.as_bytes());
    }

    fn write_string(&mut self, value: &str) {
        let bytes = value.as_bytes();
        self.push(b'S');
        self.write_length(bytes.len() as u64);
        self.extend(bytes);
    }

    /// Flush the internal buffer and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.writer.write_all(&self.buf)?;
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn finish_vec(mut self) -> Vec<u8> {
        core::mem::take(&mut self.buf)
    }
}

impl<W: Write> FormatEncoder for UbjsonEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.push(b'[');
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.push(b']');
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.push(b'{');
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        // UBJSON object keys are `<integer length><utf-8>` with no `S`
        // marker.
        self.write_length(key.len() as u64);
        self.extend(key.as_bytes());
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.push(b'}');
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.push(b'Z');
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.push(if value { b'T' } else { b'F' });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.write_string(value);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        let mut tmp = [0u8; 4];
        self.write_string(value.encode_utf8(&mut tmp));
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
        self.write_signed(value);
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.write_unsigned(value);
        Ok(())
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        match i64::try_from(value) {
            Ok(v) => {
                self.write_signed(v);
                Ok(())
            }
            Err(_) => {
                // Out of 64-bit range: high-precision decimal string.
                self.write_high_precision(&value.to_string());
                Ok(())
            }
        }
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        match u64::try_from(value) {
            Ok(v) => {
                self.write_unsigned(v);
                Ok(())
            }
            Err(_) => {
                self.write_high_precision(&value.to_string());
                Ok(())
            }
        }
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("ubjson: non-finite float cannot be encoded"));
        }
        self.push(b'D');
        self.extend(&value.to_be_bytes());
        Ok(())
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("ubjson: non-finite float cannot be encoded"));
        }
        self.push(b'd');
        self.extend(&value.to_be_bytes());
        Ok(())
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), Self::Error> {
        // Typed counted uint8 array: `[ $U #n <raw>` — no end marker.
        self.push(b'[');
        self.push(b'$');
        self.push(b'U');
        self.push(b'#');
        self.write_length(value.len() as u64);
        self.extend(value);
        Ok(())
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum DFrameKind {
    Array,
    Object,
}

#[derive(Clone, Copy)]
struct DFrame {
    kind: DFrameKind,
    /// Remaining elements/pairs in a counted container (`Some(0)` means the
    /// count is exhausted and no end marker follows).
    remaining: Option<u64>,
    /// Element type in a typed container (`$type`).
    typed: Option<u8>,
}

/// Streaming UBJSON decoder.
pub struct UbjsonDecoder<'de> {
    cur: Cursor<'de>,
    lookahead: Option<u8>,
    frames: Vec<DFrame>,
    depth: u32,
    max_depth: u32,
}

impl<'de> UbjsonDecoder<'de> {
    /// Create a UBJSON decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        UbjsonDecoder {
            cur: Cursor::new(input),
            lookahead: None,
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
        // Skip trailing no-op markers, tolerating clean EOF.
        while !self.cur.at_end() {
            if self.peek_byte()? != b'N' {
                break;
            }
            self.next_byte()?;
        }
        if self.frames.is_empty() && self.lookahead.is_none() && self.cur.at_end() {
            Ok(())
        } else {
            Err(Error::custom("ubjson: trailing bytes after value"))
        }
    }

    #[inline]
    fn peek_byte(&mut self) -> Result<u8> {
        if let Some(byte) = self.lookahead {
            return Ok(byte);
        }
        let byte = self.cur.peek()?;
        self.lookahead = Some(byte);
        Ok(byte)
    }

    #[inline]
    fn next_byte(&mut self) -> Result<u8> {
        if let Some(byte) = self.lookahead.take() {
            // The lookahead byte sits at the cursor position; consuming it
            // must advance the cursor past it.
            self.cur.seek(self.cur.pos() + 1);
            return Ok(byte);
        }
        self.cur.byte()
    }

    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("ubjson: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    /// Read the type marker for the next value.
    ///
    /// Inside a typed container the element type is pinned and no marker
    /// byte is consumed; elsewhere the marker byte is read (skipping no-ops).
    fn value_marker(&mut self) -> Result<u8> {
        if let Some(frame) = self.frames.last() {
            if let Some(typed) = frame.typed {
                if frame.kind == DFrameKind::Array {
                    // The cursor already points at the first raw payload
                    // byte; drop any pending peek of that same byte.
                    self.lookahead = None;
                    return Ok(typed);
                }
            }
        }
        loop {
            let marker = self.next_byte()?;
            if marker != b'N' {
                return Ok(marker);
            }
        }
    }

    /// Read an integer length / count (`U`/`I`/`l`/`L`, or `i` for
    /// tolerance). Negative values are invalid as lengths.
    fn read_length(&mut self) -> Result<u64> {
        let marker = self.next_byte()?;
        match marker {
            b'U' => Ok(self.cur.byte()? as u64),
            b'i' => {
                let value = self.cur.byte()? as i8;
                if value < 0 {
                    Err(Error::custom("ubjson: negative length"))
                } else {
                    Ok(value as u64)
                }
            }
            b'I' => Ok(self.cur.be_u16()? as u64),
            b'l' => {
                let value = self.cur.be_u32()? as i32;
                if value < 0 {
                    Err(Error::custom("ubjson: negative length"))
                } else {
                    Ok(value as u64)
                }
            }
            b'L' => Ok(self.cur.be_u64()?),
            other => Err(Error::custom(alloc::format!(
                "ubjson: expected integer length, got 0x{other:02x}"
            ))),
        }
    }

    fn read_number_marker(&mut self) -> Result<Number> {
        let marker = self.value_marker()?;
        match marker {
            b'i' => Ok(Number::from(self.cur.byte()? as i8)),
            b'U' => Ok(Number::U64(self.cur.byte()? as u64)),
            b'I' => Ok(Number::from(self.cur.be_u16()? as i16)),
            b'l' => Ok(Number::from(self.cur.be_u32()? as i32)),
            b'L' => Ok(Number::from(self.cur.be_u64()? as i64)),
            b'd' => {
                let raw = self.cur.take(4)?;
                let mut a = [0u8; 4];
                a.copy_from_slice(raw);
                Ok(Number::F64(f32::from_be_bytes(a) as f64))
            }
            b'D' => {
                let raw = self.cur.take(8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(raw);
                let value = f64::from_be_bytes(a);
                if !value.is_finite() {
                    return Err(Error::custom("ubjson: non-finite float"));
                }
                Ok(Number::F64(value))
            }
            b'H' => {
                let len = usize::try_from(self.read_length()?)
                    .map_err(|_| Error::custom("ubjson: high-precision length too large"))?;
                let text = self.cur.take(len)?;
                // High-precision payload is a decimal string; keep integer
                // values exact (floaty text takes the float path).
                let is_float = text.iter().any(|b| matches!(b, b'.' | b'e' | b'E'));
                let parsed = Number::parse(text, is_float)
                    .map_err(|_| Error::custom("ubjson: invalid high-precision number"))?;
                Ok(parsed)
            }
            other => Err(Error::custom(alloc::format!(
                "ubjson: expected number, got 0x{other:02x}"
            ))),
        }
    }

    /// Read a string value: `S` (marker + length + bytes) or `C` (char).
    fn read_string_marker(&mut self) -> Result<Cow<'de, str>> {
        let marker = self.value_marker()?;
        match marker {
            b'S' => {
                let len = usize::try_from(self.read_length()?)
                    .map_err(|_| Error::custom("ubjson: string length too large"))?;
                let bytes = self.cur.take(len)?;
                let s = core::str::from_utf8(bytes)
                    .map_err(|_| Error::custom("ubjson: invalid utf-8"))?;
                Ok(Cow::Borrowed(s))
            }
            b'C' => {
                let byte = self.cur.byte()?;
                let c = core::char::from_u32(byte as u32)
                    .ok_or_else(|| Error::custom("ubjson: invalid char byte"))?;
                let mut tmp = [0u8; 4];
                Ok(Cow::Owned(c.encode_utf8(&mut tmp).to_string()))
            }
            other => Err(Error::custom(alloc::format!(
                "ubjson: expected string, got 0x{other:02x}"
            ))),
        }
    }

    /// Read an object key: `<integer length><utf-8 bytes>` (no `S` marker).
    fn read_key(&mut self) -> Result<Cow<'de, str>> {
        let len = usize::try_from(self.read_length()?)
            .map_err(|_| Error::custom("ubjson: key length too large"))?;
        let bytes = self.cur.take(len)?;
        let s =
            core::str::from_utf8(bytes).map_err(|_| Error::custom("ubjson: invalid utf-8 key"))?;
        Ok(Cow::Borrowed(s))
    }

    /// Parse the container header (which may carry `$type` and/or `#count`).
    fn parse_container_header(&mut self, kind: DFrameKind) -> Result<DFrame> {
        let mut typed = None;
        let mut remaining = None;
        loop {
            match self.peek_byte()? {
                b'$' => {
                    self.next_byte()?;
                    typed = Some(self.next_byte()?);
                }
                b'#' => {
                    self.next_byte()?;
                    remaining = Some(self.read_length()?);
                }
                _ => break,
            }
        }
        Ok(DFrame {
            kind,
            remaining,
            typed,
        })
    }

    /// Consume a container terminator: `]`/`}` for end-marker containers,
    /// nothing for counted containers.
    fn take_end(&mut self, expected: u8, counted: bool) -> Result<()> {
        if counted {
            return Ok(());
        }
        let byte = self.next_byte()?;
        if byte != expected {
            return Err(Error::custom(alloc::format!(
                "ubjson: expected 0x{expected:02x}, got 0x{byte:02x}"
            )));
        }
        Ok(())
    }
}

impl<'de> FormatDecoder<'de> for UbjsonDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_byte()? {
            b'{' => {
                let frame = self.parse_container_header(DFrameKind::Object)?;
                self.frames.push(frame);
                Ok(())
            }
            other => Err(Error::invalid_type("a map", token_for(other))),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("ubjson: object end without start"))?;
        if frame.kind != DFrameKind::Object {
            return Err(Error::custom("ubjson: mismatched object end"));
        }
        self.take_end(b'}', frame.remaining.is_some())?;
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        {
            let frame = self
                .frames
                .last()
                .ok_or_else(|| Error::custom("ubjson: object key outside object"))?;
            if let Some(remaining) = frame.remaining {
                if remaining == 0 {
                    return Ok(None);
                }
            }
        }
        match self.peek_byte()? {
            b'}' => Ok(None),
            _ => {
                let key = self.read_key()?;
                if let Some(frame) = self.frames.last_mut() {
                    if let Some(remaining) = &mut frame.remaining {
                        *remaining -= 1;
                    }
                }
                Ok(Some(key))
            }
        }
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        if let Some(frame) = self.frames.last() {
            if let Some(remaining) = frame.remaining {
                return Ok(remaining > 0);
            }
        }
        Ok(self.peek_byte()? != b'}')
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_byte()? {
            b'[' => {
                let frame = self.parse_container_header(DFrameKind::Array)?;
                self.frames.push(frame);
                Ok(())
            }
            other => Err(Error::invalid_type("an array", token_for(other))),
        }
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("ubjson: array end without start"))?;
        if frame.kind != DFrameKind::Array {
            return Err(Error::custom("ubjson: mismatched array end"));
        }
        self.take_end(b']', frame.remaining.is_some())?;
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        let frame = self
            .frames
            .last()
            .ok_or_else(|| Error::custom("ubjson: array access outside array"))?;
        if let Some(remaining) = frame.remaining {
            Ok(remaining > 0)
        } else {
            Ok(self.peek_byte()? != b']')
        }
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        if let Some(frame) = self.frames.last_mut() {
            if let Some(remaining) = &mut frame.remaining {
                if *remaining > 0 {
                    *remaining -= 1;
                }
                return Ok(*remaining > 0);
            }
        }
        Ok(self.peek_byte()? != b']')
    }

    fn array_len_hint(&self) -> Option<usize> {
        self.frames.last().and_then(|frame| {
            frame.remaining.map(|remaining| {
                usize::try_from(remaining)
                    .unwrap_or(usize::MAX)
                    .min(self.cur.remaining_len())
                    .min(MAX_CONTAINER_PREALLOC)
            })
        })
    }

    fn object_len_hint(&self) -> Option<usize> {
        self.frames.last().and_then(|frame| {
            frame.remaining.map(|remaining| {
                usize::try_from(remaining)
                    .unwrap_or(usize::MAX)
                    .min(self.cur.remaining_len())
                    .min(MAX_CONTAINER_PREALLOC)
            })
        })
    }

    fn unit(&mut self) -> Result<(), Self::Error> {
        match self.value_marker()? {
            b'Z' => Ok(()),
            other => Err(Error::invalid_type("null", token_for(other))),
        }
    }

    fn bool(&mut self) -> Result<bool, Self::Error> {
        match self.value_marker()? {
            b'T' => Ok(true),
            b'F' => Ok(false),
            other => Err(Error::invalid_type("bool", token_for(other))),
        }
    }

    fn number(&mut self) -> Result<Number, Self::Error> {
        self.read_number_marker()
    }

    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error> {
        self.read_string_marker()
    }

    fn char(&mut self) -> Result<char, Self::Error> {
        let s = self.read_string_marker()?;
        let mut chars = s.chars();
        let c = chars
            .next()
            .ok_or_else(|| Error::custom("ubjson: empty char"))?;
        if chars.next().is_some() {
            return Err(Error::custom("ubjson: char is not a single scalar"));
        }
        Ok(c)
    }

    fn skip_value(&mut self) -> Result<(), Self::Error> {
        // Byte-directed recursion without constructing intermediate values.
        let marker = self.value_marker()?;
        match marker {
            b'Z' | b'T' | b'F' | b'N' => Ok(()),
            b'i' | b'U' => {
                self.cur.take(1)?;
                Ok(())
            }
            b'I' => {
                self.cur.take(2)?;
                Ok(())
            }
            b'l' => {
                self.cur.take(4)?;
                Ok(())
            }
            b'L' => {
                self.cur.take(8)?;
                Ok(())
            }
            b'd' => {
                self.cur.take(4)?;
                Ok(())
            }
            b'D' => {
                self.cur.take(8)?;
                Ok(())
            }
            b'S' | b'H' => {
                let len = usize::try_from(self.read_length()?)
                    .map_err(|_| Error::custom("ubjson: length too large"))?;
                self.cur.take(len)?;
                Ok(())
            }
            b'C' => {
                self.cur.take(1)?;
                Ok(())
            }
            b'[' => {
                self.enter_container()?;
                let frame = self.parse_container_header(DFrameKind::Array)?;
                self.frames.push(frame);
                while self.array_has_more()? {
                    self.skip_value()?;
                    self.array_entry_sep()?;
                }
                self.end_array()?;
                Ok(())
            }
            b'{' => {
                self.enter_container()?;
                let frame = self.parse_container_header(DFrameKind::Object)?;
                self.frames.push(frame);
                while self.object_key()?.is_some() {
                    self.skip_value()?;
                }
                self.end_object()?;
                Ok(())
            }
            other => Err(Error::custom(alloc::format!(
                "ubjson: cannot skip value 0x{other:02x}"
            ))),
        }
    }

    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error> {
        let byte = self.peek_byte()?;
        match byte {
            b'Z' => Ok(Token::Null),
            b'T' => Ok(Token::Bool(true)),
            b'F' => Ok(Token::Bool(false)),
            b'[' => Ok(Token::BeginArray),
            b'{' => Ok(Token::BeginObject),
            _ => {
                if let Some(frame) = self.frames.last() {
                    if let Some(typed) = frame.typed {
                        return Ok(marker_token(typed));
                    }
                }
                Ok(marker_token(byte))
            }
        }
    }

    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        let byte = self.value_marker()?;
        match byte {
            b'Z' => Ok(Token::Null),
            b'T' => Ok(Token::Bool(true)),
            b'F' => Ok(Token::Bool(false)),
            b'[' => {
                self.enter_container()?;
                let frame = self.parse_container_header(DFrameKind::Array)?;
                self.frames.push(frame);
                Ok(Token::BeginArray)
            }
            b'{' => {
                self.enter_container()?;
                let frame = self.parse_container_header(DFrameKind::Object)?;
                self.frames.push(frame);
                Ok(Token::BeginObject)
            }
            _ => Ok(marker_token(byte)),
        }
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
        self.frames.truncate(mark.frame_len);
        self.depth = mark.depth;
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

/// Map a marker byte to its [`Token`] shape (for error messages and peeks).
fn marker_token(byte: u8) -> Token<'static> {
    match byte {
        b'Z' => Token::Null,
        b'T' => Token::Bool(true),
        b'F' => Token::Bool(false),
        b'[' => Token::BeginArray,
        b'{' => Token::BeginObject,
        b']' => Token::EndArray,
        b'}' => Token::EndObject,
        b'S' | b'C' => Token::Str(Cow::Borrowed("")),
        b'i' | b'U' | b'I' | b'l' | b'L' | b'd' | b'D' | b'H' => Token::Number(Number::U64(0)),
        _ => Token::Str(Cow::Borrowed("")),
    }
}

fn token_for(byte: u8) -> &'static str {
    token_name(&marker_token(byte))
}

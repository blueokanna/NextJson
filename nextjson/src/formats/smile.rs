//! SMILE codec (Jackson Smile binary JSON, spec 1.0).
//!
//! Smile is a binary encoding of the JSON data model with a fixed 4-byte
//! header (`0x3A 0x29 0x0A <flags>`). This codec implements the documented
//! subset:
//!
//! - **Header**: version nibble 0 with the raw-binary flag set; shared
//!   string/name sharing is disabled on encode, so the stream is fully
//!   self-contained.
//! - **Values**: literals (`""`/`null`/`false`/`true`), small integers
//!   (zigzag, single byte `-16..=15`), 32/64-bit zigzag VInts, 32-bit
//!   float / 64-bit double (7-bit packed), tiny/short/long ASCII and Unicode
//!   strings.
//! - **Keys**: short ASCII/Unicode names plus long Unicode names.
//! - **Structures**: `START_ARRAY` / `END_ARRAY` / `START_OBJECT` /
//!   `END_OBJECT` markers; objects read in key mode.
//! - **Bytes**: the raw-binary form (`0xFD` + VInt length + raw bytes).
//! - **Decode side** additionally resolves short/long shared string and
//!   name references (for interop with sharing-enabled writers such as
//!   `serde_smile`).
//!
//! The JSON-compatible profile rejects non-finite floats.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

// ---- Wire constants (Smile spec 1.0) -------------------------------------

const SMILE_HEADER: [u8; 3] = [0x3A, 0x29, 0x0A];
/// Header flags: version nibble 0, raw-binary enabled, no sharing.
const SMILE_HEADER_FLAGS: u8 = 0x04;

const TOKEN_EMPTY_STRING: u8 = 0x20;
const TOKEN_NULL: u8 = 0x21;
const TOKEN_FALSE: u8 = 0x22;
const TOKEN_TRUE: u8 = 0x23;
const TOKEN_INT_32: u8 = 0x24;
const TOKEN_INT_64: u8 = 0x25;
const TOKEN_FLOAT_32: u8 = 0x28;
const TOKEN_FLOAT_64: u8 = 0x29;
const TOKEN_TINY_ASCII: u8 = 0x40; // + 5 LSB = len-1 (1..=32)
const TOKEN_SHORT_ASCII: u8 = 0x60; // + 5 LSB = len-33 (33..=64)
const TOKEN_TINY_UNICODE: u8 = 0x80; // + 5 LSB = byte-len-2 (2..=33)
const TOKEN_SHORT_UNICODE: u8 = 0xA0; // + 5 LSB = byte-len-34 (34..=64)
const TOKEN_SMALL_INT: u8 = 0xC0; // + 5 LSB = zigzag(-16..=15)
const TOKEN_LONG_ASCII: u8 = 0xE0;
const TOKEN_LONG_UNICODE: u8 = 0xE4;
const TOKEN_SHARED_VALUE_LONG: u8 = 0xEC;
const TOKEN_START_ARRAY: u8 = 0xF8;
const TOKEN_END_ARRAY: u8 = 0xF9;
const TOKEN_START_OBJECT: u8 = 0xFA;
const TOKEN_END_OBJECT: u8 = 0xFB;
const TOKEN_STRING_END: u8 = 0xFC;
const TOKEN_BINARY: u8 = 0xFD;
const TOKEN_END_CONTENT: u8 = 0xFF;

// Key-mode tokens.
const KEY_EMPTY: u8 = 0x20;
const KEY_LONG_SHARED: u8 = 0x30; // 0x30..=0x33 + 1 byte = 10-bit ref
const KEY_LONG_UNICODE: u8 = 0x34;
const KEY_SHORT_SHARED: u8 = 0x40; // 0x40..=0x7F = short key ref (6 LSB)
const KEY_SHORT_ASCII: u8 = 0x80; // 0x80..=0xBF = ASCII name len 1..=64
const KEY_SHORT_UNICODE: u8 = 0xC0; // 0xC0..=0xF7 = Unicode name len 2..=57

/// SMILE format marker.
#[derive(Clone, Copy, Debug)]
pub struct Smile;

impl Format for Smile {
    const NAME: &'static str = "smile";
    const MIME: &'static str = "application/x-jackson-smile";
    const EXTENSIONS: &'static [&'static str] = &["smile"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = SmileEncoder::new(Vec::new());
        // Trusted path; structural markers keep containers balanced.
        T::nextencode(value, &mut encoder)?;
        Ok(encoder.finish_vec())
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = SmileDecoder::new(input)?;
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// VInt / zigzag helpers (shared by encoder and decoder)
// ---------------------------------------------------------------------------

/// Write a big-endian VInt (non-last bytes MSB clear, last byte `0x80|..`).
///
/// Layout (Jackson-compatible):
/// - **≤9 bytes** carry at most 62 data bits: 8×7 non-final bytes plus a
///   6-bit final byte.
/// - **10 bytes** carry 64 data bits: 9×7 non-final bytes plus a 1-bit
///   final byte (the reader rejects a 10th byte with more than 1 data bit
///   as overflow).
///
/// So values ≥ 2^62 (bit 62 or 63 set) must take the 10-byte form; anything
/// smaller fits in ≤9 bytes.
fn write_vint(buf: &mut Vec<u8>, mut value: u64) {
    let mut tmp = [0u8; 10];
    let mut n;
    if value >> 62 != 0 {
        // 63/64-bit payload: 10 bytes; final byte holds 1 data bit.
        tmp[0] = 0x80 | (value & 1) as u8;
        n = 1;
        value >>= 1;
        for _ in 0..9 {
            tmp[n] = (value & 0x7F) as u8;
            n += 1;
            value >>= 7;
        }
        debug_assert_eq!(value, 0, "64-bit vint fully consumed");
    } else {
        // ≤62-bit payload: final byte holds up to 6 data bits.
        tmp[0] = 0x80 | (value & 0x3F) as u8;
        n = 1;
        value >>= 6;
        while value > 0 {
            tmp[n] = (value & 0x7F) as u8;
            n += 1;
            value >>= 7;
        }
    }
    for i in (0..n).rev() {
        buf.push(tmp[i]);
    }
}

/// Zigzag-encode a signed 64-bit value.
#[inline]
fn zigzag64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

/// Zigzag-encode a signed 32-bit value.
#[inline]
fn zigzag32(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

#[inline]
fn unzigzag64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// Pack a 32-bit float into 5 seven-bit bytes.
fn pack_f32(buf: &mut Vec<u8>, bits: u32) {
    for i in 0..5 {
        buf.push(((bits >> (28 - 7 * i)) & 0x7F) as u8);
    }
}

/// Pack a 64-bit double into 10 seven-bit bytes.
fn pack_f64(buf: &mut Vec<u8>, bits: u64) {
    for i in 0..10 {
        buf.push(((bits >> (63 - 7 * i)) & 0x7F) as u8);
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Streaming SMILE encoder.
pub struct SmileEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
}

impl<W: Write> SmileEncoder<W> {
    /// Create a SMILE encoder over `writer`.
    pub fn new(writer: W) -> Self {
        let mut buf = Vec::with_capacity(1024);
        buf.extend_from_slice(&SMILE_HEADER);
        buf.push(SMILE_HEADER_FLAGS);
        SmileEncoder { writer, buf }
    }

    fn push(&mut self, byte: u8) {
        self.buf.push(byte);
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Emit a string value using the most compact legal encoding.
    fn write_string(&mut self, value: &str) {
        let bytes = value.as_bytes();
        let len = bytes.len();
        if len == 0 {
            self.push(TOKEN_EMPTY_STRING);
            return;
        }
        let ascii = bytes.iter().all(|b| *b < 0x80);
        match (ascii, len) {
            (true, 1..=32) => {
                self.push(TOKEN_TINY_ASCII | (len as u8 - 1));
                self.extend(bytes);
            }
            (true, 33..=64) => {
                self.push(TOKEN_SHORT_ASCII | (len as u8 - 33));
                self.extend(bytes);
            }
            (true, _) => {
                self.push(TOKEN_LONG_ASCII);
                self.extend(bytes);
                self.push(TOKEN_STRING_END);
            }
            (false, 2..=33) => {
                self.push(TOKEN_TINY_UNICODE | (len as u8 - 2));
                self.extend(bytes);
            }
            (false, 34..=64) => {
                self.push(TOKEN_SHORT_UNICODE | (len as u8 - 34));
                self.extend(bytes);
            }
            (false, _) => {
                self.push(TOKEN_LONG_UNICODE);
                self.extend(bytes);
                self.push(TOKEN_STRING_END);
            }
        }
    }

    /// Emit an object key (property name).
    fn write_key(&mut self, key: &str) {
        let bytes = key.as_bytes();
        let len = bytes.len();
        if len == 0 {
            self.push(KEY_EMPTY);
            return;
        }
        let ascii = bytes.iter().all(|b| *b < 0x80);
        match (ascii, len) {
            (true, 1..=64) => {
                self.push(KEY_SHORT_ASCII | (len as u8 - 1));
                self.extend(bytes);
            }
            _ if len <= 57 => {
                self.push(KEY_SHORT_UNICODE | (len as u8 - 2));
                self.extend(bytes);
            }
            _ => {
                // Long (not-yet-shared) Unicode name.
                self.push(KEY_LONG_UNICODE);
                self.extend(bytes);
                self.push(TOKEN_STRING_END);
            }
        }
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

impl<W: Write> FormatEncoder for SmileEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.push(TOKEN_START_ARRAY);
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.push(TOKEN_END_ARRAY);
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.push(TOKEN_START_OBJECT);
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        self.write_key(key);
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.push(TOKEN_END_OBJECT);
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.push(TOKEN_NULL);
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.push(if value { TOKEN_TRUE } else { TOKEN_FALSE });
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
        if (-16..=15).contains(&value) {
            self.push(TOKEN_SMALL_INT | (zigzag32(value as i32) as u8 & 0x1F));
        } else if let Ok(value) = i32::try_from(value) {
            self.push(TOKEN_INT_32);
            write_vint(&mut self.buf, zigzag32(value) as u64);
        } else {
            self.push(TOKEN_INT_64);
            write_vint(&mut self.buf, zigzag64(value));
        }
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        match i64::try_from(value) {
            Ok(value) => self.write_i64(value),
            Err(_) => Err(Error::custom(
                "smile: u64 exceeds 63-bit signed range (use a smaller integer)",
            )),
        }
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        match i64::try_from(value) {
            Ok(v) => self.write_i64(v),
            Err(_) => Err(Error::custom(
                "smile: i128 out of 64-bit range (use a smaller integer)",
            )),
        }
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        match i64::try_from(value) {
            Ok(v) => self.write_i64(v),
            Err(_) => Err(Error::custom(
                "smile: u128 out of 64-bit range (use a smaller integer)",
            )),
        }
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("smile: non-finite float cannot be encoded"));
        }
        self.push(TOKEN_FLOAT_64);
        pack_f64(&mut self.buf, value.to_bits());
        Ok(())
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("smile: non-finite float cannot be encoded"));
        }
        self.push(TOKEN_FLOAT_32);
        pack_f32(&mut self.buf, value.to_bits());
        Ok(())
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), Self::Error> {
        self.push(TOKEN_BINARY);
        write_vint(&mut self.buf, value.len() as u64);
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

/// Streaming SMILE decoder.
pub struct SmileDecoder<'de> {
    input: &'de [u8],
    pos: usize,
    lookahead: Option<u8>,
    /// Shared value-string table (up to 1024 entries).
    shared_values: Vec<String>,
    /// Shared key-name table (up to 1024 entries).
    shared_keys: Vec<String>,
    /// Whether raw-binary tokens may appear (per header flag).
    raw_binary: bool,
    depth: u32,
    max_depth: u32,
}

impl<'de> SmileDecoder<'de> {
    /// Create a SMILE decoder, validating the 4-byte header.
    pub fn new(input: &'de [u8]) -> Result<Self> {
        if input.len() < 4 || input[..3] != SMILE_HEADER {
            return Err(Error::custom("smile: invalid header"));
        }
        let flags = input[3];
        if flags & 0x80 != 0 {
            return Err(Error::custom("smile: unsupported format version"));
        }
        Ok(SmileDecoder {
            input,
            pos: 4,
            lookahead: None,
            shared_values: Vec::new(),
            shared_keys: Vec::new(),
            raw_binary: flags & 0x04 != 0,
            depth: 0,
            max_depth: 128,
        })
    }

    /// Validate that the whole input was consumed (optional `0xFF` end marker
    /// is allowed).
    pub fn end(&mut self) -> Result<()> {
        self.expect_end()
    }

    fn expect_end(&mut self) -> Result<()> {
        if self.pos >= self.input.len() {
            return Ok(());
        }
        let byte = self.peek_byte()?;
        if byte == TOKEN_END_CONTENT {
            self.next_byte()?;
        }
        if self.pos >= self.input.len() {
            Ok(())
        } else {
            Err(Error::custom("smile: trailing bytes after value"))
        }
    }

    #[inline]
    fn peek_byte(&mut self) -> Result<u8> {
        if let Some(byte) = self.lookahead {
            return Ok(byte);
        }
        let byte = *self
            .input
            .get(self.pos)
            .ok_or_else(|| Error::custom("smile: unexpected end of input"))?;
        self.lookahead = Some(byte);
        Ok(byte)
    }

    #[inline]
    fn next_byte(&mut self) -> Result<u8> {
        if let Some(byte) = self.lookahead.take() {
            // The lookahead byte sits at `self.pos`; consuming it must
            // advance the position past it.
            self.pos += 1;
            return Ok(byte);
        }
        let byte = *self
            .input
            .get(self.pos)
            .ok_or_else(|| Error::custom("smile: unexpected end of input"))?;
        self.pos += 1;
        Ok(byte)
    }

    #[inline]
    fn take(&mut self, n: usize) -> Result<&'de [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| Error::custom("smile: offset overflow"))?;
        let slice = self
            .input
            .get(self.pos..end)
            .ok_or_else(|| Error::custom("smile: truncated data"))?;
        self.pos = end;
        Ok(slice)
    }

    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("smile: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    /// Read a big-endian VInt (max 10 bytes).
    ///
    /// 1–9-byte VInts carry 7 data bits per non-last byte and 6 in the
    /// final byte; a 10-byte VInt carries 64 data bits as 9×7 plus a 1-bit
    /// final byte (the writer's `write_vint` matches). The reader mirrors
    /// Jackson's layout so 64-bit values round-trip exactly.
    fn read_vint(&mut self) -> Result<u64> {
        let mut value: u64 = 0;
        let mut count = 0;
        loop {
            let byte = self.next_byte()?;
            count += 1;
            if count > 10 {
                return Err(Error::custom("smile: vint too long"));
            }
            if byte & 0x80 != 0 {
                // Final byte. A 10-byte VInt has already accumulated 63
                // data bits, so only 1 more bit fits; anything larger is
                // an overflow (Jackson-compatible check).
                if count == 10 {
                    if (byte & 0x3F) > 1 {
                        return Err(Error::custom("smile: vint overflow"));
                    }
                    value = (value << 1) | (byte & 1) as u64;
                } else {
                    value = (value << 6) | (byte & 0x3F) as u64;
                }
                break;
            }
            value = (value << 7) | (byte & 0x7F) as u64;
        }
        Ok(value)
    }

    /// Read a variable-length string terminated by `0xFC`.
    fn read_terminated_string(&mut self) -> Result<Cow<'de, str>> {
        let start = self.pos;
        // Borrow the raw slice and find the terminator.
        let tail = &self.input[start..];
        let found = tail
            .iter()
            .position(|b| *b == TOKEN_STRING_END)
            .ok_or_else(|| Error::custom("smile: unterminated long string"))?;
        let raw = &tail[..found];
        let s = core::str::from_utf8(raw).map_err(|_| Error::custom("smile: invalid utf-8"))?;
        self.pos = start + found + 1;
        Ok(Cow::Borrowed(s))
    }

    /// Read a fixed-length string value (ASCII or Unicode short forms).
    fn read_string_body(&mut self, len: usize) -> Result<Cow<'de, str>> {
        let raw = self.take(len)?;
        let s = core::str::from_utf8(raw).map_err(|_| Error::custom("smile: invalid utf-8"))?;
        Ok(Cow::Borrowed(s))
    }

    /// Read a value-mode string token and resolve shared references.
    fn read_string(&mut self, token: u8) -> Result<Cow<'de, str>> {
        let (len, share) = match token {
            TOKEN_EMPTY_STRING => return Ok(Cow::Borrowed("")),
            0x01..=0x1F => {
                let index = (token & 0x1F) as usize - 1;
                let s = self
                    .shared_values
                    .get(index)
                    .ok_or_else(|| Error::custom("smile: invalid shared string reference"))?
                    .clone();
                return Ok(Cow::Owned(s));
            }
            TOKEN_SHARED_VALUE_LONG => {
                let hi = (token & 0x03) as usize;
                let lo = self.next_byte()? as usize;
                let index = (hi << 8) | lo;
                if !(32..1024).contains(&index) {
                    return Err(Error::custom("smile: invalid long shared string reference"));
                }
                let s = self
                    .shared_values
                    .get(index.saturating_sub(32))
                    .ok_or_else(|| Error::custom("smile: invalid shared string reference"))?
                    .clone();
                return Ok(Cow::Owned(s));
            }
            TOKEN_TINY_ASCII..=0x5F => ((token & 0x1F) as usize + 1, true),
            TOKEN_SHORT_ASCII..=0x7F => ((token & 0x1F) as usize + 33, true),
            TOKEN_TINY_UNICODE..=0x9F => ((token & 0x1F) as usize + 2, true),
            TOKEN_SHORT_UNICODE..=0xBF => ((token & 0x1F) as usize + 34, true),
            TOKEN_LONG_ASCII | TOKEN_LONG_UNICODE => {
                let s = self.read_terminated_string()?;
                return Ok(s);
            }
            _ => {
                return Err(Error::custom(alloc::format!(
                    "smile: expected string value, got 0x{token:02x}"
                )));
            }
        };
        let s = self.read_string_body(len)?;
        // Referenceable strings (byte length <= 64) enter the shared table.
        if share && len <= 64 {
            if self.shared_values.len() >= 1024 {
                self.shared_values.clear();
            }
            self.shared_values.push(s.to_string());
        }
        Ok(s)
    }

    /// Read a value-mode number token.
    fn read_number(&mut self, token: u8) -> Result<Number> {
        match token {
            // `Number::from` normalizes non-negative values to `U64` so the
            // decoded value matches `Value::from(n)` (equality invariant).
            TOKEN_SMALL_INT..=0xDF => Ok(Number::from(unzigzag64((token & 0x1F) as u64))),
            TOKEN_INT_32 => {
                let v = self.read_vint()?;
                Ok(Number::from(unzigzag64(v) as i32))
            }
            TOKEN_INT_64 => {
                let v = self.read_vint()?;
                Ok(Number::from(unzigzag64(v)))
            }
            TOKEN_FLOAT_32 => {
                let raw = self.take(5)?;
                let mut bits: u32 = 0;
                for (i, byte) in raw.iter().enumerate() {
                    bits |= (*byte as u32) << (28 - 7 * i);
                }
                Ok(Number::F64(f32::from_bits(bits) as f64))
            }
            TOKEN_FLOAT_64 => {
                let raw = self.take(10)?;
                let mut bits: u64 = 0;
                for (i, byte) in raw.iter().enumerate() {
                    bits |= (*byte as u64) << (63 - 7 * i);
                }
                let value = f64::from_bits(bits);
                if !value.is_finite() {
                    return Err(Error::custom("smile: non-finite float"));
                }
                Ok(Number::F64(value))
            }
            _ => Err(Error::custom(alloc::format!(
                "smile: expected number, got 0x{token:02x}"
            ))),
        }
    }

    /// Read an object key token (key mode); returns `None` at `END_OBJECT`.
    fn read_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        let token = self.next_byte()?;
        match token {
            TOKEN_END_OBJECT => Ok(None),
            KEY_EMPTY => Ok(Some(Cow::Borrowed(""))),
            KEY_SHORT_SHARED..=0x7F => {
                // Short shared key name reference (index 0..=63).
                let index = (token & 0x3F) as usize;
                let s = self
                    .shared_keys
                    .get(index)
                    .ok_or_else(|| Error::custom("smile: invalid shared key reference"))?
                    .clone();
                Ok(Some(Cow::Owned(s)))
            }
            KEY_LONG_SHARED..=0x33 => {
                let hi = (token & 0x03) as usize;
                let lo = self.next_byte()? as usize;
                let index = (hi << 8) | lo;
                if !(64..1024).contains(&index) {
                    return Err(Error::custom("smile: invalid long shared key reference"));
                }
                let s = self
                    .shared_keys
                    .get(index.saturating_sub(64))
                    .ok_or_else(|| Error::custom("smile: invalid shared key reference"))?
                    .clone();
                Ok(Some(Cow::Owned(s)))
            }
            KEY_SHORT_ASCII..=0xBF => {
                let len = (token & 0x3F) as usize + 1;
                let s = self.read_string_body(len)?;
                self.shared_keys.push(s.to_string());
                Ok(Some(s))
            }
            KEY_SHORT_UNICODE..=0xF7 => {
                let len = (token & 0x3F) as usize + 2;
                let s = self.read_string_body(len)?;
                self.shared_keys.push(s.to_string());
                Ok(Some(s))
            }
            KEY_LONG_UNICODE => {
                let s = self.read_terminated_string()?;
                Ok(Some(s))
            }
            other => Err(Error::custom(alloc::format!(
                "smile: invalid key token 0x{other:02x}"
            ))),
        }
    }

    /// Skip any one value (byte-directed recursion).
    fn skip_value(&mut self) -> Result<()> {
        let token = self.next_byte()?;
        match token {
            TOKEN_NULL | TOKEN_TRUE | TOKEN_FALSE | TOKEN_EMPTY_STRING => Ok(()),
            TOKEN_SMALL_INT..=0xDF => Ok(()),
            TOKEN_INT_32 | TOKEN_INT_64 => {
                self.read_vint()?;
                Ok(())
            }
            TOKEN_FLOAT_32 => {
                self.take(5)?;
                Ok(())
            }
            TOKEN_FLOAT_64 => {
                self.take(10)?;
                Ok(())
            }
            TOKEN_TINY_ASCII..=0x5F => {
                self.take((token & 0x1F) as usize + 1)?;
                Ok(())
            }
            TOKEN_SHORT_ASCII..=0x7F => {
                self.take((token & 0x1F) as usize + 33)?;
                Ok(())
            }
            TOKEN_TINY_UNICODE..=0x9F => {
                self.take((token & 0x1F) as usize + 2)?;
                Ok(())
            }
            TOKEN_SHORT_UNICODE..=0xBF => {
                self.take((token & 0x1F) as usize + 34)?;
                Ok(())
            }
            TOKEN_LONG_ASCII | TOKEN_LONG_UNICODE => {
                self.read_terminated_string()?;
                Ok(())
            }
            0x01..=0x1F | TOKEN_SHARED_VALUE_LONG => {
                // Resolve the reference to advance the cursor.
                self.read_string(token)?;
                Ok(())
            }
            TOKEN_BINARY => {
                if !self.raw_binary {
                    return Err(Error::custom("smile: raw binary not enabled by header"));
                }
                let len = self.read_vint()?;
                let len = usize::try_from(len)
                    .map_err(|_| Error::custom("smile: binary length too large"))?;
                self.take(len)?;
                Ok(())
            }
            TOKEN_START_ARRAY => {
                self.enter_container()?;
                while self.array_has_more()? {
                    self.skip_value()?;
                }
                self.end_array()?;
                Ok(())
            }
            TOKEN_START_OBJECT => {
                self.enter_container()?;
                loop {
                    match self.read_key()? {
                        None => break,
                        Some(_) => self.skip_value()?,
                    }
                }
                self.depth = self.depth.saturating_sub(1);
                Ok(())
            }
            other => Err(Error::custom(alloc::format!(
                "smile: cannot skip token 0x{other:02x}"
            ))),
        }
    }
}

impl<'de> FormatDecoder<'de> for SmileDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_byte()? {
            TOKEN_START_OBJECT => Ok(()),
            other => Err(Error::invalid_type("a map", token_name(&token_for(other)))),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        match self.next_byte()? {
            TOKEN_END_OBJECT => {}
            other => {
                return Err(Error::custom(alloc::format!(
                    "smile: expected end of object, got 0x{other:02x}"
                )));
            }
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        match self.peek_byte()? {
            TOKEN_END_OBJECT => Ok(None),
            _ => self.read_key(),
        }
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        Ok(self.peek_byte()? != TOKEN_END_OBJECT)
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_byte()? {
            TOKEN_START_ARRAY => Ok(()),
            other => Err(Error::invalid_type(
                "an array",
                token_name(&token_for(other)),
            )),
        }
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        match self.next_byte()? {
            TOKEN_END_ARRAY => {}
            other => {
                return Err(Error::custom(alloc::format!(
                    "smile: expected end of array, got 0x{other:02x}"
                )));
            }
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        Ok(self.peek_byte()? != TOKEN_END_ARRAY)
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        Ok(self.peek_byte()? != TOKEN_END_ARRAY)
    }

    fn unit(&mut self) -> Result<(), Self::Error> {
        match self.next_byte()? {
            TOKEN_NULL => Ok(()),
            other => Err(Error::invalid_type("null", token_name(&token_for(other)))),
        }
    }

    fn bool(&mut self) -> Result<bool, Self::Error> {
        match self.next_byte()? {
            TOKEN_TRUE => Ok(true),
            TOKEN_FALSE => Ok(false),
            other => Err(Error::invalid_type("bool", token_name(&token_for(other)))),
        }
    }

    fn number(&mut self) -> Result<Number, Self::Error> {
        let token = self.next_byte()?;
        self.read_number(token)
    }

    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error> {
        let token = self.next_byte()?;
        self.read_string(token)
    }

    fn char(&mut self) -> Result<char, Self::Error> {
        let token = self.next_byte()?;
        let s = self.read_string(token)?;
        let mut chars = s.chars();
        let c = chars
            .next()
            .ok_or_else(|| Error::custom("smile: empty char"))?;
        if chars.next().is_some() {
            return Err(Error::custom("smile: char is not a single scalar"));
        }
        Ok(c)
    }

    fn skip_value(&mut self) -> Result<(), Self::Error> {
        self.skip_value()
    }

    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error> {
        let byte = self.peek_byte()?;
        Ok(token_for(byte))
    }

    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        let token = self.next_byte()?;
        match token {
            TOKEN_NULL => Ok(Token::Null),
            TOKEN_TRUE => Ok(Token::Bool(true)),
            TOKEN_FALSE => Ok(Token::Bool(false)),
            TOKEN_START_ARRAY => {
                self.enter_container()?;
                Ok(Token::BeginArray)
            }
            TOKEN_START_OBJECT => {
                self.enter_container()?;
                Ok(Token::BeginObject)
            }
            TOKEN_END_ARRAY => Ok(Token::EndArray),
            TOKEN_END_OBJECT => Ok(Token::EndObject),
            _ => Ok(token_for(token)),
        }
    }

    fn save(&self) -> Mark {
        Mark {
            pos: self.pos,
            depth: self.depth,
            frame_len: self.shared_keys.len(),
        }
    }

    fn restore(&mut self, mark: Mark) {
        self.pos = mark.pos;
        self.lookahead = None;
        self.shared_keys.truncate(mark.frame_len);
        self.depth = mark.depth;
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

/// Map a SMILE token byte to its [`Token`] shape (for error messages, peeks
/// and type-mismatch reporting).
fn token_for(byte: u8) -> Token<'static> {
    match byte {
        TOKEN_NULL => Token::Null,
        TOKEN_TRUE => Token::Bool(true),
        TOKEN_FALSE => Token::Bool(false),
        TOKEN_START_ARRAY => Token::BeginArray,
        TOKEN_END_ARRAY => Token::EndArray,
        TOKEN_START_OBJECT => Token::BeginObject,
        TOKEN_END_OBJECT => Token::EndObject,
        TOKEN_SMALL_INT..=0xDF | TOKEN_INT_32 | TOKEN_INT_64 | TOKEN_FLOAT_32 | TOKEN_FLOAT_64 => {
            Token::Number(Number::U64(0))
        }
        TOKEN_EMPTY_STRING
        | TOKEN_TINY_ASCII..=TOKEN_SHORT_ASCII
        | TOKEN_TINY_UNICODE..=TOKEN_SHORT_UNICODE
        | TOKEN_LONG_ASCII
        | TOKEN_LONG_UNICODE
        | 0x01..=0x1F
        | TOKEN_SHARED_VALUE_LONG => Token::Str(Cow::Borrowed("")),
        _ => Token::Str(Cow::Borrowed("")),
    }
}

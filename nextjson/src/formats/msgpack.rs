//! MessagePack codec (RFC 8949-free, self-describing compact binary).
//!
//! Implements the MessagePack scalar and container families used by the
//! JSON-compatible data model:
//! nil, booleans, integers (int8/16/32/64, uint8/16/32/64), floats (32/64),
//! strings (fixstr/str8/16/32), arrays (fixarray/16/32) and maps
//! (fixmap/16/32). 128-bit integers that do not fit in 64-bit are rejected
//! with a clear error because MessagePack has no native 128-bit type; values
//! that fit are emitted losslessly.
//!
//! Binary and extension families are outside this codec's documented subset.
//! Interoperability is covered by explicit foreign-wire fixtures.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::bin::{patch_prefix, Cursor, MAX_CONTAINER_PREALLOC};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// MessagePack format marker.
#[derive(Clone, Copy, Debug)]
pub struct MsgPack;

impl Format for MsgPack {
    const NAME: &'static str = "msgpack";
    const MIME: &'static str = "application/msgpack";
    const EXTENSIONS: &'static [&'static str] = &["msgpack", "mp"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = MsgPackEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        Ok(encoder.finish_vec())
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = MsgPackDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum FrameKind {
    Array,
    Map,
}

struct Frame {
    start: usize,
    kind: FrameKind,
    count: u64,
}

/// Streaming MessagePack encoder.
pub struct MsgPackEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    frames: Vec<Frame>,
}

impl<W: Write> MsgPackEncoder<W> {
    /// Create a MessagePack encoder over `writer`.
    pub fn new(writer: W) -> Self {
        MsgPackEncoder {
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

    fn write_string(&mut self, value: &str) -> Result<()> {
        let bytes = value.as_bytes();
        let len = bytes.len();
        if len <= 31 {
            self.push(0xA0 | len as u8);
        } else if len <= u8::MAX as usize {
            self.push(0xD9);
            self.push(len as u8);
        } else if len <= u16::MAX as usize {
            self.push(0xDA);
            self.extend(&(len as u16).to_be_bytes());
        } else if let Ok(len) = u32::try_from(len) {
            self.push(0xDB);
            self.extend(&len.to_be_bytes());
        } else {
            return Err(Error::custom("msgpack: string exceeds u32 wire limit"));
        }
        self.extend(bytes);
        Ok(())
    }

    fn patch_container(&mut self, frame: Frame) -> Result<()> {
        if frame.count > u32::MAX as u64 {
            return Err(Error::custom("msgpack: container exceeds u32 wire limit"));
        }
        match frame.kind {
            FrameKind::Array => {
                let count = frame.count;
                if count <= 15 {
                    self.buf[frame.start] = 0x90 | count as u8;
                } else if count <= u16::MAX as u64 {
                    let mut header = [0u8; 3];
                    header[0] = 0xDC;
                    header[1..].copy_from_slice(&(count as u16).to_be_bytes());
                    patch_prefix(&mut self.buf, frame.start, &header);
                } else {
                    let mut header = [0u8; 5];
                    header[0] = 0xDD;
                    header[1..].copy_from_slice(&(count as u32).to_be_bytes());
                    patch_prefix(&mut self.buf, frame.start, &header);
                }
            }
            FrameKind::Map => {
                let count = frame.count;
                if count <= 15 {
                    self.buf[frame.start] = 0x80 | count as u8;
                } else if count <= u16::MAX as u64 {
                    let mut header = [0u8; 3];
                    header[0] = 0xDE;
                    header[1..].copy_from_slice(&(count as u16).to_be_bytes());
                    patch_prefix(&mut self.buf, frame.start, &header);
                } else {
                    let mut header = [0u8; 5];
                    header[0] = 0xDF;
                    header[1..].copy_from_slice(&(count as u32).to_be_bytes());
                    patch_prefix(&mut self.buf, frame.start, &header);
                }
            }
        }
        Ok(())
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

impl<W: Write> FormatEncoder for MsgPackEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.frames.push(Frame {
            start: self.buf.len(),
            kind: FrameKind::Array,
            count: 0,
        });
        self.buf.push(0x90); // placeholder fixarray
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        if let Some(frame) = self.frames.last_mut() {
            frame.count = frame
                .count
                .checked_add(1)
                .ok_or_else(|| Error::custom("msgpack: array length overflow"))?;
        }
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("msgpack: array end without start"))?;
        self.patch_container(frame)
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.frames.push(Frame {
            start: self.buf.len(),
            kind: FrameKind::Map,
            count: 0,
        });
        self.buf.push(0x80); // placeholder fixmap
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        if let Some(frame) = self.frames.last_mut() {
            frame.count = frame
                .count
                .checked_add(1)
                .ok_or_else(|| Error::custom("msgpack: map length overflow"))?;
        }
        self.write_string(key)
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("msgpack: map end without start"))?;
        self.patch_container(frame)
    }

    fn map_key<K: crate::ser::NsonSerialize>(&mut self, key: &K) -> Result<(), Self::Error> {
        if let Some(frame) = self.frames.last_mut() {
            frame.count = frame
                .count
                .checked_add(1)
                .ok_or_else(|| Error::custom("msgpack: map length overflow"))?;
        }
        K::nextencode(key, self)
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.push(0xC0);
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.push(if value { 0xC3 } else { 0xC2 });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.write_string(value)
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        let mut buf = [0u8; 4];
        self.write_string(value.encode_utf8(&mut buf))
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
        if (-32..=127).contains(&value) {
            self.push(value as u8);
        } else if (-128..=127).contains(&value) {
            self.push(0xD0);
            self.push(value as u8);
        } else if i16::try_from(value).is_ok() {
            self.push(0xD1);
            self.extend(&(value as i16).to_be_bytes());
        } else if i32::try_from(value).is_ok() {
            self.push(0xD2);
            self.extend(&(value as i32).to_be_bytes());
        } else {
            self.push(0xD3);
            self.extend(&value.to_be_bytes());
        }
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        if value <= 127 {
            self.push(value as u8);
        } else if value <= u8::MAX as u64 {
            self.push(0xCC);
            self.push(value as u8);
        } else if value <= u16::MAX as u64 {
            self.push(0xCD);
            self.extend(&(value as u16).to_be_bytes());
        } else if value <= u32::MAX as u64 {
            self.push(0xCE);
            self.extend(&(value as u32).to_be_bytes());
        } else {
            self.push(0xCF);
            self.extend(&value.to_be_bytes());
        }
        Ok(())
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        match i64::try_from(value) {
            Ok(v) => self.write_i64(v),
            Err(_) => Err(Error::custom(
                "msgpack: i128 out of 64-bit range (use a smaller integer)",
            )),
        }
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        match u64::try_from(value) {
            Ok(v) => self.write_u64(v),
            Err(_) => Err(Error::custom(
                "msgpack: u128 out of 64-bit range (use a smaller integer)",
            )),
        }
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        self.push(0xCB);
        self.extend(&value.to_be_bytes());
        Ok(())
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        self.push(0xCA);
        self.extend(&value.to_be_bytes());
        Ok(())
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), Self::Error> {
        let len = u32::try_from(value.len())
            .map_err(|_| Error::custom("msgpack: byte string exceeds u32 wire limit"))?;
        if len <= u8::MAX as u32 {
            self.push(0xC4);
            self.push(len as u8);
        } else if len <= u16::MAX as u32 {
            self.push(0xC5);
            self.extend(&(len as u16).to_be_bytes());
        } else {
            self.push(0xC6);
            self.extend(&len.to_be_bytes());
        }
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
enum CFrameKind {
    Array,
    Map,
}

#[derive(Clone, Copy)]
struct CFrame {
    kind: CFrameKind,
    remaining: u64,
}

/// Streaming MessagePack decoder.
pub struct MsgPackDecoder<'de> {
    cur: Cursor<'de>,
    lookahead: Option<Token<'de>>,
    pending: Option<CFrame>,
    frames: Vec<CFrame>,
    depth: u32,
    max_depth: u32,
}

impl<'de> MsgPackDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        MsgPackDecoder {
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
            Err(Error::custom("msgpack: trailing bytes after value"))
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
            return Err(Error::custom("msgpack: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn read_str(&mut self) -> Result<Cow<'de, str>> {
        let len = self.read_str_len()?;
        let bytes = self.cur.take(len)?;
        let s = core::str::from_utf8(bytes).map_err(|_| Error::custom("msgpack: invalid utf-8"))?;
        Ok(Cow::Borrowed(s))
    }

    fn read_str_len(&mut self) -> Result<usize> {
        let b = self.header()?;
        match b {
            0xA0..=0xBF => Ok((b & 0x1F) as usize),
            0xD9 => Ok(self.read_u8()? as usize),
            0xDA => Ok(self.read_be_u16()? as usize),
            0xDB => usize::try_from(self.read_be_u32()?)
                .map_err(|_| Error::custom("msgpack: string length exceeds platform limit")),
            other => Err(Error::custom(alloc::format!(
                "msgpack: expected string header, got 0x{other:02x}"
            ))),
        }
    }

    fn read_number_value(&mut self) -> Result<Number> {
        let b = self.header()?;
        match b {
            0x00..=0x7F => Ok(Number::U64(b as u64)),
            0xCC => Ok(Number::U64(self.read_u8()? as u64)),
            0xCD => Ok(Number::U64(self.read_be_u16()? as u64)),
            0xCE => Ok(Number::U64(self.read_be_u32()? as u64)),
            0xCF => Ok(Number::U64(self.read_be_u64()?)),
            0xE0..=0xFF => Ok(Number::I64((b as i8) as i64)),
            0xD0 => {
                let v = self.read_u8()? as i8 as i64;
                Ok(Number::from(v))
            }
            0xD1 => {
                let v = self.read_be_u16()? as i16 as i64;
                Ok(Number::from(v))
            }
            0xD2 => {
                let v = self.read_be_u32()? as i32 as i64;
                Ok(Number::from(v))
            }
            0xD3 => {
                let v = self.read_be_u64()? as i64;
                Ok(Number::from(v))
            }
            0xCA => {
                let bytes = self.cur.take(4)?;
                let mut a = [0u8; 4];
                a.copy_from_slice(bytes);
                Ok(Number::F64(f32::from_be_bytes(a) as f64))
            }
            0xCB => {
                let bytes = self.cur.take(8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(bytes);
                Ok(Number::F64(f64::from_be_bytes(a)))
            }
            other => Err(Error::custom(alloc::format!(
                "msgpack: expected number, got 0x{other:02x}"
            ))),
        }
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        let b = self.header()?;
        match b {
            0xC0 => Ok(Token::Null),
            0xC2 => Ok(Token::Bool(false)),
            0xC3 => Ok(Token::Bool(true)),
            0x00..=0x7F | 0xCC..=0xCF | 0xD0..=0xD3 | 0xCA..=0xCB | 0xE0..=0xFF => {
                // We consumed the type byte; rewind so read_number_value sees it.
                self.cur.rewind(1);
                Ok(Token::Number(self.read_number_value()?))
            }
            0xA0..=0xBF | 0xD9..=0xDB => {
                self.cur.rewind(1);
                Ok(Token::Str(self.read_str()?))
            }
            0x90..=0x9F => {
                self.pending = Some(CFrame {
                    kind: CFrameKind::Array,
                    remaining: (b & 0x0F) as u64,
                });
                Ok(Token::BeginArray)
            }
            0xDC => {
                self.pending = Some(CFrame {
                    kind: CFrameKind::Array,
                    remaining: self.read_be_u16()? as u64,
                });
                Ok(Token::BeginArray)
            }
            0xDD => {
                self.pending = Some(CFrame {
                    kind: CFrameKind::Array,
                    remaining: self.read_be_u32()? as u64,
                });
                Ok(Token::BeginArray)
            }
            0x80..=0x8F => {
                self.pending = Some(CFrame {
                    kind: CFrameKind::Map,
                    remaining: (b & 0x0F) as u64,
                });
                Ok(Token::BeginObject)
            }
            0xDE => {
                self.pending = Some(CFrame {
                    kind: CFrameKind::Map,
                    remaining: self.read_be_u16()? as u64,
                });
                Ok(Token::BeginObject)
            }
            0xDF => {
                self.pending = Some(CFrame {
                    kind: CFrameKind::Map,
                    remaining: self.read_be_u32()? as u64,
                });
                Ok(Token::BeginObject)
            }
            other => Err(Error::custom(alloc::format!(
                "msgpack: unsupported type byte 0x{other:02x}"
            ))),
        }
    }

    fn take_pending(&mut self, kind: CFrameKind) -> Result<CFrame> {
        match self.pending.take() {
            Some(frame) if frame.kind == kind => Ok(frame),
            _ => Err(Error::custom("msgpack: container header mismatch")),
        }
    }
}

impl<'de> FormatDecoder<'de> for MsgPackDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginObject => {
                let frame = self.take_pending(CFrameKind::Map)?;
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
            .ok_or_else(|| Error::custom("msgpack: map end without start"))?;
        if frame.kind != CFrameKind::Map || frame.remaining != 0 {
            return Err(Error::custom("msgpack: map entry count mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("msgpack: object key outside map"))?;
        if frame.remaining == 0 {
            return Ok(None);
        }
        frame.remaining -= 1;
        let key = self.read_str()?;
        Ok(Some(key))
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        Ok(self
            .frames
            .last()
            .map(|f| f.kind == CFrameKind::Map && f.remaining > 0)
            .unwrap_or(false))
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => {
                let frame = self.take_pending(CFrameKind::Array)?;
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
            .ok_or_else(|| Error::custom("msgpack: array end without start"))?;
        if frame.kind != CFrameKind::Array || frame.remaining != 0 {
            return Err(Error::custom("msgpack: array element count mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        Ok(self
            .frames
            .last()
            .map(|f| f.kind == CFrameKind::Array && f.remaining > 0)
            .unwrap_or(false))
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        // Count-based containers have no separator byte; the standard array
        // decode loop calls `array_entry_sep` once after every element, so it
        // is the exact hook for consuming one element.
        if let Some(frame) = self.frames.last_mut() {
            if frame.kind == CFrameKind::Array && frame.remaining > 0 {
                frame.remaining -= 1;
            }
        }
        self.array_has_more()
    }

    fn array_len_hint(&self) -> Option<usize> {
        self.frames.last().and_then(|frame| {
            (frame.kind == CFrameKind::Array).then(|| {
                usize::try_from(frame.remaining)
                    .unwrap_or(usize::MAX)
                    .min(self.cur.remaining_len())
                    .min(MAX_CONTAINER_PREALLOC)
            })
        })
    }

    fn object_len_hint(&self) -> Option<usize> {
        self.frames.last().and_then(|frame| {
            (frame.kind == CFrameKind::Map).then(|| {
                usize::try_from(frame.remaining)
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
            .ok_or_else(|| Error::custom("msgpack: lookahead unavailable"))
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

    fn bytes(&mut self) -> Result<Cow<'de, [u8]>, Self::Error> {
        let b = self.header()?;
        match b {
            0xC4 => {
                let len = self.read_u8()? as usize;
                Ok(Cow::Borrowed(self.cur.take(len)?))
            }
            0xC5 => {
                let len = self.read_be_u16()? as usize;
                Ok(Cow::Borrowed(self.cur.take(len)?))
            }
            0xC6 => {
                let len = usize::try_from(self.read_be_u32()?)
                    .map_err(|_| Error::custom("msgpack: byte string exceeds platform limit"))?;
                Ok(Cow::Borrowed(self.cur.take(len)?))
            }
            0xA0..=0xBF | 0xD9..=0xDB => {
                let s = self.read_str()?;
                match s {
                    Cow::Borrowed(s) => Ok(Cow::Borrowed(s.as_bytes())),
                    Cow::Owned(s) => Ok(Cow::Owned(s.into_bytes())),
                }
            }
            0x90..=0x9F | 0xDC | 0xDD => {
                // Legacy encoding: `Vec<u8>` used to be written as an array of
                // small integers. Keep reading it so old data round-trips.
                self.cur.rewind(1);
                self.begin_array()?;
                let mut out = Vec::with_capacity(self.array_len_hint().unwrap_or(0));
                while self.array_has_more()? {
                    out.push(self.u8()?);
                    if !self.array_entry_sep()? {
                        break;
                    }
                }
                self.end_array()?;
                Ok(Cow::Owned(out))
            }
            other => Err(Error::custom(alloc::format!(
                "msgpack: expected bin/str/array, got 0x{other:02x}"
            ))),
        }
    }

    fn map_key<K: for<'a> crate::de::NsonDeserialize<'a>>(
        &mut self,
    ) -> Result<Option<K>, Self::Error> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("msgpack: object key outside map"))?;
        if frame.remaining == 0 {
            return Ok(None);
        }
        frame.remaining -= 1;
        let key = K::nextdecode(self)?;
        Ok(Some(key))
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

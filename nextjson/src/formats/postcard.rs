//! Postcard codec (compact, `no_std`-friendly binary format).
//!
//! Postcard is a **non-self-describing** schema-light format: on the wire an
//! integer, a string and a sequence all begin with a varint, and the reader
//! must already know the target type. The unified [`FormatDecoder`] serves
//! typed primitives (the caller picks `number` / `string` / `begin_array` /
//! ...), which is exactly the contract postcard's own `serde` implementation
//! relies on. Consequently:
//!
//! - Typed round-trips work: `encode(value) -> decode::<SameType>()`.
//! - `Option` and schema-less `Value` decoding are rejected, because they
//!   require peeking the next token, which postcard cannot classify without a
//!   target type.
//! - Signed integers, floats and out-of-64-bit 128-bit integers are rejected
//!   on encode with a clear error (postcard encodes them without a
//!   discriminator recoverable by a type-agnostic reader).
//!
//! Wire conventions (all other bytes match `postcard-rs`):
//! - unsigned integers: unsigned LEB128 varint;
//! - strings: varint byte length + UTF-8;
//! - sequences / maps: varint element count + elements;
//! - `null`: `0x00` (identical to `postcard-rs`'s `Option::None` marker);
//! - booleans: `0x01` / `0x02`. The reference implementation uses `0x00` /
//!   `0x01`, which collides with this codec's `null` marker under the unified
//!   event model; the shift keeps `null` vs `false` vs `true` distinguishable.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::de::{FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::bin::{patch_prefix, read_varint, write_varint, Cursor};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// Postcard format marker.
#[derive(Clone, Copy, Debug)]
pub struct Postcard;

impl Format for Postcard {
    const NAME: &'static str = "postcard";
    const MIME: &'static str = "application/postcard";
    const EXTENSIONS: &'static [&'static str] = &["postcard"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = PostcardEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        Ok(encoder.finish_vec())
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = PostcardDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

struct Frame {
    start: usize,
    count: u64,
}

/// Streaming Postcard encoder.
pub struct PostcardEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    frames: Vec<Frame>,
}

impl<W: Write> PostcardEncoder<W> {
    /// Create a Postcard encoder over `writer`.
    pub fn new(writer: W) -> Self {
        PostcardEncoder {
            writer,
            buf: Vec::with_capacity(1024),
            frames: Vec::new(),
        }
    }

    fn patch(&mut self, frame: Frame) {
        let mut header = Vec::with_capacity(5);
        write_varint(&mut header, frame.count);
        patch_prefix(&mut self.buf, frame.start, &header);
    }

    fn write_len(&mut self, len: usize) -> Result<()> {
        let len = u64::try_from(len)
            .map_err(|_| Error::custom("postcard: length exceeds u64 wire limit"))?;
        write_varint(&mut self.buf, len);
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

impl<W: Write> FormatEncoder for PostcardEncoder<W> {
    fn begin_array(&mut self) -> Result<()> {
        self.frames.push(Frame {
            start: self.buf.len(),
            count: 0,
        });
        self.buf.push(0x00); // placeholder varint length
        Ok(())
    }

    fn separator(&mut self) -> Result<()> {
        if let Some(frame) = self.frames.last_mut() {
            frame.count = frame
                .count
                .checked_add(1)
                .ok_or_else(|| Error::custom("postcard: sequence length overflow"))?;
        }
        Ok(())
    }

    fn end_array(&mut self) -> Result<()> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("postcard: sequence end without start"))?;
        self.patch(frame);
        Ok(())
    }

    fn begin_object(&mut self) -> Result<()> {
        self.frames.push(Frame {
            start: self.buf.len(),
            count: 0,
        });
        self.buf.push(0x00);
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<()> {
        if let Some(frame) = self.frames.last_mut() {
            frame.count = frame
                .count
                .checked_add(1)
                .ok_or_else(|| Error::custom("postcard: map length overflow"))?;
        }
        let bytes = key.as_bytes();
        self.write_len(bytes.len())?;
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    fn end_object(&mut self) -> Result<()> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("postcard: map end without start"))?;
        self.patch(frame);
        Ok(())
    }

    fn write_null(&mut self) -> Result<()> {
        self.buf.push(0x00);
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<()> {
        self.buf.push(if value { 0x02 } else { 0x01 });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<()> {
        let bytes = value.as_bytes();
        self.write_len(bytes.len())?;
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> Result<()> {
        let mut buf = [0u8; 4];
        let s = value.encode_utf8(&mut buf);
        self.write_str(s)
    }

    fn write_number(&mut self, value: &Number) -> Result<()> {
        match *value {
            Number::U64(v) => self.write_u64(v),
            Number::U128(v) => self.write_u128(v),
            Number::I64(_) | Number::I128(_) | Number::F64(_) => Err(Error::custom(
                "postcard: signed and floating types are not self-describing",
            )),
        }
    }

    fn write_i64(&mut self, _value: i64) -> Result<()> {
        Err(Error::custom(
            "postcard: signed integers are not self-describing",
        ))
    }

    fn write_u64(&mut self, value: u64) -> Result<()> {
        write_varint(&mut self.buf, value);
        Ok(())
    }

    fn write_i128(&mut self, _value: i128) -> Result<()> {
        Err(Error::custom(
            "postcard: signed integers are not self-describing",
        ))
    }

    fn write_u128(&mut self, value: u128) -> Result<()> {
        match u64::try_from(value) {
            Ok(v) => self.write_u64(v),
            Err(_) => Err(Error::custom("postcard: u128 exceeds 64 bits")),
        }
    }

    fn write_f64(&mut self, _value: f64) -> Result<()> {
        Err(Error::custom("postcard: floats are not self-describing"))
    }

    fn write_f32(&mut self, _value: f32) -> Result<()> {
        Err(Error::custom("postcard: floats are not self-describing"))
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum CFrameKind {
    Seq,
    Map,
}

#[derive(Clone, Copy)]
struct CFrame {
    kind: CFrameKind,
    remaining: u64,
}

/// Streaming Postcard decoder.
pub struct PostcardDecoder<'de> {
    cur: Cursor<'de>,
    frames: Vec<CFrame>,
    depth: u32,
    max_depth: u32,
}

impl<'de> PostcardDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        PostcardDecoder {
            cur: Cursor::new(input),
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
        if self.frames.is_empty() && self.cur.at_end() {
            Ok(())
        } else {
            Err(Error::custom("postcard: trailing bytes after value"))
        }
    }

    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("postcard: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn read_count(&mut self) -> Result<u64> {
        let pos = self.cur.pos();
        let (count, new_pos) = read_varint(self.cur.input(), pos)?;
        self.cur.seek(new_pos);
        Ok(count)
    }

    fn read_varint(&mut self) -> Result<u64> {
        self.read_count()
    }

    fn read_str(&mut self) -> Result<Cow<'de, str>> {
        let len = usize::try_from(self.read_count()?)
            .map_err(|_| Error::custom("postcard: string length exceeds platform limit"))?;
        let bytes = self.cur.take(len)?;
        let s =
            core::str::from_utf8(bytes).map_err(|_| Error::custom("postcard: invalid utf-8"))?;
        Ok(Cow::Borrowed(s))
    }

    /// postcard is not self-describing; peeking requires a target type.
    fn not_self_describing() -> Error {
        Error::custom("postcard: not self-describing (cannot peek without a target type)")
    }
}

impl<'de> FormatDecoder<'de> for PostcardDecoder<'de> {
    fn begin_object(&mut self) -> Result<()> {
        self.enter_container()?;
        let count = self.read_count()?;
        self.frames.push(CFrame {
            kind: CFrameKind::Map,
            remaining: count,
        });
        Ok(())
    }

    fn end_object(&mut self) -> Result<()> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("postcard: map end without start"))?;
        if frame.kind != CFrameKind::Map || frame.remaining != 0 {
            return Err(Error::custom("postcard: map entry count mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("postcard: object key outside map"))?;
        if frame.remaining == 0 {
            return Ok(None);
        }
        frame.remaining -= 1;
        let key = self.read_str()?;
        Ok(Some(key))
    }

    fn object_entry_sep(&mut self) -> Result<bool> {
        Ok(self
            .frames
            .last()
            .map(|f| f.kind == CFrameKind::Map && f.remaining > 0)
            .unwrap_or(false))
    }

    fn begin_array(&mut self) -> Result<()> {
        self.enter_container()?;
        let count = self.read_count()?;
        self.frames.push(CFrame {
            kind: CFrameKind::Seq,
            remaining: count,
        });
        Ok(())
    }

    fn end_array(&mut self) -> Result<()> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("postcard: sequence end without start"))?;
        if frame.kind != CFrameKind::Seq || frame.remaining != 0 {
            return Err(Error::custom("postcard: sequence count mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn array_has_more(&mut self) -> Result<bool> {
        Ok(self
            .frames
            .last()
            .map(|f| f.kind == CFrameKind::Seq && f.remaining > 0)
            .unwrap_or(false))
    }

    fn array_entry_sep(&mut self) -> Result<bool> {
        if let Some(frame) = self.frames.last_mut() {
            if frame.kind == CFrameKind::Seq && frame.remaining > 0 {
                frame.remaining -= 1;
            }
        }
        self.array_has_more()
    }

    fn unit(&mut self) -> Result<()> {
        if self.cur.byte()? == 0x00 {
            Ok(())
        } else {
            Err(Error::invalid_type("null", "non-null byte"))
        }
    }

    fn bool(&mut self) -> Result<bool> {
        match self.cur.byte()? {
            0x00 | 0x01 => Ok(false),
            0x02 => Ok(true),
            other => Err(Error::custom(alloc::format!(
                "postcard: invalid bool byte 0x{other:02x}"
            ))),
        }
    }

    fn number(&mut self) -> Result<Number> {
        Ok(Number::U64(self.read_varint()?))
    }

    fn string(&mut self) -> Result<Cow<'de, str>> {
        self.read_str()
    }

    fn char(&mut self) -> Result<char> {
        let s = self.read_str()?;
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(Error::invalid_type("a single-character string", "string")),
        }
    }

    fn skip_value(&mut self) -> Result<()> {
        Err(Self::not_self_describing())
    }

    fn peek_token(&mut self) -> Result<Token<'de>> {
        Err(Self::not_self_describing())
    }

    fn next_token(&mut self) -> Result<Token<'de>> {
        Err(Self::not_self_describing())
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
        self.frames.truncate(mark.frame_len);
        self.depth = mark.depth;
    }
}

//! Python Pickle codec (protocol 2 subset).
//!
//! Pickle is a stack-machine bytecode used by CPython for object
//! serialization. This codec emits and executes a real, interoperable
//! protocol-2 byte stream covering: `None`, booleans, integers (including
//! arbitrary-precision 128-bit via `LONG1`), IEEE-754 doubles, Unicode and
//! byte strings, lists, dictionaries and tuples (tuples decode as sequences).
//!
//! The decoder executes the bytecode into a [`Value`] and serves the
//! [`crate::de::FormatDecoder`] interface from it (replayed through the
//! shared [`crate::formats::tree`] relay), so any `NsonDeserialize` target
//! can be produced from Python-produced pickles and vice versa.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::NsonDeserialize;
use crate::error::{Error, Result};
use crate::formats::bin::Cursor;
use crate::formats::tree;
use crate::formats::Format;
use crate::map::Map;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::value::Value;
use crate::write::Write;

/// Pickle format marker.
#[derive(Clone, Copy, Debug)]
pub struct Pickle;

impl Format for Pickle {
    const NAME: &'static str = "pickle";
    const MIME: &'static str = "application/python-pickle";
    const EXTENSIONS: &'static [&'static str] = &["pkl", "pickle"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = PickleEncoder::new(Vec::new());
        // Protocol header.
        encoder.buf.push(0x80); // PROTO
        encoder.buf.push(0x02); // protocol 2
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.buf.push(0x2E); // STOP
        Ok(encoder.finish_vec())
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let value = execute_pickle(input)?;
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value));
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Streaming pickle (protocol 2) encoder.
pub struct PickleEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
}

impl<W: Write> PickleEncoder<W> {
    /// Create a pickle encoder over `writer`.
    pub fn new(writer: W) -> Self {
        PickleEncoder {
            writer,
            buf: Vec::with_capacity(1024),
        }
    }

    fn write_binint(&mut self, value: i64) {
        if (0..=0xFF).contains(&value) {
            self.buf.push(0x4B); // BININT1
            self.buf.push(value as u8);
        } else if (0..=0xFFFF).contains(&value) {
            self.buf.push(0x4D); // BININT2
            self.buf.extend_from_slice(&(value as u16).to_le_bytes());
        } else if i32::try_from(value).is_ok() {
            self.buf.push(0x4A); // BININT
            self.buf.extend_from_slice(&(value as i32).to_le_bytes());
        } else {
            self.write_long(value as i128);
        }
    }

    fn write_binuint(&mut self, value: u64) {
        if value <= 0xFF {
            self.buf.push(0x4B);
            self.buf.push(value as u8);
        } else if value <= 0xFFFF {
            self.buf.push(0x4D);
            self.buf.extend_from_slice(&(value as u16).to_le_bytes());
        } else if value <= i32::MAX as u64 {
            // BININT is a signed 32-bit integer on the wire: values in
            // [0x8000_0000, 0xFFFF_FFFF] must not be written here or they
            // decode as negative (sign-bit set).
            self.buf.push(0x4A);
            self.buf.extend_from_slice(&(value as u32).to_le_bytes());
        } else {
            self.write_long(value as i128);
        }
    }

    fn write_long(&mut self, value: i128) {
        self.buf.push(0x8A); // LONG1
        let mut bytes = Vec::with_capacity(17);
        let mut v = value;
        loop {
            bytes.push((v & 0xFF) as u8);
            v >>= 8;
            // Stop when the remaining bits are pure sign extension.
            if v == 0 || v == -1 {
                break;
            }
        }
        // The top byte must carry the sign bit, otherwise the decoder (and
        // CPython) classify the value by that bit:
        // - negative value whose top byte is 0x00..0x7F needs a 0xFF byte;
        // - non-negative value whose top byte is 0x80..0xFF needs a 0x00 byte.
        let top = *bytes.last().unwrap_or(&0);
        if value < 0 && top & 0x80 == 0 {
            bytes.push(0xFF);
        } else if value >= 0 && top & 0x80 != 0 {
            bytes.push(0x00);
        }
        self.buf.push(bytes.len() as u8);
        self.buf.extend_from_slice(&bytes);
    }

    fn write_binunicode(&mut self, value: &str) -> Result<()> {
        let bytes = value.as_bytes();
        let len = u32::try_from(bytes.len())
            .map_err(|_| Error::custom("pickle: string exceeds u32 wire limit"))?;
        self.buf.push(0x58); // BINUNICODE
        self.buf.extend_from_slice(&len.to_le_bytes());
        self.buf.extend_from_slice(bytes);
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

impl<W: Write> FormatEncoder for PickleEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.buf.push(0x5D); // EMPTY_LIST
        self.buf.push(0x28); // MARK
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.buf.push(0x65); // APPENDS
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.buf.push(0x7D); // EMPTY_DICT
        self.buf.push(0x28); // MARK
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        self.write_binunicode(key)
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.buf.push(0x75); // SETITEMS
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.buf.push(0x4E); // NONE
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.buf.push(if value { 0x88 } else { 0x89 });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.write_binunicode(value)
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        let mut buf = [0u8; 4];
        let s = value.encode_utf8(&mut buf);
        self.write_binunicode(s)
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
        self.write_binint(value);
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.write_binuint(value);
        Ok(())
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        match i64::try_from(value) {
            Ok(v) => self.write_binint(v),
            Err(_) => self.write_long(value),
        }
        Ok(())
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        match u64::try_from(value) {
            Ok(v) => self.write_binuint(v),
            Err(_) => {
                if value <= i128::MAX as u128 {
                    self.write_long(value as i128);
                } else {
                    return Err(Error::custom("pickle: u128 exceeds signed 128-bit"));
                }
            }
        }
        Ok(())
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("pickle: non-finite float"));
        }
        self.buf.push(0x47); // BINFLOAT
        self.buf.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        self.write_f64(value as f64)
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), Self::Error> {
        let len = u32::try_from(value.len())
            .map_err(|_| Error::custom("pickle: bytes exceeds u32 wire limit"))?;
        if len <= 0xFF {
            self.buf.push(0x8C); // SHORT_BINBYTES
            self.buf.push(len as u8);
        } else {
            self.buf.push(0x42); // BINBYTES
            self.buf.extend_from_slice(&len.to_le_bytes());
        }
        self.buf.extend_from_slice(value);
        Ok(())
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Decoder (pickle VM -> Value)
// ---------------------------------------------------------------------------

const MARK: u8 = 0x28;
const STOP: u8 = 0x2E;
const NONE: u8 = 0x4E;
const INT: u8 = 0x49;
const BININT: u8 = 0x4A;
const BININT1: u8 = 0x4B;
const BININT2: u8 = 0x4D;
const FLOAT: u8 = 0x46;
const BINFLOAT: u8 = 0x47;
const EMPTY_LIST: u8 = 0x5D;
const APPENDS: u8 = 0x65;
const EMPTY_DICT: u8 = 0x7D;
const SETITEMS: u8 = 0x75;
const BINUNICODE: u8 = 0x58;
const SHORT_BINUNICODE: u8 = 0x8C;
const BINSTRING: u8 = 0x54;
const BINBYTES: u8 = 0x42;
const SHORT_BINSTRING: u8 = 0x55;
const SHORT_BINBYTES: u8 = 0x8D;
const LONG1: u8 = 0x8A;
const PROTO: u8 = 0x80;
const NEWTRUE: u8 = 0x88;
const NEWFALSE: u8 = 0x89;
const TUPLE: u8 = 0x69;
const TUPLE1: u8 = 0x85;
const TUPLE2: u8 = 0x86;
const TUPLE3: u8 = 0x87;
const LIST: u8 = 0x6C;
const DICT: u8 = 0x64;

enum StackItem {
    Value(Value),
    Mark,
}

/// Execute a protocol-2 pickle byte stream into a [`Value`].
///
/// Every nested container is delimited by a `MARK`, so `mark_depth` tracks
/// the current nesting depth and caps it (like every other codec) before the
/// resulting tree can drive unbounded recursion in the shared token relay or
/// `Value` decoding.
pub(crate) fn execute_pickle(input: &[u8]) -> Result<Value> {
    let mut cur = Cursor::new(input);
    let mut stack: Vec<StackItem> = Vec::new();
    let mut mark_depth = 0u32;
    const MAX_DEPTH: u32 = 128;
    loop {
        let op = cur
            .byte()
            .map_err(|_| Error::custom("pickle: truncated stream"))?;
        match op {
            PROTO => {
                let version = cur.byte()?;
                if version > 2 {
                    return Err(Error::custom("pickle: protocol version exceeds 2"));
                }
            }
            STOP => {
                let value = match stack.pop() {
                    Some(StackItem::Value(v)) => v,
                    _ => return Err(Error::custom("pickle: STOP with empty stack")),
                };
                if !cur.at_end() {
                    // Tolerate a trailing newline that CPython may append.
                    let rest = &input[cur.pos()..];
                    if !rest.iter().all(|b| *b == b'\n') {
                        return Err(Error::custom("pickle: trailing bytes after STOP"));
                    }
                }
                return Ok(value);
            }
            NONE => stack.push(StackItem::Value(Value::Null)),
            NEWTRUE => stack.push(StackItem::Value(Value::Bool(true))),
            NEWFALSE => stack.push(StackItem::Value(Value::Bool(false))),
            INT => {
                let line = cur.until_inclusive(b'\n')?;
                let text = core::str::from_utf8(&line[..line.len() - 1])
                    .map_err(|_| Error::custom("pickle: invalid INT"))?;
                let n: i64 = text
                    .trim()
                    .parse()
                    .map_err(|_| Error::custom("pickle: invalid INT value"))?;
                stack.push(StackItem::Value(Value::from(n)));
            }
            BININT => {
                let b = cur.take(4)?;
                let mut a = [0u8; 4];
                a.copy_from_slice(b);
                stack.push(StackItem::Value(Value::from(i32::from_le_bytes(a))));
            }
            BININT1 => {
                let n = cur.byte()?;
                stack.push(StackItem::Value(Value::from(n)));
            }
            BININT2 => {
                let b = cur.take(2)?;
                let mut a = [0u8; 2];
                a.copy_from_slice(b);
                stack.push(StackItem::Value(Value::from(u16::from_le_bytes(a))));
            }
            LONG1 => {
                let len = cur.byte()? as usize;
                let bytes = cur.take(len)?;
                let n = decode_long(bytes)?;
                stack.push(StackItem::Value(Value::from(n)));
            }
            FLOAT => {
                let line = cur.until_inclusive(b'\n')?;
                let text = core::str::from_utf8(&line[..line.len() - 1])
                    .map_err(|_| Error::custom("pickle: invalid FLOAT"))?;
                let f: f64 = text
                    .trim()
                    .parse()
                    .map_err(|_| Error::custom("pickle: invalid FLOAT value"))?;
                if !f.is_finite() {
                    return Err(Error::custom("pickle: non-finite FLOAT value"));
                }
                stack.push(StackItem::Value(Value::from(f)));
            }
            BINFLOAT => {
                let b = cur.take(8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                let value = f64::from_be_bytes(a);
                if !value.is_finite() {
                    return Err(Error::custom("pickle: non-finite BINFLOAT value"));
                }
                stack.push(StackItem::Value(Value::from(value)));
            }
            BINUNICODE | BINSTRING | BINBYTES => {
                let len = usize::try_from(cur.le_u32()?)
                    .map_err(|_| Error::custom("pickle: string length exceeds platform limit"))?;
                let bytes = cur.take(len)?;
                let s = decode_unicode(bytes)?;
                stack.push(StackItem::Value(Value::from(s)));
            }
            SHORT_BINUNICODE | SHORT_BINSTRING | SHORT_BINBYTES => {
                let len = cur.byte()? as usize;
                let bytes = cur.take(len)?;
                let s = decode_unicode(bytes)?;
                stack.push(StackItem::Value(Value::from(s)));
            }
            EMPTY_LIST => {
                stack.push(StackItem::Value(Value::Array(Vec::new())));
            }
            EMPTY_DICT => {
                stack.push(StackItem::Value(Value::Object(Map::new())));
            }
            MARK => {
                if mark_depth >= MAX_DEPTH {
                    return Err(Error::custom("pickle: recursion limit exceeded"));
                }
                mark_depth += 1;
                stack.push(StackItem::Mark);
            }
            APPENDS => {
                let mut items = Vec::new();
                loop {
                    match stack.pop() {
                        Some(StackItem::Mark) => {
                            mark_depth = mark_depth.saturating_sub(1);
                            break;
                        }
                        Some(StackItem::Value(v)) => items.push(v),
                        None => return Err(Error::custom("pickle: APPENDS without MARK")),
                    }
                }
                items.reverse();
                match stack.last_mut() {
                    Some(StackItem::Value(Value::Array(list))) => list.extend(items),
                    _ => return Err(Error::custom("pickle: APPENDS target is not a list")),
                }
            }
            SETITEMS => {
                let mut items = Vec::new();
                loop {
                    match stack.pop() {
                        Some(StackItem::Mark) => {
                            mark_depth = mark_depth.saturating_sub(1);
                            break;
                        }
                        Some(StackItem::Value(v)) => items.push(v),
                        None => return Err(Error::custom("pickle: SETITEMS without MARK")),
                    }
                }
                items.reverse();
                let map = match stack.last_mut() {
                    Some(StackItem::Value(Value::Object(m))) => m,
                    _ => return Err(Error::custom("pickle: SETITEMS target is not a dict")),
                };
                if items.len() % 2 != 0 {
                    return Err(Error::custom("pickle: SETITEMS with odd item count"));
                }
                let mut it = items.into_iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    let key = value_to_key(k)?;
                    map.insert(key, v);
                }
            }
            LIST => {
                let mut items = Vec::new();
                loop {
                    match stack.pop() {
                        Some(StackItem::Mark) => {
                            mark_depth = mark_depth.saturating_sub(1);
                            break;
                        }
                        Some(StackItem::Value(v)) => items.push(v),
                        None => return Err(Error::custom("pickle: LIST without MARK")),
                    }
                }
                items.reverse();
                stack.push(StackItem::Value(Value::Array(items)));
            }
            DICT => {
                let mut items = Vec::new();
                loop {
                    match stack.pop() {
                        Some(StackItem::Mark) => {
                            mark_depth = mark_depth.saturating_sub(1);
                            break;
                        }
                        Some(StackItem::Value(v)) => items.push(v),
                        None => return Err(Error::custom("pickle: DICT without MARK")),
                    }
                }
                items.reverse();
                let mut map = Map::new();
                if items.len() % 2 != 0 {
                    return Err(Error::custom("pickle: DICT with odd item count"));
                }
                let mut it = items.into_iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    let key = value_to_key(k)?;
                    map.insert(key, v);
                }
                stack.push(StackItem::Value(Value::Object(map)));
            }
            TUPLE => {
                let mut items = Vec::new();
                loop {
                    match stack.pop() {
                        Some(StackItem::Mark) => {
                            mark_depth = mark_depth.saturating_sub(1);
                            break;
                        }
                        Some(StackItem::Value(v)) => items.push(v),
                        None => return Err(Error::custom("pickle: TUPLE without MARK")),
                    }
                }
                items.reverse();
                stack.push(StackItem::Value(Value::Array(items)));
            }
            TUPLE1 | TUPLE2 | TUPLE3 => {
                let n = match op {
                    TUPLE1 => 1,
                    TUPLE2 => 2,
                    _ => 3,
                };
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    match stack.pop() {
                        Some(StackItem::Value(v)) => items.push(v),
                        _ => return Err(Error::custom("pickle: TUPLE without elements")),
                    }
                }
                items.reverse();
                stack.push(StackItem::Value(Value::Array(items)));
            }
            other => {
                return Err(Error::custom(alloc::format!(
                    "pickle: unsupported opcode 0x{other:02x}"
                )))
            }
        }
    }
}

fn decode_unicode(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| Error::custom("pickle: invalid utf-8 string"))
}

/// Decode a little-endian two's-complement big integer.
fn decode_long(bytes: &[u8]) -> Result<i128> {
    if bytes.len() > 16 {
        return Err(Error::custom("pickle: LONG exceeds 128 bits"));
    }
    if bytes.is_empty() {
        return Ok(0);
    }
    let negative = bytes[bytes.len() - 1] & 0x80 != 0;
    let mut value: i128 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        value |= (b as i128) << (8 * i);
    }
    if negative {
        // Sign-extend beyond the byte length.
        for i in bytes.len()..16 {
            value |= 0xFF_i128 << (8 * i);
        }
    }
    Ok(value)
}

fn value_to_key(value: Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(match n {
            Number::I64(v) => v.to_string(),
            Number::U64(v) => v.to_string(),
            Number::I128(v) => v.to_string(),
            Number::U128(v) => v.to_string(),
            Number::F64(v) => v.to_string(),
        }),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(Error::custom("pickle: dict key must be a string")),
    }
}

/// Pickle decoder that serves the unified interface from a parsed [`Value`].
///
/// The pickle VM produces a [`Value`] which is replayed through the shared
/// [`crate::formats::TreeDecoder`].
pub type PickleDecoder<'de> = tree::TreeDecoder<'de>;

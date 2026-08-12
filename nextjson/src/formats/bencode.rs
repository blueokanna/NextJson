//! Bencode codec (BitTorrent metainfo).
//!
//! Bencode is a minimal self-describing binary format with only four value
//! types: arbitrary-precision integers (`i<decimal>e`), byte strings
//! (`<len>:<bytes>`), lists (`l...e`) and dictionaries (`d...e`). Keys are
//! byte strings. Booleans are encoded as integers `1` / `0`. Because the
//! format has no representation for them, `null` and floats are rejected on
//! encode; integers are arbitrary-precision, so the full `i128` range is
//! supported and only `u128` values above `i128::MAX` are rejected.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::bin::Cursor;
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// Bencode format marker.
#[derive(Clone, Copy, Debug)]
pub struct Bencode;

impl Format for Bencode {
    const NAME: &'static str = "bencode";
    const MIME: &'static str = "application/x-bittorrent";
    const EXTENSIONS: &'static [&'static str] = &["torrent", "bencode"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = BencodeEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.finish_vec()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = BencodeDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Streaming bencode encoder.
pub struct BencodeEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    frames: Vec<EncodeFrame>,
}

enum EncodeFrame {
    List,
    Dict {
        entries: Vec<(String, Vec<u8>)>,
        pending: Option<(String, usize)>,
    },
}

impl<W: Write> BencodeEncoder<W> {
    /// Create a bencode encoder over `writer`.
    pub fn new(writer: W) -> Self {
        BencodeEncoder {
            writer,
            buf: Vec::with_capacity(1024),
            frames: Vec::new(),
        }
    }

    fn write_int_decimal(&mut self, value: i128) {
        self.buf.push(b'i');
        if value < 0 {
            self.buf.push(b'-');
        }
        let mag = value.unsigned_abs();
        let mut digits = [0u8; 39];
        let mut n = mag;
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        self.buf.extend_from_slice(&digits[i..]);
        self.buf.push(b'e');
    }

    fn write_bytestring(&mut self, value: &str) {
        let bytes = value.as_bytes();
        self.write_unsigned_len(bytes.len());
        self.buf.push(b':');
        self.buf.extend_from_slice(bytes);
    }

    fn write_unsigned_len(&mut self, mut value: usize) {
        let mut digits = [0u8; 39];
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.buf.extend_from_slice(&digits[i..]);
    }

    fn finish_dict_entry(&mut self) -> Result<()> {
        let (entries, pending) = match self.frames.last_mut() {
            Some(EncodeFrame::Dict { entries, pending }) => (entries, pending),
            _ => return Err(Error::custom("bencode: object key outside dictionary")),
        };
        let Some((key, value_start)) = pending.take() else {
            return Ok(());
        };
        if self.buf.len() == value_start {
            return Err(Error::custom("bencode: dictionary key has no value"));
        }
        entries.push((key, self.buf.split_off(value_start)));
        Ok(())
    }

    /// Flush the internal buffer and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        if !self.frames.is_empty() {
            return Err(Error::custom(
                "bencode: encoder finished inside a container",
            ));
        }
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn finish_vec(mut self) -> Result<Vec<u8>> {
        if !self.frames.is_empty() {
            return Err(Error::custom(
                "bencode: encoder finished inside a container",
            ));
        }
        Ok(core::mem::take(&mut self.buf))
    }
}

impl<W: Write> FormatEncoder for BencodeEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.buf.push(b'l');
        self.frames.push(EncodeFrame::List);
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        match self.frames.pop() {
            Some(EncodeFrame::List) => {}
            Some(frame) => {
                self.frames.push(frame);
                return Err(Error::custom("bencode: mismatched list end"));
            }
            None => return Err(Error::custom("bencode: list end without start")),
        }
        self.buf.push(b'e');
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.buf.push(b'd');
        self.frames.push(EncodeFrame::Dict {
            entries: Vec::new(),
            pending: None,
        });
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        self.finish_dict_entry()?;
        match self.frames.last_mut() {
            Some(EncodeFrame::Dict { pending, .. }) => {
                *pending = Some((String::from(key), self.buf.len()));
            }
            _ => return Err(Error::custom("bencode: object key outside dictionary")),
        }
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.finish_dict_entry()?;
        let mut entries = match self.frames.pop() {
            Some(EncodeFrame::Dict { entries, .. }) => entries,
            Some(frame) => {
                self.frames.push(frame);
                return Err(Error::custom("bencode: mismatched dictionary end"));
            }
            None => return Err(Error::custom("bencode: dictionary end without start")),
        };
        entries.sort_unstable_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        if entries
            .windows(2)
            .any(|pair| pair[0].0.as_bytes() == pair[1].0.as_bytes())
        {
            return Err(Error::custom("bencode: duplicate dictionary key"));
        }
        for (key, value) in entries {
            self.write_bytestring(&key);
            self.buf.extend_from_slice(&value);
        }
        self.buf.push(b'e');
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        Err(Error::custom("bencode: no null type"))
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.write_int_decimal(if value { 1 } else { 0 });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.write_bytestring(value);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        let mut buf = [0u8; 4];
        let s = value.encode_utf8(&mut buf);
        self.write_bytestring(s);
        Ok(())
    }

    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error> {
        match *value {
            Number::I64(v) => self.write_i64(v),
            Number::U64(v) => self.write_u64(v),
            Number::I128(v) => self.write_i128(v),
            Number::U128(v) => self.write_u128(v),
            Number::F64(_) => Err(Error::custom("bencode: no float type")),
        }
    }

    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        self.write_int_decimal(value as i128);
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.write_int_decimal(value as i128);
        Ok(())
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.write_int_decimal(value);
        Ok(())
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        match i128::try_from(value) {
            Ok(v) => {
                self.write_int_decimal(v);
                Ok(())
            }
            Err(_) => Err(Error::custom("bencode: u128 exceeds i128 range")),
        }
    }

    fn write_f64(&mut self, _value: f64) -> Result<(), Self::Error> {
        Err(Error::custom("bencode: no float type"))
    }

    fn write_f32(&mut self, _value: f32) -> Result<(), Self::Error> {
        Err(Error::custom("bencode: no float type"))
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), Self::Error> {
        self.write_unsigned_len(value.len());
        self.buf.push(b':');
        self.buf.extend_from_slice(value);
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
    List,
    Dict,
}

#[derive(Clone, Copy)]
struct CFrame {
    kind: CFrameKind,
}

/// Streaming bencode decoder.
pub struct BencodeDecoder<'de> {
    cur: Cursor<'de>,
    lookahead: Option<Token<'de>>,
    frames: Vec<CFrame>,
    depth: u32,
    max_depth: u32,
}

impl<'de> BencodeDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        BencodeDecoder {
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
        if self.lookahead.is_none() && self.frames.is_empty() && self.cur.at_end() {
            Ok(())
        } else {
            Err(Error::custom("bencode: trailing bytes after value"))
        }
    }

    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("bencode: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn read_int(&mut self) -> Result<i128> {
        if self.cur.byte()? != b'i' {
            return Err(Error::custom("bencode: expected integer marker 'i'"));
        }
        let mut byte = self.cur.byte()?;
        let negative = byte == b'-';
        if negative {
            byte = self.cur.byte()?;
        }
        if !byte.is_ascii_digit() {
            return Err(Error::custom("bencode: integer requires decimal digits"));
        }
        if byte == b'0' {
            if negative {
                return Err(Error::custom("bencode: negative zero is not canonical"));
            }
            return match self.cur.byte()? {
                b'e' => Ok(0),
                _ => Err(Error::custom("bencode: integer has a leading zero")),
            };
        }

        let limit = if negative {
            (i128::MAX as u128) + 1
        } else {
            i128::MAX as u128
        };
        let mut magnitude = (byte - b'0') as u128;
        loop {
            let b = self.cur.byte()?;
            match b {
                b'e' => {
                    if negative {
                        return if magnitude == limit {
                            Ok(i128::MIN)
                        } else {
                            Ok(-(magnitude as i128))
                        };
                    }
                    return Ok(magnitude as i128);
                }
                b'0'..=b'9' => {
                    magnitude = magnitude
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((b - b'0') as u128))
                        .filter(|&v| v <= limit)
                        .ok_or_else(|| Error::custom("bencode: integer overflow"))?;
                }
                other => {
                    return Err(Error::custom(alloc::format!(
                        "bencode: invalid integer byte 0x{other:02x}"
                    )))
                }
            }
        }
    }

    fn read_str(&mut self) -> Result<Cow<'de, str>> {
        let mut len: usize = 0;
        let mut digits = 0usize;
        let mut leading_zero = false;
        loop {
            let b = self.cur.byte()?;
            match b {
                b':' => break,
                b'0'..=b'9' => {
                    if digits == 0 {
                        leading_zero = b == b'0';
                    } else if leading_zero {
                        return Err(Error::custom("bencode: string length has a leading zero"));
                    }
                    digits += 1;
                    len = len
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((b - b'0') as usize))
                        .ok_or_else(|| Error::custom("bencode: string length overflow"))?;
                }
                other => {
                    return Err(Error::custom(alloc::format!(
                        "bencode: invalid string length byte 0x{other:02x}"
                    )))
                }
            }
        }
        if digits == 0 {
            return Err(Error::custom("bencode: empty string length"));
        }
        let bytes = self.cur.take(len)?;
        let s = core::str::from_utf8(bytes).map_err(|_| Error::custom("bencode: invalid utf-8"))?;
        Ok(Cow::Borrowed(s))
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        let b = self.cur.peek()?;
        match b {
            b'i' => {
                self.cur.byte()?;
                self.cur.rewind(1);
                let n = self.read_int()?;
                // Canonical number convention: non-negative -> U64, so that
                // `From<i64>` values and parsed values compare equal.
                let number = if n < 0 {
                    if n >= i64::MIN as i128 {
                        Number::I64(n as i64)
                    } else {
                        Number::I128(n)
                    }
                } else if n <= u64::MAX as i128 {
                    Number::U64(n as u64)
                } else {
                    Number::U128(n as u128)
                };
                Ok(Token::Number(number))
            }
            b'l' => {
                self.cur.byte()?;
                Ok(Token::BeginArray)
            }
            b'd' => {
                self.cur.byte()?;
                Ok(Token::BeginObject)
            }
            b'e' => {
                let frame = self
                    .frames
                    .last()
                    .copied()
                    .ok_or_else(|| Error::custom("bencode: terminator without container"))?;
                self.cur.byte()?;
                match frame.kind {
                    CFrameKind::List => Ok(Token::EndArray),
                    CFrameKind::Dict => Ok(Token::EndObject),
                }
            }
            b'0'..=b'9' => Ok(Token::Str(self.read_str()?)),
            other => Err(Error::custom(alloc::format!(
                "bencode: unexpected byte 0x{other:02x}"
            ))),
        }
    }
}

impl<'de> FormatDecoder<'de> for BencodeDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginObject => {
                self.frames.push(CFrame {
                    kind: CFrameKind::Dict,
                });
                Ok(())
            }
            other => Err(Error::invalid_type("a dict", token_name(&other))),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("bencode: dict end without start"))?;
        if frame.kind != CFrameKind::Dict {
            return Err(Error::custom("bencode: malformed dict end"));
        }
        match self.next_token()? {
            Token::EndObject => {
                self.depth = self.depth.saturating_sub(1);
                Ok(())
            }
            other => Err(Error::invalid_type("a dict terminator", token_name(&other))),
        }
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        let _ = self
            .frames
            .last()
            .ok_or_else(|| Error::custom("bencode: object key outside dict"))?;
        if matches!(self.peek_token()?, Token::EndObject) {
            return Ok(None);
        }
        match self.next_token()? {
            Token::Str(key) => Ok(Some(key)),
            other => Err(Error::invalid_type("a string key", token_name(&other))),
        }
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        Ok(!matches!(self.peek_token()?, Token::EndObject))
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => {
                self.frames.push(CFrame {
                    kind: CFrameKind::List,
                });
                Ok(())
            }
            other => Err(Error::invalid_type("a list", token_name(&other))),
        }
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("bencode: list end without start"))?;
        if frame.kind != CFrameKind::List {
            return Err(Error::custom("bencode: malformed list end"));
        }
        match self.next_token()? {
            Token::EndArray => {
                self.depth = self.depth.saturating_sub(1);
                Ok(())
            }
            other => Err(Error::invalid_type("a list terminator", token_name(&other))),
        }
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        Ok(!matches!(self.peek_token()?, Token::EndArray))
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.array_has_more()
    }

    fn unit(&mut self) -> Result<(), Self::Error> {
        Err(Error::custom("bencode: no null type"))
    }

    fn bool(&mut self) -> Result<bool, Self::Error> {
        match self.number()? {
            Number::I64(0) | Number::U64(0) => Ok(false),
            Number::I64(1) | Number::U64(1) => Ok(true),
            other => Err(Error::custom(alloc::format!(
                "bencode: bool must be 0 or 1, got {}",
                format_number(&other)
            ))),
        }
    }

    fn number(&mut self) -> Result<Number, Self::Error> {
        match self.next_token()? {
            Token::Number(n) => Ok(n),
            other => Err(Error::invalid_type("an integer", token_name(&other))),
        }
    }

    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error> {
        match self.next_token()? {
            Token::Str(s) => Ok(s),
            other => Err(Error::invalid_type("a string", token_name(&other))),
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
        Ok(self.lookahead.as_ref().expect("just set").clone())
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
        self.frames.truncate(mark.frame_len);
        self.depth = mark.depth;
    }

    fn bytes(&mut self) -> Result<Cow<'de, [u8]>, Self::Error> {
        // bencode byte strings are length-prefixed raw bytes (`len:bytes`),
        // with no UTF-8 requirement. Reuse the string length parser, then
        // take the raw slice without validating UTF-8.
        let mut len: usize = 0;
        let mut digits = 0usize;
        let mut leading_zero = false;
        loop {
            let b = self.cur.byte()?;
            match b {
                b':' => break,
                b'0'..=b'9' => {
                    if digits == 0 {
                        leading_zero = b == b'0';
                    } else if leading_zero {
                        return Err(Error::custom("bencode: string length has a leading zero"));
                    }
                    digits += 1;
                    len = len
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((b - b'0') as usize))
                        .ok_or_else(|| Error::custom("bencode: string length overflow"))?;
                }
                other => {
                    return Err(Error::custom(alloc::format!(
                        "bencode: invalid string length byte 0x{other:02x}"
                    )))
                }
            }
        }
        if digits == 0 {
            return Err(Error::custom("bencode: empty string length"));
        }
        Ok(Cow::Borrowed(self.cur.take(len)?))
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

/// Render a number for error messages without depending on `Display`.
fn format_number(n: &Number) -> alloc::string::String {
    match n {
        Number::I64(v) => alloc::string::ToString::to_string(v),
        Number::U64(v) => alloc::string::ToString::to_string(v),
        Number::I128(v) => alloc::string::ToString::to_string(v),
        Number::U128(v) => alloc::string::ToString::to_string(v),
        Number::F64(v) => alloc::string::ToString::to_string(v),
    }
}

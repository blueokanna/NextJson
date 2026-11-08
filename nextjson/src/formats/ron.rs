//! RON codec (Rusty Object Notation).
//!
//! Supports maps `{key: value, ...}`, sequences `[a, b]`, tuples `(a, b)`,
//! quoted and bare strings, `Some(x)` / `None` / `()` optionals, booleans,
//! and numbers. Structs with named fields are decoded as maps.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, OptionTag, Token};
use crate::error::{Error, Result};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// RON format marker.
#[derive(Clone, Copy, Debug)]
pub struct Ron;

impl Format for Ron {
    const NAME: &'static str = "ron";
    const MIME: &'static str = "text/ron";
    const EXTENSIONS: &'static [&'static str] = &["ron"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = RonEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.complete()?;
        Ok(core::mem::take(&mut encoder.buf))
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = RonDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Streaming RON encoder.
pub struct RonEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    /// Container frames: whether each open container is an array and whether
    /// its first entry has been written yet.
    frames: Vec<(bool, bool)>,
    some_frames: crate::stack::Stack<usize>,
    pending_some: usize,
}

impl<W: Write> RonEncoder<W> {
    /// Create a RON encoder over `writer`.
    pub fn new(writer: W) -> Self {
        RonEncoder {
            writer,
            buf: Vec::with_capacity(512),
            frames: Vec::new(),
            some_frames: crate::stack::Stack::new(),
            pending_some: 0,
        }
    }

    /// Separator before an array element.
    fn value_sep(&mut self) -> Result<()> {
        if let Some((is_array, first)) = self.frames.last_mut() {
            if *is_array {
                if *first {
                    *first = false;
                } else {
                    self.buf.push(b',');
                    self.buf.push(b' ');
                }
            }
        }
        for _ in 0..self.pending_some {
            self.buf.extend_from_slice(b"Some(");
        }
        self.pending_some = 0;
        Ok(())
    }

    /// Separator before an object key.
    fn key_sep(&mut self) -> Result<()> {
        if let Some((is_array, first)) = self.frames.last_mut() {
            if !*is_array {
                if *first {
                    *first = false;
                } else {
                    self.buf.push(b',');
                    self.buf.push(b' ');
                }
            }
        }
        Ok(())
    }

    fn write_quoted(&mut self, text: &str) {
        self.buf.push(b'"');
        let mut rest = text.as_bytes();
        while let Some(offset) = crate::scan::find_string_special(rest) {
            self.buf.extend_from_slice(&rest[..offset]);
            match rest[offset] {
                b'"' => self.buf.extend_from_slice(b"\\\""),
                b'\\' => self.buf.extend_from_slice(b"\\\\"),
                b'\n' => self.buf.extend_from_slice(b"\\n"),
                b'\r' => self.buf.extend_from_slice(b"\\r"),
                b'\t' => self.buf.extend_from_slice(b"\\t"),
                byte => {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    self.buf.extend_from_slice(b"\\x");
                    self.buf.push(HEX[(byte >> 4) as usize]);
                    self.buf.push(HEX[(byte & 15) as usize]);
                }
            }
            rest = &rest[offset + 1..];
        }
        self.buf.extend_from_slice(rest);
        self.buf.push(b'"');
    }

    fn complete(&self) -> Result<()> {
        if !self.frames.is_empty() || !self.some_frames.is_empty() {
            return Err(Error::custom("ron: unfinished container or option"));
        }
        Ok(())
    }

    /// Flush and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.complete()?;
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }
}

impl<W: Write> FormatEncoder for RonEncoder<W> {
    type Error = crate::error::Error;
    fn write_some(&mut self) -> Result<()> {
        if self.some_frames.len() >= 128 {
            return Err(Error::custom("ron: option depth exceeded"));
        }
        self.some_frames.push(self.frames.len());
        self.pending_some += 1;
        Ok(())
    }
    fn end_some(&mut self) -> Result<()> {
        if self.pending_some != 0 || self.some_frames.pop() != Some(self.frames.len()) {
            return Err(Error::custom("ron: incomplete option payload"));
        }
        self.buf.push(b')');
        Ok(())
    }
    fn write_none(&mut self) -> Result<()> {
        self.value_sep()?;
        self.buf.extend_from_slice(b"None");
        Ok(())
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.frames.push((true, true));
        self.buf.push(b'[');
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.frames.pop();
        self.buf.push(b']');
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.frames.push((false, true));
        self.buf.push(b'{');
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        self.key_sep()?;
        self.write_quoted(key);
        self.buf.push(b':');
        self.buf.push(b' ');
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.frames.pop();
        self.buf.push(b'}');
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(b"()");
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf
            .extend_from_slice(if value { b"true" } else { b"false" });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.write_quoted(value);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.push(b'\'');
        let mut bytes = [0; 4];
        match value {
            '\'' => self.buf.extend_from_slice(b"\\'"),
            '\\' => self.buf.extend_from_slice(b"\\\\"),
            '\n' => self.buf.extend_from_slice(b"\\n"),
            '\r' => self.buf.extend_from_slice(b"\\r"),
            '\t' => self.buf.extend_from_slice(b"\\t"),
            c if c < '\u{20}' => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                self.buf.extend_from_slice(b"\\x");
                self.buf.push(HEX[(c as usize) >> 4]);
                self.buf.push(HEX[(c as usize) & 15]);
            }
            c => self
                .buf
                .extend_from_slice(c.encode_utf8(&mut bytes).as_bytes()),
        }
        self.buf.push(b'\'');
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
        self.value_sep()?;
        crate::ser::write_i64_into(&mut self.buf, value);
        Ok(())
    }
    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.value_sep()?;
        crate::ser::write_u64_into(&mut self.buf, value);
        Ok(())
    }
    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.value_sep()?;
        crate::ser::write_signed_integer_into(&mut self.buf, value);
        Ok(())
    }
    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        self.value_sep()?;
        crate::ser::write_unsigned_integer_into(&mut self.buf, value);
        Ok(())
    }
    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("ron: non-finite float"));
        }
        self.value_sep()?;
        crate::ser::write_float_into(&mut self.buf, value)
    }
    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(Error::custom("ron: non-finite float"));
        }
        self.value_sep()?;
        crate::ser::write_float_into(&mut self.buf, value)
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Streaming RON decoder.
pub struct RonDecoder<'de> {
    input: &'de [u8],
    pos: usize,
    lookahead: Option<Token<'de>>,
    lookahead_mark: Mark,
    /// Container frames: the expected closing byte and whether it is an object.
    frames: Vec<(u8, bool, usize)>,
    option_frames: Vec<bool>,
    /// Nesting of `Some(...)` wrappers (a token-level recursion that the
    /// container depth check does not cover).
    some_depth: u32,
    depth: u32,
    max_depth: u32,
}

impl<'de> RonDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        RonDecoder {
            input,
            pos: 0,
            lookahead: None,
            lookahead_mark: Mark::new(0, 0),
            frames: Vec::new(),
            option_frames: Vec::new(),
            some_depth: 0,
            depth: 0,
            max_depth: 128,
        }
    }

    /// Validate that the whole input was consumed.
    pub fn end(&mut self) -> Result<()> {
        self.expect_end()
    }

    fn expect_end(&mut self) -> Result<()> {
        self.skip_ws()?;
        if self.lookahead.is_none()
            && self.pos == self.input.len()
            && self.frames.is_empty()
            && self.option_frames.is_empty()
            && self.depth == 0
        {
            Ok(())
        } else {
            Err(Error::custom("ron: trailing bytes after value"))
        }
    }

    fn skip_ws(&mut self) -> Result<()> {
        loop {
            self.pos = crate::scan::skip_whitespace(self.input, self.pos);
            let rest = &self.input[self.pos..];
            if rest.starts_with(b"//") {
                self.pos += rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
            } else if rest.starts_with(b"/*") {
                self.pos += 2;
                let mut depth = 1u32;
                while depth != 0 {
                    let rest = &self.input[self.pos..];
                    if rest.starts_with(b"/*") {
                        depth += 1;
                        if depth > self.max_depth {
                            return Err(Error::custom("ron: comment depth exceeded"));
                        }
                        self.pos += 2;
                    } else if rest.starts_with(b"*/") {
                        depth -= 1;
                        self.pos += 2;
                    } else if rest.is_empty() {
                        return Err(Error::custom("ron: unterminated comment"));
                    } else {
                        self.pos += 1;
                    }
                }
            } else {
                return Ok(());
            }
        }
    }

    fn entry_sep(&mut self) -> Result<bool> {
        self.skip_ws()?;
        let close = self
            .frames
            .last()
            .map(|frame| frame.0)
            .ok_or_else(|| Error::custom("ron: separator outside container"))?;
        if self.input.get(self.pos) == Some(&close) {
            return Ok(false);
        }
        if self.input.get(self.pos) != Some(&b',') {
            return Err(Error::custom("ron: expected ',' between entries"));
        }
        self.pos += 1;
        self.skip_ws()?;
        Ok(self.input.get(self.pos) != Some(&close))
    }

    fn peek_byte(&mut self) -> Result<u8> {
        self.skip_ws()?;
        self.input
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::custom("ron: unexpected end of input"))
    }

    fn read_quoted(&mut self) -> Result<Cow<'de, str>> {
        self.pos += 1;
        let start = self.pos;
        let mut owned: Option<String> = None;
        loop {
            let offset = crate::scan::find_string_special(&self.input[self.pos..])
                .ok_or_else(|| Error::custom("ron: unterminated string"))?;
            let end = self.pos + offset;
            let text = core::str::from_utf8(&self.input[self.pos..end])
                .map_err(|_| Error::custom("ron: invalid UTF-8"))?;
            if let Some(out) = &mut owned {
                out.push_str(text);
            }
            self.pos = end + 1;
            match self.input[end] {
                b'"' => {
                    return match owned {
                        Some(out) => Ok(Cow::Owned(out)),
                        None => Ok(Cow::Borrowed(text)),
                    }
                }
                b'\\' => {
                    if owned.is_none() {
                        let prefix = core::str::from_utf8(&self.input[start..end])
                            .map_err(|_| Error::custom("ron: invalid UTF-8"))?;
                        owned = Some(String::from(prefix));
                    }
                    let escaped = self.read_escape()?;
                    owned.as_mut().expect("allocated").push(escaped);
                }
                _ => {
                    // Literal controls are legal in RON strings, unlike JSON.
                    if owned.is_none() {
                        owned = Some(String::from(
                            core::str::from_utf8(&self.input[start..end])
                                .map_err(|_| Error::custom("ron: invalid UTF-8"))?,
                        ));
                    }
                    owned
                        .as_mut()
                        .expect("allocated")
                        .push(self.input[end] as char);
                }
            }
        }
    }

    fn read_char_literal(&mut self) -> Result<char> {
        self.pos += 1; // opening quote
        let first = *self
            .input
            .get(self.pos)
            .ok_or_else(|| Error::custom("ron: unterminated character"))?;
        let value = if first == b'\\' {
            self.pos += 1;
            self.read_escape()?
        } else {
            let width = match first {
                b'\'' => return Err(Error::custom("ron: empty character")),
                0..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return Err(Error::custom("ron: invalid character UTF-8")),
            };
            let bytes = self
                .input
                .get(self.pos..self.pos.saturating_add(width))
                .ok_or_else(|| Error::custom("ron: truncated character"))?;
            let text = core::str::from_utf8(bytes)
                .map_err(|_| Error::custom("ron: invalid character UTF-8"))?;
            self.pos += width;
            text.chars().next().expect("nonempty character")
        };
        if self.input.get(self.pos) != Some(&b'\'') {
            return Err(Error::custom("ron: expected closing character quote"));
        }
        self.pos += 1;
        Ok(value)
    }

    fn read_escape(&mut self) -> Result<char> {
        let byte = *self
            .input
            .get(self.pos)
            .ok_or_else(|| Error::custom("ron: truncated escape"))?;
        self.pos += 1;
        match byte {
            b'"' => Ok('"'),
            b'\'' => Ok('\''),
            b'\\' => Ok('\\'),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'0' => Ok('\0'),
            b'x' => {
                let bytes = self
                    .input
                    .get(self.pos..self.pos.saturating_add(2))
                    .ok_or_else(|| Error::custom("ron: truncated ASCII escape"))?;
                let high = crate::lex::hex_digit(bytes[0]);
                let low = crate::lex::hex_digit(bytes[1]);
                let value = high
                    .zip(low)
                    .map(|(h, l)| h * 16 + l)
                    .filter(|v| v.is_ascii())
                    .ok_or_else(|| Error::custom("ron: invalid ASCII escape"))?;
                self.pos += 2;
                Ok(value as char)
            }
            b'u' => {
                if self.input.get(self.pos) != Some(&b'{') {
                    return Err(Error::custom("ron: expected '{' in Unicode escape"));
                }
                self.pos += 1;
                let mut value = 0u32;
                let mut digits = 0;
                while let Some(&byte) = self.input.get(self.pos) {
                    if byte == b'}' {
                        break;
                    }
                    if digits == 6 {
                        return Err(Error::custom("ron: Unicode escape too long"));
                    }
                    value = value * 16
                        + crate::lex::hex_digit(byte)
                            .ok_or_else(|| Error::custom("ron: invalid Unicode escape"))?
                            as u32;
                    digits += 1;
                    self.pos += 1;
                }
                if digits == 0 || self.input.get(self.pos) != Some(&b'}') {
                    return Err(Error::custom("ron: incomplete Unicode escape"));
                }
                self.pos += 1;
                char::from_u32(value).ok_or_else(|| Error::custom("ron: invalid Unicode scalar"))
            }
            _ => Err(Error::custom("ron: unknown escape")),
        }
    }

    fn read_ident(&mut self) -> Result<&'de str> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = &self.input[start..self.pos];
        core::str::from_utf8(raw).map_err(|_| Error::custom("ron: invalid identifier"))
    }

    /// Body of `Some(...)`: consume the wrapping parentheses and unwrap the
    /// inner token. `some_depth` (checked by the caller) bounds the recursion.
    fn read_some_body(&mut self) -> Result<Token<'de>> {
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b'(') {
            self.pos += 1;
            let inner = self.read_token()?;
            match inner {
                Token::BeginArray | Token::BeginObject => {
                    self.frames.last_mut().expect("container frame").2 += 1;
                }
                _ => {
                    self.skip_ws()?;
                    if self.input.get(self.pos) != Some(&b')') {
                        return Err(Error::custom("ron: expected ')' after Some(...)"));
                    }
                    self.pos += 1;
                }
            }
            Ok(inner)
        } else {
            Err(Error::custom("ron: expected '(' after Some"))
        }
    }

    fn read_number_text(&mut self) -> Result<&'de str> {
        let start = self.pos;
        if self.input.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-') {
                self.pos += 1;
            } else {
                break;
            }
        }
        core::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| Error::custom("ron: invalid number"))
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        let b = self.peek_byte()?;
        match b {
            b'[' => {
                self.pos += 1;
                self.frames.push((b']', false, 0));
                Ok(Token::BeginArray)
            }
            b']' => {
                self.pos += 1;
                if self.frames.last().map(|f| f.0) == Some(b']') {
                    Ok(Token::EndArray)
                } else {
                    Err(Error::custom("ron: mismatched ']'"))
                }
            }
            b'{' => {
                self.pos += 1;
                self.frames.push((b'}', true, 0));
                Ok(Token::BeginObject)
            }
            b'}' => {
                self.pos += 1;
                if self.frames.last().map(|f| f.0) == Some(b'}') {
                    Ok(Token::EndObject)
                } else {
                    Err(Error::custom("ron: mismatched '}'"))
                }
            }
            b'(' => {
                // Tuple or struct form: decide by peeking for `ident:`.
                self.pos += 1;
                self.skip_ws()?;
                if self.input.get(self.pos) == Some(&b')') {
                    self.pos += 1;
                    return Ok(Token::Null);
                }
                let is_struct = self.looks_like_struct()?;
                self.frames.push((b')', is_struct, 0));
                if is_struct {
                    Ok(Token::BeginObject)
                } else {
                    Ok(Token::BeginArray)
                }
            }
            b')' => {
                self.pos += 1;
                match self.frames.last() {
                    Some((b')', true, _)) => Ok(Token::EndObject),
                    Some((b')', false, _)) => Ok(Token::EndArray),
                    _ => Err(Error::custom("ron: mismatched ')'")),
                }
            }
            b'"' => Ok(Token::Str(self.read_quoted()?)),
            b'\'' => {
                let value = self.read_char_literal()?;
                let mut bytes = [0; 4];
                Ok(Token::Str(Cow::Owned(String::from(
                    value.encode_utf8(&mut bytes),
                ))))
            }
            b'-' | b'0'..=b'9' | b'+' => {
                let text = self.read_number_text()?;
                Ok(Token::Number(parse_ron_number(text)?))
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let ident = self.read_ident()?;
                match ident {
                    "None" => Ok(Token::Null),
                    "Some" => {
                        // `Some(...)` unwraps to the inner value. A container
                        // inner value closes with `)` after its own end. The
                        // recursion is bounded like container nesting.
                        if self.some_depth >= self.max_depth {
                            return Err(Error::custom("ron: recursion limit exceeded"));
                        }
                        self.some_depth += 1;
                        let result = self.read_some_body();
                        self.some_depth -= 1;
                        result
                    }
                    "true" => Ok(Token::Bool(true)),
                    "false" => Ok(Token::Bool(false)),
                    _ => Ok(Token::Str(Cow::Borrowed(ident))),
                }
            }
            other => Err(Error::custom(alloc::format!(
                "ron: unexpected byte 0x{other:02x}"
            ))),
        }
    }

    /// After consuming `(`, decide whether it is a struct (`ident:`) or a
    /// tuple.
    fn looks_like_struct(&mut self) -> Result<bool> {
        let saved = self.pos;
        self.skip_ws()?;
        // Empty tuple `()` -> array.
        if self.input.get(self.pos) == Some(&b')') {
            self.pos = saved;
            return Ok(false);
        }
        // Peek an identifier followed by `:`.
        let mut i = self.pos;
        while i < self.input.len()
            && (self.input[i].is_ascii_alphanumeric() || self.input[i] == b'_')
        {
            i += 1;
        }
        while i < self.input.len() && matches!(self.input[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        let is_struct = self.input.get(i) == Some(&b':');
        self.pos = saved;
        Ok(is_struct)
    }
}

fn parse_ron_number(text: &str) -> Result<Number> {
    crate::formats::tree::parse_cell_number(text)
}

impl<'de> FormatDecoder<'de> for RonDecoder<'de> {
    type Error = crate::error::Error;
    fn option_tag(&mut self) -> Result<OptionTag> {
        if self.lookahead.is_some() {
            let mark = self.lookahead_mark;
            self.restore(mark);
        }
        self.skip_ws()?;
        let start = self.pos;
        if self
            .input
            .get(start)
            .is_some_and(|b| b.is_ascii_alphabetic())
        {
            let name = self.read_ident()?;
            if name == "None" {
                return Ok(OptionTag::None);
            }
            if name == "Some" {
                self.skip_ws()?;
                if self.input.get(self.pos) != Some(&b'(') {
                    return Err(Error::custom("ron: expected '(' after Some"));
                }
                if self.option_frames.len() >= self.max_depth as usize {
                    return Err(Error::custom("ron: option depth exceeded"));
                }
                self.pos += 1;
                self.option_frames.push(true);
                return Ok(OptionTag::Some);
            }
            self.pos = start;
        }
        // Retain decoding of legacy unwrapped Some payloads.
        if self.option_frames.len() >= self.max_depth as usize {
            return Err(Error::custom("ron: option depth exceeded"));
        }
        self.option_frames.push(false);
        Ok(OptionTag::Some)
    }
    fn end_some(&mut self) -> Result<()> {
        let wrapped = self
            .option_frames
            .pop()
            .ok_or_else(|| Error::custom("ron: option end without start"))?;
        if wrapped {
            self.skip_ws()?;
            if self.input.get(self.pos) != Some(&b')') {
                return Err(Error::custom("ron: expected ')' after Some"));
            }
            self.pos += 1;
        }
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginObject => Ok(()),
            other => Err(Error::invalid_type("a struct/map", token_name(&other))),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.leave_container()?;
        let r = match self.next_token()? {
            Token::EndObject => Ok(()),
            other => Err(Error::invalid_type("'}' or ')'", token_name(&other))),
        };
        let wrappers = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("ron: missing container"))?
            .2;
        self.consume_pending_parens(wrappers)?;
        r
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        self.skip_ws()?;
        let close = self.frames.last().map(|frame| frame.0);
        if self.lookahead.is_none() && self.input.get(self.pos).copied() == close {
            return Ok(None);
        }
        if matches!(self.lookahead, Some(Token::EndObject)) {
            return Ok(None);
        }
        let key = match self.next_token()? {
            Token::Str(s) => s,
            other => return Err(Error::invalid_type("a string key", token_name(&other))),
        };
        self.skip_ws()?;
        if self.input.get(self.pos) != Some(&b':') {
            return Err(Error::custom("ron: expected ':' after key"));
        }
        self.pos += 1;
        Ok(Some(key))
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.entry_sep()
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => Ok(()),
            other => Err(Error::invalid_type("a sequence", token_name(&other))),
        }
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.leave_container()?;
        let r = match self.next_token()? {
            Token::EndArray => Ok(()),
            other => Err(Error::invalid_type("']' or ')'", token_name(&other))),
        };
        let wrappers = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("ron: missing container"))?
            .2;
        self.consume_pending_parens(wrappers)?;
        r
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        if let Some(token) = &self.lookahead {
            return Ok(!matches!(token, Token::EndArray));
        }
        self.skip_ws()?;
        Ok(self.input.get(self.pos).copied() != self.frames.last().map(|frame| frame.0))
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.entry_sep()
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
        if self.lookahead.is_none() && self.peek_byte()? == b'\'' {
            return self.read_char_literal();
        }
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
            token @ (Token::EndArray | Token::EndObject) => {
                Err(Error::invalid_type("a value", token_name(&token)))
            }
            _ => {
                self.next_token()?;
                Ok(())
            }
        }
    }

    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error> {
        if self.lookahead.is_none() {
            self.lookahead_mark = self.save();
            self.lookahead = Some(self.read_token()?);
        }
        Ok(self.lookahead.as_ref().expect("set").clone())
    }

    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        if let Some(t) = self.lookahead.take() {
            return Ok(t);
        }
        self.read_token()
    }

    fn save(&self) -> Mark {
        if self.lookahead.is_some() {
            return self.lookahead_mark;
        }
        Mark {
            pos: self.pos,
            depth: self.depth,
            frame_len: self.frames.len(),
            tag: Some(self.option_frames.len() as u8),
        }
    }

    fn restore(&mut self, mark: Mark) {
        self.pos = mark.pos;
        self.lookahead = None;
        self.frames.truncate(mark.frame_len);
        self.option_frames.truncate(mark.tag.unwrap_or(0) as usize);
        self.depth = mark.depth;
    }
}

impl<'de> RonDecoder<'de> {
    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("ron: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave_container(&mut self) -> Result<()> {
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn consume_pending_parens(&mut self, count: usize) -> Result<()> {
        for _ in 0..count {
            self.skip_ws()?;
            if self.input.get(self.pos) != Some(&b')') {
                return Err(Error::custom("ron: expected ')' after Some(...)"));
            }
            self.pos += 1;
        }
        Ok(())
    }
}

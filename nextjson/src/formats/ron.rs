//! RON codec (Rusty Object Notation).
//!
//! Supports maps `{key: value, ...}`, sequences `[a, b]`, tuples `(a, b)`,
//! quoted and bare strings, `Some(x)` / `None` / `()` optionals, booleans,
//! and numbers. Structs with named fields are decoded as maps.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
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
        encoder.writer.write_all(&encoder.buf)?;
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
}

impl<W: Write> RonEncoder<W> {
    /// Create a RON encoder over `writer`.
    pub fn new(writer: W) -> Self {
        RonEncoder {
            writer,
            buf: Vec::with_capacity(512),
            frames: Vec::new(),
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

    fn write_quoted(&mut self, s: &str) {
        self.buf.push(b'"');
        for &b in s.as_bytes() {
            match b {
                b'"' => self.buf.extend_from_slice(b"\\\""),
                b'\\' => self.buf.extend_from_slice(b"\\\\"),
                b'\n' => self.buf.extend_from_slice(b"\\n"),
                b'\t' => self.buf.extend_from_slice(b"\\t"),
                b'\r' => self.buf.extend_from_slice(b"\\r"),
                0x08 => self.buf.extend_from_slice(b"\\b"),
                0x0C => self.buf.extend_from_slice(b"\\f"),
                other => self.buf.push(other),
            }
        }
        self.buf.push(b'"');
    }

    /// Flush and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }
}

impl<W: Write> FormatEncoder for RonEncoder<W> {
    type Error = crate::error::Error;

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
        self.buf.extend_from_slice(b"None");
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
        self.write_quoted(&value.to_string());
        Ok(())
    }

    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(number_bytes(value).as_bytes());
        Ok(())
    }

    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(value.to_string().as_bytes());
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(value.to_string().as_bytes());
        Ok(())
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(value.to_string().as_bytes());
        Ok(())
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(value.to_string().as_bytes());
        Ok(())
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(value.to_string().as_bytes());
        Ok(())
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        self.write_f64(value as f64)
    }
}

fn number_bytes(n: &Number) -> String {
    match n {
        Number::I64(v) => v.to_string(),
        Number::U64(v) => v.to_string(),
        Number::I128(v) => v.to_string(),
        Number::U128(v) => v.to_string(),
        Number::F64(v) => v.to_string(),
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
    scratch: String,
    /// Container frames: the expected closing byte and whether it is an object.
    frames: Vec<(u8, bool)>,
    /// Unclosed `)` from a `Some(...)` wrapper around a container.
    pending_parens: usize,
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
            scratch: String::new(),
            frames: Vec::new(),
            pending_parens: 0,
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
        if self.lookahead.is_none() && self.pos >= self.input.len() {
            Ok(())
        } else {
            Err(Error::custom("ron: trailing bytes after value"))
        }
    }

    fn skip_ws(&mut self) -> Result<()> {
        while self.pos < self.input.len()
            && matches!(self.input[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
        Ok(())
    }

    fn peek_byte(&mut self) -> Result<u8> {
        self.skip_ws()?;
        self.input
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::custom("ron: unexpected end of input"))
    }

    fn read_quoted(&mut self) -> Result<String> {
        self.pos += 1; // opening quote
        self.scratch.clear();
        loop {
            if self.pos >= self.input.len() {
                return Err(Error::custom("ron: unterminated string"));
            }
            match self.input[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Ok(core::mem::take(&mut self.scratch));
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.input.len() {
                        return Err(Error::custom("ron: unterminated escape"));
                    }
                    let c = match self.input[self.pos] {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'"' => '"',
                        b'\\' => '\\',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        other => other as char,
                    };
                    self.scratch.push(c);
                    self.pos += 1;
                }
                b => {
                    let len = utf8_len(b).ok_or_else(|| Error::custom("ron: invalid utf-8"))?;
                    let chunk = self
                        .input
                        .get(self.pos..self.pos + len)
                        .ok_or_else(|| Error::custom("ron: truncated utf-8"))?;
                    let s = core::str::from_utf8(chunk)
                        .map_err(|_| Error::custom("ron: invalid utf-8"))?;
                    self.scratch.push_str(s);
                    self.pos += len;
                }
            }
        }
    }

    fn read_ident(&mut self) -> Result<String> {
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
        Ok(core::str::from_utf8(raw)
            .map_err(|_| Error::custom("ron: invalid identifier"))?
            .to_string())
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
                    self.pending_parens += 1;
                }
                _ => {
                    self.skip_ws()?;
                    if self.input.get(self.pos) == Some(&b')') {
                        self.pos += 1;
                    }
                }
            }
            Ok(inner)
        } else {
            Err(Error::custom("ron: expected '(' after Some"))
        }
    }

    fn read_number_text(&mut self) -> Result<String> {
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
        Ok(core::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| Error::custom("ron: invalid number"))?
            .to_string())
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        let b = self.peek_byte()?;
        match b {
            b'[' => {
                self.pos += 1;
                self.frames.push((b']', false));
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
                self.frames.push((b'}', true));
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
                let is_struct = self.looks_like_struct()?;
                self.frames.push((b')', is_struct));
                if is_struct {
                    Ok(Token::BeginObject)
                } else {
                    Ok(Token::BeginArray)
                }
            }
            b')' => {
                self.pos += 1;
                match self.frames.last() {
                    Some((b')', true)) => Ok(Token::EndObject),
                    Some((b')', false)) => Ok(Token::EndArray),
                    _ => Err(Error::custom("ron: mismatched ')'")),
                }
            }
            b'"' => Ok(Token::Str(Cow::Owned(self.read_quoted()?))),
            b'-' | b'0'..=b'9' | b'+' => {
                let text = self.read_number_text()?;
                Ok(Token::Number(parse_ron_number(&text)?))
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let ident = self.read_ident()?;
                match ident.as_str() {
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
                    _ => Ok(Token::Str(Cow::Owned(ident))),
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
    if let Ok(v) = text.parse::<i64>() {
        return Ok(Number::from(v));
    }
    if let Ok(v) = text.parse::<u64>() {
        return Ok(Number::from(v));
    }
    if let Ok(v) = text.parse::<f64>() {
        return Ok(Number::F64(v));
    }
    Err(Error::custom(alloc::format!(
        "ron: invalid number {text:?}"
    )))
}

fn utf8_len(b: u8) -> Option<usize> {
    match b {
        0x00..=0x7F => Some(1),
        0xC0..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF7 => Some(4),
        _ => None,
    }
}

impl<'de> FormatDecoder<'de> for RonDecoder<'de> {
    type Error = crate::error::Error;

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
        self.frames.pop();
        self.consume_pending_paren()?;
        r
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        if matches!(self.peek_token()?, Token::EndObject) {
            return Ok(None);
        }
        let key = match self.next_token()? {
            Token::Str(s) => s,
            other => return Err(Error::invalid_type("a string key", token_name(&other))),
        };
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b':') {
            self.pos += 1;
        }
        Ok(Some(key))
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b',') {
            self.pos += 1;
        }
        Ok(!matches!(self.peek_token()?, Token::EndObject))
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
        self.frames.pop();
        self.consume_pending_paren()?;
        r
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        Ok(!matches!(self.peek_token()?, Token::EndArray))
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b',') {
            self.pos += 1;
        }
        self.array_has_more()
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
        Ok(self.lookahead.as_ref().expect("set").clone())
    }

    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        if let Some(t) = self.lookahead.take() {
            return Ok(t);
        }
        self.read_token()
    }

    fn save(&self) -> Mark {
        Mark {
            pos: self.pos,
            depth: self.depth,
            frame_len: self.frames.len(),
        }
    }

    fn restore(&mut self, mark: Mark) {
        self.pos = mark.pos;
        self.lookahead = None;
        self.frames.truncate(mark.frame_len);
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

    fn consume_pending_paren(&mut self) -> Result<()> {
        if self.pending_parens > 0 {
            self.skip_ws()?;
            if self.input.get(self.pos) == Some(&b')') {
                self.pos += 1;
                self.pending_parens -= 1;
            } else {
                return Err(Error::custom("ron: expected ')' after Some(...)"));
            }
        }
        Ok(())
    }
}

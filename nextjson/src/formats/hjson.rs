//! Hjson codec (human-oriented JSON).
//!
//! The encoder emits standard JSON (Hjson is a JSON superset). The decoder
//! accepts Hjson's lenient syntax: `#`, `//` and `/* */` comments, unquoted
//! keys, unquoted single-line strings, quoted strings, trailing commas, and
//! relaxed numbers. Multi-line unquoted strings are not supported.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::encoding::{EncodeConfig, Encoder};
use crate::error::{Error, Result};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::NsonSerialize;

/// Hjson format marker.
#[derive(Clone, Copy, Debug)]
pub struct Hjson;

impl Format for Hjson {
    const NAME: &'static str = "hjson";
    const MIME: &'static str = "application/hjson";
    const EXTENSIONS: &'static [&'static str] = &["hjson"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = Encoder::for_vec(EncodeConfig::compact());
        T::nextencode(value, &mut encoder)?;
        encoder.finish_vec()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = HjsonDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

/// The Hjson encoder is the standard JSON encoder (Hjson is a superset).
pub type HjsonEncoder<W> = Encoder<W>;

/// Streaming Hjson decoder.
pub struct HjsonDecoder<'de> {
    input: &'de [u8],
    pos: usize,
    lookahead: Option<Token<'de>>,
    scratch: String,
    depth: u32,
    max_depth: u32,
}

impl<'de> HjsonDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        HjsonDecoder {
            input,
            pos: 0,
            lookahead: None,
            scratch: String::new(),
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
            Err(Error::custom("hjson: trailing bytes after value"))
        }
    }

    fn skip_ws(&mut self) -> Result<()> {
        loop {
            while self.pos < self.input.len()
                && matches!(self.input[self.pos], b' ' | b'\t' | b'\n' | b'\r')
            {
                self.pos += 1;
            }
            if self.pos >= self.input.len() {
                break;
            }
            match self.input[self.pos] {
                b'#' => {
                    while self.pos < self.input.len() && self.input[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    continue;
                }
                b'/' if self.input.get(self.pos + 1) == Some(&b'/') => {
                    while self.pos < self.input.len() && self.input[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    continue;
                }
                b'/' if self.input.get(self.pos + 1) == Some(&b'*') => {
                    self.pos += 2;
                    loop {
                        if self.pos >= self.input.len() {
                            return Err(Error::custom("hjson: unterminated block comment"));
                        }
                        if self.input[self.pos] == b'*'
                            && self.input.get(self.pos + 1) == Some(&b'/')
                        {
                            self.pos += 2;
                            break;
                        }
                        self.pos += 1;
                    }
                    continue;
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn peek_byte(&mut self) -> Result<u8> {
        self.skip_ws()?;
        self.input
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::custom("hjson: unexpected end of input"))
    }

    fn read_quoted(&mut self) -> Result<String> {
        self.pos += 1; // opening quote
        self.scratch.clear();
        loop {
            if self.pos >= self.input.len() {
                return Err(Error::custom("hjson: unterminated string"));
            }
            match self.input[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Ok(core::mem::take(&mut self.scratch));
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.input.len() {
                        return Err(Error::custom("hjson: unterminated escape"));
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
                    let len = utf8_len(b).ok_or_else(|| Error::custom("hjson: invalid utf-8"))?;
                    let chunk = self
                        .input
                        .get(self.pos..self.pos + len)
                        .ok_or_else(|| Error::custom("hjson: truncated utf-8"))?;
                    let s = core::str::from_utf8(chunk)
                        .map_err(|_| Error::custom("hjson: invalid utf-8"))?;
                    self.scratch.push_str(s);
                    self.pos += len;
                }
            }
        }
    }

    /// Read an unquoted string until a structural character, `#` comment, or
    /// `//` line comment.
    fn read_unquoted(&mut self) -> Result<String> {
        self.scratch.clear();
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if matches!(b, b',' | b'}' | b']' | b'\n' | b'\r' | b'\t') {
                break;
            }
            if b == b'/' && self.input.get(self.pos + 1) == Some(&b'/') {
                break;
            }
            if b == b'#' {
                // Trailing `#` comment: `skip_ws` (called by the caller after
                // this value) consumes it up to the line end.
                break;
            }
            let len = utf8_len(b).ok_or_else(|| Error::custom("hjson: invalid utf-8"))?;
            let chunk = self
                .input
                .get(self.pos..self.pos + len)
                .ok_or_else(|| Error::custom("hjson: truncated utf-8"))?;
            let s =
                core::str::from_utf8(chunk).map_err(|_| Error::custom("hjson: invalid utf-8"))?;
            self.scratch.push_str(s);
            self.pos += len;
        }
        Ok(core::mem::take(&mut self.scratch).trim().to_string())
    }

    fn read_number(&mut self) -> Result<Number> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = core::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| Error::custom("hjson: invalid number"))?;
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
            "hjson: invalid number {text:?}"
        )))
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        let b = self.peek_byte()?;
        match b {
            b'{' => {
                self.pos += 1;
                Ok(Token::BeginObject)
            }
            b'}' => {
                self.pos += 1;
                Ok(Token::EndObject)
            }
            b'[' => {
                self.pos += 1;
                Ok(Token::BeginArray)
            }
            b']' => {
                self.pos += 1;
                Ok(Token::EndArray)
            }
            b'"' => Ok(Token::Str(Cow::Owned(self.read_quoted()?))),
            b'-' | b'+' | b'0'..=b'9' => {
                let n = self.read_number()?;
                Ok(Token::Number(n))
            }
            b't' | b'f' | b'n' => {
                // A key or value that begins with t/f/n: classify by word.
                let ident = self.read_unquoted()?;
                match ident.as_str() {
                    "true" => Ok(Token::Bool(true)),
                    "false" => Ok(Token::Bool(false)),
                    "null" => Ok(Token::Null),
                    _ => Ok(Token::Str(Cow::Owned(ident))),
                }
            }
            _ => {
                // Unquoted value (string or key).
                let s = self.read_unquoted()?;
                match s.as_str() {
                    "true" => Ok(Token::Bool(true)),
                    "false" => Ok(Token::Bool(false)),
                    "null" => Ok(Token::Null),
                    _ => Ok(Token::Str(Cow::Owned(s))),
                }
            }
        }
    }
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

impl<'de> FormatDecoder<'de> for HjsonDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginObject => Ok(()),
            other => Err(Error::invalid_type("'{'", token_name(&other))),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.leave_container()?;
        match self.next_token()? {
            Token::EndObject => Ok(()),
            other => Err(Error::invalid_type("'}'", token_name(&other))),
        }
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b'}') {
            return Ok(None);
        }
        let key = if self.input.get(self.pos) == Some(&b'"') {
            match self.next_token()? {
                Token::Str(s) => s,
                other => {
                    return Err(Error::invalid_type(
                        "a string or unquoted key",
                        token_name(&other),
                    ))
                }
            }
        } else {
            Cow::Owned(self.read_unquoted_key()?)
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
            self.skip_ws()?;
        }
        Ok(self.input.get(self.pos) != Some(&b'}'))
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => Ok(()),
            other => Err(Error::invalid_type("'['", token_name(&other))),
        }
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.leave_container()?;
        match self.next_token()? {
            Token::EndArray => Ok(()),
            other => Err(Error::invalid_type("']'", token_name(&other))),
        }
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        Ok(!matches!(self.peek_token()?, Token::EndArray))
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b',') {
            self.pos += 1;
            self.skip_ws()?;
        }
        Ok(self.input.get(self.pos) != Some(&b']'))
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
            frame_len: 0,
        }
    }

    fn restore(&mut self, mark: Mark) {
        self.pos = mark.pos;
        self.lookahead = None;
        self.depth = mark.depth;
    }
}

impl<'de> HjsonDecoder<'de> {
    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("hjson: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave_container(&mut self) -> Result<()> {
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    /// Read an unquoted key (stops at the `:` separator).
    fn read_unquoted_key(&mut self) -> Result<String> {
        self.scratch.clear();
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if matches!(b, b':' | b'\n' | b'\r' | b'\t') {
                break;
            }
            self.scratch.push(b as char);
            self.pos += 1;
        }
        Ok(core::mem::take(&mut self.scratch).trim().to_string())
    }
}

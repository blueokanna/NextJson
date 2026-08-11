//! JSON5 codec (JSON superset with a documented subset of ES5 productions).
//!
//! The encoder emits standard JSON (JSON5 is a superset). The decoder
//! accepts: `//` and `/* */` comments, unquoted ASCII identifier keys,
//! single- and double-quoted strings (with common escapes; lone surrogate
//! escapes become U+FFFD), line continuations, trailing commas, hexadecimal
//! / leading-`+` / dot-leading numbers, `Infinity`, `NaN`, and `-Infinity`.
//! Unicode identifier characters in unquoted keys are not supported.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::encoding::{EncodeConfig, Encoder};
use crate::error::{Error, Result};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::NsonSerialize;

/// JSON5 format marker.
#[derive(Clone, Copy, Debug)]
pub struct Json5;

impl Format for Json5 {
    const NAME: &'static str = "json5";
    const MIME: &'static str = "application/json5";
    const EXTENSIONS: &'static [&'static str] = &["json5"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = Encoder::for_vec(EncodeConfig::compact());
        T::nextencode(value, &mut encoder)?;
        encoder.finish_vec()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = Json5Decoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

/// The JSON5 encoder is the standard JSON encoder (JSON5 is a superset).
pub type Json5Encoder<W> = Encoder<W>;

/// Streaming JSON5 decoder.
pub struct Json5Decoder<'de> {
    input: &'de [u8],
    pos: usize,
    lookahead: Option<Token<'de>>,
    scratch: String,
    depth: u32,
    max_depth: u32,
}

impl<'de> Json5Decoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        Json5Decoder {
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
            Err(Error::custom("json5: trailing bytes after value"))
        }
    }

    fn skip_ws(&mut self) -> Result<()> {
        loop {
            while self.pos < self.input.len()
                && matches!(
                    self.input[self.pos],
                    b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C | 0xA0
                )
            {
                self.pos += 1;
            }
            if self.pos < self.input.len() && self.input[self.pos] == b'/' {
                match self.input.get(self.pos + 1) {
                    Some(b'/') => {
                        while self.pos < self.input.len() && self.input[self.pos] != b'\n' {
                            self.pos += 1;
                        }
                        continue;
                    }
                    Some(b'*') => {
                        self.pos += 2;
                        loop {
                            if self.pos >= self.input.len() {
                                return Err(Error::custom("json5: unterminated block comment"));
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
                    _ => {}
                }
            }
            break;
        }
        Ok(())
    }

    fn peek_byte(&mut self) -> Result<u8> {
        self.skip_ws()?;
        self.input
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::custom("json5: unexpected end of input"))
    }

    fn read_string(&mut self) -> Result<String> {
        let quote = self.input[self.pos];
        self.pos += 1;
        self.scratch.clear();
        loop {
            if self.pos >= self.input.len() {
                return Err(Error::custom("json5: unterminated string"));
            }
            match self.input[self.pos] {
                c if c == quote => {
                    self.pos += 1;
                    return Ok(core::mem::take(&mut self.scratch));
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.input.len() {
                        return Err(Error::custom("json5: unterminated escape"));
                    }
                    match self.input[self.pos] {
                        b'n' => {
                            self.scratch.push('\n');
                            self.pos += 1;
                        }
                        b't' => {
                            self.scratch.push('\t');
                            self.pos += 1;
                        }
                        b'r' => {
                            self.scratch.push('\r');
                            self.pos += 1;
                        }
                        b'b' => {
                            self.scratch.push('\u{8}');
                            self.pos += 1;
                        }
                        b'f' => {
                            self.scratch.push('\u{c}');
                            self.pos += 1;
                        }
                        b'v' => {
                            self.scratch.push('\u{b}');
                            self.pos += 1;
                        }
                        b'0' => {
                            self.scratch.push('\0');
                            self.pos += 1;
                        }
                        b'\n' => {
                            // Line continuation.
                            self.pos += 1;
                        }
                        b'\r' => {
                            self.pos += 1;
                            if self.input.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'x' => {
                            let cp = self.read_hex(2)?;
                            // Lone surrogates / invalid scalars are replaced
                            // with U+FFFD per the JSON5 spec (never silently
                            // dropped).
                            self.scratch
                                .push(char::from_u32(cp as u32).unwrap_or('\u{FFFD}'));
                        }
                        b'u' => {
                            let cp = self.read_hex(4)?;
                            self.scratch
                                .push(char::from_u32(cp as u32).unwrap_or('\u{FFFD}'));
                        }
                        other => {
                            self.scratch.push(other as char);
                            self.pos += 1;
                        }
                    }
                }
                b => {
                    let len = utf8_len(b).ok_or_else(|| Error::custom("json5: invalid utf-8"))?;
                    let chunk = self
                        .input
                        .get(self.pos..self.pos + len)
                        .ok_or_else(|| Error::custom("json5: truncated utf-8"))?;
                    let s = core::str::from_utf8(chunk)
                        .map_err(|_| Error::custom("json5: invalid utf-8"))?;
                    self.scratch.push_str(s);
                    self.pos += len;
                }
            }
        }
    }

    fn read_hex(&mut self, n: usize) -> Result<u16> {
        let mut v: u16 = 0;
        for _ in 0..n {
            if self.pos >= self.input.len() {
                return Err(Error::custom("json5: truncated hex escape"));
            }
            let d = match self.input[self.pos] {
                b'0'..=b'9' => (self.input[self.pos] - b'0') as u16,
                b'a'..=b'f' => (self.input[self.pos] - b'a' + 10) as u16,
                b'A'..=b'F' => (self.input[self.pos] - b'A' + 10) as u16,
                _ => return Err(Error::custom("json5: invalid hex escape")),
            };
            v = v * 16 + d;
            self.pos += 1;
        }
        Ok(v)
    }

    fn read_number(&mut self) -> Result<Number> {
        let start = self.pos;
        let mut negative = false;
        if self.input.get(self.pos) == Some(&b'+') || self.input.get(self.pos) == Some(&b'-') {
            negative = self.input[self.pos] == b'-';
            self.pos += 1;
        }
        // Hex numbers.
        if self.pos + 1 < self.input.len()
            && self.input[self.pos] == b'0'
            && matches!(self.input[self.pos + 1], b'x' | b'X')
        {
            self.pos += 2;
            let hstart = self.pos;
            while self.pos < self.input.len()
                && (self.input[self.pos].is_ascii_hexdigit() || self.input[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let digits: String = core::str::from_utf8(&self.input[hstart..self.pos])
                .map_err(|_| Error::custom("json5: invalid hex number"))?
                .chars()
                .filter(|c| *c != '_')
                .collect();
            let value = u64::from_str_radix(&digits, 16)
                .map_err(|_| Error::custom("json5: invalid hex number"))?;
            if negative {
                // Preserve the leading `-` (JSON5 allows negative hex).
                return Ok(Number::from(-(value as i128)));
            }
            return Ok(Number::from(value));
        }
        // Infinity / NaN handled in read_token.
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-' | b'_') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = core::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| Error::custom("json5: invalid number"))?
            .replace('_', "");
        let text = text.trim_start_matches('+');
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
            "json5: invalid number {text:?}"
        )))
    }

    fn read_identifier(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(core::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| Error::custom("json5: invalid identifier"))?
            .to_string())
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
            b'"' | b'\'' => Ok(Token::Str(Cow::Owned(self.read_string()?))),
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                if b == b'.' && !self.input.get(self.pos + 1).is_some_and(u8::is_ascii_digit) {
                    // A bare `.` is not a number.
                    return Err(Error::custom("json5: invalid number"));
                }
                let n = self.read_number()?;
                Ok(Token::Number(n))
            }
            b't' | b'f' | b'n' | b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'$' => {
                // Unquoted key or a bare value identifier.
                let ident = self.read_identifier()?;
                match ident.as_str() {
                    "true" => Ok(Token::Bool(true)),
                    "false" => Ok(Token::Bool(false)),
                    "null" => Ok(Token::Null),
                    "Infinity" => Ok(Token::Number(Number::F64(f64::INFINITY))),
                    "NaN" => Ok(Token::Number(Number::F64(f64::NAN))),
                    _ => Ok(Token::Str(Cow::Owned(ident))),
                }
            }
            other => Err(Error::custom(alloc::format!(
                "json5: unexpected byte 0x{other:02x}"
            ))),
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

impl<'de> FormatDecoder<'de> for Json5Decoder<'de> {
    fn begin_object(&mut self) -> Result<()> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginObject => Ok(()),
            other => Err(Error::invalid_type("'{'", token_name(&other))),
        }
    }

    fn end_object(&mut self) -> Result<()> {
        self.leave_container()?;
        match self.next_token()? {
            Token::EndObject => Ok(()),
            other => Err(Error::invalid_type("'}'", token_name(&other))),
        }
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        if matches!(self.peek_token()?, Token::EndObject) {
            return Ok(None);
        }
        if matches!(self.peek_token()?, Token::BeginObject) {
            return Ok(None);
        }
        let key = match self.next_token()? {
            Token::Str(s) => s,
            other => {
                return Err(Error::invalid_type(
                    "a string or identifier key",
                    token_name(&other),
                ))
            }
        };
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b':') {
            self.pos += 1;
        }
        Ok(Some(key))
    }

    fn object_entry_sep(&mut self) -> Result<bool> {
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b',') {
            self.pos += 1;
        }
        Ok(!matches!(self.peek_token()?, Token::EndObject))
    }

    fn begin_array(&mut self) -> Result<()> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => Ok(()),
            other => Err(Error::invalid_type("'['", token_name(&other))),
        }
    }

    fn end_array(&mut self) -> Result<()> {
        self.leave_container()?;
        match self.next_token()? {
            Token::EndArray => Ok(()),
            other => Err(Error::invalid_type("']'", token_name(&other))),
        }
    }

    fn array_has_more(&mut self) -> Result<bool> {
        Ok(!matches!(self.peek_token()?, Token::EndArray))
    }

    fn array_entry_sep(&mut self) -> Result<bool> {
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b',') {
            self.pos += 1;
        }
        Ok(!matches!(self.peek_token()?, Token::EndArray))
    }

    fn unit(&mut self) -> Result<()> {
        match self.next_token()? {
            Token::Null => Ok(()),
            other => Err(Error::invalid_type("null", token_name(&other))),
        }
    }

    fn bool(&mut self) -> Result<bool> {
        match self.next_token()? {
            Token::Bool(b) => Ok(b),
            other => Err(Error::invalid_type("bool", token_name(&other))),
        }
    }

    fn number(&mut self) -> Result<Number> {
        match self.next_token()? {
            Token::Number(n) => Ok(n),
            other => Err(Error::invalid_type("number", token_name(&other))),
        }
    }

    fn string(&mut self) -> Result<Cow<'de, str>> {
        match self.next_token()? {
            Token::Str(s) => Ok(s),
            other => Err(Error::invalid_type("string", token_name(&other))),
        }
    }

    fn char(&mut self) -> Result<char> {
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

    fn skip_value(&mut self) -> Result<()> {
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

    fn peek_token(&mut self) -> Result<Token<'de>> {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.read_token()?);
        }
        Ok(self.lookahead.as_ref().expect("set").clone())
    }

    fn next_token(&mut self) -> Result<Token<'de>> {
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

impl<'de> Json5Decoder<'de> {
    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("json5: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave_container(&mut self) -> Result<()> {
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }
}

//! S-expression codec (Lisp-family textual notation).
//!
//! Encodes: lists `(a b c)`, atoms (bare tokens or quoted strings), numbers,
//! booleans `#t`/`#f`, `null` as `nil`, and maps as association lists
//! `((key value) ...)`. Decoding accepts all of these plus `true`/`false`
//! atoms for compatibility.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{token_name, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// S-expression format marker.
#[derive(Clone, Copy, Debug)]
pub struct Sexpr;

impl Format for Sexpr {
    const NAME: &'static str = "sexpr";
    const MIME: &'static str = "text/x-sexpr";
    const EXTENSIONS: &'static [&'static str] = &["sexp", "sx", "scm"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = SexprEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.writer.write_all(&encoder.buf)?;
        Ok(core::mem::take(&mut encoder.buf))
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = SexprDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

/// A bare atom needs quoting if it contains these characters.
fn needs_quoting(s: &str) -> bool {
    s.is_empty()
        || s.bytes()
            .any(|b| b <= b' ' || b == b'(' || b == b')' || b == b'"' || b == b';' || b == b'\\')
}

fn write_quoted(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for &b in s.as_bytes() {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            other => out.push(other),
        }
    }
    out.push(b'"');
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum EFrameKind {
    List,
    Alist,
}

struct EFrame {
    kind: EFrameKind,
    pair_open: bool,
    any: bool,
}

/// Streaming S-expression encoder.
pub struct SexprEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    frames: Vec<EFrame>,
}

impl<W: Write> SexprEncoder<W> {
    /// Create an S-expression encoder over `writer`.
    pub fn new(writer: W) -> Self {
        SexprEncoder {
            writer,
            buf: Vec::with_capacity(512),
            frames: Vec::new(),
        }
    }

    fn write_atom(&mut self, s: &str) {
        if needs_quoting(s) {
            write_quoted(&mut self.buf, s);
        } else {
            self.buf.extend_from_slice(s.as_bytes());
        }
    }

    fn value_sep(&mut self) -> Result<()> {
        if let Some(frame) = self.frames.last_mut() {
            match frame.kind {
                EFrameKind::List => {
                    if frame.any {
                        self.buf.push(b' ');
                    }
                    frame.any = true;
                }
                EFrameKind::Alist => {
                    // Within a pair, key and value are space-separated.
                    self.buf.push(b' ');
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
}

impl<W: Write> FormatEncoder for SexprEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.frames.push(EFrame {
            kind: EFrameKind::List,
            pair_open: false,
            any: false,
        });
        self.buf.push(b'(');
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.frames
            .pop()
            .ok_or_else(|| Error::custom("sexpr: list end without start"))?;
        self.buf.push(b')');
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.frames.push(EFrame {
            kind: EFrameKind::Alist,
            pair_open: false,
            any: false,
        });
        self.buf.push(b'(');
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| Error::custom("sexpr: key outside alist"))?;
        if frame.pair_open {
            // Close the previous pair.
            self.buf.push(b')');
            self.buf.push(b' ');
        }
        frame.pair_open = true;
        frame.any = true;
        self.buf.push(b'(');
        self.write_atom(key);
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("sexpr: alist end without start"))?;
        if frame.pair_open {
            self.buf.push(b')');
        }
        self.buf.push(b')');
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf.extend_from_slice(b"nil");
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.buf
            .extend_from_slice(if value { b"#t" } else { b"#f" });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.value_sep()?;
        self.write_atom(value);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        self.write_str(&value.to_string())
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

#[derive(Clone, Copy, PartialEq)]
enum DFrameKind {
    List,
    Alist,
}

#[derive(Clone, Copy)]
struct DFrame {
    kind: DFrameKind,
}

/// Streaming S-expression decoder.
pub struct SexprDecoder<'de> {
    input: &'de [u8],
    pos: usize,
    lookahead: Option<Token<'de>>,
    scratch: String,
    frames: Vec<DFrame>,
    depth: u32,
    max_depth: u32,
}

impl<'de> SexprDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        SexprDecoder {
            input,
            pos: 0,
            lookahead: None,
            scratch: String::new(),
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
        if self.lookahead.is_none() && self.frames.is_empty() && self.pos >= self.input.len() {
            Ok(())
        } else {
            Err(Error::custom("sexpr: trailing bytes after value"))
        }
    }

    fn skip_ws(&mut self) -> Result<()> {
        loop {
            while self.pos < self.input.len()
                && matches!(self.input[self.pos], b' ' | b'\t' | b'\n' | b'\r')
            {
                self.pos += 1;
            }
            // Comments start with `;`.
            if self.pos < self.input.len() && self.input[self.pos] == b';' {
                while self.pos < self.input.len() && self.input[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn read_quoted(&mut self) -> Result<String> {
        self.pos += 1; // opening quote
        self.scratch.clear();
        loop {
            if self.pos >= self.input.len() {
                return Err(Error::custom("sexpr: unterminated string"));
            }
            match self.input[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Ok(core::mem::take(&mut self.scratch));
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.input.len() {
                        return Err(Error::custom("sexpr: unterminated escape"));
                    }
                    let c = match self.input[self.pos] {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'"' => '"',
                        b'\\' => '\\',
                        other => other as char,
                    };
                    self.scratch.push(c);
                    self.pos += 1;
                }
                b => {
                    // UTF-8 continuation.
                    let len = utf8_len(b).ok_or_else(|| Error::custom("sexpr: invalid utf-8"))?;
                    let chunk = self
                        .input
                        .get(self.pos..self.pos + len)
                        .ok_or_else(|| Error::custom("sexpr: truncated utf-8"))?;
                    let s = core::str::from_utf8(chunk)
                        .map_err(|_| Error::custom("sexpr: invalid utf-8"))?;
                    self.scratch.push_str(s);
                    self.pos += len;
                }
            }
        }
    }

    fn read_atom(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b <= b' ' || b == b'(' || b == b')' || b == b'"' || b == b';' {
                break;
            }
            self.pos += 1;
        }
        // A stop character with zero bytes consumed would produce an empty
        // atom and leave `pos` unchanged; callers that loop on `has_more`
        // would then spin forever. Reject it instead.
        if self.pos == start {
            return Err(Error::custom("sexpr: empty atom"));
        }
        let raw = &self.input[start..self.pos];
        let s = core::str::from_utf8(raw).map_err(|_| Error::custom("sexpr: invalid atom"))?;
        Ok(s.to_string())
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        self.skip_ws()?;
        if self.pos >= self.input.len() {
            return Err(Error::custom("sexpr: unexpected end of input"));
        }
        match self.input[self.pos] {
            b'(' => {
                self.pos += 1;
                Ok(Token::BeginArray)
            }
            b')' => {
                let top = self.frames.last().copied();
                match top {
                    Some(DFrame {
                        kind: DFrameKind::List,
                    }) => {
                        self.pos += 1;
                        Ok(Token::EndArray)
                    }
                    Some(DFrame {
                        kind: DFrameKind::Alist,
                    }) => {
                        self.pos += 1;
                        Ok(Token::EndObject)
                    }
                    None => Err(Error::custom("sexpr: unmatched ')'")),
                }
            }
            b'"' => Ok(Token::Str(Cow::Owned(self.read_quoted()?))),
            _ => {
                let atom = self.read_atom()?;
                atom_to_token(atom)
            }
        }
    }
}

/// Convert a bare atom into a token.
fn atom_to_token(atom: String) -> Result<Token<'static>> {
    match atom.as_str() {
        "nil" | "null" => Ok(Token::Null),
        "#t" | "true" => Ok(Token::Bool(true)),
        "#f" | "false" => Ok(Token::Bool(false)),
        _ => {
            if let Ok(v) = atom.parse::<i64>() {
                return Ok(Token::Number(Number::from(v)));
            }
            if let Ok(v) = atom.parse::<f64>() {
                return Ok(Token::Number(Number::F64(v)));
            }
            Ok(Token::Str(Cow::Owned(atom)))
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

impl<'de> FormatDecoder<'de> for SexprDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        // An alist is a `(...)` container; the target decides whether a
        // paren begins an object (alist) or an array (list).
        match self.next_token()? {
            Token::BeginArray => {
                self.frames.push(DFrame {
                    kind: DFrameKind::Alist,
                });
                Ok(())
            }
            other => Err(Error::invalid_type("an alist", token_name(&other))),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        let r = match self.next_token()? {
            Token::EndObject => Ok(()),
            other => Err(Error::invalid_type(
                "an alist terminator",
                token_name(&other),
            )),
        };
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("sexpr: alist end without start"))?;
        if frame.kind != DFrameKind::Alist {
            return Err(Error::custom("sexpr: alist frame mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        r
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        // Alist format: `((key value) (key value) ...)`.
        self.skip_ws()?;
        if self.pos >= self.input.len() {
            return Err(Error::custom("sexpr: unexpected end of input"));
        }
        if self.input[self.pos] == b')' {
            // End of the alist; leave it for end_object.
            return Ok(None);
        }
        // Pair starts with `(`.
        if self.input[self.pos] != b'(' {
            return Err(Error::custom("sexpr: expected a key/value pair"));
        }
        self.pos += 1;
        self.skip_ws()?;
        let key = if self.input.get(self.pos) == Some(&b'"') {
            Cow::Owned(self.read_quoted()?)
        } else {
            Cow::Owned(self.read_atom()?)
        };
        self.skip_ws()?;
        Ok(Some(key))
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        // Close the current `(key value)` pair; the alist's own `)` is left
        // for `end_object`.
        self.skip_ws()?;
        if self.input.get(self.pos) == Some(&b')') {
            self.pos += 1;
            self.skip_ws()?;
            Ok(self.input.get(self.pos) != Some(&b')'))
        } else {
            Err(Error::custom("sexpr: expected ')' after pair value"))
        }
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => {
                self.frames.push(DFrame {
                    kind: DFrameKind::List,
                });
                Ok(())
            }
            other => Err(Error::invalid_type("a list", token_name(&other))),
        }
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        let r = match self.next_token()? {
            Token::EndArray => Ok(()),
            other => Err(Error::invalid_type("a list terminator", token_name(&other))),
        };
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("sexpr: list end without start"))?;
        if frame.kind != DFrameKind::List {
            return Err(Error::custom("sexpr: list frame mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        r
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        self.skip_ws()?;
        Ok(self.input.get(self.pos) != Some(&b')'))
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.skip_ws()?;
        Ok(self.input.get(self.pos) != Some(&b')'))
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
            Token::BeginObject | Token::BeginArray => {
                // Generic skip: consume balanced delimiters via save/restore.
                let saved = self.pos;
                self.next_token()?;
                let mut depth = 1usize;
                loop {
                    self.skip_ws()?;
                    if self.pos >= self.input.len() {
                        self.pos = saved;
                        return Err(Error::custom("sexpr: unbalanced container"));
                    }
                    match self.input[self.pos] {
                        b'(' => {
                            depth += 1;
                            self.pos += 1;
                        }
                        b')' => {
                            depth -= 1;
                            self.pos += 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        b'"' => {
                            self.read_quoted()?;
                        }
                        _ => {
                            self.read_atom()?;
                        }
                    }
                }
                self.lookahead = None;
                Ok(())
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

impl<'de> SexprDecoder<'de> {
    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("sexpr: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }
}

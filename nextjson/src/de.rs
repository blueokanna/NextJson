//! Deserialization: a unified token-stream decoder, the [`NsonDeserialize`]
//! trait, and standard-library implementations.
//!
//! # Architecture note
//!
//! The `Decoder` holds one of two input sources:
//!
//! - **`Bytes`**: lazy single-token-lookahead lexing over `&[u8]`; unescaped
//!   strings borrow the input with zero allocation;
//! - **`Tree`**: replay of an in-memory `Vec<Token>` (used by internally /
//!   adjacently tagged enums and `Value`-driven decoding).
//!
//! Both sources expose the exact same decode primitives, so macro-generated
//! code never needs a second set of mechanisms.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, BinaryHeap, LinkedList, VecDeque};
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::{Range, RangeFrom, RangeInclusive, RangeTo, RangeToInclusive};
use core::time::Duration;

use crate::error::{Error, ErrorKind, Result};
use crate::map::Map;
use crate::number::Number;
use crate::value::Value;

/// Parser configuration.
#[derive(Clone, Debug)]
pub struct DecodeConfig {
    /// Maximum nesting depth (protects against stack overflow). Default 128.
    pub max_depth: u32,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        DecodeConfig { max_depth: 128 }
    }
}

impl DecodeConfig {
    /// Default config.
    pub fn new() -> Self {
        DecodeConfig::default()
    }
    /// Set the maximum nesting depth.
    pub fn max_depth(mut self, depth: u32) -> Self {
        self.max_depth = depth;
        self
    }
}

/// Save point for untagged-enum backtracking.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pos: usize,
    depth: u32,
}

/// Internal token shared by the streaming lexer and content replay.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub enum Token<'de> {
    /// `null`
    Null,
    /// boolean
    Bool(bool),
    /// number
    Number(Number),
    /// string
    Str(Cow<'de, str>),
    /// `{`
    BeginObject,
    /// `}`
    EndObject,
    /// `[`
    BeginArray,
    /// `]`
    EndArray,
}

/// Deserialization trait.
///
/// Unlike serde's visitor callbacks, [`decode_into`](NsonDeserialize::decode_into)
/// decodes a value directly into a caller-provided [`MaybeUninit`] slot,
/// supporting memory reuse and zero initialization cost.
pub trait NsonDeserialize<'de>: Sized {
    /// Decode into `out`. Contract: on `Ok` return, `out` is fully initialized.
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()>;

    /// Convenience: allocate an uninitialized slot and call
    /// [`decode_into`](NsonDeserialize::decode_into).
    fn decode(decoder: &mut Decoder<'de>) -> Result<Self> {
        let mut out = MaybeUninit::uninit();
        Self::decode_into(decoder, &mut out)?;
        // SAFETY: guaranteed by the decode_into contract — on Ok, out is
        // fully initialized. This is one of the library's audited
        // `MaybeUninit` boundaries.
        #[allow(unsafe_code)]
        Ok(unsafe { out.assume_init() })
    }
}

// ---------------------------------------------------------------------------
// Input sources
// ---------------------------------------------------------------------------

/// Byte-stream source: lazy lexing over `&[u8]`.
struct BytesReader<'de> {
    input: &'de [u8],
    pos: usize,
    lookahead: Option<Token<'de>>,
}

/// Content-replay source: replay over an in-memory token vector.
struct TreeReader<'de> {
    tokens: Vec<Token<'de>>,
    pos: usize,
    lookahead: Option<Token<'de>>,
}

enum Inner<'de> {
    Bytes(BytesReader<'de>),
    Tree(TreeReader<'de>),
}

impl<'de> BytesReader<'de> {
    #[inline]
    fn skip_ws(&mut self) -> usize {
        let input = self.input;
        let mut pos = self.pos;
        while pos < input.len() && matches!(input[pos], b' ' | b'\t' | b'\n' | b'\r') {
            pos += 1;
        }
        self.pos = pos;
        pos
    }

    fn err_at(&self, kind: ErrorKind, pos: usize) -> Error {
        let (line, col) = line_col(self.input, pos);
        Error::new(kind, Some(line), Some(col), pos)
    }

    fn err(&self, kind: ErrorKind) -> Error {
        self.err_at(kind, self.pos)
    }

    fn next_token(&mut self, scratch: &mut String) -> Result<Token<'de>> {
        if let Some(t) = self.lookahead.take() {
            return Ok(t);
        }
        self.lex(scratch)
    }

    fn peek_token(&mut self, scratch: &mut String) -> Result<Token<'de>> {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.lex(scratch)?);
        }
        Ok(self.lookahead.as_ref().expect("just set").clone())
    }

    fn lex(&mut self, scratch: &mut String) -> Result<Token<'de>> {
        let input = self.input;
        let pos = self.skip_ws();
        if pos >= input.len() {
            return Err(self.err_at(ErrorKind::Eof, pos));
        }
        self.pos = pos;
        match input[pos] {
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
            b'"' => self.lex_string(scratch),
            b't' => self.lex_literal(pos, b"true", Token::Bool(true)),
            b'f' => self.lex_literal(pos, b"false", Token::Bool(false)),
            b'n' => self.lex_literal(pos, b"null", Token::Null),
            b'-' | b'0'..=b'9' => self.lex_number(),
            other => Err(self.err_at(
                ErrorKind::Expected {
                    what: "a JSON value",
                    found: Some(other),
                },
                pos,
            )),
        }
    }

    fn end(&mut self) -> Result<()> {
        let pos = self.skip_ws();
        if self.lookahead.is_none() && pos == self.input.len() {
            Ok(())
        } else {
            Err(self.err_at(
                ErrorKind::Expected {
                    what: "end of input",
                    found: self.input.get(pos).copied(),
                },
                pos,
            ))
        }
    }

    fn lex_literal(&mut self, pos: usize, lit: &[u8], tok: Token<'de>) -> Result<Token<'de>> {
        let input = self.input;
        if input.len() - pos < lit.len() || &input[pos..pos + lit.len()] != lit {
            return Err(self.err_at(
                ErrorKind::Expected {
                    what: "a JSON literal",
                    found: input.get(pos).copied(),
                },
                pos,
            ));
        }
        let end = pos + lit.len();
        if let Some(&b) = input.get(end) {
            if b.is_ascii_alphanumeric() || b == b'_' {
                return Err(self.err_at(
                    ErrorKind::Expected {
                        what: "a JSON literal",
                        found: Some(b),
                    },
                    pos,
                ));
            }
        }
        self.pos = end;
        Ok(tok)
    }

    fn lex_string(&mut self, scratch: &mut String) -> Result<Token<'de>> {
        let input = self.input;
        let start = self.pos + 1;
        let mut i = start;
        while i < input.len() {
            match input[i] {
                b'"' => {
                    let raw = &input[start..i];
                    match core::str::from_utf8(raw) {
                        Ok(s) => {
                            self.pos = i + 1;
                            return Ok(Token::Str(Cow::Borrowed(s)));
                        }
                        Err(_) => return Err(self.err_at(ErrorKind::InvalidUtf8, i)),
                    }
                }
                b'\\' => {
                    let s = self.unescape(input, start, i, scratch)?;
                    return Ok(Token::Str(Cow::Owned(s)));
                }
                0x00..=0x1F => return Err(self.err_at(ErrorKind::ControlCharInString, i)),
                _ => i += 1,
            }
        }
        Err(self.err_at(ErrorKind::Eof, input.len()))
    }

    /// Handle a string containing escapes; `start` is after the opening quote,
    /// `i` points at the first `\`.
    fn unescape(&mut self, input: &[u8], start: usize, mut i: usize, scratch: &mut String) -> Result<String> {
        scratch.clear();
        let seg = core::str::from_utf8(&input[start..i]).map_err(|_| self.err_at(ErrorKind::InvalidUtf8, i))?;
        scratch.push_str(seg);
        while i < input.len() {
            let b = input[i];
            if b == b'"' {
                self.pos = i + 1;
                return Ok(core::mem::take(scratch));
            } else if b == b'\\' {
                i += 1;
                if i >= input.len() {
                    return Err(self.err_at(ErrorKind::Eof, i));
                }
                match input[i] {
                    b'"' => scratch.push('"'),
                    b'\\' => scratch.push('\\'),
                    b'/' => scratch.push('/'),
                    b'b' => scratch.push('\u{8}'),
                    b'f' => scratch.push('\u{c}'),
                    b'n' => scratch.push('\n'),
                    b'r' => scratch.push('\r'),
                    b't' => scratch.push('\t'),
                    b'u' => {
                        let hi = parse_hex4(input, i + 1)
                            .ok_or_else(|| self.err_at(ErrorKind::InvalidEscape('u'), i))?;
                        if (0xD800..=0xDBFF).contains(&hi) {
                            // high surrogate: must be followed by \uXXXX low.
                            if input.get(i + 5..i + 7) == Some(&b"\\u"[..]) {
                                let lo = parse_hex4(input, i + 7)
                                    .ok_or_else(|| self.err_at(ErrorKind::InvalidSurrogate, i))?;
                                if (0xDC00..=0xDFFF).contains(&lo) {
                                    let cp = 0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                                    let c = char::from_u32(cp).expect("valid scalar value");
                                    let mut buf = [0u8; 4];
                                    scratch.push_str(c.encode_utf8(&mut buf));
                                    i += 10;
                                } else {
                                    return Err(self.err_at(ErrorKind::InvalidSurrogate, i));
                                }
                            } else {
                                return Err(self.err_at(ErrorKind::InvalidSurrogate, i));
                            }
                        } else if (0xDC00..=0xDFFF).contains(&hi) {
                            return Err(self.err_at(ErrorKind::InvalidSurrogate, i));
                        } else {
                            let c = char::from_u32(hi as u32).expect("valid scalar value");
                            let mut buf = [0u8; 4];
                            scratch.push_str(c.encode_utf8(&mut buf));
                            i += 4;
                        }
                    }
                    other => return Err(self.err_at(ErrorKind::InvalidEscape(other as char), i)),
                }
                i += 1;
            } else if b < 0x20 {
                return Err(self.err_at(ErrorKind::ControlCharInString, i));
            } else {
                let seg_start = i;
                while i < input.len() && input[i] != b'"' && input[i] != b'\\' && input[i] >= 0x20 {
                    i += 1;
                }
                let seg = core::str::from_utf8(&input[seg_start..i])
                    .map_err(|_| self.err_at(ErrorKind::InvalidUtf8, i))?;
                scratch.push_str(seg);
            }
        }
        Err(self.err_at(ErrorKind::Eof, input.len()))
    }

    fn lex_number(&mut self) -> Result<Token<'de>> {
        let input = self.input;
        let start = self.pos;
        let mut i = self.pos;
        if input[i] == b'-' {
            i += 1;
            if i >= input.len() {
                return Err(self.err_at(ErrorKind::InvalidNumber, i));
            }
        }
        match input[i] {
            b'0' => {
                i += 1;
                if i < input.len() && input[i].is_ascii_digit() {
                    return Err(self.err_at(ErrorKind::InvalidNumber, i));
                }
            }
            b'1'..=b'9' => {
                i += 1;
                while i < input.len() && input[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => return Err(self.err_at(ErrorKind::InvalidNumber, i)),
        }
        let mut is_float = false;
        if i < input.len() && input[i] == b'.' {
            is_float = true;
            i += 1;
            let dstart = i;
            while i < input.len() && input[i].is_ascii_digit() {
                i += 1;
            }
            if i == dstart {
                return Err(self.err_at(ErrorKind::InvalidNumber, i));
            }
        }
        if i < input.len() && (input[i] == b'e' || input[i] == b'E') {
            is_float = true;
            i += 1;
            if i < input.len() && (input[i] == b'+' || input[i] == b'-') {
                i += 1;
            }
            let dstart = i;
            while i < input.len() && input[i].is_ascii_digit() {
                i += 1;
            }
            if i == dstart {
                return Err(self.err_at(ErrorKind::InvalidNumber, i));
            }
        }
        let raw = &input[start..i];
        self.pos = i;
        let n = Number::parse(raw, is_float).map_err(|_| self.err_at(ErrorKind::InvalidNumber, start))?;
        Ok(Token::Number(n))
    }

    fn object_key(&mut self, scratch: &mut String) -> Result<Option<Cow<'de, str>>> {
        let input = self.input;
        let pos = self.skip_ws();
        if pos >= input.len() {
            return Err(self.err_at(ErrorKind::Eof, pos));
        }
        match input[pos] {
            b'}' => return Ok(None),
            b'"' => {}
            other => {
                return Err(self.err_at(
                    ErrorKind::Expected {
                        what: "a string key or '}'",
                        found: Some(other),
                    },
                    pos,
                ))
            }
        }
        self.pos = pos;
        let key = self.lex_string(scratch)?;
        let p2 = self.skip_ws();
        if p2 >= input.len() {
            return Err(self.err_at(ErrorKind::Eof, p2));
        }
        if input[p2] != b':' {
            return Err(self.err_at(
                ErrorKind::Expected {
                    what: "':'",
                    found: Some(input[p2]),
                },
                p2,
            ));
        }
        self.pos = p2 + 1;
        match key {
            Token::Str(s) => Ok(Some(s)),
            _ => unreachable!("lex_string always yields Str"),
        }
    }

    fn object_entry_sep(&mut self) -> Result<bool> {
        let input = self.input;
        let pos = self.skip_ws();
        if pos >= input.len() {
            return Err(self.err_at(ErrorKind::Eof, pos));
        }
        match input[pos] {
            b',' => {
                self.pos = pos + 1;
                Ok(true)
            }
            b'}' => Ok(false),
            other => Err(self.err_at(
                ErrorKind::Expected {
                    what: "',' or '}'",
                    found: Some(other),
                },
                pos,
            )),
        }
    }

    fn array_has_more(&mut self) -> Result<bool> {
        let input = self.input;
        let pos = self.skip_ws();
        if pos >= input.len() {
            return Err(self.err_at(ErrorKind::Eof, pos));
        }
        Ok(input[pos] != b']')
    }

    fn array_entry_sep(&mut self) -> Result<bool> {
        let input = self.input;
        let pos = self.skip_ws();
        if pos >= input.len() {
            return Err(self.err_at(ErrorKind::Eof, pos));
        }
        match input[pos] {
            b',' => {
                self.pos = pos + 1;
                Ok(true)
            }
            b']' => Ok(false),
            other => Err(self.err_at(
                ErrorKind::Expected {
                    what: "',' or ']'",
                    found: Some(other),
                },
                pos,
            )),
        }
    }
}

impl<'de> TreeReader<'de> {
    fn err(&self, kind: ErrorKind) -> Error {
        Error::new(kind, None, None, self.pos)
    }

    fn next_token(&mut self) -> Result<Token<'de>> {
        if let Some(t) = self.lookahead.take() {
            self.pos += 1;
            return Ok(t);
        }
        if self.pos >= self.tokens.len() {
            return Err(self.err(ErrorKind::Eof));
        }
        let t = self.tokens[self.pos].clone();
        self.pos += 1;
        Ok(t)
    }

    fn peek_token(&mut self) -> Result<Token<'de>> {
        if self.lookahead.is_none() {
            if self.pos >= self.tokens.len() {
                return Err(self.err(ErrorKind::Eof));
            }
            self.lookahead = Some(self.tokens[self.pos].clone());
        }
        Ok(self.lookahead.as_ref().expect("just set").clone())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        if matches!(self.peek_token()?, Token::EndObject) {
            return Ok(None);
        }
        match self.next_token()? {
            Token::Str(s) => Ok(Some(s)),
            _ => Err(self.err(ErrorKind::Expected {
                what: "a string key",
                found: None,
            })),
        }
    }

    fn object_entry_sep(&mut self) -> Result<bool> {
        Ok(!matches!(self.peek_token()?, Token::EndObject))
    }

    fn array_has_more(&mut self) -> Result<bool> {
        Ok(!matches!(self.peek_token()?, Token::EndArray))
    }

    fn array_entry_sep(&mut self) -> Result<bool> {
        Ok(!matches!(self.peek_token()?, Token::EndArray))
    }

    fn end(&self) -> Result<()> {
        if self.lookahead.is_none() && self.pos == self.tokens.len() {
            Ok(())
        } else {
            Err(self.err(ErrorKind::Expected {
                what: "end of token stream",
                found: None,
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// JSON decoder over a unified token stream.
pub struct Decoder<'de> {
    inner: Inner<'de>,
    scratch: String,
    depth: u32,
    max_depth: u32,
}

impl<'de> Decoder<'de> {
    /// Create a decoder from byte input (default config).
    pub fn new(input: &'de [u8]) -> Self {
        Decoder::with_config(input, DecodeConfig::default())
    }

    /// Create a decoder from byte input (custom config).
    pub fn with_config(input: &'de [u8], config: DecodeConfig) -> Self {
        Decoder {
            inner: Inner::Bytes(BytesReader {
                input,
                pos: 0,
                lookahead: None,
            }),
            scratch: String::new(),
            depth: 0,
            max_depth: config.max_depth,
        }
    }

    /// Create a decoder over an in-memory token stream.
    pub fn from_tokens(tokens: Vec<Token<'de>>) -> Self {
        Decoder {
            inner: Inner::Tree(TreeReader {
                tokens,
                pos: 0,
                lookahead: None,
            }),
            scratch: String::new(),
            depth: 0,
            max_depth: 128,
        }
    }

    /// The maximum nesting depth.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    pub(crate) fn err(&self, kind: ErrorKind) -> Error {
        match &self.inner {
            Inner::Bytes(r) => r.err(kind),
            Inner::Tree(r) => r.err(kind),
        }
    }

    pub(crate) fn next_token(&mut self) -> Result<Token<'de>> {
        match &mut self.inner {
            Inner::Bytes(r) => r.next_token(&mut self.scratch),
            Inner::Tree(r) => r.next_token(),
        }
    }

    pub(crate) fn peek_token(&mut self) -> Result<Token<'de>> {
        match &mut self.inner {
            Inner::Bytes(r) => r.peek_token(&mut self.scratch),
            Inner::Tree(r) => r.peek_token(),
        }
    }

    /// Save the current position (for untagged-enum backtracking).
    pub fn save(&self) -> Mark {
        let pos = match &self.inner {
            Inner::Bytes(r) => r.pos,
            Inner::Tree(r) => r.pos,
        };
        Mark { pos, depth: self.depth }
    }

    /// Restore a position saved with [`save`](Decoder::save).
    pub fn restore(&mut self, mark: Mark) {
        match &mut self.inner {
            Inner::Bytes(r) => {
                r.pos = mark.pos;
                r.lookahead = None;
            }
            Inner::Tree(r) => {
                r.pos = mark.pos;
                r.lookahead = None;
            }
        }
        self.depth = mark.depth;
    }

    /// Verify that the input contains no value or token after the decoded value.
    ///
    /// Whitespace at the end of byte input is accepted. Top-level helpers call
    /// this automatically; direct [`Decoder`] users can call it after `decode`.
    pub fn end(&mut self) -> Result<()> {
        match &mut self.inner {
            Inner::Bytes(r) => r.end(),
            Inner::Tree(r) => r.end(),
        }
    }

    /// Consume `{` (with depth check).
    pub fn begin_object(&mut self) -> Result<()> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginObject => Ok(()),
            other => Err(self.invalid_type("'{'", &other)),
        }
    }

    /// Consume `}`.
    pub fn end_object(&mut self) -> Result<()> {
        let r = match self.next_token()? {
            Token::EndObject => Ok(()),
            other => Err(self.invalid_type("'}'", &other)),
        };
        self.depth = self.depth.saturating_sub(1);
        r
    }

    /// Read the next object key; returns `None` on `}` (not consumed).
    pub fn object_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        match &mut self.inner {
            Inner::Bytes(r) => r.object_key(&mut self.scratch),
            Inner::Tree(r) => r.object_key(),
        }
    }

    /// Object entry separator: `true` if more entries follow (`，` consumed),
    /// `false` at object end (`}` left for `end_object`).
    pub fn object_entry_sep(&mut self) -> Result<bool> {
        match &mut self.inner {
            Inner::Bytes(r) => r.object_entry_sep(),
            Inner::Tree(r) => r.object_entry_sep(),
        }
    }

    /// Consume `[`.
    pub fn begin_array(&mut self) -> Result<()> {
        self.enter_container()?;
        match self.next_token()? {
            Token::BeginArray => Ok(()),
            other => Err(self.invalid_type("'['", &other)),
        }
    }

    /// Consume `]`.
    pub fn end_array(&mut self) -> Result<()> {
        let r = match self.next_token()? {
            Token::EndArray => Ok(()),
            other => Err(self.invalid_type("']'", &other)),
        };
        self.depth = self.depth.saturating_sub(1);
        r
    }

    /// Whether the array has more elements (`]` not consumed).
    pub fn array_has_more(&mut self) -> Result<bool> {
        match &mut self.inner {
            Inner::Bytes(r) => r.array_has_more(),
            Inner::Tree(r) => r.array_has_more(),
        }
    }

    /// Array entry separator: `true` if more elements follow (`，` consumed),
    /// `false` at array end (`]` left for `end_array`).
    pub fn array_entry_sep(&mut self) -> Result<bool> {
        match &mut self.inner {
            Inner::Bytes(r) => r.array_entry_sep(),
            Inner::Tree(r) => r.array_entry_sep(),
        }
    }

    #[inline]
    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(self.err(ErrorKind::RecursionLimitExceeded));
        }
        self.depth += 1;
        Ok(())
    }

    /// Consume `null`.
    pub fn unit(&mut self) -> Result<()> {
        match self.next_token()? {
            Token::Null => Ok(()),
            other => Err(self.invalid_type("null", &other)),
        }
    }

    /// Read a boolean.
    pub fn bool(&mut self) -> Result<bool> {
        match self.next_token()? {
            Token::Bool(b) => Ok(b),
            other => Err(self.invalid_type("bool", &other)),
        }
    }

    /// Read a number.
    pub fn number(&mut self) -> Result<Number> {
        match self.next_token()? {
            Token::Number(n) => Ok(n),
            other => Err(self.invalid_type("number", &other)),
        }
    }

    /// Read a string (may borrow input).
    pub fn string(&mut self) -> Result<Cow<'de, str>> {
        match self.next_token()? {
            Token::Str(s) => Ok(s),
            other => Err(self.invalid_type("string", &other)),
        }
    }

    /// Read a single character (must be a one-character string).
    pub fn char(&mut self) -> Result<char> {
        match self.next_token()? {
            Token::Str(s) => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(c),
                    _ => Err(self.err(ErrorKind::InvalidType {
                        expected: "a single-character string",
                        found: "string",
                    })),
                }
            }
            other => Err(self.invalid_type("char", &other)),
        }
    }

    /// Skip any one value (recursive, depth-limited).
    pub fn skip_value(&mut self) -> Result<()> {
        match self.peek_token()? {
            Token::BeginObject => {
                self.begin_object()?;
                while self.object_key()?.is_some() {
                    self.skip_value()?;
                    if !self.object_entry_sep()? {
                        break;
                    }
                }
                self.end_object()?;
            }
            Token::BeginArray => {
                self.begin_array()?;
                while self.array_has_more()? {
                    self.skip_value()?;
                    if !self.array_entry_sep()? {
                        break;
                    }
                }
                self.end_array()?;
            }
            _ => {
                self.next_token()?;
            }
        }
        Ok(())
    }

    fn invalid_type(&self, expected: &'static str, found: &Token<'de>) -> Error {
        let found_name = match found {
            Token::Null => "null",
            Token::Bool(_) => "bool",
            Token::Number(_) => "number",
            Token::Str(_) => "string",
            Token::BeginObject => "object",
            Token::EndObject => "end of object",
            Token::BeginArray => "array",
            Token::EndArray => "end of array",
        };
        self.err(ErrorKind::InvalidType {
            expected,
            found: found_name,
        })
    }
}

/// Compute the 1-based line / column of a byte offset.
fn line_col(input: &[u8], pos: usize) -> (u32, u32) {
    let pos = pos.min(input.len());
    let mut line = 1u32;
    let mut col = 1u32;
    for &b in &input[..pos] {
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Parse a 4-digit hex value.
fn parse_hex4(input: &[u8], start: usize) -> Option<u16> {
    if start + 4 > input.len() {
        return None;
    }
    let mut v: u16 = 0;
    for &b in &input[start..start + 4] {
        let d = match b {
            b'0'..=b'9' => (b - b'0') as u16,
            b'a'..=b'f' => (b - b'a' + 10) as u16,
            b'A'..=b'F' => (b - b'A' + 10) as u16,
            _ => return None,
        };
        v = v * 16 + d;
    }
    Some(v)
}

// ---------------------------------------------------------------------------
// std / alloc NsonDeserialize implementations
// ---------------------------------------------------------------------------

macro_rules! impl_signed_de {
    ($($t:ty),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
                let n = decoder.number()?;
                let exact = n
                    .as_i128()
                    .ok_or_else(|| Error::invalid_type("an integer", "a number"))?;
                let v = <$t>::try_from(exact)
                    .map_err(|_| Error::invalid_type("an integer", "a number"))?;
                out.write(v);
                Ok(())
            }
        }
    )*};
}
impl_signed_de!(i8, i16, i32, i64, i128, isize);

macro_rules! impl_unsigned_de {
    ($($t:ty),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
                let n = decoder.number()?;
                let exact = n
                    .as_u128()
                    .ok_or_else(|| Error::invalid_type("an unsigned integer", "a number"))?;
                let v = <$t>::try_from(exact)
                    .map_err(|_| Error::invalid_type("an unsigned integer", "a number"))?;
                out.write(v);
                Ok(())
            }
        }
    )*};
}
impl_unsigned_de!(u8, u16, u32, u64, u128, usize);

impl<'de> NsonDeserialize<'de> for f64 {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let n = decoder.number()?;
        out.write(n.as_f64());
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for f32 {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let n = decoder.number()?;
        out.write(n.as_f64() as f32);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for bool {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let b = decoder.bool()?;
        out.write(b);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for char {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let c = decoder.char()?;
        out.write(c);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for String {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let s = decoder.string()?;
        out.write(s.into_owned());
        Ok(())
    }
}

impl<'de, 'a> NsonDeserialize<'de> for &'a str
where
    'de: 'a,
{
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        match decoder.string()? {
            Cow::Borrowed(b) => {
                out.write(b);
                Ok(())
            }
            Cow::Owned(_) => Err(Error::invalid_type(
                "a borrowed string (no escape sequences)",
                "a string",
            )),
        }
    }
}

impl<'de> NsonDeserialize<'de> for Cow<'de, str> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let s = decoder.string()?;
        out.write(s);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for &'de [u8] {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        match decoder.string()? {
            Cow::Borrowed(b) => {
                out.write(b.as_bytes());
                Ok(())
            }
            Cow::Owned(_) => Err(Error::invalid_type(
                "a borrowed byte string (no escape sequences)",
                "a string",
            )),
        }
    }
}

impl<'de> NsonDeserialize<'de> for Cow<'de, [u8]> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        match decoder.string()? {
            Cow::Borrowed(b) => out.write(Cow::Borrowed(b.as_bytes())),
            Cow::Owned(o) => out.write(Cow::Owned(o.into_bytes())),
        };
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for () {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        decoder.unit()?;
        out.write(());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Option<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        if matches!(decoder.peek_token()?, Token::Null) {
            decoder.next_token()?;
            out.write(None);
        } else {
            let v = T::decode(decoder)?;
            out.write(Some(v));
        }
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Vec<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let mut v = Vec::new();
        decoder.begin_array()?;
        while decoder.array_has_more()? {
            v.push(T::decode(decoder)?);
            if !decoder.array_entry_sep()? {
                break;
            }
        }
        decoder.end_array()?;
        out.write(v);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Box<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = T::decode(decoder)?;
        out.write(Box::new(v));
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Box<str> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let s = decoder.string()?;
        out.write(s.into_owned().into_boxed_str());
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Box<[u8]> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let s = decoder.string()?;
        out.write(s.into_owned().into_bytes().into_boxed_slice());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Rc<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = T::decode(decoder)?;
        out.write(Rc::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Arc<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = T::decode(decoder)?;
        out.write(Arc::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de> + Copy> NsonDeserialize<'de> for Cell<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = T::decode(decoder)?;
        out.write(Cell::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RefCell<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = T::decode(decoder)?;
        out.write(RefCell::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for VecDeque<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = Vec::<T>::decode(decoder)?;
        out.write(v.into());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for LinkedList<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = Vec::<T>::decode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de> + Ord> NsonDeserialize<'de> for BinaryHeap<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = Vec::<T>::decode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de> + Ord> NsonDeserialize<'de> for BTreeSet<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = Vec::<T>::decode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

impl<'de, K: for<'a> NsonDeserialize<'a> + Ord, V: NsonDeserialize<'de>> NsonDeserialize<'de>
    for BTreeMap<K, V>
{
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let mut map = BTreeMap::new();
        decoder.begin_object()?;
        while let Some(key) = decoder.object_key()? {
            let v = V::decode(decoder)?;
            let k = decode_map_key::<K>(key.as_ref())?;
            map.insert(k, v);
            if !decoder.object_entry_sep()? {
                break;
            }
        }
        decoder.end_object()?;
        out.write(map);
        Ok(())
    }
}

/// Decode `K` from an object key string.
///
/// JSON object keys are strings: numeric / boolean keys parse directly from
/// the raw content, while string keys need quotes. Try raw first, then quoted.
fn decode_map_key<K: for<'a> NsonDeserialize<'a>>(key: &str) -> Result<K> {
    let raw = key.as_bytes();
    let mut d0 = Decoder::new(raw);
    if let Ok(v) = K::decode(&mut d0) {
        if d0.end().is_ok() {
            return Ok(v);
        }
    }
    let mut d1 = Decoder::from_tokens(vec![Token::Str(Cow::Borrowed(key))]);
    let value = K::decode(&mut d1)?;
    d1.end()?;
    Ok(value)
}

#[cfg(feature = "std")]
impl<'de, K: for<'a> NsonDeserialize<'a> + Eq + core::hash::Hash, V: NsonDeserialize<'de>>
    NsonDeserialize<'de> for std::collections::HashMap<K, V>
{
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let mut map = std::collections::HashMap::new();
        decoder.begin_object()?;
        while let Some(key) = decoder.object_key()? {
            let v = V::decode(decoder)?;
            let k = decode_map_key::<K>(key.as_ref())?;
            map.insert(k, v);
            if !decoder.object_entry_sep()? {
                break;
            }
        }
        decoder.end_object()?;
        out.write(map);
        Ok(())
    }
}

#[cfg(feature = "std")]
impl<'de, T: NsonDeserialize<'de> + Eq + core::hash::Hash> NsonDeserialize<'de>
    for std::collections::HashSet<T>
{
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = Vec::<T>::decode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

macro_rules! impl_tuple_de {
    ($(($first:ident : $First:ident $(, $i:ident : $T:ident)*)),* $(,)?) => {$(
        impl<'de, $First: NsonDeserialize<'de> $(, $T: NsonDeserialize<'de>)*> NsonDeserialize<'de> for ($First, $( $T, )*) {
            #[allow(non_snake_case)]
            fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
                decoder.begin_array()?;
                let $first = $First::decode(decoder)?;
                $(
                    if !decoder.array_entry_sep()? {
                        return Err(Error::invalid_length(0, "a tuple"));
                    }
                    let $i = $T::decode(decoder)?;
                )*
                if decoder.array_entry_sep()? {
                    return Err(Error::invalid_length(0, "a tuple"));
                }
                decoder.end_array()?;
                out.write(($first, $( $i, )*));
                Ok(())
            }
        }
    )*};
}

impl_tuple_de! {
    (a: A),
    (a: A, b: B),
    (a: A, b: B, c: C),
    (a: A, b: B, c: C, d: D),
    (a: A, b: B, c: C, d: D, e: E),
    (a: A, b: B, c: C, d: D, e: E, f: F),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K, l: L),
}

impl<'de, T: NsonDeserialize<'de>, const N: usize> NsonDeserialize<'de> for [T; N] {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = Vec::<T>::decode(decoder)?;
        let arr: [T; N] = v
            .try_into()
            .map_err(|_| Error::invalid_length(0, "an array of fixed length"))?;
        out.write(arr);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>, E: NsonDeserialize<'de>> NsonDeserialize<'de>
    for core::result::Result<T, E>
{
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        decoder.begin_object()?;
        let key = decoder
            .object_key()?
            .ok_or_else(|| Error::invalid_length(0, "a Result"))?;
        match key.as_ref() {
            "Ok" => {
                let v = T::decode(decoder)?;
                out.write(Ok(v));
            }
            "Err" => {
                let v = E::decode(decoder)?;
                out.write(Err(v));
            }
            other => return Err(Error::unknown_variant(other.to_string())),
        }
        if decoder.object_entry_sep()? {
            return Err(Error::custom("expected a single-key Result object"));
        }
        decoder.end_object()?;
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Duration {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let n = decoder.number()?;
        let nanos = n
            .as_u64()
            .ok_or_else(|| Error::invalid_type("u64", "a number"))?;
        out.write(Duration::from_nanos(nanos));
        Ok(())
    }
}

#[cfg(feature = "std")]
macro_rules! impl_parse_str_de {
    ($($t:ty => $what:literal),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
                let s = decoder.string()?;
                let v: $t = s.parse().map_err(|_| Error::invalid_type($what, "a string"))?;
                out.write(v);
                Ok(())
            }
        }
    )*};
}

#[cfg(feature = "std")]
impl_parse_str_de! {
    std::net::IpAddr => "an IP address",
    std::net::Ipv4Addr => "an IPv4 address",
    std::net::Ipv6Addr => "an IPv6 address",
    std::net::SocketAddr => "a socket address",
    std::net::SocketAddrV4 => "an IPv4 socket address",
    std::net::SocketAddrV6 => "an IPv6 socket address",
}

#[cfg(feature = "std")]
impl<'de> NsonDeserialize<'de> for std::path::PathBuf {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let s = decoder.string()?;
        out.write(std::path::PathBuf::from(s.into_owned()));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Range<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let (start, end) = <(T, T) as NsonDeserialize>::decode(decoder)?;
        out.write(start..end);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeInclusive<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let (start, end) = <(T, T) as NsonDeserialize>::decode(decoder)?;
        out.write(start..=end);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeFrom<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let start = T::decode(decoder)?;
        out.write(start..);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeTo<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let end = T::decode(decoder)?;
        out.write(..end);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeToInclusive<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let end = T::decode(decoder)?;
        out.write(..=end);
        Ok(())
    }
}

impl<'de, T: ?Sized> NsonDeserialize<'de> for PhantomData<T> {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        decoder.unit()?;
        out.write(PhantomData);
        Ok(())
    }
}

macro_rules! impl_atomic_de {
    ($($t:ty => $inner:ident),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
                let v = <$inner as NsonDeserialize>::decode(decoder)?;
                out.write(<$t>::new(v));
                Ok(())
            }
        }
    )*};
}
impl_atomic_de! {
    core::sync::atomic::AtomicBool => bool,
    core::sync::atomic::AtomicI8 => i8,
    core::sync::atomic::AtomicI16 => i16,
    core::sync::atomic::AtomicI32 => i32,
    core::sync::atomic::AtomicI64 => i64,
    core::sync::atomic::AtomicIsize => isize,
    core::sync::atomic::AtomicU8 => u8,
    core::sync::atomic::AtomicU16 => u16,
    core::sync::atomic::AtomicU32 => u32,
    core::sync::atomic::AtomicU64 => u64,
    core::sync::atomic::AtomicUsize => usize,
}

// ---------------------------------------------------------------------------
// Number / Map / Value
// ---------------------------------------------------------------------------

impl<'de> NsonDeserialize<'de> for Number {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let n = decoder.number()?;
        out.write(n);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Map {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let mut map = Map::new();
        decoder.begin_object()?;
        while let Some(key) = decoder.object_key()? {
            let v = Value::decode(decoder)?;
            map.insert(key.into_owned(), v);
            if !decoder.object_entry_sep()? {
                break;
            }
        }
        decoder.end_object()?;
        out.write(map);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Value {
    fn decode_into(decoder: &mut Decoder<'de>, out: &mut MaybeUninit<Self>) -> Result<()> {
        let v = match decoder.peek_token()? {
            Token::Null => {
                decoder.next_token()?;
                Value::Null
            }
            Token::Bool(_) => Value::Bool(decoder.bool()?),
            Token::Number(_) => Value::Number(decoder.number()?),
            Token::Str(_) => Value::String(decoder.string()?.into_owned()),
            Token::BeginObject => {
                let mut map = Map::new();
                decoder.begin_object()?;
                while let Some(key) = decoder.object_key()? {
                    let v = Value::decode(decoder)?;
                    map.insert(key.into_owned(), v);
                    if !decoder.object_entry_sep()? {
                        break;
                    }
                }
                decoder.end_object()?;
                Value::Object(map)
            }
            Token::BeginArray => {
                let mut arr = Vec::new();
                decoder.begin_array()?;
                while decoder.array_has_more()? {
                    arr.push(Value::decode(decoder)?);
                    if !decoder.array_entry_sep()? {
                        break;
                    }
                }
                decoder.end_array()?;
                Value::Array(arr)
            }
            _ => return Err(decoder.err(ErrorKind::Eof)),
        };
        out.write(v);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Content replay helpers
// ---------------------------------------------------------------------------

/// Read one full value from the current position as a token sequence.
///
/// Uses container primitives so separators are handled correctly; depth is
/// bounded by `max_depth`.
pub(crate) fn read_token_tree<'de>(decoder: &mut Decoder<'de>) -> Result<Vec<Token<'de>>> {
    match decoder.peek_token()? {
        Token::BeginObject => {
            decoder.begin_object()?;
            let mut out = vec![Token::BeginObject];
            while let Some(key) = decoder.object_key()? {
                out.push(Token::Str(key));
                out.extend(read_token_tree(decoder)?);
                if !decoder.object_entry_sep()? {
                    break;
                }
            }
            decoder.end_object()?;
            out.push(Token::EndObject);
            Ok(out)
        }
        Token::BeginArray => {
            decoder.begin_array()?;
            let mut out = vec![Token::BeginArray];
            while decoder.array_has_more()? {
                out.extend(read_token_tree(decoder)?);
                if !decoder.array_entry_sep()? {
                    break;
                }
            }
            decoder.end_array()?;
            out.push(Token::EndArray);
            Ok(out)
        }
        _ => Ok(vec![decoder.next_token()?]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn parse_basic() {
        let v = Value::decode(&mut Decoder::new(br#"{"a":[1,2,{"b":null}]}"#)).unwrap();
        assert_eq!(v["a"][1], Value::Number(Number::U64(2)));
        assert_eq!(v["a"][2]["b"], Value::Null);
    }

    #[test]
    fn borrowed_string() {
        let input = br#""hello world""#;
        let s: &str = NsonDeserialize::decode(&mut Decoder::new(input)).unwrap();
        assert_eq!(s, "hello world");
    }

    #[test]
    fn escaped_string_owns() {
        let input = br#""a\nb\u0041""#;
        let s: String = NsonDeserialize::decode(&mut Decoder::new(input)).unwrap();
        assert_eq!(s, "a\nbA");
    }

    #[test]
    fn surrogate_pairs() {
        let input = br#""\ud83d\udca9""#;
        let s: String = NsonDeserialize::decode(&mut Decoder::new(input)).unwrap();
        assert_eq!(s, "\u{1f4a9}");
        assert!(String::decode(&mut Decoder::new(br#""\udca9""#)).is_err());
        assert!(String::decode(&mut Decoder::new(br#""\ud83d""#)).is_err());
    }

    #[test]
    fn numbers_edge() {
        assert_eq!(i64::decode(&mut Decoder::new(b"42")).unwrap(), 42);
        assert_eq!(i64::decode(&mut Decoder::new(b"4.2e1")).unwrap(), 42);
        assert!(i64::decode(&mut Decoder::new(b"4.5")).is_err());
        assert_eq!(u64::decode(&mut Decoder::new(b"-0")).unwrap(), 0);
        assert!(u64::decode(&mut Decoder::new(b"-1")).is_err());
        assert!(f64::decode(&mut Decoder::new(b"1e400")).is_err());
        assert!(Number::decode(&mut Decoder::new(b"1e400")).is_err());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Value::decode(&mut Decoder::new(b"01")).is_err());
        assert!(Value::decode(&mut Decoder::new(b"truex")).is_err());
        assert!(Value::decode(&mut Decoder::new(b"\"a\x01b\"")).is_err());
        assert!(Value::decode(&mut Decoder::new(br#""\x""#)).is_err());
        assert!(Value::decode(&mut Decoder::new(b"1.")).is_err());
    }

    #[test]
    fn depth_limit() {
        let deep = format!("{}0{}", "[".repeat(200), "]".repeat(200));
        let mut d = Decoder::new(deep.as_bytes());
        assert!(Value::decode(&mut d).is_err());
        let mut d = Decoder::with_config(deep.as_bytes(), DecodeConfig::default().max_depth(1000));
        assert!(Value::decode(&mut d).is_ok());
    }

    #[test]
    fn skip_value_works() {
        let mut d = Decoder::new(br#"{"a": [1, {"b": 2}], "c": "x"}"#);
        d.begin_object().unwrap();
        assert_eq!(d.object_key().unwrap().unwrap().as_ref(), "a");
        d.skip_value().unwrap();
        assert!(d.object_entry_sep().unwrap());
        assert_eq!(d.object_key().unwrap().unwrap().as_ref(), "c");
        d.skip_value().unwrap();
        assert!(!d.object_entry_sep().unwrap());
        d.end_object().unwrap();
    }

    #[test]
    fn map_keys() {
        let mut d = Decoder::new(br#"{"1":"a","2":"b"}"#);
        let m: std::collections::HashMap<i32, String> = NsonDeserialize::decode(&mut d).unwrap();
        assert_eq!(m.get(&1).unwrap(), "a");
        assert_eq!(m.get(&2).unwrap(), "b");
    }

    #[test]
    fn error_position() {
        let err = Value::decode(&mut Decoder::new(b"{\n  \"a\": tru\n}")).unwrap_err();
        assert_eq!(err.line(), Some(2));
        assert_eq!(err.column(), Some(8));
    }
}

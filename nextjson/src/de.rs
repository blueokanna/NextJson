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
//! Both sources expose the exact same nextdecode primitives, so macro-generated
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
use core::ops::{Range, RangeFrom, RangeInclusive, RangeTo, RangeToInclusive};
use core::time::Duration;

use crate::error::{Error, ErrorKind, FormatError, Result};
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
    pub(crate) pos: usize,
    pub(crate) depth: u32,
    pub(crate) frame_len: usize,
}

impl Mark {
    /// Create a save point from a source offset and a nesting depth.
    pub fn new(pos: usize, depth: u32) -> Self {
        Mark {
            pos,
            depth,
            frame_len: 0,
        }
    }
    /// The source offset captured by this mark.
    pub fn pos(&self) -> usize {
        self.pos
    }
    /// The nesting depth captured by this mark.
    pub fn depth(&self) -> u32 {
        self.depth
    }
}

/// Human-readable name of a token for type-mismatch diagnostics.
pub(crate) fn token_name(token: &Token<'_>) -> &'static str {
    match token {
        Token::Null => "null",
        Token::Bool(_) => "bool",
        Token::Number(_) => "number",
        Token::Str(_) => "string",
        Token::BeginObject => "object",
        Token::EndObject => "end of object",
        Token::BeginArray => "array",
        Token::EndArray => "end of array",
    }
}

/// Result of [`FormatDecoder::option_tag`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionTag {
    /// `Option::None` (already consumed by `option_tag`).
    None,
    /// `Option::Some` — the next token is the payload.
    Some,
}

/// Default width-specific signed reads (built on [`FormatDecoder::number`]).
macro_rules! impl_signed_reads {
    ($($read:ident => $t:ty),* $(,)?) => {$(
        /// Read a `$t` (default: read a number and convert).
        ///
        /// Binary codecs that preserve source width on the wire override this.
        fn $read(&mut self) -> Result<$t, Self::Error> {
            let n = self.number()?;
            let exact = n
                .as_i128()
                .ok_or_else(|| Self::Error::custom("expected an integer"))?;
            <$t>::try_from(exact).map_err(|_| Self::Error::custom("integer out of range"))
        }
    )*};
}

/// Default width-specific unsigned reads (built on [`FormatDecoder::number`]).
macro_rules! impl_unsigned_reads {
    ($($read:ident => $t:ty),* $(,)?) => {$(
        /// Read a `$t` (default: read a number and convert).
        ///
        /// Binary codecs that preserve source width on the wire override this.
        fn $read(&mut self) -> Result<$t, Self::Error> {
            let n = self.number()?;
            let exact = n
                .as_u128()
                .ok_or_else(|| Self::Error::custom("expected an unsigned integer"))?;
            <$t>::try_from(exact).map_err(|_| Self::Error::custom("integer out of range"))
        }
    )*};
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

/// Safe caller-provided storage used by [`NsonDeserialize::nextdecode_into`].
///
/// The value starts uninitialized, but unlike a public `MaybeUninit<T>`
/// contract it cannot be read before a successful [`write`](DecodeSlot::write).
/// Incorrect third-party implementations therefore cannot cause undefined
/// behavior in safe code.
pub struct DecodeSlot<T> {
    value: Option<T>,
}

impl<T> DecodeSlot<T> {
    /// Create an empty nextdecode slot.
    pub const fn new() -> Self {
        DecodeSlot { value: None }
    }

    /// Replace the slot contents with a fully initialized value.
    pub fn write(&mut self, value: T) {
        self.value = Some(value);
    }

    /// Remove and return the initialized value, if present.
    pub fn take(&mut self) -> Option<T> {
        self.value.take()
    }

    /// Return whether the slot currently contains a value.
    pub fn is_initialized(&self) -> bool {
        self.value.is_some()
    }
}

impl<T> Default for DecodeSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Deserialization trait.
///
/// [`nextdecode_into`](NsonDeserialize::nextdecode_into) decodes a value
/// directly into caller-provided storage, supporting memory
/// reuse without requiring `T: Default` or constructing a placeholder `T`.
/// Format-neutral input contract implemented by every source codec.
///
/// `NsonDeserialize::nextdecode_into` is generic over this trait, so one type
/// implementation can consume every codec whose data model represents that
/// type. A codec may stream from its input or replay a validated value tree.
/// The method surface mirrors the unified token stream: containers, scalars,
/// backtracking, and [`skip_value`](FormatDecoder::skip_value).
pub trait FormatDecoder<'de> {
    /// The error type produced by this format's methods.
    ///
    /// External codecs may use their own error type; it only needs to wrap
    /// [`crate::error::Error`] so generic deserialization code can propagate
    /// format failures. The built-in formats all use [`crate::error::Error`].
    type Error: FormatError;

    /// Consume `{` (with depth check).
    fn begin_object(&mut self) -> Result<(), Self::Error>;
    /// Consume `}`.
    fn end_object(&mut self) -> Result<(), Self::Error>;
    /// Read the next object key; returns `None` on `}` (not consumed).
    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error>;
    /// Object entry separator: `true` if more entries follow, `false` at end.
    fn object_entry_sep(&mut self) -> Result<bool, Self::Error>;
    /// Consume `[` (with depth check).
    fn begin_array(&mut self) -> Result<(), Self::Error>;
    /// Consume `]`.
    fn end_array(&mut self) -> Result<(), Self::Error>;
    /// Whether the array has more elements (`]` not consumed).
    fn array_has_more(&mut self) -> Result<bool, Self::Error>;
    /// Array entry separator: `true` if more elements follow, `false` at end.
    fn array_entry_sep(&mut self) -> Result<bool, Self::Error>;
    /// Consume `null`.
    fn unit(&mut self) -> Result<(), Self::Error>;
    /// Read a boolean.
    fn bool(&mut self) -> Result<bool, Self::Error>;
    /// Read a number.
    fn number(&mut self) -> Result<Number, Self::Error>;
    /// Read a string (may borrow the source).
    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error>;
    /// Read a single character (a one-scalar string).
    fn char(&mut self) -> Result<char, Self::Error>;
    /// Skip any one value.
    fn skip_value(&mut self) -> Result<(), Self::Error>;
    /// Peek the next token without consuming it (container-flatten support).
    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error>;
    /// Consume and return the next token.
    fn next_token(&mut self) -> Result<Token<'de>, Self::Error>;
    /// Save the current position (for untagged-enum backtracking).
    fn save(&self) -> Mark;
    /// Restore a position saved with [`save`](FormatDecoder::save).
    fn restore(&mut self, mark: Mark);

    impl_signed_reads! {
        i8 => i8, i16 => i16, i32 => i32, i64 => i64, i128 => i128, isize => isize,
    }
    impl_unsigned_reads! {
        u8 => u8, u16 => u16, u32 => u32, u64 => u64, u128 => u128, usize => usize,
    }

    /// Read a byte sequence (may borrow the source).
    ///
    /// The default accepts either a string (borrowed when unescaped) or an
    /// array of `u8` values, matching both JSON spellings. Binary codecs
    /// override this to read their native byte-string wire type.
    fn bytes(&mut self) -> Result<Cow<'de, [u8]>, Self::Error> {
        match self.peek_token()? {
            Token::Str(_) => match self.string()? {
                Cow::Borrowed(s) => Ok(Cow::Borrowed(s.as_bytes())),
                Cow::Owned(s) => Ok(Cow::Owned(s.into_bytes())),
            },
            Token::BeginArray => {
                self.begin_array()?;
                let mut out = Vec::new();
                while self.array_has_more()? {
                    out.push(self.u8()?);
                    if !self.array_entry_sep()? {
                        break;
                    }
                }
                self.end_array()?;
                Ok(Cow::Owned(out))
            }
            _ => Err(Self::Error::custom(
                "expected a byte string or an array of bytes",
            )),
        }
    }

    /// Report whether the next value is `Option::None` or `Option::Some`.
    ///
    /// The default maps `null` to [`OptionTag::None`] (consuming it) and any
    /// other token to [`OptionTag::Some`], which is exactly the JSON shape.
    /// Binary codecs override this to read a distinguishing tag so `None`
    /// stays distinct from a `Some` payload.
    fn option_tag(&mut self) -> Result<OptionTag, Self::Error> {
        match self.peek_token()? {
            Token::Null => {
                self.next_token()?;
                Ok(OptionTag::None)
            }
            _ => Ok(OptionTag::Some),
        }
    }

    /// Read the next map key.
    ///
    /// Returns `None` at object end. The default reads a string key and
    /// parses it into `K` (trying the raw content first, then as a quoted
    /// string), which is the JSON shape. Binary codecs override this to read
    /// the key as a plain value, supporting non-string keys such as
    /// `BTreeMap<u8, V>` without string round-tripping.
    fn map_key<K: for<'a> NsonDeserialize<'a>>(&mut self) -> Result<Option<K>, Self::Error> {
        match self.object_key()? {
            None => Ok(None),
            Some(key) => {
                // `?` converts `nextjson::Error` into `Self::Error` via the
                // `From` bound on `FormatError`.
                let k = nextdecode_map_key::<K>(key.as_ref())?;
                Ok(Some(k))
            }
        }
    }

    /// Whether this format produces human-readable output.
    ///
    /// Text formats return `true`; binary codecs return `false`. Types that
    /// decode differently for humans branch on this, mirroring serde's
    /// `Deserializer::is_human_readable`.
    fn is_human_readable(&self) -> bool {
        true
    }
}

impl<'de> FormatDecoder<'de> for Decoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        Decoder::begin_object(self)
    }
    fn end_object(&mut self) -> Result<(), Self::Error> {
        Decoder::end_object(self)
    }
    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        Decoder::object_key(self)
    }
    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        Decoder::object_entry_sep(self)
    }
    fn begin_array(&mut self) -> Result<(), Self::Error> {
        Decoder::begin_array(self)
    }
    fn end_array(&mut self) -> Result<(), Self::Error> {
        Decoder::end_array(self)
    }
    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        Decoder::array_has_more(self)
    }
    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        Decoder::array_entry_sep(self)
    }
    fn unit(&mut self) -> Result<(), Self::Error> {
        Decoder::unit(self)
    }
    fn bool(&mut self) -> Result<bool, Self::Error> {
        Decoder::bool(self)
    }
    fn number(&mut self) -> Result<Number, Self::Error> {
        Decoder::number(self)
    }
    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error> {
        Decoder::string(self)
    }
    fn char(&mut self) -> Result<char, Self::Error> {
        Decoder::char(self)
    }
    fn skip_value(&mut self) -> Result<(), Self::Error> {
        Decoder::skip_value(self)
    }
    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error> {
        Decoder::peek_token(self)
    }
    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        Decoder::next_token(self)
    }
    fn save(&self) -> Mark {
        Decoder::save(self)
    }
    fn restore(&mut self, mark: Mark) {
        Decoder::restore(self, mark)
    }
}

/// Deserialization trait: decode `Self` from any [`FormatDecoder`].
pub trait NsonDeserialize<'de>: Sized {
    /// Decode into `out`.
    ///
    /// Implementations must call [`DecodeSlot::write`] before returning `Ok`.
    /// [`nextdecode`](NsonDeserialize::nextdecode) validates this invariant.
    /// Errors are produced in the input format's own error type
    /// ([`FormatDecoder::Error`]).
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error>;

    /// Decode and return a value.
    fn nextdecode<D: FormatDecoder<'de>>(decoder: &mut D) -> Result<Self, D::Error> {
        let mut out = DecodeSlot::new();
        Self::nextdecode_into(decoder, &mut out)?;
        out.take().ok_or_else(|| {
            FormatError::custom(
                "NsonDeserialize::nextdecode_into returned success without writing a value",
            )
        })
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
        let tail = &input[start..];
        let mut relative = 0;
        while relative < tail.len() {
            match tail[relative] {
                b'"' | b'\\' => break,
                0x00..=0x1f => {
                    return Err(self.err_at(ErrorKind::ControlCharInString, start + relative));
                }
                _ => relative += 1,
            }
        }
        if relative == tail.len() {
            return Err(self.err_at(ErrorKind::Eof, input.len()));
        }
        let end = start + relative;
        if input[end] == b'\\' {
            let string = self.unescape(input, start, end, scratch)?;
            return Ok(Token::Str(Cow::Owned(string)));
        }
        let raw = &input[start..end];
        let string = core::str::from_utf8(raw)
            .map_err(|error| self.err_at(ErrorKind::InvalidUtf8, start + error.valid_up_to()))?;
        self.pos = end + 1;
        Ok(Token::Str(Cow::Borrowed(string)))
    }

    /// Handle a string containing escapes; `start` is after the opening quote,
    /// `i` points at the first `\`.
    fn unescape(
        &mut self,
        input: &[u8],
        start: usize,
        mut i: usize,
        scratch: &mut String,
    ) -> Result<String> {
        scratch.clear();
        let seg = core::str::from_utf8(&input[start..i])
            .map_err(|_| self.err_at(ErrorKind::InvalidUtf8, i))?;
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
                                    let cp = 0x10000
                                        + ((hi as u32 - 0xD800) << 10)
                                        + (lo as u32 - 0xDC00);
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
        let n = Number::parse(raw, is_float)
            .map_err(|_| self.err_at(ErrorKind::InvalidNumber, start))?;
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
            _ => Err(self.err_at(ErrorKind::Custom("invalid object key token".into()), pos)),
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
                let next = self.skip_ws();
                if input.get(next) == Some(&b'}') {
                    return Err(self.err_at(
                        ErrorKind::Expected {
                            what: "an object key after ','",
                            found: Some(b'}'),
                        },
                        next,
                    ));
                }
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
                let next = self.skip_ws();
                if input.get(next) == Some(&b']') {
                    return Err(self.err_at(
                        ErrorKind::Expected {
                            what: "an array element after ','",
                            found: Some(b']'),
                        },
                        next,
                    ));
                }
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
        Mark {
            pos,
            depth: self.depth,
            frame_len: 0,
        }
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
    /// this automatically; direct [`Decoder`] users can call it after `nextdecode`.
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
    ($($t:ty => $read:ident),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn nextdecode_into<D: FormatDecoder<'de>>(decoder: &mut D, out: &mut DecodeSlot<Self>) -> Result<(), D::Error> {
                let v = decoder.$read()?;
                out.write(v);
                Ok(())
            }
        }
    )*};
}
impl_signed_de! {
    i8 => i8, i16 => i16, i32 => i32, i64 => i64, i128 => i128, isize => isize,
}

macro_rules! impl_unsigned_de {
    ($($t:ty => $read:ident),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn nextdecode_into<D: FormatDecoder<'de>>(decoder: &mut D, out: &mut DecodeSlot<Self>) -> Result<(), D::Error> {
                let v = decoder.$read()?;
                out.write(v);
                Ok(())
            }
        }
    )*};
}
impl_unsigned_de! {
    u8 => u8, u16 => u16, u32 => u32, u64 => u64, u128 => u128, usize => usize,
}

impl<'de> NsonDeserialize<'de> for f64 {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let n = decoder.number()?;
        out.write(n.as_f64());
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for f32 {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let n = decoder.number()?;
        let value = n.as_f64() as f32;
        if !value.is_finite() {
            return Err(Error::new(ErrorKind::NumberOutOfRange, None, None, 0).into());
        }
        out.write(value);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for bool {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let b = decoder.bool()?;
        out.write(b);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for char {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let c = decoder.char()?;
        out.write(c);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for String {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let s = decoder.string()?;
        out.write(s.into_owned());
        Ok(())
    }
}

impl<'de, 'a> NsonDeserialize<'de> for &'a str
where
    'de: 'a,
{
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        match decoder.string()? {
            Cow::Borrowed(b) => {
                out.write(b);
                Ok(())
            }
            Cow::Owned(_) => Err(Error::invalid_type(
                "a borrowed string (no escape sequences)",
                "a string",
            )
            .into()),
        }
    }
}

impl<'de> NsonDeserialize<'de> for Cow<'de, str> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let s = decoder.string()?;
        out.write(s);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for &'de [u8] {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        match decoder.bytes()? {
            Cow::Borrowed(b) => {
                out.write(b);
                Ok(())
            }
            Cow::Owned(_) => Err(Error::invalid_type(
                "a borrowed byte string (no escape sequences)",
                "bytes",
            )
            .into()),
        }
    }
}

impl<'de> NsonDeserialize<'de> for Cow<'de, [u8]> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let b = decoder.bytes()?;
        out.write(b);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for () {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        decoder.unit()?;
        out.write(());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Option<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        match decoder.option_tag()? {
            OptionTag::None => out.write(None),
            OptionTag::Some => {
                let v = T::nextdecode(decoder)?;
                out.write(Some(v));
            }
        }
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Vec<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let mut v = Vec::new();
        decoder.begin_array()?;
        while decoder.array_has_more()? {
            v.push(T::nextdecode(decoder)?);
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
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = T::nextdecode(decoder)?;
        out.write(Box::new(v));
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Box<str> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let s = decoder.string()?;
        out.write(s.into_owned().into_boxed_str());
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Box<[u8]> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let b = decoder.bytes()?;
        out.write(b.into_owned().into_boxed_slice());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Rc<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = T::nextdecode(decoder)?;
        out.write(Rc::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Arc<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = T::nextdecode(decoder)?;
        out.write(Arc::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de> + Copy> NsonDeserialize<'de> for Cell<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = T::nextdecode(decoder)?;
        out.write(Cell::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RefCell<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = T::nextdecode(decoder)?;
        out.write(RefCell::new(v));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for VecDeque<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = Vec::<T>::nextdecode(decoder)?;
        out.write(v.into());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for LinkedList<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = Vec::<T>::nextdecode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de> + Ord> NsonDeserialize<'de> for BinaryHeap<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = Vec::<T>::nextdecode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de> + Ord> NsonDeserialize<'de> for BTreeSet<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = Vec::<T>::nextdecode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

impl<'de, K: for<'a> NsonDeserialize<'a> + Ord, V: NsonDeserialize<'de>> NsonDeserialize<'de>
    for BTreeMap<K, V>
{
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let mut map = BTreeMap::new();
        decoder.begin_object()?;
        while let Some(k) = decoder.map_key::<K>()? {
            let v = V::nextdecode(decoder)?;
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
pub(crate) fn nextdecode_map_key<K: for<'a> NsonDeserialize<'a>>(key: &str) -> Result<K> {
    let raw = key.as_bytes();
    let mut d0 = Decoder::new(raw);
    if let Ok(v) = K::nextdecode(&mut d0) {
        if d0.end().is_ok() {
            return Ok(v);
        }
    }
    let mut d1 = Decoder::from_tokens(vec![Token::Str(Cow::Borrowed(key))]);
    let value = K::nextdecode(&mut d1)?;
    d1.end()?;
    Ok(value)
}

#[cfg(feature = "std")]
impl<'de, K: for<'a> NsonDeserialize<'a> + Eq + core::hash::Hash, V: NsonDeserialize<'de>>
    NsonDeserialize<'de> for std::collections::HashMap<K, V>
{
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let mut map = std::collections::HashMap::new();
        decoder.begin_object()?;
        while let Some(k) = decoder.map_key::<K>()? {
            let v = V::nextdecode(decoder)?;
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
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = Vec::<T>::nextdecode(decoder)?;
        out.write(v.into_iter().collect());
        Ok(())
    }
}

macro_rules! impl_tuple_de {
    ($(($first:ident : $First:ident $(, $i:ident : $T:ident)*)),* $(,)?) => {$(
        impl<'de, $First: NsonDeserialize<'de> $(, $T: NsonDeserialize<'de>)*> NsonDeserialize<'de> for ($First, $( $T, )*) {
            #[allow(non_snake_case)]
            fn nextdecode_into<__D: FormatDecoder<'de>>(decoder: &mut __D, out: &mut DecodeSlot<Self>) -> Result<(), __D::Error> {
                decoder.begin_array()?;
                let $first = $First::nextdecode(decoder)?;
                $(
                    if !decoder.array_entry_sep()? {
                        return Err(Error::invalid_length(0, "a tuple").into());
                    }
                    let $i = $T::nextdecode(decoder)?;
                )*
                if decoder.array_entry_sep()? {
                    return Err(Error::invalid_length(0, "a tuple").into());
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
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let v = Vec::<T>::nextdecode(decoder)?;
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
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        decoder.begin_object()?;
        let key = decoder
            .object_key()?
            .ok_or_else(|| Error::invalid_length(0, "a Result"))?;
        match key.as_ref() {
            "Ok" => {
                let v = T::nextdecode(decoder)?;
                out.write(Ok(v));
            }
            "Err" => {
                let v = E::nextdecode(decoder)?;
                out.write(Err(v));
            }
            other => return Err(Error::unknown_variant(other.to_string()).into()),
        }
        if decoder.object_entry_sep()? {
            return Err(Error::custom("expected a single-key Result object").into());
        }
        decoder.end_object()?;
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Duration {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let n = decoder.number()?;
        let nanos = n
            .as_u128()
            .ok_or_else(|| Error::invalid_type("u128", "a number"))?;
        let seconds = u64::try_from(nanos / 1_000_000_000)
            .map_err(|_| Error::new(ErrorKind::NumberOutOfRange, None, None, 0))?;
        let subsec_nanos = (nanos % 1_000_000_000) as u32;
        out.write(Duration::new(seconds, subsec_nanos));
        Ok(())
    }
}

#[cfg(feature = "std")]
macro_rules! impl_parse_str_de {
    ($($t:ty => $what:literal),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn nextdecode_into<D: FormatDecoder<'de>>(decoder: &mut D, out: &mut DecodeSlot<Self>) -> Result<(), D::Error> {
                let s = decoder.string()?;
                let v: $t = s.parse().map_err(|_| Error::invalid_type($what, "a string").into())?;
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
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let s = decoder.string()?;
        out.write(std::path::PathBuf::from(s.into_owned()));
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for Range<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let (start, end) = <(T, T) as NsonDeserialize>::nextdecode(decoder)?;
        out.write(start..end);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeInclusive<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let (start, end) = <(T, T) as NsonDeserialize>::nextdecode(decoder)?;
        out.write(start..=end);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeFrom<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let start = T::nextdecode(decoder)?;
        out.write(start..);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeTo<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let end = T::nextdecode(decoder)?;
        out.write(..end);
        Ok(())
    }
}

impl<'de, T: NsonDeserialize<'de>> NsonDeserialize<'de> for RangeToInclusive<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let end = T::nextdecode(decoder)?;
        out.write(..=end);
        Ok(())
    }
}

impl<'de, T: ?Sized> NsonDeserialize<'de> for PhantomData<T> {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        decoder.unit()?;
        out.write(PhantomData);
        Ok(())
    }
}

macro_rules! impl_atomic_de {
    ($($t:ty => $inner:ident),* $(,)?) => {$(
        impl<'de> NsonDeserialize<'de> for $t {
            fn nextdecode_into<D: FormatDecoder<'de>>(decoder: &mut D, out: &mut DecodeSlot<Self>) -> Result<(), D::Error> {
                let v = <$inner as NsonDeserialize>::nextdecode(decoder)?;
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
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let n = decoder.number()?;
        out.write(n);
        Ok(())
    }
}

impl<'de> NsonDeserialize<'de> for Map {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let mut map = Map::new();
        decoder.begin_object()?;
        while let Some(key) = decoder.object_key()? {
            let v = Value::nextdecode(decoder)?;
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
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
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
                    let v = Value::nextdecode(decoder)?;
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
                    arr.push(Value::nextdecode(decoder)?);
                    if !decoder.array_entry_sep()? {
                        break;
                    }
                }
                decoder.end_array()?;
                Value::Array(arr)
            }
            _ => return Err(Error::custom("unexpected token while decoding a value").into()),
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
/// bounded by `max_depth`. Works over any [`FormatDecoder`].
pub(crate) fn read_token_tree<'de, D: FormatDecoder<'de>>(
    decoder: &mut D,
) -> Result<Vec<Token<'de>>, D::Error> {
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
        let v = Value::nextdecode(&mut Decoder::new(br#"{"a":[1,2,{"b":null}]}"#)).unwrap();
        assert_eq!(v["a"][1], Value::Number(Number::U64(2)));
        assert_eq!(v["a"][2]["b"], Value::Null);
    }

    #[test]
    fn borrowed_string() {
        let input = br#""hello world""#;
        let s: &str = NsonDeserialize::nextdecode(&mut Decoder::new(input)).unwrap();
        assert_eq!(s, "hello world");
    }

    #[test]
    fn escaped_string_owns() {
        let input = br#""a\nb\u0041""#;
        let s: String = NsonDeserialize::nextdecode(&mut Decoder::new(input)).unwrap();
        assert_eq!(s, "a\nbA");
    }

    #[test]
    fn surrogate_pairs() {
        let input = br#""\ud83d\udca9""#;
        let s: String = NsonDeserialize::nextdecode(&mut Decoder::new(input)).unwrap();
        assert_eq!(s, "\u{1f4a9}");
        assert!(String::nextdecode(&mut Decoder::new(br#""\udca9""#)).is_err());
        assert!(String::nextdecode(&mut Decoder::new(br#""\ud83d""#)).is_err());
    }

    #[test]
    fn numbers_edge() {
        assert_eq!(i64::nextdecode(&mut Decoder::new(b"42")).unwrap(), 42);
        assert_eq!(i64::nextdecode(&mut Decoder::new(b"4.2e1")).unwrap(), 42);
        assert!(i64::nextdecode(&mut Decoder::new(b"4.5")).is_err());
        assert_eq!(u64::nextdecode(&mut Decoder::new(b"-0")).unwrap(), 0);
        assert!(u64::nextdecode(&mut Decoder::new(b"-1")).is_err());
        assert!(f64::nextdecode(&mut Decoder::new(b"1e400")).is_err());
        assert!(Number::nextdecode(&mut Decoder::new(b"1e400")).is_err());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Value::nextdecode(&mut Decoder::new(b"01")).is_err());
        assert!(Value::nextdecode(&mut Decoder::new(b"truex")).is_err());
        assert!(Value::nextdecode(&mut Decoder::new(b"\"a\x01b\"")).is_err());
        assert!(Value::nextdecode(&mut Decoder::new(br#""\x""#)).is_err());
        assert!(Value::nextdecode(&mut Decoder::new(b"1.")).is_err());
    }

    #[test]
    fn depth_limit() {
        let deep = format!("{}0{}", "[".repeat(200), "]".repeat(200));
        let mut d = Decoder::new(deep.as_bytes());
        assert!(Value::nextdecode(&mut d).is_err());
        let mut d = Decoder::with_config(deep.as_bytes(), DecodeConfig::default().max_depth(1000));
        assert!(Value::nextdecode(&mut d).is_ok());
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
        let m: alloc::collections::BTreeMap<i32, String> =
            NsonDeserialize::nextdecode(&mut d).unwrap();
        assert_eq!(m.get(&1).unwrap(), "a");
        assert_eq!(m.get(&2).unwrap(), "b");
    }

    #[test]
    fn error_position() {
        let err = Value::nextdecode(&mut Decoder::new(b"{\n  \"a\": tru\n}")).unwrap_err();
        assert_eq!(err.line(), Some(2));
        assert_eq!(err.column(), Some(8));
    }
}

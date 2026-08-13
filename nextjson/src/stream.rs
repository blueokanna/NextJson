//! Streaming JSON decoding from a `std::io::Read` source.
//!
//! [`StreamDecoder`] pulls bytes from the reader on demand instead of
//! buffering the complete input up front, which is what the top-level
//! [`crate::from_reader`] helper needs for network sockets, pipes and other
//! incremental sources. It implements the same format-neutral
//! [`FormatDecoder`] contract as [`crate::Decoder`], so every derived
//! `NsonDeserialize` works against it unchanged.
//!
//! Two trade-offs follow from streaming:
//!
//! - **Owned strings** - a streamed input cannot be borrowed for the lifetime
//!   of the decoded value, so `string()` / `bytes()` always return
//!   `Cow::Owned`. Types that require borrowing (`&str`, `&[u8]`,
//!   `nextjson::Bytes`) cannot be decoded from a stream.
//! - **Retained buffer** - to honour the [`save`]/[`restore`] backtracking
//!   contract used by untagged enums without an error channel in `restore`,
//!   the decoder keeps every byte it has read (including consumed prefixes).
//!   Memory therefore grows with the total input; the win is that decoding
//!   starts as soon as the first bytes arrive and does not wait for the
//!   whole payload. Applications that need constant-memory streaming for a
//!   single value should chunk at the protocol level instead.
//!
//! [`save`]: FormatDecoder::save
//! [`restore`]: FormatDecoder::restore

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use crate::de::{DecodeConfig, FormatDecoder, Mark, NsonDeserialize, OptionTag, Token};
use crate::error::{Error, ErrorKind, Result};
use crate::lex::{hex_digit, line_col, simple_escape};
use crate::number::Number;

/// How many bytes are pulled from the reader in one fill.
const CHUNK: usize = 4096;

/// A JSON decoder that incrementally pulls its input from `R: Read`.
///
/// See the [module documentation](crate::stream) for the streaming semantics.
pub struct StreamDecoder<R> {
    reader: R,
    /// All bytes read so far. Never shrinks: `restore` may jump back to any
    /// previously saved position, and there is no error channel in
    /// [`FormatDecoder::restore`].
    buf: Vec<u8>,
    /// Absolute read position into `buf`.
    pos: usize,
    lookahead: Option<Token<'static>>,
    scratch: String,
    depth: u32,
    max_depth: u32,
    /// Type description installed via
    /// [`set_expecting`](FormatDecoder::set_expecting), used to enrich
    /// container-level type-mismatch errors.
    expecting: Option<&'static str>,
}

impl<R> StreamDecoder<R> {
    /// Create a stream decoder over a reader (default config).
    pub fn new(reader: R) -> Self {
        StreamDecoder::with_config(reader, DecodeConfig::default())
    }

    /// Create a stream decoder over a reader (custom config).
    pub fn with_config(reader: R, config: DecodeConfig) -> Self {
        StreamDecoder {
            reader,
            buf: Vec::new(),
            pos: 0,
            lookahead: None,
            scratch: String::new(),
            depth: 0,
            max_depth: config.max_depth,
            expecting: None,
        }
    }

    /// The maximum nesting depth.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Consume the decoder and return the underlying reader.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Verify that the input contains no value or token after the decoded value.
    ///
    /// Trailing whitespace is accepted. Top-level helpers call this
    /// automatically; direct [`StreamDecoder`] users can call it after
    /// `nextdecode`.
    pub fn end(&mut self) -> Result<()>
    where
        R: std::io::Read,
    {
        self.end_impl()
    }

    fn err_at(&self, kind: ErrorKind, pos: usize) -> Error {
        let (line, col) = line_col(&self.buf, pos);
        Error::new(kind, Some(line), Some(col), pos)
    }

    fn err(&self, kind: ErrorKind) -> Error {
        self.err_at(kind, self.pos)
    }

    fn invalid_type(&self, expected: &'static str, found: &Token<'static>) -> Error {
        // When a type description was installed via `set_expecting`, replace
        // the bare structural token expectation (like `'{'`) with the type's
        // name so the message says what the user actually tried to decode.
        let expected = crate::lex::expecting_for(expected, self.expecting);
        self.err(ErrorKind::InvalidType {
            expected,
            found: crate::de::token_name(found),
        })
    }

    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(self.err(ErrorKind::RecursionLimitExceeded));
        }
        self.depth += 1;
        Ok(())
    }
}

impl<R: std::io::Read> StreamDecoder<R> {
    /// Read more bytes until `buf.len() >= upto` or the reader hits EOF.
    fn fill(&mut self, upto: usize) -> Result<()> {
        while self.buf.len() < upto {
            let mut chunk = [0u8; CHUNK];
            let n = self
                .reader
                .read(&mut chunk)
                .map_err(|e| Error::custom(alloc::format!("io error: {e}")))?;
            if n == 0 {
                break;
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
        Ok(())
    }

    /// The byte at `i`, erroring with [`ErrorKind::Eof`] if the input ends
    /// before `i` becomes available.
    fn byte(&mut self, i: usize) -> Result<u8, Error> {
        self.fill(i + 1)?;
        match self.buf.get(i) {
            Some(&b) => Ok(b),
            None => Err(self.err_at(ErrorKind::Eof, self.buf.len())),
        }
    }

    /// Whether there is at least one more byte beyond `i`.
    fn has_more(&mut self, i: usize) -> Result<bool> {
        self.fill(i + 1)?;
        Ok(self.buf.len() > i)
    }

    // -- token machinery ---------------------------------------------------

    fn skip_ws(&mut self) -> usize {
        loop {
            let Some(&b) = self.buf.get(self.pos) else {
                return self.pos;
            };
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => return self.pos,
            }
        }
    }

    fn lex_next(&mut self) -> Result<Token<'static>> {
        if let Some(t) = self.lookahead.take() {
            return Ok(t);
        }
        self.lex()
    }

    fn lex_peek(&mut self) -> Result<Token<'static>> {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.lex()?);
        }
        Ok(self.lookahead.as_ref().expect("just set").clone())
    }

    fn lex(&mut self) -> Result<Token<'static>> {
        let pos = self.skip_ws();
        self.pos = pos;
        let b = self.byte(pos)?;
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
            b'"' => self.lex_string(),
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

    fn lex_literal(
        &mut self,
        pos: usize,
        lit: &[u8],
        tok: Token<'static>,
    ) -> Result<Token<'static>> {
        self.fill(pos + lit.len() + 1)?;
        let window = &self.buf[pos..];
        if window.len() < lit.len() || &window[..lit.len()] != lit {
            return Err(self.err_at(
                ErrorKind::Expected {
                    what: "a JSON literal",
                    found: window.first().copied(),
                },
                pos,
            ));
        }
        let end = pos + lit.len();
        if let Some(&b) = self.buf.get(end) {
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

    /// Read a JSON string; always produces an owned value.
    fn lex_string(&mut self) -> Result<Token<'static>> {
        let start = self.pos + 1;
        let mut i = start;
        loop {
            // Scan a run of unescaped bytes. Each iteration pulls the byte
            // first, so a multi-byte UTF-8 character is never split across
            // fill boundaries.
            let seg_start = i;
            loop {
                if !self.has_more(i)? {
                    break; // data exhausted (EOF handled below)
                }
                let b = self.buf[i];
                if b == b'"' || b == b'\\' {
                    break;
                }
                if b < 0x20 {
                    return Err(self.err_at(ErrorKind::ControlCharInString, i));
                }
                i += 1;
            }
            if i > seg_start {
                let seg = core::str::from_utf8(&self.buf[seg_start..i]).map_err(|e| {
                    self.err_at(ErrorKind::InvalidUtf8, seg_start + e.valid_up_to())
                })?;
                self.scratch.push_str(seg);
            }
            match self.buf.get(i).copied() {
                None => {
                    // True end of input: no closing quote ever arrives.
                    if !self.has_more(i)? {
                        return Err(self.err_at(ErrorKind::Eof, self.buf.len()));
                    }
                    // More bytes arrived; continue scanning.
                }
                Some(b'"') => {
                    self.pos = i + 1;
                    let done = core::mem::take(&mut self.scratch);
                    return Ok(Token::Str(Cow::Owned(done)));
                }
                Some(b'\\') => {
                    self.unescape_into(i)?;
                    i = self.pos;
                }
                Some(_) => unreachable!("loop guarantees a quote, backslash, or control byte"),
            }
        }
    }

    /// Handle an escape sequence whose backslash sits at `i`. Advances
    /// `self.pos` past the whole escape and appends the decoded text.
    fn unescape_into(&mut self, i: usize) -> Result<()> {
        let mut j = i + 1;
        let esc = self.byte(j)?;
        match esc {
            b'u' => {
                let hi = self.hex4(j + 1)?;
                if (0xD800..=0xDBFF).contains(&hi) {
                    // high surrogate: must be followed by \uXXXX low.
                    let b1 = self.byte(j + 5)?;
                    let b2 = self.byte(j + 6)?;
                    if b1 != b'\\' || b2 != b'u' {
                        return Err(self.err_at(ErrorKind::InvalidSurrogate, j));
                    }
                    let lo = self.hex4(j + 7)?;
                    if !(0xDC00..=0xDFFF).contains(&lo) {
                        return Err(self.err_at(ErrorKind::InvalidSurrogate, j));
                    }
                    let cp = 0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                    let c = char::from_u32(cp).expect("valid scalar value");
                    self.scratch.push(c);
                    j += 10;
                } else if (0xDC00..=0xDFFF).contains(&hi) {
                    return Err(self.err_at(ErrorKind::InvalidSurrogate, j));
                } else {
                    let c = char::from_u32(hi as u32).expect("valid scalar value");
                    self.scratch.push(c);
                    j += 4;
                }
            }
            other => match simple_escape(other) {
                Some(c) => self.scratch.push(c),
                None => return Err(self.err_at(ErrorKind::InvalidEscape(other as char), j)),
            },
        }
        self.pos = j + 1;
        Ok(())
    }

    /// Read four hex digits starting at `start`.
    fn hex4(&mut self, start: usize) -> Result<u16, Error> {
        self.fill(start + 4)?;
        let mut v: u16 = 0;
        for k in 0..4 {
            let Some(&b) = self.buf.get(start + k) else {
                return Err(self.err_at(ErrorKind::Eof, self.buf.len()));
            };
            let d = hex_digit(b)
                .ok_or_else(|| self.err_at(ErrorKind::InvalidEscape('u'), start + k))?;
            v = v * 16 + d as u16;
        }
        Ok(v)
    }

    fn lex_number(&mut self) -> Result<Token<'static>> {
        let start = self.pos;
        let mut i = self.pos;
        // sign
        if self.byte(i)? == b'-' {
            i += 1;
            if !self.has_more(i)? {
                return Err(self.err_at(ErrorKind::InvalidNumber, i));
            }
        }
        // integer part
        match self.byte(i)? {
            b'0' => {
                i += 1;
                if self.has_more(i)? && self.buf[i].is_ascii_digit() {
                    return Err(self.err_at(ErrorKind::InvalidNumber, i));
                }
            }
            b'1'..=b'9' => {
                i += 1;
                while self.has_more(i)? && self.buf[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => return Err(self.err_at(ErrorKind::InvalidNumber, i)),
        }
        let mut is_float = false;
        // fraction
        if self.has_more(i)? && self.buf[i] == b'.' {
            is_float = true;
            i += 1;
            let dstart = i;
            while self.has_more(i)? && self.buf[i].is_ascii_digit() {
                i += 1;
            }
            if i == dstart {
                return Err(self.err_at(ErrorKind::InvalidNumber, i));
            }
        }
        // exponent
        if self.has_more(i)? && (self.buf[i] == b'e' || self.buf[i] == b'E') {
            is_float = true;
            i += 1;
            if self.has_more(i)? && (self.buf[i] == b'+' || self.buf[i] == b'-') {
                i += 1;
            }
            let dstart = i;
            while self.has_more(i)? && self.buf[i].is_ascii_digit() {
                i += 1;
            }
            if i == dstart {
                return Err(self.err_at(ErrorKind::InvalidNumber, i));
            }
        }
        let raw = &self.buf[start..i];
        self.pos = i;
        let n = Number::parse(raw, is_float)
            .map_err(|_| self.err_at(ErrorKind::InvalidNumber, start))?;
        Ok(Token::Number(n))
    }

    fn key_impl(&mut self) -> Result<Option<Cow<'static, str>>> {
        let pos = self.skip_ws();
        self.pos = pos;
        // Ensure the key byte (or `}`) is present before deciding; the
        // reader may not have delivered it yet.
        self.fill(pos + 1)?;
        let Some(&b) = self.buf.get(pos) else {
            return Err(self.err_at(ErrorKind::Eof, pos));
        };
        match b {
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
        let key = self.lex_string()?;
        let p2 = self.skip_ws();
        self.pos = p2;
        self.fill(p2 + 1)?;
        let Some(&c) = self.buf.get(p2) else {
            return Err(self.err_at(ErrorKind::Eof, p2));
        };
        if c != b':' {
            return Err(self.err_at(
                ErrorKind::Expected {
                    what: "':'",
                    found: Some(c),
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

    fn obj_sep_impl(&mut self) -> Result<bool> {
        let pos = self.skip_ws();
        self.pos = pos;
        self.fill(pos + 1)?;
        let Some(&b) = self.buf.get(pos) else {
            return Err(self.err_at(ErrorKind::Eof, pos));
        };
        match b {
            b',' => {
                self.pos = pos + 1;
                let next = self.skip_ws();
                if self.buf.get(next) == Some(&b'}') {
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

    fn arr_more_impl(&mut self) -> Result<bool> {
        let pos = self.skip_ws();
        self.pos = pos;
        self.fill(pos + 1)?;
        let Some(&b) = self.buf.get(pos) else {
            return Err(self.err_at(ErrorKind::Eof, pos));
        };
        Ok(b != b']')
    }

    fn arr_sep_impl(&mut self) -> Result<bool> {
        let pos = self.skip_ws();
        self.pos = pos;
        self.fill(pos + 1)?;
        let Some(&b) = self.buf.get(pos) else {
            return Err(self.err_at(ErrorKind::Eof, pos));
        };
        match b {
            b',' => {
                self.pos = pos + 1;
                let next = self.skip_ws();
                if self.buf.get(next) == Some(&b']') {
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

    fn end_impl(&mut self) -> Result<()> {
        let pos = self.skip_ws();
        self.pos = pos;
        if self.lookahead.is_none() && !self.has_more(pos)? {
            Ok(())
        } else {
            let found = self.buf.get(pos).copied();
            Err(self.err_at(
                ErrorKind::Expected {
                    what: "end of input",
                    found,
                },
                pos,
            ))
        }
    }

    fn skip_impl(&mut self) -> Result<()> {
        match self.lex_peek()? {
            Token::BeginObject => {
                self.begin_object_impl()?;
                while self.key_impl()?.is_some() {
                    self.skip_impl()?;
                    if !self.obj_sep_impl()? {
                        break;
                    }
                }
                self.end_object_impl()?;
            }
            Token::BeginArray => {
                self.begin_array_impl()?;
                while self.arr_more_impl()? {
                    self.skip_impl()?;
                    if !self.arr_sep_impl()? {
                        break;
                    }
                }
                self.end_array_impl()?;
            }
            _ => {
                self.lex_next()?;
            }
        }
        Ok(())
    }

    fn begin_object_impl(&mut self) -> Result<()> {
        self.enter_container()?;
        match self.lex_next()? {
            Token::BeginObject => Ok(()),
            other => Err(self.invalid_type("'{'", &other)),
        }
    }

    fn end_object_impl(&mut self) -> Result<()> {
        let r = match self.lex_next()? {
            Token::EndObject => Ok(()),
            other => Err(self.invalid_type("'}'", &other)),
        };
        self.depth = self.depth.saturating_sub(1);
        r
    }

    fn begin_array_impl(&mut self) -> Result<()> {
        self.enter_container()?;
        match self.lex_next()? {
            Token::BeginArray => Ok(()),
            other => Err(self.invalid_type("'['", &other)),
        }
    }

    fn end_array_impl(&mut self) -> Result<()> {
        let r = match self.lex_next()? {
            Token::EndArray => Ok(()),
            other => Err(self.invalid_type("']'", &other)),
        };
        self.depth = self.depth.saturating_sub(1);
        r
    }
}

/// Lift a `Token<'static>` to any lifetime (owned strings only).
fn lift_token<'de>(token: Token<'static>) -> Token<'de> {
    match token {
        Token::Null => Token::Null,
        Token::Bool(b) => Token::Bool(b),
        Token::Number(n) => Token::Number(n),
        Token::Str(s) => Token::Str(Cow::Owned(s.into_owned())),
        Token::BeginObject => Token::BeginObject,
        Token::EndObject => Token::EndObject,
        Token::BeginArray => Token::BeginArray,
        Token::EndArray => Token::EndArray,
    }
}

impl<'de, R: std::io::Read> FormatDecoder<'de> for StreamDecoder<R> {
    type Error = Error;

    fn begin_object(&mut self) -> Result<()> {
        self.begin_object_impl()
    }

    fn end_object(&mut self) -> Result<()> {
        self.end_object_impl()
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        // `key_impl` always returns an owned string; `Cow<'static, str>`
        // coerces to `Cow<'de, str>`.
        self.key_impl()
    }

    fn object_entry_sep(&mut self) -> Result<bool> {
        self.obj_sep_impl()
    }

    fn begin_array(&mut self) -> Result<()> {
        self.begin_array_impl()
    }

    fn end_array(&mut self) -> Result<()> {
        self.end_array_impl()
    }

    fn array_has_more(&mut self) -> Result<bool> {
        self.arr_more_impl()
    }

    fn array_entry_sep(&mut self) -> Result<bool> {
        self.arr_sep_impl()
    }

    fn unit(&mut self) -> Result<()> {
        match self.lex_next()? {
            Token::Null => Ok(()),
            other => Err(self.invalid_type("null", &other)),
        }
    }

    fn bool(&mut self) -> Result<bool> {
        match self.lex_next()? {
            Token::Bool(b) => Ok(b),
            other => Err(self.invalid_type("bool", &other)),
        }
    }

    fn number(&mut self) -> Result<Number> {
        match self.lex_next()? {
            Token::Number(n) => Ok(n),
            other => Err(self.invalid_type("number", &other)),
        }
    }

    fn string(&mut self) -> Result<Cow<'de, str>> {
        match self.lex_next()? {
            Token::Str(s) => Ok(Cow::Owned(s.into_owned())),
            other => Err(self.invalid_type("string", &other)),
        }
    }

    fn char(&mut self) -> Result<char> {
        match self.lex_next()? {
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

    fn bytes(&mut self) -> Result<Cow<'de, [u8]>> {
        match self.lex_peek()? {
            Token::Str(_) => match self.string()? {
                Cow::Owned(s) => Ok(Cow::Owned(s.into_bytes())),
                Cow::Borrowed(_) => unreachable!("stream decoder always owns strings"),
            },
            Token::BeginArray => {
                self.begin_array_impl()?;
                let mut out = Vec::new();
                while self.arr_more_impl()? {
                    out.push(self.u8()?);
                    if !self.arr_sep_impl()? {
                        break;
                    }
                }
                self.end_array_impl()?;
                Ok(Cow::Owned(out))
            }
            _ => Err(Error::custom("expected a byte string or an array of bytes")),
        }
    }

    fn skip_value(&mut self) -> Result<()> {
        self.skip_impl()
    }

    fn peek_token(&mut self) -> Result<Token<'de>> {
        self.lex_peek().map(lift_token)
    }

    fn next_token(&mut self) -> Result<Token<'de>> {
        self.lex_next().map(lift_token)
    }

    fn save(&self) -> Mark {
        Mark::new(self.pos, self.depth)
    }

    fn restore(&mut self, mark: Mark) {
        self.pos = mark.pos;
        self.lookahead = None;
        self.depth = mark.depth;
    }

    fn set_expecting(&mut self, expecting: &'static str) -> Option<&'static str> {
        self.expecting.replace(expecting)
    }

    fn option_tag(&mut self) -> Result<OptionTag> {
        match self.lex_peek()? {
            Token::Null => {
                self.lex_next()?;
                Ok(OptionTag::None)
            }
            _ => Ok(OptionTag::Some),
        }
    }

    fn map_key<K: for<'a> NsonDeserialize<'a>>(&mut self) -> Result<Option<K>> {
        match self.key_impl()? {
            None => Ok(None),
            Some(key) => {
                let k = crate::de::nextdecode_map_key::<K>(key.as_ref())?;
                Ok(Some(k))
            }
        }
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

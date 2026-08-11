//! BSON codec (MongoDB binary documents).
//!
//! Implements the BSON document format used by MongoDB and many drivers:
//! doubles, UTF-8 strings, embedded documents, arrays (numeric-keyed
//! documents), binary values as strings, booleans, `null`, int32 and int64.
//! Every element is `<type:1><cstring name><value>` and every document is
//! `<int32 length><elements><0x00>`.
//!
//! BSON is document-oriented: a root scalar (a bare integer, string, ...) is
//! not representable on the wire and is rejected on both encode and decode
//! with an explicit error. Wrap scalars in a document (e.g. `{"0": value}`)
//! or use a struct/map target.

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

/// BSON format marker.
#[derive(Clone, Copy, Debug)]
pub struct Bson;

impl Format for Bson {
    const NAME: &'static str = "bson";
    const MIME: &'static str = "application/bson";
    const EXTENSIONS: &'static [&'static str] = &["bson"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = BsonEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.finish_vec()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = BsonDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Element type bytes.
const T_DOUBLE: u8 = 0x01;
const T_STRING: u8 = 0x02;
const T_DOC: u8 = 0x03;
const T_ARRAY: u8 = 0x04;
const T_BOOL: u8 = 0x08;
const T_NULL: u8 = 0x0A;
const T_INT32: u8 = 0x10;
const T_INT64: u8 = 0x12;

/// Streaming BSON encoder.
pub struct BsonEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    docs: Vec<DocFrame>,
    pending_type: Option<usize>,
    root_written: bool,
}

enum DocKind {
    Object,
    Array { next_index: u64 },
}

struct DocFrame {
    start: usize,
    kind: DocKind,
}

impl<W: Write> BsonEncoder<W> {
    /// Create a BSON encoder over `writer`.
    pub fn new(writer: W) -> Self {
        BsonEncoder {
            writer,
            buf: Vec::with_capacity(1024),
            docs: Vec::new(),
            pending_type: None,
            root_written: false,
        }
    }

    fn write_cstring(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    fn element_start(&mut self, ty: u8) -> Result<()> {
        match self.pending_type.take() {
            Some(pos) => {
                self.buf[pos] = ty;
                Ok(())
            }
            None if self.docs.is_empty() => Err(Error::custom(
                "bson: requires a top-level document (wrap scalars in an object)",
            )),
            None => Err(Error::custom(
                "bson: object key or array separator required",
            )),
        }
    }

    fn begin_doc(&mut self, ty: u8, kind: DocKind) -> Result<()> {
        if self.docs.is_empty() {
            if self.root_written {
                return Err(Error::custom("bson: multiple root documents"));
            }
            self.root_written = true;
        } else {
            self.element_start(ty)?;
        }
        let start = self.buf.len();
        self.buf.extend_from_slice(&[0, 0, 0, 0]);
        self.docs.push(DocFrame { start, kind });
        Ok(())
    }

    fn end_doc(&mut self, expect_array: bool) -> Result<()> {
        if self.pending_type.is_some() {
            return Err(Error::custom("bson: element name has no value"));
        }
        let frame = self
            .docs
            .last()
            .ok_or_else(|| Error::custom("bson: document end without start"))?;
        if matches!(frame.kind, DocKind::Array { .. }) != expect_array {
            return Err(Error::custom("bson: mismatched document end"));
        }
        let start = frame.start;
        let len = self
            .buf
            .len()
            .checked_sub(start)
            .and_then(|len| len.checked_add(1))
            .and_then(|len| i32::try_from(len).ok())
            .ok_or_else(|| Error::custom("bson: document exceeds i32 wire limit"))?;
        self.docs.pop();
        self.buf.push(0);
        self.buf[start..start + 4].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }

    fn validate_finished(&self) -> Result<()> {
        if !self.root_written {
            return Err(Error::custom("bson: encoder did not receive a document"));
        }
        if !self.docs.is_empty() || self.pending_type.is_some() {
            return Err(Error::custom("bson: encoder finished inside a document"));
        }
        Ok(())
    }

    /// Flush the internal buffer and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.validate_finished()?;
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn finish_vec(mut self) -> Result<Vec<u8>> {
        self.validate_finished()?;
        Ok(core::mem::take(&mut self.buf))
    }
}

impl<W: Write> FormatEncoder for BsonEncoder<W> {
    fn begin_array(&mut self) -> Result<()> {
        self.begin_doc(T_ARRAY, DocKind::Array { next_index: 0 })
    }

    fn separator(&mut self) -> Result<()> {
        // Array elements are `<type><"N"><value>`; the type byte is patched
        // by the element's value method.
        if self.pending_type.is_some() {
            return Err(Error::custom("bson: array value required after separator"));
        }
        let index = match self.docs.last_mut() {
            Some(DocFrame {
                kind: DocKind::Array { next_index },
                ..
            }) => {
                let index = *next_index;
                *next_index = next_index
                    .checked_add(1)
                    .ok_or_else(|| Error::custom("bson: array index overflow"))?;
                index
            }
            _ => return Err(Error::custom("bson: array separator outside array")),
        };
        self.pending_type = Some(self.buf.len());
        self.buf.push(0);
        let mut key_buf = [0u8; 20];
        let key = render_index(index, &mut key_buf);
        self.buf.extend_from_slice(key.as_bytes());
        self.buf.push(0);
        Ok(())
    }

    fn end_array(&mut self) -> Result<()> {
        self.end_doc(true)
    }

    fn begin_object(&mut self) -> Result<()> {
        self.begin_doc(T_DOC, DocKind::Object)
    }

    fn key(&mut self, key: &str) -> Result<()> {
        if key.as_bytes().contains(&0) {
            return Err(Error::custom("bson: object key contains NUL"));
        }
        if !matches!(
            self.docs.last().map(|frame| &frame.kind),
            Some(DocKind::Object)
        ) {
            return Err(Error::custom("bson: object key outside object"));
        }
        if self.pending_type.is_some() {
            return Err(Error::custom("bson: object value required after key"));
        }
        self.pending_type = Some(self.buf.len());
        self.buf.push(0);
        self.write_cstring(key);
        Ok(())
    }

    fn end_object(&mut self) -> Result<()> {
        self.end_doc(false)
    }

    fn write_null(&mut self) -> Result<()> {
        self.element_start(T_NULL)?;
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<()> {
        self.element_start(T_BOOL)?;
        self.buf.push(if value { 1 } else { 0 });
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<()> {
        let len = value
            .len()
            .checked_add(1)
            .and_then(|len| i32::try_from(len).ok())
            .ok_or_else(|| Error::custom("bson: string exceeds i32 wire limit"))?;
        self.element_start(T_STRING)?;
        self.buf.extend_from_slice(&len.to_le_bytes());
        self.buf.extend_from_slice(value.as_bytes());
        self.buf.push(0);
        Ok(())
    }

    fn write_char(&mut self, value: char) -> Result<()> {
        let mut buf = [0u8; 4];
        let s = value.encode_utf8(&mut buf);
        self.write_str(s)
    }

    fn write_number(&mut self, value: &Number) -> Result<()> {
        match *value {
            Number::I64(v) => self.write_i64(v),
            Number::U64(v) => self.write_u64(v),
            Number::I128(v) => self.write_i128(v),
            Number::U128(v) => self.write_u128(v),
            Number::F64(v) => self.write_f64(v),
        }
    }

    fn write_i64(&mut self, value: i64) -> Result<()> {
        if i32::try_from(value).is_ok() {
            self.element_start(T_INT32)?;
            self.buf.extend_from_slice(&(value as i32).to_le_bytes());
        } else {
            self.element_start(T_INT64)?;
            self.buf.extend_from_slice(&value.to_le_bytes());
        }
        Ok(())
    }

    fn write_u64(&mut self, value: u64) -> Result<()> {
        match i64::try_from(value) {
            Ok(v) => self.write_i64(v),
            Err(_) => Err(Error::custom("bson: u64 exceeds int64 range")),
        }
    }

    fn write_i128(&mut self, value: i128) -> Result<()> {
        match i64::try_from(value) {
            Ok(v) => self.write_i64(v),
            Err(_) => Err(Error::custom("bson: i128 exceeds int64 range")),
        }
    }

    fn write_u128(&mut self, value: u128) -> Result<()> {
        match i64::try_from(value) {
            Ok(v) => self.write_i64(v),
            Err(_) => Err(Error::custom("bson: u128 exceeds int64 range")),
        }
    }

    fn write_f64(&mut self, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::custom(
                "bson: non-finite float is outside the data model",
            ));
        }
        self.element_start(T_DOUBLE)?;
        self.buf.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn write_f32(&mut self, value: f32) -> Result<()> {
        self.write_f64(value as f64)
    }
}

/// Render an array index into a stack buffer (fast path for < 1e9 elements).
fn render_index(index: u64, buf: &mut [u8; 20]) -> alloc::string::String {
    let mut n = index;
    let mut end = buf.len();
    loop {
        end -= 1;
        buf[end] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    // Copy out (string cannot borrow the stack buffer of the caller safely).
    let mut out = String::with_capacity(buf.len() - end);
    out.push_str(core::str::from_utf8(&buf[end..]).expect("ascii digits"));
    out
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum CFrameKind {
    Doc,
    Array,
}

#[derive(Clone, Copy)]
struct CFrame {
    kind: CFrameKind,
    /// Absolute offset of the int32 length field (for length validation).
    start: usize,
    /// Declared document length including the trailing NUL.
    declared_len: usize,
}

/// Streaming BSON decoder.
pub struct BsonDecoder<'de> {
    cur: Cursor<'de>,
    lookahead: Option<Token<'de>>,
    pending_type: Option<u8>,
    frames: Vec<CFrame>,
    depth: u32,
    max_depth: u32,
}

impl<'de> BsonDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        BsonDecoder {
            cur: Cursor::new(input),
            lookahead: None,
            pending_type: None,
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
            Err(Error::custom("bson: trailing bytes after value"))
        }
    }

    fn enter_container(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(Error::custom("bson: recursion limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn read_cstring(&mut self) -> Result<Cow<'de, str>> {
        let bytes = self.cur.until_inclusive(0)?;
        let s = core::str::from_utf8(&bytes[..bytes.len() - 1])
            .map_err(|_| Error::custom("bson: invalid utf-8 field name"))?;
        Ok(Cow::Borrowed(s))
    }

    /// Resolve the type byte of the next value.
    ///
    /// - Inside an object: the type stored by `object_key`.
    /// - Inside an array: read the `<type><name>` element header.
    /// - At the root: a scalar target is structurally invalid (BSON is
    ///   document-oriented), so report a clear error.
    fn next_type(&mut self) -> Result<u8> {
        if let Some(ty) = self.pending_type.take() {
            return Ok(ty);
        }
        let in_array = self
            .frames
            .last()
            .map(|f| f.kind == CFrameKind::Array)
            .unwrap_or(false);
        if in_array {
            let ty = self.cur.byte()?;
            let _name = self.read_cstring()?;
            return Ok(ty);
        }
        Err(Error::custom(
            "bson: scalar value outside a document (BSON is document-oriented)",
        ))
    }

    fn read_string_value(&mut self) -> Result<Cow<'de, str>> {
        let wire_len = self.cur.le_u32()?;
        if wire_len > i32::MAX as u32 {
            return Err(Error::custom("bson: invalid negative string length"));
        }
        let len = usize::try_from(wire_len)
            .map_err(|_| Error::custom("bson: string length exceeds platform limit"))?;
        if len == 0 {
            return Err(Error::custom("bson: empty string length"));
        }
        let bytes = self.cur.take(len - 1)?;
        if self.cur.byte()? != 0 {
            return Err(Error::custom("bson: string is not NUL-terminated"));
        }
        let s = core::str::from_utf8(bytes).map_err(|_| Error::custom("bson: invalid utf-8"))?;
        Ok(Cow::Borrowed(s))
    }

    fn read_doc_header(&mut self) -> Result<(usize, usize)> {
        let start = self.cur.pos();
        let wire_len = self.cur.le_u32()?;
        if wire_len > i32::MAX as u32 {
            return Err(Error::custom("bson: invalid negative document length"));
        }
        let len = usize::try_from(wire_len)
            .map_err(|_| Error::custom("bson: document length exceeds platform limit"))?;
        if len < 5 {
            return Err(Error::custom("bson: document too small"));
        }
        Ok((start, len))
    }
}

impl<'de> FormatDecoder<'de> for BsonDecoder<'de> {
    fn begin_object(&mut self) -> Result<()> {
        self.enter_container()?;
        if self.frames.is_empty() && self.lookahead.is_none() {
            // Root: classify the document before committing, so a numeric-key
            // array document is not silently decoded as an object.
            let saved = self.cur.pos();
            let _ = self.read_doc_header()?;
            let is_array = match self.cur.byte()? {
                0 => false, // empty document -> object
                _ => self.read_cstring()? == "0",
            };
            self.cur.seek(saved);
            if is_array {
                return Err(Error::invalid_type("a document", "array"));
            }
            let (start, len) = self.read_doc_header()?;
            self.frames.push(CFrame {
                kind: CFrameKind::Doc,
                start,
                declared_len: len,
            });
            return Ok(());
        }
        match self.next_token()? {
            Token::BeginObject => {
                let (start, len) = self.read_doc_header()?;
                self.frames.push(CFrame {
                    kind: CFrameKind::Doc,
                    start,
                    declared_len: len,
                });
                Ok(())
            }
            other => Err(Error::invalid_type("a document", token_name(&other))),
        }
    }

    fn end_object(&mut self) -> Result<()> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("bson: document end without start"))?;
        if frame.kind != CFrameKind::Doc {
            return Err(Error::custom("bson: document frame mismatch"));
        }
        if self.cur.byte()? != 0 {
            return Err(Error::custom("bson: document is not 0x00-terminated"));
        }
        if self.cur.pos() != frame.start + frame.declared_len {
            return Err(Error::custom("bson: document length mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        if self.cur.peek()? == 0 {
            return Ok(None);
        }
        let ty = self.cur.byte()?;
        self.pending_type = Some(ty);
        let name = self.read_cstring()?;
        Ok(Some(name))
    }

    fn object_entry_sep(&mut self) -> Result<bool> {
        Ok(self.cur.peek()? != 0)
    }

    fn begin_array(&mut self) -> Result<()> {
        self.enter_container()?;
        if self.frames.is_empty() && self.lookahead.is_none() {
            // Root: only a numeric-key array document is an array.
            let saved = self.cur.pos();
            let _ = self.read_doc_header()?;
            let is_array = match self.cur.byte()? {
                0 => false, // empty document -> object
                _ => self.read_cstring()? == "0",
            };
            self.cur.seek(saved);
            if !is_array {
                return Err(Error::invalid_type("an array", "document"));
            }
            let (start, len) = self.read_doc_header()?;
            self.frames.push(CFrame {
                kind: CFrameKind::Array,
                start,
                declared_len: len,
            });
            return Ok(());
        }
        match self.next_token()? {
            Token::BeginArray => {
                let (start, len) = self.read_doc_header()?;
                self.frames.push(CFrame {
                    kind: CFrameKind::Array,
                    start,
                    declared_len: len,
                });
                Ok(())
            }
            other => Err(Error::invalid_type("an array", token_name(&other))),
        }
    }

    fn end_array(&mut self) -> Result<()> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("bson: array end without start"))?;
        if frame.kind != CFrameKind::Array {
            return Err(Error::custom("bson: array frame mismatch"));
        }
        if self.cur.byte()? != 0 {
            return Err(Error::custom("bson: array is not 0x00-terminated"));
        }
        if self.cur.pos() != frame.start + frame.declared_len {
            return Err(Error::custom("bson: document length mismatch"));
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn array_has_more(&mut self) -> Result<bool> {
        Ok(self.cur.peek()? != 0)
    }

    fn array_entry_sep(&mut self) -> Result<bool> {
        Ok(self.cur.peek()? != 0)
    }

    fn unit(&mut self) -> Result<()> {
        if let Some(t) = self.lookahead.take() {
            return match t {
                Token::Null => Ok(()),
                other => Err(Error::invalid_type("null", token_name(&other))),
            };
        }
        let ty = self.next_type()?;
        match ty {
            T_NULL => Ok(()),
            other => Err(Error::custom(alloc::format!(
                "bson: expected null, found type 0x{other:02x}"
            ))),
        }
    }

    fn bool(&mut self) -> Result<bool> {
        if let Some(t) = self.lookahead.take() {
            return match t {
                Token::Bool(b) => Ok(b),
                other => Err(Error::invalid_type("bool", token_name(&other))),
            };
        }
        let ty = self.next_type()?;
        match ty {
            T_BOOL => match self.cur.byte()? {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(Error::custom("bson: boolean byte must be 0 or 1")),
            },
            other => Err(Error::custom(alloc::format!(
                "bson: expected bool, found type 0x{other:02x}"
            ))),
        }
    }

    fn number(&mut self) -> Result<Number> {
        if let Some(t) = self.lookahead.take() {
            return match t {
                Token::Number(n) => Ok(n),
                other => Err(Error::invalid_type("number", token_name(&other))),
            };
        }
        let ty = self.next_type()?;
        match ty {
            T_DOUBLE => {
                let value = f64::from_le_bytes(self.cur.take(8)?.try_into().unwrap());
                Number::from_f64(value).ok_or_else(|| {
                    Error::custom("bson: non-finite float is outside the data model")
                })
            }
            T_INT32 => Ok(Number::from(i32::from_le_bytes(
                self.cur.take(4)?.try_into().unwrap(),
            ))),
            T_INT64 => Ok(Number::from(i64::from_le_bytes(
                self.cur.take(8)?.try_into().unwrap(),
            ))),
            other => Err(Error::custom(alloc::format!(
                "bson: expected number, found type 0x{other:02x}"
            ))),
        }
    }

    fn string(&mut self) -> Result<Cow<'de, str>> {
        if let Some(t) = self.lookahead.take() {
            return match t {
                Token::Str(s) => Ok(s),
                other => Err(Error::invalid_type("string", token_name(&other))),
            };
        }
        let ty = self.next_type()?;
        match ty {
            T_STRING => self.read_string_value(),
            other => Err(Error::custom(alloc::format!(
                "bson: expected string, found type 0x{other:02x}"
            ))),
        }
    }

    fn char(&mut self) -> Result<char> {
        let s = self.string()?;
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(Error::invalid_type("a single-character string", "string")),
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
        Ok(self.lookahead.as_ref().expect("just set").clone())
    }

    fn next_token(&mut self) -> Result<Token<'de>> {
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
        self.pending_type = None;
        self.frames.truncate(mark.frame_len);
        self.depth = mark.depth;
    }
}

impl<'de> BsonDecoder<'de> {
    /// Produce the next token. At the root the value is a BSON document, so
    /// classify it as array or object by non-destructively peeking the first
    /// element name ("0" -> array) and restore the cursor for `begin_*`.
    fn read_token(&mut self) -> Result<Token<'de>> {
        if self.frames.is_empty() && self.pending_type.is_none() {
            let saved = self.cur.pos();
            let _ = self.read_doc_header()?;
            let is_array = match self.cur.byte()? {
                0 => false, // empty document -> object
                _ => self.read_cstring()? == "0",
            };
            self.cur.seek(saved);
            return Ok(if is_array {
                Token::BeginArray
            } else {
                Token::BeginObject
            });
        }
        let ty = self.next_type()?;
        match ty {
            T_DOUBLE => {
                let v = f64::from_le_bytes(self.cur.take(8)?.try_into().unwrap());
                let number = Number::from_f64(v).ok_or_else(|| {
                    Error::custom("bson: non-finite float is outside the data model")
                })?;
                Ok(Token::Number(number))
            }
            T_STRING => {
                let s = self.read_string_value()?;
                Ok(Token::Str(s))
            }
            T_DOC => {
                // Document header is read by begin_object.
                Ok(Token::BeginObject)
            }
            T_ARRAY => {
                // Array header is read by begin_array.
                Ok(Token::BeginArray)
            }
            T_BOOL => {
                let v = match self.cur.byte()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::custom("bson: boolean byte must be 0 or 1")),
                };
                Ok(Token::Bool(v))
            }
            T_NULL => Ok(Token::Null),
            T_INT32 => {
                let v = Number::from(i32::from_le_bytes(self.cur.take(4)?.try_into().unwrap()));
                Ok(Token::Number(v))
            }
            T_INT64 => {
                let v = Number::from(i64::from_le_bytes(self.cur.take(8)?.try_into().unwrap()));
                Ok(Token::Number(v))
            }
            other => Err(Error::custom(alloc::format!(
                "bson: unsupported element type 0x{other:02x}"
            ))),
        }
    }
}

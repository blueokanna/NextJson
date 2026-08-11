//! URL query-string codec (`application/x-www-form-urlencoded`).
//!
//! Encodes a flat key/value map as `key=value&key=value` with RFC 3986
//! percent-encoding (spaces become `+`). Values are stringified scalars;
//! nested containers are rejected because the format is flat by design.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// URL form-encoding format marker.
#[derive(Clone, Copy, Debug)]
pub struct UrlForm;

impl Format for UrlForm {
    const NAME: &'static str = "urlform";
    const MIME: &'static str = "application/x-www-form-urlencoded";
    const EXTENSIONS: &'static [&'static str] = &["form"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = UrlFormEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.writer.write_all(&encoder.buf)?;
        Ok(core::mem::take(&mut encoder.buf))
    }
    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = UrlFormDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

/// Percent-encode a string for a query value.
fn percent_encode(out: &mut Vec<u8>, s: &str) {
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b),
            b' ' => out.push(b'+'),
            other => {
                out.push(b'%');
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push(HEX[(other >> 4) as usize]);
                out.push(HEX[(other & 0xF) as usize]);
            }
        }
    }
}

/// Percent-decode a query component (in place on the scratch buffer).
///
/// `%XX` escapes and `+` are decoded at the byte level, then the whole
/// component is validated as UTF-8 so `%C3%A9` yields `é` rather than two
/// Latin-1 characters. A component without `%` or `+` is copied verbatim.
fn percent_decode(out: &mut String, bytes: &[u8]) -> Result<()> {
    if !bytes.contains(&b'%') && !bytes.contains(&b'+') {
        let s = core::str::from_utf8(bytes).map_err(|_| Error::custom("urlform: invalid utf-8"))?;
        out.push_str(s);
        return Ok(());
    }
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                decoded.push(b' ');
                i += 1;
            }
            b'%' => {
                // Need bytes[i+1] and bytes[i+2]; the strict bound is
                // `i + 2 < bytes.len()`, i.e. `i + 2 >= bytes.len()` is an
                // out-of-bounds read.
                if i + 2 >= bytes.len() {
                    return Err(Error::custom("urlform: truncated percent escape"));
                }
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                decoded.push(hi << 4 | lo);
                i += 3;
            }
            b => {
                decoded.push(b);
                i += 1;
            }
        }
    }
    let s = core::str::from_utf8(&decoded).map_err(|_| Error::custom("urlform: invalid utf-8"))?;
    out.push_str(s);
    Ok(())
}

fn hex_val(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(Error::custom("urlform: invalid percent escape")),
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Streaming URL-form encoder (flat key/value map only).
pub struct UrlFormEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    first: bool,
    in_object: bool,
    root_written: bool,
    pending_key: Option<String>,
}

impl<W: Write> UrlFormEncoder<W> {
    /// Create a URL-form encoder over `writer`.
    pub fn new(writer: W) -> Self {
        UrlFormEncoder {
            writer,
            buf: Vec::with_capacity(256),
            first: true,
            in_object: false,
            root_written: false,
            pending_key: None,
        }
    }

    /// Flush and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn flush_value(&mut self, value: &str) -> Result<()> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| Error::custom("urlform: value without key"))?;
        if !self.first {
            self.buf.push(b'&');
        }
        self.first = false;
        percent_encode(&mut self.buf, &key);
        self.buf.push(b'=');
        percent_encode(&mut self.buf, value);
        Ok(())
    }
}

impl<W: Write> FormatEncoder for UrlFormEncoder<W> {
    fn begin_array(&mut self) -> Result<()> {
        Err(Error::custom(
            "urlform: nested containers are not representable",
        ))
    }

    fn separator(&mut self) -> Result<()> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<()> {
        Err(Error::custom(
            "urlform: nested containers are not representable",
        ))
    }

    fn begin_object(&mut self) -> Result<()> {
        if self.root_written {
            return Err(Error::custom(
                "urlform: nested containers are not representable",
            ));
        }
        self.root_written = true;
        self.in_object = true;
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<()> {
        self.pending_key = Some(key.to_string());
        Ok(())
    }

    fn end_object(&mut self) -> Result<()> {
        self.in_object = false;
        Ok(())
    }

    fn write_null(&mut self) -> Result<()> {
        self.flush_value("null")
    }

    fn write_bool(&mut self, value: bool) -> Result<()> {
        self.flush_value(if value { "true" } else { "false" })
    }

    fn write_str(&mut self, value: &str) -> Result<()> {
        self.flush_value(value)
    }

    fn write_char(&mut self, value: char) -> Result<()> {
        let mut buf = [0u8; 4];
        let s = value.encode_utf8(&mut buf);
        self.flush_value(s)
    }

    fn write_number(&mut self, value: &Number) -> Result<()> {
        let s = number_to_string(value);
        self.flush_value(&s)
    }

    fn write_i64(&mut self, value: i64) -> Result<()> {
        self.flush_value(&value.to_string())
    }

    fn write_u64(&mut self, value: u64) -> Result<()> {
        self.flush_value(&value.to_string())
    }

    fn write_i128(&mut self, value: i128) -> Result<()> {
        self.flush_value(&value.to_string())
    }

    fn write_u128(&mut self, value: u128) -> Result<()> {
        self.flush_value(&value.to_string())
    }

    fn write_f64(&mut self, value: f64) -> Result<()> {
        self.flush_value(&value.to_string())
    }

    fn write_f32(&mut self, value: f32) -> Result<()> {
        self.flush_value(&value.to_string())
    }
}

fn number_to_string(n: &Number) -> String {
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

/// Streaming URL-form decoder.
pub struct UrlFormDecoder<'de> {
    input: &'de [u8],
    pos: usize,
    pending_key: Option<String>,
    pending_value: Option<String>,
    lookahead: Option<Token<'de>>,
    in_object: bool,
    root_written: bool,
}

impl<'de> UrlFormDecoder<'de> {
    /// Create a decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        UrlFormDecoder {
            input,
            pos: 0,
            pending_key: None,
            pending_value: None,
            lookahead: None,
            in_object: false,
            root_written: false,
        }
    }

    /// Validate that the whole input was consumed.
    pub fn end(&mut self) -> Result<()> {
        self.expect_end()
    }

    fn expect_end(&mut self) -> Result<()> {
        if self.pos >= self.input.len() {
            Ok(())
        } else {
            Err(Error::custom("urlform: trailing bytes after value"))
        }
    }

    fn read_pair(&mut self) -> Result<Option<(String, String)>> {
        if self.pos >= self.input.len() {
            return Ok(None);
        }
        let rest = &self.input[self.pos..];
        let end = rest
            .iter()
            .position(|b| *b == b'&')
            .map(|i| self.pos + i)
            .unwrap_or(self.input.len());
        let pair = &self.input[self.pos..end];
        self.pos = if end < self.input.len() { end + 1 } else { end };
        let eq = pair.iter().position(|b| *b == b'=').unwrap_or(pair.len());
        let mut key = String::new();
        percent_decode(&mut key, &pair[..eq])?;
        let mut value = String::new();
        let vstart = if eq < pair.len() { eq + 1 } else { eq };
        percent_decode(&mut value, &pair[vstart..])?;
        Ok(Some((key, value)))
    }
}

impl<'de> FormatDecoder<'de> for UrlFormDecoder<'de> {
    fn begin_object(&mut self) -> Result<()> {
        if self.root_written {
            return Err(Error::custom(
                "urlform: nested containers are not representable",
            ));
        }
        self.root_written = true;
        self.in_object = true;
        Ok(())
    }

    fn end_object(&mut self) -> Result<()> {
        self.in_object = false;
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>> {
        if !self.in_object {
            return Err(Error::custom("urlform: object key outside object"));
        }
        // A new entry starts: discard any stale lookahead left by a peek on
        // the previous entry's value.
        self.lookahead = None;
        match self.read_pair()? {
            Some((k, v)) => {
                self.pending_key = Some(k.clone());
                self.pending_value = Some(v);
                Ok(Some(Cow::Owned(k)))
            }
            None => Ok(None),
        }
    }

    fn object_entry_sep(&mut self) -> Result<bool> {
        Ok(self.pos < self.input.len())
    }

    fn begin_array(&mut self) -> Result<()> {
        Err(Error::custom(
            "urlform: nested containers are not representable",
        ))
    }

    fn end_array(&mut self) -> Result<()> {
        Err(Error::custom(
            "urlform: nested containers are not representable",
        ))
    }

    fn array_has_more(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn array_entry_sep(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn unit(&mut self) -> Result<()> {
        self.take_value()?;
        Ok(())
    }

    fn bool(&mut self) -> Result<bool> {
        match self.take_value()?.as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            other => Err(Error::custom(alloc::format!(
                "urlform: invalid bool value {other:?}"
            ))),
        }
    }

    fn number(&mut self) -> Result<Number> {
        let s = self.take_value()?;
        parse_number(&s)
    }

    fn string(&mut self) -> Result<Cow<'de, str>> {
        Ok(Cow::Owned(self.take_value()?))
    }

    fn char(&mut self) -> Result<char> {
        let s = self.take_value()?;
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(Error::invalid_type("a single-character string", "string")),
        }
    }

    fn skip_value(&mut self) -> Result<()> {
        self.take_value()?;
        Ok(())
    }

    fn peek_token(&mut self) -> Result<Token<'de>> {
        if self.lookahead.is_none() {
            self.lookahead = Some(if self.in_object {
                // Peek must NOT consume the pending value: `Option` / `Value`
                // / untagged decoding peeks first and then reads through
                // `take_value`, which must still see the same value.
                let v = self
                    .pending_value
                    .as_deref()
                    .ok_or_else(|| Error::custom("urlform: missing value"))?;
                Token::Str(Cow::Owned(v.to_string()))
            } else {
                // Root: a URL-form document is always an object. `Value`
                // peeks before `begin_object`, so classify the root here.
                Token::BeginObject
            });
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
            depth: 0,
            frame_len: 0,
        }
    }

    fn restore(&mut self, mark: Mark) {
        self.pos = mark.pos;
    }
}

impl<'de> UrlFormDecoder<'de> {
    fn take_value(&mut self) -> Result<String> {
        // The value belongs to the current entry. `peek_token` may have
        // cloned it into the lookahead (it never consumes `pending_value`);
        // prefer the lookahead, then the pending value, and never read a
        // further pair here.
        if let Some(Token::Str(s)) = self.lookahead.take() {
            return Ok(s.into_owned());
        }
        if let Some(v) = self.pending_value.take() {
            return Ok(v);
        }
        Err(Error::custom("urlform: missing value"))
    }

    fn read_token(&mut self) -> Result<Token<'de>> {
        let v = self.take_value()?;
        Ok(Token::Str(Cow::Owned(v)))
    }
}

/// Parse a scalar string into a [`Number`].
pub(crate) fn parse_number(s: &str) -> Result<Number> {
    if let Ok(v) = s.parse::<i64>() {
        return Ok(Number::from(v));
    }
    if let Ok(v) = s.parse::<u64>() {
        return Ok(Number::from(v));
    }
    if let Ok(v) = s.parse::<f64>() {
        return Ok(Number::F64(v));
    }
    Err(Error::custom(alloc::format!(
        "urlform: invalid number {s:?}"
    )))
}

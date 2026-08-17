//! EDN codec (Extensible Data Notation, Clojure's data format).
//!
//! Text, self-describing format. The JSON-compatible subset covers:
//!
//! - `nil`, `true`, `false`
//! - integers (decimal, `0x` hex) and floats
//! - strings with escapes, vectors `[...]`, lists `(...)` (both → arrays)
//! - maps `{...}` (string or keyword keys; keyword keys decode to their
//!   name) — decoded via a [`Value`] tree, document-shaped like TOML/INI
//!
//! Honestly rejected (no JSON-model equivalent): symbols, keywords as
//! *values*, characters, sets, tagged literals, arbitrary-precision `M`/`N`
//! numbers and radix literals. `#_` discard forms are supported.
//!
//! The encoder collects the event stream into a [`Value`] and emits EDN
//! text when the root closes; objects become maps with string keys.

use alloc::string::String;
use alloc::vec::Vec;

use crate::de::NsonDeserialize;
use crate::error::{Error, Result};
use crate::formats::tree;
use crate::formats::Format;
use crate::map::Map;
use crate::number::Number;
use crate::ser::NsonSerialize;
use crate::value::Value;
use crate::write::Write;

/// EDN format marker.
#[derive(Clone, Copy, Debug)]
pub struct Edn;

/// Document-decoded EDN decoder: parses the whole document into a [`Value`]
/// and serves the unified event stream from it.
pub type EdnDecoder<'de> = tree::TreeDecoder<'de>;

impl Format for Edn {
    const NAME: &'static str = "edn";
    const MIME: &'static str = "application/edn";
    const EXTENSIONS: &'static [&'static str] = &["edn"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = EdnEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.finish()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let value = parse_edn(input)?;
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value)?);
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Encoder (collect into Value, emit at the end)
// ---------------------------------------------------------------------------

/// EDN encoder that collects one event stream and emits it on [`finish`](Self::finish).
pub struct EdnEncoder<W: Write> {
    writer: W,
    collector: tree::CollectEncoder,
}

impl<W: Write> EdnEncoder<W> {
    /// Create an EDN encoder over `writer`.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            collector: tree::CollectEncoder::new(),
        }
    }

    /// Emit the collected document, flush, and return the writer.
    pub fn finish(mut self) -> Result<W> {
        let root = self.collector.take_root()?;
        let mut out = Vec::with_capacity(256);
        emit_edn(&root, &mut out)?;
        self.writer.write_all(&out)?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

impl<W: Write> crate::ser::FormatEncoder for EdnEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.collector.begin_array()
    }
    fn separator(&mut self) -> Result<(), Self::Error> {
        self.collector.separator()
    }
    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.collector.end_array()
    }
    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.collector.begin_object()
    }
    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        self.collector.key(key)
    }
    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.collector.end_object()
    }
    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.collector.write_null()
    }
    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.collector.write_bool(value)
    }
    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.collector.write_str(value)
    }
    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        self.collector.write_char(value)
    }
    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error> {
        self.collector.write_number(value)
    }
    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        self.collector.write_i64(value)
    }
    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.collector.write_u64(value)
    }
    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.collector.write_i128(value)
    }
    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        self.collector.write_u128(value)
    }
    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        self.collector.write_f64(value)
    }
    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        self.collector.write_f32(value)
    }
    fn write_none(&mut self) -> Result<(), Self::Error> {
        self.collector.write_none()
    }
    fn is_human_readable(&self) -> bool {
        true
    }
}

/// Emit an EDN string literal.
fn emit_string(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for &b in s.as_bytes() {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0x00..=0x1F => {
                out.extend_from_slice(b"\\u");
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push(HEX[(b >> 4) as usize]);
                out.push(HEX[(b & 0xF) as usize]);
            }
            other => out.push(other),
        }
    }
    out.push(b'"');
}

/// Emit a collected [`Value`] tree as EDN text.
fn emit_edn(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => {
            out.extend_from_slice(b"nil");
            Ok(())
        }
        Value::Bool(true) => {
            out.extend_from_slice(b"true");
            Ok(())
        }
        Value::Bool(false) => {
            out.extend_from_slice(b"false");
            Ok(())
        }
        Value::Number(n) => {
            let text = tree::number_string(n)?;
            out.extend_from_slice(text.as_bytes());
            Ok(())
        }
        Value::String(s) => {
            emit_string(out, s);
            Ok(())
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                emit_edn(item, out)?;
            }
            out.push(b']');
            Ok(())
        }
        Value::Object(map) => {
            out.push(b'{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.push(b' ');
                emit_string(out, key);
                out.push(b' ');
                emit_edn(item, out)?;
            }
            out.push(b' ');
            out.push(b'}');
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder (parse into a Value tree)
// ---------------------------------------------------------------------------

struct EdnParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> EdnParser<'a> {
    fn new(input: &'a [u8]) -> Self {
        EdnParser { input, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_ws(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b',') => {
                    self.pos += 1;
                }
                Some(b';') => {
                    // Comment to end of line.
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn err(&self, msg: &str) -> Error {
        Error::custom(alloc::format!("edn: {msg} at byte {}", self.pos))
    }

    fn parse_value(&mut self) -> Result<Value> {
        self.skip_ws()?; // `#_` discard form skips the following value (in any position).
        if self.peek() == Some(b'#') && self.input.get(self.pos + 1) == Some(&b'_') {
            self.parse_discard()?;
            return self.parse_value();
        }
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b'[') => self.parse_vector(),
            Some(b'(') => self.parse_list(),
            Some(b'{') => self.parse_map(),
            Some(b'#') => self.parse_dispatch(),
            Some(b':') => Err(self.err("keyword is not representable as a value")),
            Some(b'\\') => Err(self.err("character literal is not representable")),
            Some(_) => self.parse_atom(),
        }
    }

    fn parse_string(&mut self) -> Result<String> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let b = self.bump().ok_or_else(|| self.err("unterminated string"))?;
            match b {
                b'"' => {
                    return String::from_utf8(out).map_err(|_| self.err("invalid utf-8 in string"));
                }
                b'\\' => {
                    let esc = self.bump().ok_or_else(|| self.err("truncated escape"))?;
                    match esc {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'u' => {
                            let hi = self.parse_hex4()?;
                            if (0xD800..=0xDBFF).contains(&hi) {
                                // Expect a low surrogate.
                                if self.peek() == Some(b'\\')
                                    && self.input.get(self.pos + 1) == Some(&b'u')
                                {
                                    self.pos += 2;
                                    let lo = self.parse_hex4()?;
                                    if (0xDC00..=0xDFFF).contains(&lo) {
                                        let cp = 0x10000
                                            + ((hi as u32 - 0xD800) << 10)
                                            + (lo as u32 - 0xDC00);
                                        out.extend_from_slice(
                                            char::from_u32(cp)
                                                .expect("valid scalar")
                                                .encode_utf8(&mut [0u8; 4])
                                                .as_bytes(),
                                        );
                                    } else {
                                        return Err(self.err("invalid surrogate pair"));
                                    }
                                } else {
                                    return Err(self.err("lone high surrogate"));
                                }
                            } else if (0xDC00..=0xDFFF).contains(&hi) {
                                return Err(self.err("lone low surrogate"));
                            } else {
                                out.extend_from_slice(
                                    char::from_u32(hi as u32)
                                        .expect("valid scalar")
                                        .encode_utf8(&mut [0u8; 4])
                                        .as_bytes(),
                                );
                            }
                        }
                        other => {
                            return Err(
                                self.err(&alloc::format!("unsupported escape \\{}", other as char))
                            );
                        }
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let b = self
                .bump()
                .ok_or_else(|| self.err("truncated \\u escape"))?;
            let digit = crate::lex::hex_digit(b).ok_or_else(|| self.err("invalid hex digit"))?;
            value = value * 16 + digit as u16;
        }
        Ok(value)
    }

    fn parse_vector(&mut self) -> Result<Value> {
        debug_assert_eq!(self.peek(), Some(b'['));
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_ws()?;
            match self.peek() {
                None => return Err(self.err("unterminated vector")),
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                Some(b'#') if self.input.get(self.pos + 1) == Some(&b'_') => {
                    self.parse_discard()?;
                }
                _ => items.push(self.parse_value()?),
            }
        }
    }

    fn parse_list(&mut self) -> Result<Value> {
        debug_assert_eq!(self.peek(), Some(b'('));
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_ws()?;
            match self.peek() {
                None => return Err(self.err("unterminated list")),
                Some(b')') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                Some(b'#') if self.input.get(self.pos + 1) == Some(&b'_') => {
                    self.parse_discard()?;
                }
                _ => items.push(self.parse_value()?),
            }
        }
    }

    fn parse_map(&mut self) -> Result<Value> {
        debug_assert_eq!(self.peek(), Some(b'{'));
        self.pos += 1;
        let mut map = Map::new();
        loop {
            self.skip_ws()?;
            match self.peek() {
                None => return Err(self.err("unterminated map")),
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                Some(b'#') if self.input.get(self.pos + 1) == Some(&b'_') => {
                    self.parse_discard()?;
                }
                _ => {
                    let key = self.parse_map_key()?;
                    let value = self.parse_value()?;
                    if map.insert(key, value).is_some() {
                        return Err(self.err("duplicate map key"));
                    }
                }
            }
        }
    }

    /// Parse a map key: either a string or a keyword (decoded to its name).
    fn parse_map_key(&mut self) -> Result<String> {
        self.skip_ws()?;
        match self.peek() {
            Some(b'"') => self.parse_string(),
            Some(b':') => {
                self.pos += 1;
                // Keyword: `:name` or `:ns/name`; the name part becomes the key.
                let start = self.pos;
                while let Some(b) = self.peek() {
                    if is_symbol_char(b) || b == b'/' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                if self.pos == start {
                    return Err(self.err("empty keyword"));
                }
                let raw = &self.input[start..self.pos];
                // Drop a leading `::` (auto-resolved) or namespace prefix.
                let name = match raw.iter().position(|b| *b == b'/') {
                    Some(slash) => &raw[slash + 1..],
                    None => raw,
                };
                String::from_utf8(name.to_vec()).map_err(|_| self.err("invalid utf-8 in keyword"))
            }
            _ => Err(self.err("map key must be a string or keyword")),
        }
    }

    /// `#_` discard form: parse and drop the next value.
    fn parse_discard(&mut self) -> Result<()> {
        debug_assert_eq!(self.peek(), Some(b'#'));
        self.pos += 2; // `#_`
        self.parse_value()?;
        Ok(())
    }

    fn parse_dispatch(&mut self) -> Result<Value> {
        debug_assert_eq!(self.peek(), Some(b'#'));
        match self.input.get(self.pos + 1) {
            Some(b'_') => Err(self.err("discard form in value position")),
            Some(b'{') => Err(self.err("set is not representable")),
            Some(_) => Err(self.err("tagged literal is not representable")),
            None => Err(self.err("truncated dispatch form")),
        }
    }

    fn parse_atom(&mut self) -> Result<Value> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if is_delimiter(b) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err("unexpected character"));
        }
        let raw = &self.input[start..self.pos];
        let text = core::str::from_utf8(raw).map_err(|_| self.err("invalid utf-8 in token"))?;
        match text {
            "nil" => return Ok(Value::Null),
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            _ => {}
        }
        if text.ends_with('M') || text.ends_with('N') {
            return Err(self.err("arbitrary-precision number is not representable"));
        }
        // Signed decimal integer (hex handled inside `parse_edn_int`).
        if let Ok(int) = parse_edn_int(text) {
            return Ok(Value::Number(int));
        }
        // Float.
        if text.contains(['.', 'e', 'E']) {
            let v: f64 = text.parse().map_err(|_| self.err("invalid float"))?;
            if !v.is_finite() {
                return Err(self.err("non-finite float is not representable"));
            }
            return Ok(Value::Number(Number::F64(v)));
        }
        Err(self.err(&alloc::format!("symbol {text:?} is not representable")))
    }
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b'\r' | b',' | b';' | b'(' | b')' | b'[' | b']' | b'{' | b'}'
    )
}

fn is_symbol_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'+' | b'-'
                | b'*'
                | b'/'
                | b'!'
                | b'?'
                | b'_'
                | b'.'
                | b'&'
                | b'%'
                | b'='
                | b'<'
                | b'>'
        )
}

/// Parse a signed decimal or `0x`-hex integer into a [`Number`].
fn parse_edn_int(text: &str) -> Result<Number> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        if negative {
            return Err(Error::custom(
                "edn: negative hex integers are not supported",
            ));
        }
        let value: u64 =
            u64::from_str_radix(hex, 16).map_err(|_| Error::custom("edn: invalid hex integer"))?;
        return Ok(Number::U64(value));
    }
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::custom("edn: not an integer"));
    }
    let magnitude: u128 = body
        .parse()
        .map_err(|_| Error::custom("edn: integer overflow"))?;
    if negative {
        if magnitude <= (i64::MAX as u128) + 1 {
            if magnitude == (i64::MAX as u128) + 1 {
                Ok(Number::I64(i64::MIN))
            } else {
                Ok(Number::I64(-(magnitude as i64)))
            }
        } else {
            Ok(Number::I128(-(magnitude as i128)))
        }
    } else if magnitude <= u64::MAX as u128 {
        Ok(Number::U64(magnitude as u64))
    } else {
        Ok(Number::U128(magnitude))
    }
}

/// Parse an EDN document into a [`Value`] tree.
fn parse_edn(input: &[u8]) -> Result<Value> {
    let mut parser = EdnParser::new(input);
    let value = parser.parse_value()?;
    parser.skip_ws()?;
    if parser.pos < input.len() {
        return Err(parser.err("trailing data after value"));
    }
    Ok(value)
}

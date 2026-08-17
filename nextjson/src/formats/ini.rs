//! INI codec (Windows-style configuration text).
//!
//! Document-shaped text format: a headerless "global" section followed by
//! named `[section]` blocks of `key = value` lines.
//!
//! - Comments: `;` and `#` run to end of line.
//! - Values are stringified scalars (numbers / booleans keep their textual
//!   form); strings may be quoted with `'` (literal) or `"` (with `\\`,
//!   `\"`, `\n`, `\t`, `\r` escapes).
//! - The JSON model maps to INI as: a top-level object's scalar entries live
//!   in the global section; its object entries become `[section]` blocks.
//! - Arrays, `null` and nested sections are not representable and are
//!   rejected honestly.
//! - Repeated keys use the last value (common INI semantics).
//!
//! On encode, string values that look numeric or boolean are quoted so the
//! round-trip is unambiguous; on decode, unquoted values are type-guessed
//! back (`true`/`false`, integers, floats) while quoted values stay strings.
//!
//! Decode parses the whole document into a [`Value`] first (document-shaped),
//! then serves the unified event stream from it — the same pattern as TOML.

use alloc::string::{String, ToString};
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

/// INI format marker.
#[derive(Clone, Copy, Debug)]
pub struct Ini;

/// Document-decoded INI decoder: parses the whole document into a [`Value`]
/// and serves the unified event stream from it.
pub type IniDecoder<'de> = tree::TreeDecoder<'de>;

impl Format for Ini {
    const NAME: &'static str = "ini";
    const MIME: &'static str = "text/plain";
    const EXTENSIONS: &'static [&'static str] = &["ini", "cfg", "conf"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = IniEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.finish()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let value = parse_ini(input)?;
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value)?);
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Encoder (collect into Value, emit at the end)
// ---------------------------------------------------------------------------

/// INI encoder that collects one event stream and emits it on [`finish`](Self::finish).
pub struct IniEncoder<W: Write> {
    writer: W,
    collector: tree::CollectEncoder,
}

impl<W: Write> IniEncoder<W> {
    /// Create an INI encoder over `writer`.
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
        emit_ini(&root, &mut out)?;
        self.writer.write_all(&out)?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

impl<W: Write> crate::ser::FormatEncoder for IniEncoder<W> {
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

/// Stringify a scalar [`Value`] for an INI value slot.
fn scalar_text(v: &Value) -> Result<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => tree::number_string(n),
        Value::Bool(b) => Ok(b.to_string()),
        other => Err(Error::custom(alloc::format!(
            "ini: {other:?} is not representable as a scalar value"
        ))),
    }
}

/// Whether an unquoted string would be type-guessed into a non-string
/// (number/boolean) on decode. A `Value::String` that looks numeric must be
/// quoted on encode so the round-trip is unambiguous; genuine `Number` /
/// `Bool` values are written bare so they can be guessed back.
fn looks_non_string(value: &str) -> bool {
    if matches!(value, "true" | "false") {
        return true;
    }
    if parse_ini_int(value).is_ok() {
        return true;
    }
    value.contains(['.', 'e', 'E'])
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        && tree::parse_float(value).is_some()
}

/// Escape an INI value: quote only when necessary. `from_string` says the
/// text originated from a `Value::String`; strings that look numeric/boolean
/// must be quoted so the decoder guesses the string type back.
fn emit_value(out: &mut Vec<u8>, value: &str, from_string: bool) {
    let needs_quotes = value.contains(['=', ';', '#', '\n', '\r'])
        || value.starts_with(char::is_whitespace)
        || value.ends_with(char::is_whitespace)
        || (from_string && looks_non_string(value));
    if needs_quotes {
        out.push(b'"');
        for &b in value.as_bytes() {
            match b {
                b'\\' => out.extend_from_slice(b"\\\\"),
                b'"' => out.extend_from_slice(b"\\\""),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\t' => out.extend_from_slice(b"\\t"),
                b'\r' => out.extend_from_slice(b"\\r"),
                other => out.push(other),
            }
        }
        out.push(b'"');
    } else {
        out.extend_from_slice(value.as_bytes());
    }
}

/// Emit the collected Value tree as INI text.
fn emit_ini(root: &Value, out: &mut Vec<u8>) -> Result<()> {
    let map = root
        .as_object()
        .ok_or_else(|| Error::custom("ini: root must be an object (a table)"))?;
    // First pass: global-section scalars.
    for (key, value) in map.iter() {
        if matches!(value, Value::Object(_)) {
            continue;
        }
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(b" = ");
        emit_value(out, &scalar_text(value)?, matches!(value, Value::String(_)));
        out.push(b'\n');
    }
    // Second pass: `[section]` blocks.
    for (key, value) in map.iter() {
        let Value::Object(section) = value else {
            continue;
        };
        out.push(b'[');
        out.extend_from_slice(key.as_bytes());
        out.push(b']');
        out.push(b'\n');
        for (skey, svalue) in section.iter() {
            out.extend_from_slice(skey.as_bytes());
            out.extend_from_slice(b" = ");
            emit_value(
                out,
                &scalar_text(svalue)?,
                matches!(svalue, Value::String(_)),
            );
            out.push(b'\n');
        }
        out.push(b'\n');
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Decoder (parse into a Value tree)
// ---------------------------------------------------------------------------

/// Strip an INI comment (`;` or `#` outside a quoted region).
fn strip_comment(line: &[u8]) -> &[u8] {
    let mut in_single = false;
    let mut in_double = false;
    for (i, &b) in line.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b';' | b'#' if !in_single && !in_double => return &line[..i],
            _ => {}
        }
    }
    line
}

fn trim(mut line: &[u8]) -> &[u8] {
    while let Some((first, rest)) = line.split_first() {
        if first.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = line.split_last() {
        if last.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    line
}

/// Parse a quoted string value, returning the raw text.
///
/// `'...'` is literal; `"..."` supports `\\`, `\"`, `\n`, `\t`, `\r`.
fn parse_quoted(raw: &[u8], quote: u8) -> Result<String> {
    let mut out = Vec::with_capacity(raw.len());
    if quote == b'\'' {
        return String::from_utf8(raw.to_vec())
            .map_err(|_| Error::custom("ini: invalid utf-8 in literal string"));
    }
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if b == b'\\' {
            i += 1;
            let esc = *raw
                .get(i)
                .ok_or_else(|| Error::custom("ini: truncated escape"))?;
            match esc {
                b'\\' => out.push(b'\\'),
                b'"' => out.push(b'"'),
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                other => {
                    return Err(Error::custom(alloc::format!(
                        "ini: unsupported escape \\{}",
                        other as char
                    )));
                }
            }
            i += 1;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| Error::custom("ini: invalid utf-8 in value"))
}

/// Classify an unquoted INI value into a typed [`Value`].
///
/// INI is a stringly-typed format, but the JSON data model is typed, so
/// round-tripping `nextjson`-produced INI (where numbers/booleans are
/// written as their plain text form) requires guessing the type back.
/// Unquoted values are therefore parsed as `true`/`false`, then as an
/// integer, then as a float, and fall back to a string otherwise. Quoted
/// values always stay strings.
fn classify_value(raw: &[u8]) -> Value {
    let Ok(text) = core::str::from_utf8(raw) else {
        return Value::String(String::from_utf8_lossy(raw).into_owned());
    };
    match text {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    // Integral value? (kept within the JSON number model.)
    if let Ok(n) = parse_ini_int(text) {
        return Value::Number(n);
    }
    // Float value?
    if text.contains(['.', 'e', 'E'])
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        if let Some(v) = tree::parse_float(text) {
            return Value::Number(Number::F64(v));
        }
    }
    Value::String(text.to_string())
}

/// Parse a decimal integer (optionally signed) into a [`Number`].
fn parse_ini_int(text: &str) -> Result<Number> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return Err(Error::custom("ini: not an integer"));
    }
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::custom("ini: not an integer"));
    }
    let magnitude: u128 = body
        .parse()
        .map_err(|_| Error::custom("ini: integer overflow"))?;
    if negative {
        if magnitude == (i64::MAX as u128) + 1 {
            Ok(Number::I64(i64::MIN))
        } else if magnitude <= i64::MAX as u128 {
            Ok(Number::I64(-(magnitude as i64)))
        } else {
            Ok(Number::I128(-(magnitude as i128)))
        }
    } else if magnitude <= u64::MAX as u128 {
        Ok(Number::U64(magnitude as u64))
    } else {
        Ok(Number::U128(magnitude))
    }
}

/// Parse one key/value line into `(key, value)`.
fn parse_pair(line: &[u8]) -> Result<(String, Value)> {
    let eq = line
        .iter()
        .position(|b| *b == b'=')
        .ok_or_else(|| Error::custom("ini: expected `key = value`"))?;
    let key = trim(&line[..eq]);
    let key = core::str::from_utf8(key).map_err(|_| Error::custom("ini: invalid utf-8 in key"))?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(Error::custom("ini: empty key"));
    }
    let value = trim(&line[eq + 1..]);
    // A quoted value is always a string; an unquoted value is type-guessed.
    let value = if value.len() >= 2
        && ((value[0] == b'"' && value[value.len() - 1] == b'"')
            || (value[0] == b'\'' && value[value.len() - 1] == b'\''))
    {
        Value::String(parse_quoted(&value[1..value.len() - 1], value[0])?)
    } else {
        classify_value(value)
    };
    Ok((key, value))
}

/// Parse an INI document into a [`Value`] tree.
///
/// Repeated sections merge; repeated keys within a section use the last
/// value (common INI semantics).
fn parse_ini(input: &[u8]) -> Result<Value> {
    let mut root: Map = Map::new();
    let mut current_name: Option<String> = None;
    let mut current: Map = Map::new();
    let mut lines = input.split(|b| *b == b'\n');
    for line in lines.by_ref() {
        let line = strip_comment(line);
        let line = trim(line);
        if line.is_empty() {
            continue;
        }
        if line[0] == b'[' {
            // Commit the previous section, then start (or reopen) a section.
            if let Some(name) = current_name.take() {
                match root.get_mut(&name) {
                    Some(Value::Object(existing)) => {
                        for (k, v) in core::mem::take(&mut current) {
                            existing.insert(k, v);
                        }
                    }
                    _ => {
                        root.insert(name, Value::Object(core::mem::take(&mut current)));
                    }
                }
            }
            let close = line
                .iter()
                .rposition(|b| *b == b']')
                .ok_or_else(|| Error::custom("ini: unterminated section header"))?;
            let name = trim(&line[1..close]);
            let name = core::str::from_utf8(name)
                .map_err(|_| Error::custom("ini: invalid utf-8 in section"))?;
            let name = name.trim();
            if name.is_empty() {
                return Err(Error::custom("ini: empty section name"));
            }
            current_name = Some(name.to_string());
            continue;
        }
        let (key, value) = parse_pair(line)?;
        match &current_name {
            None => {
                if root.insert(key, value).is_some() {
                    return Err(Error::custom("ini: duplicate key in global section"));
                }
            }
            Some(_) => {
                if current.insert(key, value).is_some() {
                    return Err(Error::custom("ini: duplicate key in section"));
                }
            }
        }
    }
    // Commit the final section.
    if let Some(name) = current_name {
        match root.get_mut(&name) {
            Some(Value::Object(existing)) => {
                for (k, v) in current {
                    existing.insert(k, v);
                }
            }
            _ => {
                root.insert(name, Value::Object(current));
            }
        }
    }
    Ok(Value::Object(root))
}

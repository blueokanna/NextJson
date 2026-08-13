//! TOML codec (v1.0 core subset).
//!
//! The decoder parses a core TOML document into a [`Value`] and serves the
//! unified event interface from it: `key = value` pairs, dotted keys,
//! `[table]` and `[[array-of-table]]` headers, basic and literal strings,
//! decimal integers, floats, booleans, arrays and inline tables. Multi-line
//! (`"""`/`'''`) strings, hex/octal/binary integers, `inf`/`nan` and
//! date-time values are outside this subset and rejected or treated as
//! strings. The encoder collects the event stream into a [`Value`] and emits
//! TOML when the root closes, because TOML is document-shaped (tables must
//! be emitted after their keys).

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::de::NsonDeserialize;
use crate::error::{Error, Result};
use crate::formats::tree;
use crate::formats::Format;
use crate::map::Map;
use crate::ser::NsonSerialize;
use crate::value::Value;
use crate::write::Write;

/// TOML format marker.
#[derive(Clone, Copy, Debug)]
pub struct Toml;

impl Format for Toml {
    const NAME: &'static str = "toml";
    const MIME: &'static str = "application/toml";
    const EXTENSIONS: &'static [&'static str] = &["toml"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = TomlEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.finish()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let value = parse_toml(input)?;
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value));
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Encoder (collect into Value, emit at the end)
// ---------------------------------------------------------------------------

/// TOML encoder that collects one event stream and emits it on [`finish`](Self::finish).
pub struct TomlEncoder<W: Write> {
    writer: W,
    collector: tree::CollectEncoder,
}

impl<W: Write> TomlEncoder<W> {
    /// Create a TOML encoder over `writer`.
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
        emit_toml(&root, &mut out, &mut Vec::new())?;
        self.writer.write_all(&out)?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

tree::impl_collecting_format_encoder!(TomlEncoder);

/// Emit a [`Value`] as TOML.
///
/// TOML is document-shaped: a valid document is a sequence of `key = value`
/// pairs (and table headers), so a bare scalar or array at the root has no
/// wire representation. Like BSON's "requires a top-level document", this is
/// rejected honestly instead of emitting bytes no conforming parser accepts.
fn emit_toml(value: &Value, out: &mut Vec<u8>, path: &mut Vec<String>) -> Result<()> {
    match value {
        Value::Object(map) => emit_table(map, out, path),
        _ => Err(Error::custom("toml: requires a top-level table")),
    }
}

fn emit_table(map: &Map, out: &mut Vec<u8>, path: &mut Vec<String>) -> Result<()> {
    // Split into scalar keys and sub-tables.
    let mut scalars: Vec<(String, Value)> = Vec::new();
    let mut tables: Vec<(String, Value)> = Vec::new();
    let mut arrays_of_tables: Vec<(String, Value)> = Vec::new();
    for (k, v) in map.iter() {
        match v {
            Value::Object(_) => tables.push((k.to_string(), v.clone())),
            Value::Array(items) if items.iter().all(|i| matches!(i, Value::Object(_))) => {
                arrays_of_tables.push((k.to_string(), v.clone()));
            }
            _ => scalars.push((k.to_string(), v.clone())),
        }
    }
    // Emit scalar keys first.
    for (k, v) in &scalars {
        out.extend_from_slice(basic_key(k).as_bytes());
        out.extend_from_slice(b" = ");
        emit_scalar(v, out)?;
        out.push(b'\n');
    }
    if !scalars.is_empty() && (!tables.is_empty() || !arrays_of_tables.is_empty()) {
        out.push(b'\n');
    }
    // Sub-tables.
    for (k, v) in &tables {
        path.push(k.clone());
        out.push(b'[');
        out.extend_from_slice(join_path(path).as_bytes());
        out.extend_from_slice(b"]\n");
        if let Value::Object(m) = v {
            emit_table(m, out, path)?
        }
        out.push(b'\n');
        path.pop();
    }
    for (k, v) in &arrays_of_tables {
        if let Value::Array(items) = v {
            for item in items {
                path.push(k.clone());
                out.extend_from_slice(b"[[");
                out.extend_from_slice(join_path(path).as_bytes());
                out.extend_from_slice(b"]]\n");
                if let Value::Object(m) = item {
                    emit_table(m, out, path)?;
                }
                out.push(b'\n');
                path.pop();
            }
        }
    }
    Ok(())
}

fn join_path(path: &[String]) -> String {
    path.iter()
        .map(|p| basic_key(p))
        .collect::<Vec<_>>()
        .join(".")
}

fn basic_key(key: &str) -> String {
    // Bare keys must be alphanumeric + `-`/`_`; otherwise quote.
    if !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        key.to_string()
    } else {
        let mut out = String::with_capacity(key.len() + 2);
        out.push('"');
        for c in key.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }
}

fn emit_scalar(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => Err(Error::custom("toml: no null type")),
        Value::Bool(b) => {
            out.extend_from_slice(if *b { b"true" } else { b"false" });
            Ok(())
        }
        Value::Number(n) => {
            out.extend_from_slice(tree::number_string(n).as_bytes());
            Ok(())
        }
        Value::String(s) => {
            out.push(b'"');
            for c in s.chars() {
                match c {
                    '"' => out.extend_from_slice(b"\\\""),
                    '\\' => out.extend_from_slice(b"\\\\"),
                    '\n' => out.extend_from_slice(b"\\n"),
                    '\t' => out.extend_from_slice(b"\\t"),
                    '\r' => out.extend_from_slice(b"\\r"),
                    c if (c as u32) < 0x20 => {
                        out.extend_from_slice(&alloc::format!("\\u{:04X}", c as u32).into_bytes());
                    }
                    _ => {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                }
            }
            out.push(b'"');
            Ok(())
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                emit_scalar(item, out)?;
            }
            out.push(b']');
            Ok(())
        }
        Value::Object(m) => {
            out.push(b'{');
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                out.extend_from_slice(basic_key(k).as_bytes());
                out.extend_from_slice(b" = ");
                emit_scalar(v, out)?;
            }
            out.push(b'}');
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder (TOML -> Value -> tokens)
// ---------------------------------------------------------------------------

/// TOML decoder serving the unified interface from a parsed [`Value`].
///
/// The parsed document is replayed through the shared
/// [`crate::formats::TreeDecoder`].
pub type TomlDecoder<'de> = tree::TreeDecoder<'de>;

// ---------------------------------------------------------------------------
// TOML parser
// ---------------------------------------------------------------------------

/// Parse a TOML document into a [`Value`] (root is always an object).
fn parse_toml(input: &[u8]) -> Result<Value> {
    let text = core::str::from_utf8(input).map_err(|_| Error::custom("toml: invalid utf-8"))?;
    let mut p = Parser {
        text,
        pos: 0,
        root: Map::new(),
        current: None, // (path segments, table)
        depth: 0,
    };
    p.parse_document()?;
    Ok(Value::Object(p.root))
}

/// Reference to the current table during TOML parsing.
#[derive(Clone)]
enum TableRef {
    /// A regular table at a dotted path.
    Path(Vec<String>),
    /// The newest element of an array of tables at a dotted path.
    ArrayElement(Vec<String>),
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
    root: Map,
    current: Option<TableRef>,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn parse_document(&mut self) -> Result<()> {
        loop {
            self.skip_ws_and_comments()?;
            // Skip statement separators (leading newlines, blank lines).
            while self.pos < self.text.len()
                && matches!(self.text.as_bytes()[self.pos], b'\n' | b'\r')
            {
                self.pos += 1;
            }
            self.skip_ws_and_comments()?;
            if self.pos >= self.text.len() {
                break;
            }
            let c = self.text.as_bytes()[self.pos];
            if c == b'[' {
                self.parse_table_header()?;
            } else {
                self.parse_key_value()?;
            }
            // Expect a newline or end.
            self.skip_ws_and_comments()?;
            if self.pos < self.text.len() {
                let b = self.text.as_bytes()[self.pos];
                if b != b'\n' && b != b'\r' {
                    return Err(Error::custom("toml: expected newline after statement"));
                }
                while self.pos < self.text.len()
                    && matches!(self.text.as_bytes()[self.pos], b'\n' | b'\r')
                {
                    self.pos += 1;
                }
            }
        }
        Ok(())
    }

    fn skip_ws_and_comments(&mut self) -> Result<()> {
        loop {
            while self.pos < self.text.len()
                && matches!(self.text.as_bytes()[self.pos], b' ' | b'\t' | b'\r')
            {
                self.pos += 1;
            }
            if self.pos < self.text.len() && self.text.as_bytes()[self.pos] == b'#' {
                while self.pos < self.text.len() && self.text.as_bytes()[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn parse_table_header(&mut self) -> Result<()> {
        let array = self.text.as_bytes().get(self.pos + 1) == Some(&b'[');
        self.pos += if array { 2 } else { 1 };
        let mut segments = Vec::new();
        loop {
            self.skip_ws_and_comments()?;
            segments.push(self.parse_key_segment()?);
            self.skip_ws_and_comments()?;
            match self.text.as_bytes().get(self.pos) {
                Some(b'.') => {
                    self.pos += 1;
                }
                Some(b']') if !array => {
                    self.pos += 1;
                    break;
                }
                Some(b']') if array && self.text.as_bytes().get(self.pos + 1) == Some(&b']') => {
                    self.pos += 2;
                    break;
                }
                _ => return Err(Error::custom("toml: malformed table header")),
            }
        }
        if array {
            self.table_at_array(&segments)?;
            self.current = Some(TableRef::ArrayElement(segments));
        } else {
            self.table_at(&segments, true)?;
            self.current = Some(TableRef::Path(segments));
        }
        Ok(())
    }

    /// Navigate to (and optionally create) the table at `path`.
    ///
    /// An array-of-tables segment descends into its newest element, matching
    /// TOML's `[a.b]` after `[[a]]`. Redefining a table (`[a]` twice, or
    /// `[a]` after `a = 1`) is an error per the TOML spec.
    fn table_at(&mut self, path: &[String], create: bool) -> Result<&mut Map> {
        let mut map = &mut self.root;
        for (i, seg) in path.iter().enumerate() {
            let is_last = i + 1 == path.len();
            if is_last && create && map.contains_key(seg) {
                return Err(Error::custom(alloc::format!(
                    "toml: table `{}` is already defined",
                    path.join(".")
                )));
            }
            if map.get(seg).is_none() && create {
                map.insert(seg.to_string(), Value::Object(Map::new()));
            }
            map = match map.get_mut(seg) {
                Some(Value::Object(m)) => m,
                Some(Value::Array(arr)) => match arr.last_mut() {
                    Some(Value::Object(m)) => m,
                    _ => return Err(Error::custom("toml: not a table in array")),
                },
                _ => return Err(Error::custom("toml: key is not a table")),
            };
        }
        Ok(map)
    }

    /// Navigate to the array at `path`, appending a fresh table element, and
    /// return its map.
    fn table_at_array(&mut self, path: &[String]) -> Result<&mut Map> {
        let mut map = &mut self.root;
        for (i, seg) in path.iter().enumerate() {
            if i + 1 == path.len() {
                if map.get(seg).is_none() {
                    map.insert(seg.to_string(), Value::Array(Vec::new()));
                }
                let arr = match map.get_mut(seg) {
                    Some(Value::Array(a)) => a,
                    _ => return Err(Error::custom("toml: not an array of tables")),
                };
                arr.push(Value::Object(Map::new()));
                return match arr.last_mut() {
                    Some(Value::Object(m)) => Ok(m),
                    _ => Err(Error::custom("toml: not a table")),
                };
            }
            if map.get(seg).is_none() {
                map.insert(seg.to_string(), Value::Object(Map::new()));
            }
            map = match map.get_mut(seg) {
                Some(Value::Object(m)) => m,
                _ => return Err(Error::custom("toml: not a table")),
            };
        }
        Err(Error::custom("toml: empty array-of-tables path"))
    }

    /// Return the map of the newest element of the array at `path`.
    fn table_at_array_last(&mut self, path: &[String]) -> Result<&mut Map> {
        let mut map = &mut self.root;
        for (i, seg) in path.iter().enumerate() {
            if i + 1 == path.len() {
                let arr = match map.get_mut(seg) {
                    Some(Value::Array(a)) => a,
                    _ => return Err(Error::custom("toml: not an array of tables")),
                };
                return match arr.last_mut() {
                    Some(Value::Object(m)) => Ok(m),
                    _ => Err(Error::custom("toml: empty array of tables")),
                };
            }
            map = match map.get_mut(seg) {
                Some(Value::Object(m)) => m,
                _ => return Err(Error::custom("toml: not a table")),
            };
        }
        Err(Error::custom("toml: empty array-of-tables path"))
    }

    fn parse_key_value(&mut self) -> Result<()> {
        let key = self.parse_dotted_key()?;
        self.skip_ws_and_comments()?;
        if self.text.as_bytes().get(self.pos) != Some(&b'=') {
            return Err(Error::custom("toml: expected '=' after key"));
        }
        self.pos += 1;
        self.skip_ws_and_comments()?;
        let value = self.parse_value()?;
        // Insert into the current table (path cloned to end the borrow).
        let current = self.current.clone();
        match current {
            Some(TableRef::Path(path)) => {
                let map = self.table_at(&path, false)?;
                insert_dotted(map, &key, value)
            }
            Some(TableRef::ArrayElement(path)) => {
                let map = self.table_at_array_last(&path)?;
                insert_dotted(map, &key, value)
            }
            None => {
                let map = &mut self.root;
                insert_dotted(map, &key, value)
            }
        }
    }

    fn parse_dotted_key(&mut self) -> Result<Vec<String>> {
        let mut parts = vec![self.parse_key_segment()?];
        loop {
            self.skip_ws_and_comments()?;
            if self.text.as_bytes().get(self.pos) == Some(&b'.') {
                self.pos += 1;
                parts.push(self.parse_key_segment()?);
            } else {
                break;
            }
        }
        Ok(parts)
    }

    fn parse_key_segment(&mut self) -> Result<String> {
        self.skip_ws_and_comments()?;
        let b = self.text.as_bytes().get(self.pos).copied();
        match b {
            Some(b'"') => self.parse_basic_string(),
            Some(b'\'') => self.parse_literal_string(),
            _ => {
                let start = self.pos;
                while self.pos < self.text.len() {
                    let c = self.text.as_bytes()[self.pos];
                    if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                if self.pos == start {
                    return Err(Error::custom("toml: empty key"));
                }
                Ok(self.text[start..self.pos].to_string())
            }
        }
    }

    fn parse_value(&mut self) -> Result<Value> {
        self.skip_ws_and_comments()?;
        let b = self.text.as_bytes().get(self.pos).copied().unwrap_or(0);
        match b {
            b'"' => {
                if self.text.as_bytes().get(self.pos + 1) == Some(&b'"')
                    && self.text.as_bytes().get(self.pos + 2) == Some(&b'"')
                {
                    Ok(Value::from(self.parse_multi_basic_string()?))
                } else {
                    Ok(Value::from(self.parse_basic_string()?))
                }
            }
            b'\'' => {
                if self.text.as_bytes().get(self.pos + 1) == Some(&b'\'')
                    && self.text.as_bytes().get(self.pos + 2) == Some(&b'\'')
                {
                    Ok(Value::from(self.parse_multi_literal_string()?))
                } else {
                    Ok(Value::from(self.parse_literal_string()?))
                }
            }
            b'[' | b'{' => {
                if self.depth >= 128 {
                    return Err(Error::custom("toml: nesting limit exceeded"));
                }
                self.depth += 1;
                let value = if b == b'[' {
                    self.parse_array()
                } else {
                    self.parse_inline_table()
                };
                self.depth -= 1;
                value
            }
            b't' => {
                self.expect_lit(b"true")?;
                Ok(Value::from(true))
            }
            b'f' => {
                self.expect_lit(b"false")?;
                Ok(Value::from(false))
            }
            b'+' | b'-' | b'0'..=b'9' => self.parse_number_or_date(),
            _ => Err(Error::custom("toml: unexpected value")),
        }
    }

    fn parse_number_or_date(&mut self) -> Result<Value> {
        let start = self.pos;
        while self.pos < self.text.len() {
            let c = self.text.as_bytes()[self.pos];
            if c.is_ascii_alphanumeric()
                || matches!(c, b'+' | b'-' | b'.' | b'_' | b':' | b'T' | b'Z' | b' ')
            {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = self.text[start..self.pos].trim_end();
        // Date-time or date: keep as string.
        if raw.contains('-') && (raw.contains(':') || raw.len() >= 10) && !is_number(raw) {
            return Ok(Value::from(raw.to_string()));
        }
        let clean = raw.replace('_', "");
        if let Ok(v) = clean.parse::<i64>() {
            return Ok(Value::from(v));
        }
        if let Ok(v) = clean.parse::<u64>() {
            return Ok(Value::from(v));
        }
        if let Ok(v) = clean.parse::<f64>() {
            return Ok(Value::from(v));
        }
        Err(Error::custom(alloc::format!(
            "toml: invalid number {raw:?}"
        )))
    }

    fn parse_array(&mut self) -> Result<Value> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws_and_comments()?;
            match self.text.as_bytes().get(self.pos) {
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\n') | Some(b'\r') => {
                    self.pos += 1;
                    continue;
                }
                None => return Err(Error::custom("toml: unterminated array")),
                _ => {
                    items.push(self.parse_value()?);
                    self.skip_ws_and_comments()?;
                    match self.text.as_bytes().get(self.pos) {
                        Some(b',') => {
                            self.pos += 1;
                        }
                        Some(b']') => {}
                        _ => return Err(Error::custom("toml: expected ',' or ']'")),
                    }
                }
            }
        }
        Ok(Value::Array(items))
    }

    fn parse_inline_table(&mut self) -> Result<Value> {
        self.pos += 1; // '{'
        let mut map = Map::new();
        loop {
            self.skip_ws_and_comments()?;
            match self.text.as_bytes().get(self.pos) {
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                None => return Err(Error::custom("toml: unterminated inline table")),
                _ => {
                    let key = self.parse_dotted_key()?;
                    self.skip_ws_and_comments()?;
                    if self.text.as_bytes().get(self.pos) != Some(&b'=') {
                        return Err(Error::custom("toml: expected '=' in inline table"));
                    }
                    self.pos += 1;
                    let value = self.parse_value()?;
                    map.insert(key.join("."), value);
                    self.skip_ws_and_comments()?;
                    match self.text.as_bytes().get(self.pos) {
                        Some(b',') => {
                            self.pos += 1;
                        }
                        Some(b'}') => {}
                        _ => return Err(Error::custom("toml: expected ',' or '}'")),
                    }
                }
            }
        }
        Ok(Value::Object(map))
    }

    fn parse_basic_string(&mut self) -> Result<String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            if self.pos >= self.text.len() {
                return Err(Error::custom("toml: unterminated string"));
            }
            let b = self.text.as_bytes()[self.pos];
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.text.len() {
                        return Err(Error::custom("toml: unterminated escape"));
                    }
                    match self.text.as_bytes()[self.pos] {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let cp = self.read_hex(4)?;
                            out.push(
                                char::from_u32(cp)
                                    .ok_or_else(|| Error::custom("toml: invalid unicode escape"))?,
                            );
                        }
                        b'U' => {
                            let cp = self.read_hex(8)?;
                            out.push(
                                char::from_u32(cp)
                                    .ok_or_else(|| Error::custom("toml: invalid unicode escape"))?,
                            );
                        }
                        other => {
                            return Err(Error::custom(alloc::format!(
                                "toml: invalid escape '\\{}'",
                                other as char
                            )))
                        }
                    }
                    self.pos += 1;
                }
                b'\n' => return Err(Error::custom("toml: newline in basic string")),
                _ => {
                    let len = utf8_len(b).ok_or_else(|| Error::custom("toml: invalid utf-8"))?;
                    let chunk = &self.text[self.pos..self.pos + len];
                    out.push_str(chunk);
                    self.pos += len;
                }
            }
        }
    }

    fn parse_literal_string(&mut self) -> Result<String> {
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.text.len() && self.text.as_bytes()[self.pos] != b'\'' {
            self.pos += 1;
        }
        if self.pos >= self.text.len() {
            return Err(Error::custom("toml: unterminated literal string"));
        }
        let s = self.text[start..self.pos].to_string();
        self.pos += 1;
        Ok(s)
    }

    /// Multi-line basic string (`"""..."""`), TOML 1.0.
    ///
    /// The newline immediately following the opening delimiter is trimmed;
    /// a backslash at the end of a line trims that newline plus all following
    /// whitespace (line-ending backslash); escapes behave like basic strings;
    /// trailing whitespace before the closing delimiter is trimmed.
    fn parse_multi_basic_string(&mut self) -> Result<String> {
        self.pos += 3; // opening `"""`
        self.skip_crlf();
        let mut out = String::new();
        loop {
            if self.pos + 3 <= self.text.len()
                && &self.text.as_bytes()[self.pos..self.pos + 3] == b"\"\"\""
            {
                self.pos += 3;
                break;
            }
            if self.pos >= self.text.len() {
                return Err(Error::custom("toml: unterminated multi-line string"));
            }
            let b = self.text.as_bytes()[self.pos];
            if b == b'\\' {
                self.pos += 1;
                if self.pos >= self.text.len() {
                    return Err(Error::custom("toml: unterminated escape"));
                }
                // Line-ending backslash: trim it and all following whitespace.
                let nb = self.text.as_bytes()[self.pos];
                if nb == b'\n'
                    || (nb == b'\r' && self.text.as_bytes().get(self.pos + 1) == Some(&b'\n'))
                {
                    if nb == b'\r' {
                        self.pos += 1;
                    }
                    self.pos += 1; // '\n'
                    while self.pos < self.text.len()
                        && matches!(self.text.as_bytes()[self.pos], b' ' | b'\t' | b'\n' | b'\r')
                    {
                        self.pos += 1;
                    }
                    continue;
                }
                match nb {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'u' => {
                        let cp = self.read_hex(4)?;
                        out.push(
                            char::from_u32(cp)
                                .ok_or_else(|| Error::custom("toml: invalid unicode escape"))?,
                        );
                    }
                    b'U' => {
                        let cp = self.read_hex(8)?;
                        out.push(
                            char::from_u32(cp)
                                .ok_or_else(|| Error::custom("toml: invalid unicode escape"))?,
                        );
                    }
                    other => {
                        return Err(Error::custom(alloc::format!(
                            "toml: invalid escape '\\{}'",
                            other as char
                        )))
                    }
                }
                self.pos += 1;
            } else {
                let len = utf8_len(b).ok_or_else(|| Error::custom("toml: invalid utf-8"))?;
                let chunk = &self.text[self.pos..self.pos + len];
                out.push_str(chunk);
                self.pos += len;
            }
        }
        // Trim trailing whitespace (spaces / tabs / newlines) before the
        // closing delimiter, per the TOML multi-line string rules.
        Ok(trim_whitespace_end(&out).to_string())
    }

    /// Multi-line literal string (`'''...'''`), TOML 1.0.
    ///
    /// No escapes; the newline immediately following the opening delimiter is
    /// trimmed; trailing whitespace before the closing delimiter is trimmed.
    fn parse_multi_literal_string(&mut self) -> Result<String> {
        self.pos += 3; // opening `'''`
        self.skip_crlf();
        let start = self.pos;
        loop {
            if self.pos + 3 <= self.text.len()
                && &self.text.as_bytes()[self.pos..self.pos + 3] == b"'''"
            {
                break;
            }
            if self.pos >= self.text.len() {
                return Err(Error::custom(
                    "toml: unterminated multi-line literal string",
                ));
            }
            self.pos += 1;
        }
        let s = self.text[start..self.pos].to_string();
        self.pos += 3;
        Ok(trim_whitespace_end(&s).to_string())
    }

    /// Skip a single CRLF / LF after an opening multi-line delimiter.
    fn skip_crlf(&mut self) {
        if self.pos < self.text.len() && self.text.as_bytes()[self.pos] == b'\r' {
            self.pos += 1;
        }
        if self.pos < self.text.len() && self.text.as_bytes()[self.pos] == b'\n' {
            self.pos += 1;
        }
    }

    fn read_hex(&mut self, n: usize) -> Result<u32> {
        let mut v: u32 = 0;
        for _ in 0..n {
            self.pos += 1;
            let b = self.text.as_bytes().get(self.pos).copied().unwrap_or(0);
            let d = crate::lex::hex_digit(b)
                .ok_or_else(|| Error::custom("toml: invalid hex escape"))?;
            v = v * 16 + d as u32;
        }
        Ok(v)
    }

    fn expect_lit(&mut self, lit: &[u8]) -> Result<()> {
        if self.text.len() - self.pos < lit.len()
            || &self.text.as_bytes()[self.pos..self.pos + lit.len()] != lit
        {
            return Err(Error::custom("toml: invalid literal"));
        }
        self.pos += lit.len();
        Ok(())
    }
}

/// Trim trailing ASCII whitespace (spaces, tabs, CR, LF) from a multi-line
/// string before its closing delimiter, per the TOML multi-line rules.
fn trim_whitespace_end(s: &str) -> &str {
    s.trim_end_matches([' ', '\t', '\n', '\r'])
}

/// Insert a (possibly dotted) key into a map, creating intermediate tables.
///
/// Duplicate keys are an error per the TOML spec, not a silent overwrite.
fn insert_dotted(map: &mut Map, key: &[String], value: Value) -> Result<()> {
    if key.len() == 1 {
        if map.contains_key(&key[0]) {
            return Err(Error::custom(alloc::format!(
                "toml: duplicate key `{}`",
                key[0]
            )));
        }
        map.insert(key[0].clone(), value);
        return Ok(());
    }
    let mut current = map;
    for (i, seg) in key.iter().enumerate() {
        let is_last = i + 1 == key.len();
        if is_last {
            if current.contains_key(seg) {
                return Err(Error::custom(alloc::format!(
                    "toml: duplicate key `{}`",
                    seg
                )));
            }
            current.insert(seg.clone(), value.clone());
            return Ok(());
        }
        if current.get(seg).is_none() {
            current.insert(seg.clone(), Value::Object(Map::new()));
        }
        current = match current.get_mut(seg) {
            Some(Value::Object(m)) => m,
            _ => return Err(Error::custom("toml: dotted key is not a table")),
        };
    }
    Ok(())
}

fn is_number(raw: &str) -> bool {
    raw.bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'_' | b'e' | b'E'))
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

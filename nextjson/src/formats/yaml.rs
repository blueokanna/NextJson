//! YAML codec (block and flow style subset).
//!
//! The encoder emits block-style YAML. The decoder accepts block mappings and
//! sequences (indentation-based), flow `{}` / `[]` collections, quoted and
//! plain scalars, booleans, `null` / `~`, and `#` comments. Anchors, aliases,
//! tags and multi-document streams are outside this subset and rejected.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::NsonDeserialize;
use crate::error::{Error, Result};
use crate::formats::tree;
use crate::formats::Format;
use crate::map::Map;
use crate::ser::NsonSerialize;
use crate::value::Value;
use crate::write::Write;

/// YAML format marker.
#[derive(Clone, Copy, Debug)]
pub struct Yaml;

impl Format for Yaml {
    const NAME: &'static str = "yaml";
    const MIME: &'static str = "application/yaml";
    const EXTENSIONS: &'static [&'static str] = &["yaml", "yml"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = YamlEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        encoder.finish()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let value = parse_yaml(input)?;
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value));
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Encoder (collect into Value, emit YAML)
// ---------------------------------------------------------------------------

/// YAML encoder that collects one event stream and emits it on [`finish`](Self::finish).
pub struct YamlEncoder<W: Write> {
    writer: W,
    collector: tree::CollectEncoder,
}

impl<W: Write> YamlEncoder<W> {
    /// Create a YAML encoder over `writer`.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            collector: tree::CollectEncoder::new(),
        }
    }

    /// Emit the collected value, flush, and return the writer.
    pub fn finish(mut self) -> Result<W> {
        let root = self.collector.take_root()?;
        let mut out = Vec::with_capacity(256);
        emit_yaml(&root, &mut out, 0)?;
        self.writer.write_all(&out)?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

tree::impl_collecting_format_encoder!(YamlEncoder);

/// Emit a [`Value`] as block-style YAML.
fn emit_yaml(value: &Value, out: &mut Vec<u8>, indent: usize) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter() {
                emit_mapping_entry(k, v, out, indent)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                write_indent(out, indent);
                out.extend_from_slice(b"-");
                emit_yaml_inline_or_block(item, out, indent)?;
                out.push(b'\n');
            }
            Ok(())
        }
        _ => {
            write_indent(out, indent);
            emit_scalar_yaml(value, out)?;
            out.push(b'\n');
            Ok(())
        }
    }
}

fn emit_mapping_entry(key: &str, value: &Value, out: &mut Vec<u8>, indent: usize) -> Result<()> {
    write_indent(out, indent);
    out.extend_from_slice(yaml_key(key).as_bytes());
    out.extend_from_slice(b":");
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.extend_from_slice(b" {}\n");
            } else {
                out.push(b'\n');
                emit_yaml(value, out, indent + 2)?;
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.extend_from_slice(b" []\n");
            } else {
                out.extend_from_slice(b"\n");
                for item in items {
                    write_indent(out, indent + 2);
                    out.extend_from_slice(b"-");
                    emit_yaml_inline_or_block(item, out, indent + 2)?;
                    out.push(b'\n');
                }
            }
        }
        _ => {
            out.push(b' ');
            emit_scalar_yaml(value, out)?;
            out.push(b'\n');
        }
    }
    Ok(())
}

/// Emit `- <value>` after the dash (used by sequence items).
fn emit_yaml_inline_or_block(value: &Value, out: &mut Vec<u8>, indent: usize) -> Result<()> {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.extend_from_slice(b" {}\n");
            } else {
                out.extend_from_slice(b"\n");
                for (k, v) in map.iter() {
                    write_indent(out, indent + 2);
                    out.extend_from_slice(yaml_key(k).as_bytes());
                    out.extend_from_slice(b":");
                    emit_yaml_value_tail(v, out, indent + 2)?;
                }
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.extend_from_slice(b" []\n");
            } else {
                out.extend_from_slice(b"\n");
                for item in items {
                    write_indent(out, indent + 2);
                    out.extend_from_slice(b"-");
                    emit_yaml_inline_or_block(item, out, indent + 2)?;
                    out.push(b'\n');
                }
            }
        }
        _ => {
            out.push(b' ');
            emit_scalar_yaml(value, out)?;
            out.push(b'\n');
        }
    }
    Ok(())
}

fn emit_yaml_value_tail(value: &Value, out: &mut Vec<u8>, indent: usize) -> Result<()> {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.extend_from_slice(b" {}\n");
            } else {
                out.push(b'\n');
                for (k, v) in map.iter() {
                    write_indent(out, indent + 2);
                    out.extend_from_slice(yaml_key(k).as_bytes());
                    out.extend_from_slice(b":");
                    emit_yaml_value_tail(v, out, indent + 2)?;
                }
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.extend_from_slice(b" []\n");
            } else {
                out.extend_from_slice(b"\n");
                for item in items {
                    write_indent(out, indent + 2);
                    out.extend_from_slice(b"-");
                    emit_yaml_inline_or_block(item, out, indent + 2)?;
                    out.push(b'\n');
                }
            }
        }
        _ => {
            out.push(b' ');
            emit_scalar_yaml(value, out)?;
            out.push(b'\n');
        }
    }
    Ok(())
}

fn write_indent(out: &mut Vec<u8>, indent: usize) {
    for _ in 0..indent {
        out.extend_from_slice(b"  ");
    }
}

fn yaml_key(key: &str) -> String {
    // Keys that look like numbers or contain specials are quoted.
    if key.is_empty()
        || key.parse::<f64>().is_ok()
        || key.bytes().any(|b| {
            matches!(
                b,
                b':' | b'#' | b'{' | b'}' | b'[' | b']' | b',' | b'\n' | b'\t'
            )
        })
    {
        let mut out = String::with_capacity(key.len() + 2);
        out.push('\'');
        out.push_str(&key.replace('\'', "''"));
        out.push('\'');
        out
    } else {
        key.to_string()
    }
}

fn emit_scalar_yaml(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => out.extend_from_slice(tree::number_string(n).as_bytes()),
        Value::String(s) => {
            if s.is_empty() {
                out.extend_from_slice(b"\"\"");
            } else if s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
                && !is_yaml_special(s)
            {
                out.extend_from_slice(s.as_bytes());
            } else {
                out.push(b'\'');
                out.extend_from_slice(s.replace('\'', "''").as_bytes());
                out.push(b'\'');
            }
        }
        _ => {
            return Err(Error::custom(
                "yaml: unexpected container in scalar position",
            ))
        }
    }
    Ok(())
}

fn is_yaml_special(s: &str) -> bool {
    matches!(
        s,
        "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
    )
}

// ---------------------------------------------------------------------------
// Decoder (YAML -> Value -> tokens)
// ---------------------------------------------------------------------------

/// YAML decoder serving the unified interface from a parsed [`Value`].
///
/// The parsed document is replayed through the shared
/// [`crate::formats::TreeDecoder`].
pub type YamlDecoder<'de> = tree::TreeDecoder<'de>;

// ---------------------------------------------------------------------------
// YAML parser (subset)
// ---------------------------------------------------------------------------

/// Parse a YAML document into a [`Value`].
fn parse_yaml(input: &[u8]) -> Result<Value> {
    let text = core::str::from_utf8(input).map_err(|_| Error::custom("yaml: invalid utf-8"))?;
    let lines = split_lines(text);
    let mut p = YamlParser {
        lines,
        pos: 0,
        depth: 0,
    };
    p.skip_blank()?;
    // Optional document marker.
    if p.current()
        .is_some_and(|l| l.trim_start().starts_with("---"))
    {
        p.pos += 1;
        p.skip_blank()?;
    }
    if p.pos >= p.lines.len() {
        return Ok(Value::Null);
    }
    let indent = leading_spaces(
        p.current_line()
            .ok_or_else(|| Error::custom("yaml: empty"))?,
    )?;
    let value = p.parse_node(indent)?;
    p.skip_blank()?;
    if p.pos < p.lines.len() {
        return Err(Error::custom("yaml: trailing content after document"));
    }
    Ok(value)
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect()
}

struct YamlParser {
    lines: Vec<String>,
    pos: usize,
    depth: u32,
}

impl YamlParser {
    fn current_line(&self) -> Option<&str> {
        self.lines.get(self.pos).map(|s| s.as_str())
    }

    fn current(&self) -> Option<&str> {
        self.current_line()
    }

    fn skip_blank(&mut self) -> Result<()> {
        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            let trimmed = line.trim_start();
            // Blank lines and full-line comments are skipped. `---` is NOT:
            // it is only a document-start marker at the very top (handled by
            // `parse_yaml`), and an indented `---: x` is a legal mapping key.
            if trimmed.is_empty() || trimmed.starts_with('#') {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Parse a node at the given indentation.
    fn parse_node(&mut self, indent: usize) -> Result<Value> {
        if self.depth >= 128 {
            return Err(Error::custom("yaml: nesting limit exceeded"));
        }
        self.depth += 1;
        let result = self.parse_node_inner(indent);
        self.depth -= 1;
        result
    }

    fn parse_node_inner(&mut self, indent: usize) -> Result<Value> {
        self.skip_blank()?;
        let line = self
            .current()
            .ok_or_else(|| Error::custom("yaml: unexpected end of input"))?;
        let stripped = line.trim_start();
        if leading_spaces(line)? != indent {
            return Err(Error::custom("yaml: inconsistent indentation"));
        }
        if stripped.starts_with('{') || stripped.starts_with('[') {
            let value = parse_scalar(stripped)?;
            self.pos += 1;
            return Ok(value);
        }
        if stripped.starts_with("- ") || stripped == "-" {
            self.parse_sequence(indent)
        } else if contains_colon_separator(stripped) {
            self.parse_mapping(indent)
        } else {
            let value = parse_scalar(stripped)?;
            self.pos += 1;
            Ok(value)
        }
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<Value> {
        let mut items = Vec::new();
        loop {
            self.skip_blank()?;
            let line = match self.current() {
                Some(l) => l.to_string(),
                None => break,
            };
            if leading_spaces(&line)? != indent {
                break;
            }
            let stripped = line.trim_start();
            if !stripped.starts_with('-') {
                break;
            }
            let rest = stripped[1..].trim_start();
            self.pos += 1;
            if rest.is_empty() {
                // `-` followed by an indented block.
                let child_indent = self
                    .current()
                    .map(leading_spaces)
                    .transpose()?
                    .unwrap_or(indent + 1);
                if child_indent > indent {
                    items.push(self.parse_node(child_indent)?);
                } else {
                    items.push(Value::Null);
                }
            } else if contains_colon_separator(rest) {
                // `- key: value` — inline mapping item.
                let map = self.parse_mapping_from_rest(indent, rest)?;
                items.push(Value::Object(map));
            } else {
                let value = parse_scalar(rest)?;
                items.push(value);
                // A following line indented deeper is part of a nested block
                // attached to this item (e.g. `- name: x` handled above).
            }
        }
        Ok(Value::Array(items))
    }

    fn parse_mapping(&mut self, indent: usize) -> Result<Value> {
        let mut map = Map::new();
        loop {
            self.skip_blank()?;
            let line = match self.current() {
                Some(l) => l.to_string(),
                None => break,
            };
            if leading_spaces(&line)? != indent {
                break;
            }
            let stripped = line.trim_start();
            if stripped.starts_with('-') && indent == 0 {
                break; // a sequence at the same indent ends the mapping
            }
            if !contains_colon_separator(stripped) {
                break;
            }
            self.pos += 1;
            let (key, rest) = split_key_value(stripped);
            let key = parse_scalar(key)?.as_string_lossy();
            let rest = rest.trim_start();
            if rest.is_empty() {
                let child_indent = self
                    .current()
                    .map(leading_spaces)
                    .transpose()?
                    .unwrap_or(indent + 1);
                if child_indent > indent {
                    map.insert(key, self.parse_node(child_indent)?);
                } else {
                    map.insert(key, Value::Null);
                }
            } else {
                let value = parse_scalar(rest)?;
                map.insert(key, value);
            }
        }
        Ok(Value::Object(map))
    }

    /// Parse `- key: value` (the `- ` already consumed).
    fn parse_mapping_from_rest(&mut self, indent: usize, rest: &str) -> Result<Map> {
        let mut map = Map::new();
        let (key, value_rest) = split_key_value(rest);
        let key = parse_scalar(key)?.as_string_lossy();
        let value_rest = value_rest.trim_start();
        if value_rest.is_empty() {
            let child_indent = self
                .current()
                .map(leading_spaces)
                .transpose()?
                .unwrap_or(indent + 2);
            if child_indent > indent {
                map.insert(key, self.parse_node(child_indent)?);
            } else {
                map.insert(key, Value::Null);
            }
        } else {
            map.insert(key, parse_scalar(value_rest)?);
        }
        // Additional `key: value` lines at deeper indent.
        loop {
            self.skip_blank()?;
            let line = match self.current() {
                Some(l) => l.to_string(),
                None => break,
            };
            let line_indent = leading_spaces(&line)?;
            if line_indent <= indent {
                break;
            }
            let stripped = line.trim_start();
            if stripped.starts_with('-') || !contains_colon_separator(stripped) {
                break;
            }
            self.pos += 1;
            let (k, r) = split_key_value(stripped);
            let k = parse_scalar(k)?.as_string_lossy();
            let r = r.trim_start();
            if r.is_empty() {
                // `key:` with an indented block: recurse into it, like
                // `parse_mapping`, instead of flattening its children up.
                let child_indent = self
                    .current()
                    .map(leading_spaces)
                    .transpose()?
                    .unwrap_or(line_indent + 1);
                if child_indent > line_indent {
                    map.insert(k, self.parse_node(child_indent)?);
                } else {
                    map.insert(k, Value::Null);
                }
            } else {
                map.insert(k, parse_scalar(r)?);
            }
        }
        Ok(map)
    }
}

/// Whether a line contains a `key: value` colon (not a `:` inside quotes).
fn contains_colon_separator(line: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ':' if !in_single && !in_double => {
                let next = chars.get(i + 1).copied();
                if next == Some(' ') || next.is_none() {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

fn split_key_value(line: &str) -> (&str, &str) {
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ':' if !in_single && !in_double => {
                return (&line[..i], &line[i + 1..]);
            }
            _ => {}
        }
        i += 1;
    }
    (line, "")
}

fn leading_spaces(line: &str) -> Result<usize> {
    Ok(line.len() - line.trim_start().len())
}

/// Parse a scalar string into a [`Value`].
fn parse_scalar(raw: &str) -> Result<Value> {
    parse_scalar_at_depth(raw, 0)
}

fn parse_scalar_at_depth(raw: &str, depth: u32) -> Result<Value> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Value::Null);
    }
    // Strip trailing comment.
    let raw = strip_comment(raw);
    let raw = raw.trim_end();
    // Quoted strings.
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return Ok(Value::from(unescape_double(&raw[1..raw.len() - 1])?));
    }
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return Ok(Value::from(raw[1..raw.len() - 1].replace("''", "'")));
    }
    match raw {
        "null" | "~" | "Null" | "NULL" => return Ok(Value::Null),
        "true" | "True" | "TRUE" => return Ok(Value::from(true)),
        "false" | "False" | "FALSE" => return Ok(Value::from(false)),
        _ => {}
    }
    // Flow collections.
    if raw.starts_with('{') {
        return parse_flow_map(raw, depth);
    }
    if raw.starts_with('[') {
        return parse_flow_seq(raw, depth);
    }
    // Numbers.
    if let Ok(v) = raw.parse::<i64>() {
        return Ok(Value::from(v));
    }
    if let Ok(v) = raw.parse::<u64>() {
        return Ok(Value::from(v));
    }
    if let Ok(v) = raw.parse::<f64>() {
        return Ok(Value::from(v));
    }
    Ok(Value::from(raw.to_string()))
}

fn strip_comment(line: &str) -> &str {
    // `#` preceded by whitespace (or at the start) starts a comment, but only
    // outside quoted scalars: `"hello # world"` must keep its content.
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single
                && !in_double
                && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') =>
            {
                return &line[..i];
            }
            _ => {}
        }
        i += 1;
    }
    line
}

fn unescape_double(s: &str) -> Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => return Err(Error::custom("yaml: unterminated escape")),
        }
    }
    Ok(out)
}

fn parse_flow_map(raw: &str, depth: u32) -> Result<Value> {
    if depth >= 128 {
        return Err(Error::custom("yaml: nesting limit exceeded"));
    }
    let inner = raw
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| Error::custom("yaml: malformed flow map"))?;
    let mut map = Map::new();
    if inner.trim().is_empty() {
        return Ok(Value::Object(map));
    }
    for part in split_flow(inner) {
        let (k, v) = split_key_value(part);
        let key = parse_scalar_at_depth(k.trim(), depth + 1)?.as_string_lossy();
        let value = parse_scalar_at_depth(v.trim(), depth + 1)?;
        map.insert(key, value);
    }
    Ok(Value::Object(map))
}

fn parse_flow_seq(raw: &str, depth: u32) -> Result<Value> {
    if depth >= 128 {
        return Err(Error::custom("yaml: nesting limit exceeded"));
    }
    let inner = raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| Error::custom("yaml: malformed flow sequence"))?;
    let mut items = Vec::new();
    if inner.trim().is_empty() {
        return Ok(Value::Array(items));
    }
    for part in split_flow(inner) {
        items.push(parse_scalar_at_depth(part.trim(), depth + 1)?);
    }
    Ok(Value::Array(items))
}

/// Split a flow collection on top-level commas (respecting nesting/quotes).
fn split_flow(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '{' | '[' if !in_single && !in_double => depth += 1,
            '}' | ']' if !in_single && !in_double => depth -= 1,
            ',' if depth == 0 && !in_single && !in_double => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

impl Value {
    /// The string content of a scalar (for keys).
    fn as_string_lossy(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Number(n) => tree::number_string(n),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".to_string(),
            _ => alloc::format!("{self}"),
        }
    }
}

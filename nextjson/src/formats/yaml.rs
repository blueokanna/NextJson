//! YAML codec (block and flow style subset).
//!
//! The encoder emits block-style YAML. The decoder accepts block mappings and
//! sequences (indentation-based), flow `{}` / `[]` collections, quoted and
//! plain scalars, booleans, `null` / `~`, `#` comments, block scalars
//! (`|` / `>` with chomping and indentation indicators), and anchors
//! (`&name`) / aliases (`*name`) in block context (resolved by copying, so
//! self-referential documents fail with `unknown anchor`). Tags and
//! multi-document streams are outside this subset and rejected.

use alloc::collections::BTreeMap;
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
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value)?);
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
            if map.is_empty() {
                out.extend_from_slice(b"{}\n");
                return Ok(());
            }
            for (k, v) in map.iter() {
                emit_mapping_entry(k, v, out, indent)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.extend_from_slice(b"[]\n");
                return Ok(());
            }
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
        if key.bytes().any(|b| b < 0x20) {
            return yaml_double_quote(key);
        }
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
        Value::Number(n) => out.extend_from_slice(tree::number_string(n)?.as_bytes()),
        Value::String(s) => {
            if s.is_empty() {
                out.extend_from_slice(b"\"\"");
            } else if s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
                && !is_yaml_special(s)
            {
                out.extend_from_slice(s.as_bytes());
            } else if s.bytes().any(|b| b < 0x20) {
                // Single-quoted scalars fold line breaks and forbid control
                // characters, so strings containing them must use a
                // double-quoted scalar with escapes.
                out.extend_from_slice(yaml_double_quote(s).as_bytes());
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

/// Render `s` as a YAML double-quoted scalar whose escapes the decoder's
/// `unescape_double` understands: `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, and
/// `\uXXXX` for the remaining control characters. Non-ASCII characters stay
/// as raw UTF-8 (valid inside YAML double-quoted scalars).
fn yaml_double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}' => out.push_str("\\0"),
            c if (c as u32) < 0x20 => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let cp = c as u32;
                out.push_str("\\u00");
                out.push(HEX[((cp >> 4) & 0xF) as usize] as char);
                out.push(HEX[(cp & 0xF) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
        anchors: BTreeMap::new(),
        alias_budget: 1_000_000,
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
    // A document-end marker (`...`) is accepted after the single document.
    if p.current().is_some_and(|l| l.trim_start() == "...") {
        p.pos += 1;
        p.skip_blank()?;
    }
    if p.pos < p.lines.len() {
        let line = p.current_line().unwrap_or("");
        if line.trim_start().starts_with("---") {
            return Err(Error::custom(
                "yaml: multi-document streams are not supported",
            ));
        }
        return Err(Error::custom("yaml: trailing content after document"));
    }
    Ok(value)
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect()
}

/// Block-scalar trailing-newline policy (`|`, `|-`, `|+`, `>`...).
enum BlockChomp {
    /// Default: keep a single trailing newline.
    Clip,
    /// `-`: strip all trailing newlines.
    Strip,
    /// `+`: keep all trailing newlines.
    Keep,
}

struct YamlParser {
    lines: Vec<String>,
    pos: usize,
    depth: u32,
    /// Anchors (`&name value`) registered during parsing, referenced by
    /// later `*name` aliases. Values are copied on resolution.
    anchors: BTreeMap<String, Value>,
    /// Budget of nodes that may be produced by alias expansion. Aliases copy
    /// their anchored value, so a small document could otherwise reference a
    /// large anchor many times and amplify memory without bound.
    alias_budget: usize,
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
        if stripped.starts_with('|') || stripped.starts_with('>') {
            let header = stripped.to_string();
            self.pos += 1;
            return self.parse_block_scalar(indent, &header);
        }
        if stripped.starts_with('&') {
            let anchored = stripped.to_string();
            self.pos += 1;
            return self.parse_anchored(&anchored, indent);
        }
        if stripped.starts_with('*') {
            let alias = stripped.to_string();
            let value = self.resolve_alias(&alias)?;
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
            } else if rest.starts_with('{') || rest.starts_with('[') {
                // Flow collection item (`- {a: 1}` / `- [1, 2]`). Must be
                // checked before `contains_colon_separator`, which would
                // mistake the colons inside a flow map for a `key: value`
                // separator.
                let value = parse_scalar(rest)?;
                items.push(value);
            } else if rest.starts_with('|') || rest.starts_with('>') {
                items.push(self.parse_block_scalar(indent, rest)?);
            } else if rest.starts_with('&') {
                items.push(self.parse_anchored(rest, indent)?);
            } else if rest.starts_with('*') {
                items.push(self.resolve_alias(rest)?);
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
            if key == "<<" {
                let merged = if rest.is_empty() {
                    let child_indent = self
                        .current()
                        .map(leading_spaces)
                        .transpose()?
                        .unwrap_or(indent + 1);
                    if child_indent > indent {
                        self.parse_node(child_indent)?
                    } else {
                        Value::Null
                    }
                } else if rest.starts_with('|') || rest.starts_with('>') {
                    self.parse_block_scalar(indent, rest)?
                } else if rest.starts_with('&') {
                    self.parse_anchored(rest, indent)?
                } else if rest.starts_with('*') {
                    self.resolve_alias(rest)?
                } else {
                    parse_scalar(rest)?
                };
                merge_map(&mut map, merged)?;
                continue;
            }
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
            } else if rest.starts_with('|') || rest.starts_with('>') {
                map.insert(key, self.parse_block_scalar(indent, rest)?);
            } else if rest.starts_with('&') {
                map.insert(key, self.parse_anchored(rest, indent)?);
            } else if rest.starts_with('*') {
                map.insert(key, self.resolve_alias(rest)?);
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
        if key == "<<" {
            let merged = if value_rest.is_empty() {
                let child_indent = self
                    .current()
                    .map(leading_spaces)
                    .transpose()?
                    .unwrap_or(indent + 2);
                if child_indent > indent {
                    self.parse_node(child_indent)?
                } else {
                    Value::Null
                }
            } else if value_rest.starts_with('|') || value_rest.starts_with('>') {
                self.parse_block_scalar(indent, value_rest)?
            } else if value_rest.starts_with('&') {
                self.parse_anchored(value_rest, indent)?
            } else if value_rest.starts_with('*') {
                self.resolve_alias(value_rest)?
            } else {
                parse_scalar(value_rest)?
            };
            merge_map(&mut map, merged)?;
        } else if value_rest.is_empty() {
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
        } else if value_rest.starts_with('|') || value_rest.starts_with('>') {
            map.insert(key, self.parse_block_scalar(indent, value_rest)?);
        } else if value_rest.starts_with('&') {
            map.insert(key, self.parse_anchored(value_rest, indent)?);
        } else if value_rest.starts_with('*') {
            map.insert(key, self.resolve_alias(value_rest)?);
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
            if k == "<<" {
                let merged = if r.is_empty() {
                    let child_indent = self
                        .current()
                        .map(leading_spaces)
                        .transpose()?
                        .unwrap_or(line_indent + 1);
                    if child_indent > line_indent {
                        self.parse_node(child_indent)?
                    } else {
                        Value::Null
                    }
                } else if r.starts_with('|') || r.starts_with('>') {
                    self.parse_block_scalar(line_indent, r)?
                } else if r.starts_with('&') {
                    self.parse_anchored(r, line_indent)?
                } else if r.starts_with('*') {
                    self.resolve_alias(r)?
                } else {
                    parse_scalar(r)?
                };
                merge_map(&mut map, merged)?;
            } else if r.is_empty() {
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
            } else if r.starts_with('|') || r.starts_with('>') {
                map.insert(k, self.parse_block_scalar(line_indent, r)?);
            } else if r.starts_with('&') {
                map.insert(k, self.parse_anchored(r, line_indent)?);
            } else if r.starts_with('*') {
                map.insert(k, self.resolve_alias(r)?);
            } else {
                map.insert(k, parse_scalar(r)?);
            }
        }
        Ok(map)
    }

    /// Parse a block scalar (`|` literal or `>` folded). The header line has
    /// already been consumed (`self.pos` points past it); `parent_indent` is
    /// the indentation of the node that contains the block scalar. Supports
    /// chomping indicators (`-` / `+`) and an explicit indentation indicator
    /// (`|2`, `|2-`, ...), per the YAML block-scalar header grammar.
    fn parse_block_scalar(&mut self, parent_indent: usize, header: &str) -> Result<Value> {
        let header = strip_comment(header).trim_end();
        let folded = header.starts_with('>');
        let mut chomp = BlockChomp::Clip;
        let mut indent_indicator: Option<usize> = None;
        for ch in header[1..].chars() {
            match ch {
                '0'..='9' => indent_indicator = Some(ch as usize - '0' as usize),
                '-' => chomp = BlockChomp::Strip,
                '+' => chomp = BlockChomp::Keep,
                _ => return Err(Error::custom("yaml: invalid block scalar header")),
            }
        }
        // Collect content lines (blank lines included). A non-blank line at
        // or above the parent indent ends the block.
        let mut raw_lines: Vec<String> = Vec::new();
        let mut first_content_indent: Option<usize> = None;
        while let Some(line) = self.current().map(str::to_string) {
            let li = leading_spaces(&line)?;
            if !line.trim().is_empty() && li <= parent_indent {
                break;
            }
            if !line.trim().is_empty() && first_content_indent.is_none() {
                first_content_indent = Some(li);
            }
            raw_lines.push(line);
            self.pos += 1;
        }
        let block_indent = match indent_indicator {
            Some(n) => parent_indent + n,
            None => first_content_indent.unwrap_or(parent_indent + 1),
        };
        let mut content: Vec<String> = Vec::new();
        for line in &raw_lines {
            if line.trim().is_empty() {
                content.push(String::new());
            } else {
                content.push(line.get(block_indent..).unwrap_or("").to_string());
            }
        }
        // Drop trailing blank lines, remembering how many there were so the
        // `Keep` chomping policy can restore the exact newline count.
        let mut trailing_blanks = 0;
        while content.last().is_some_and(|l| l.is_empty()) {
            content.pop();
            trailing_blanks += 1;
        }
        let mut text = if folded {
            // YAML folding: a single line break between content lines folds
            // to a space; blank lines (one or more consecutive) fold to a
            // single line break.
            let mut s = String::new();
            let mut pending_blank = false;
            let mut first = true;
            for line in &content {
                if line.is_empty() {
                    pending_blank = true;
                } else {
                    if !first {
                        s.push(if pending_blank { '\n' } else { ' ' });
                    }
                    first = false;
                    s.push_str(line);
                    pending_blank = false;
                }
            }
            s
        } else {
            content.join("\n")
        };
        match chomp {
            BlockChomp::Strip => {}
            BlockChomp::Clip => {
                if !content.is_empty() {
                    text.push('\n');
                }
            }
            // Keep preserves every trailing newline: one per remaining content
            // line's terminator plus one per dropped trailing blank line.
            BlockChomp::Keep => {
                for _ in 0..=trailing_blanks {
                    text.push('\n');
                }
            }
        }
        Ok(Value::from(text))
    }

    /// Parse a value carrying an anchor (`&name value`, or `&name` followed by
    /// an indented block) and register it for later `*name` aliases.
    ///
    /// The anchor's header line must already be consumed (`self.pos` points
    /// past it). Anchors are resolved by copying the value, matching
    /// serde_yaml for acyclic documents; a self-referential anchor therefore
    /// fails with `unknown anchor` instead of recursing.
    fn parse_anchored(&mut self, rest: &str, indent: usize) -> Result<Value> {
        let (name, after) = split_anchor(rest);
        if name.is_empty() {
            return Err(Error::custom("yaml: empty anchor name"));
        }
        let value = if after.is_empty() {
            let child_indent = self
                .current()
                .map(leading_spaces)
                .transpose()?
                .unwrap_or(indent + 1);
            if child_indent > indent {
                self.parse_node(child_indent)?
            } else {
                Value::Null
            }
        } else if after.starts_with('|') || after.starts_with('>') {
            self.parse_block_scalar(indent, after)?
        } else {
            parse_scalar(after)?
        };
        self.anchors.insert(name.to_string(), value.clone());
        Ok(value)
    }

    /// Resolve an alias reference (`*name`) to the anchored value (a copy).
    ///
    /// Expansion is charged against [`alias_budget`](YamlParser::alias_budget)
    /// so a document cannot amplify a small anchor into unbounded memory.
    fn resolve_alias(&mut self, raw: &str) -> Result<Value> {
        let name = raw[1..].trim();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return Err(Error::custom("yaml: malformed alias"));
        }
        let value = self
            .anchors
            .get(name)
            .cloned()
            .ok_or_else(|| Error::custom(alloc::format!("yaml: unknown anchor `{name}`")))?;
        let mut nodes = 0usize;
        count_value_nodes(&value, &mut nodes, self.alias_budget)?;
        self.alias_budget -= nodes;
        Ok(value)
    }
}

/// Count the nodes of a value tree against a budget (used to bound alias
/// expansion). Fails once the budget is exhausted.
fn count_value_nodes(v: &Value, count: &mut usize, budget: usize) -> Result<()> {
    if *count >= budget {
        return Err(Error::custom("yaml: alias expansion limit exceeded"));
    }
    *count += 1;
    match v {
        Value::Array(items) => {
            for item in items {
                count_value_nodes(item, count, budget)?;
            }
        }
        Value::Object(map) => {
            for (_, val) in map.iter() {
                count_value_nodes(val, count, budget)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Split an anchor header (`&name rest`) into the anchor name and the
/// remaining value text.
fn split_anchor(raw: &str) -> (&str, &str) {
    let after = &raw[1..];
    match after.find(char::is_whitespace) {
        Some(i) => (&after[..i], after[i..].trim_start()),
        None => (after, ""),
    }
}

/// Split a `!!tag value` / `!tag value` scalar into the tag name (without the
/// leading `!`(s)) and the remaining value text. Returns `(raw, None)` when
/// there is no tag. A `!` prefix is always a tag indicator in YAML, so a bare
/// `!foo` never silently becomes a plain string.
fn split_tag(raw: &str) -> (&str, Option<&str>) {
    if let Some(rest) = raw.strip_prefix('!') {
        let rest = rest.strip_prefix('!').unwrap_or(rest);
        if let Some(end) = rest.find(char::is_whitespace) {
            return (rest[end..].trim(), Some(&rest[..end]));
        }
        return ("", Some(rest));
    }
    (raw, None)
}

/// Parse a value carrying a standard `!!tag` (forced typing). Unsupported
/// tags are errors, never silent stringification; quoted values are
/// unquoted, and invalid escapes inside `!!str` are errors (not silently
/// kept as raw text).
fn parse_tagged(raw: &str, tag: &str) -> Result<Value> {
    match tag {
        "str" => {
            let raw = raw.trim();
            if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
                return Ok(Value::from(unescape_double(&raw[1..raw.len() - 1])?));
            }
            if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
                return Ok(Value::from(raw[1..raw.len() - 1].replace("''", "'")));
            }
            Ok(Value::from(raw.to_string()))
        }
        "int" => {
            let s = unquote_scalar(raw);
            if let Ok(v) = s.parse::<i64>() {
                return Ok(Value::from(v));
            }
            if let Ok(v) = s.parse::<u64>() {
                return Ok(Value::from(v));
            }
            Err(Error::custom("yaml: invalid !!int value"))
        }
        "float" => {
            if is_yaml_non_finite(raw) {
                return Err(Error::custom(
                    "yaml: non-finite float values are not supported",
                ));
            }
            let s = unquote_scalar(raw);
            if let Ok(v) = s.parse::<f64>() {
                return Ok(Value::from(v));
            }
            Err(Error::custom("yaml: invalid !!float value"))
        }
        "bool" => {
            let s = unquote_scalar(raw);
            if matches!(s.as_str(), "true" | "True" | "TRUE") {
                return Ok(Value::from(true));
            }
            if matches!(s.as_str(), "false" | "False" | "FALSE") {
                return Ok(Value::from(false));
            }
            Err(Error::custom("yaml: invalid !!bool value"))
        }
        "null" => Ok(Value::Null),
        _ => Err(Error::custom(alloc::format!(
            "yaml: unsupported tag `!!{tag}`"
        ))),
    }
}

/// Whether a scalar is a YAML non-finite float spelling (`.inf` / `.nan`).
fn is_yaml_non_finite(raw: &str) -> bool {
    matches!(
        raw,
        ".inf"
            | "+.inf"
            | "-.inf"
            | ".Inf"
            | "+.Inf"
            | "-.Inf"
            | ".INF"
            | "+.INF"
            | "-.INF"
            | ".nan"
            | "+.nan"
            | "-.nan"
            | ".NaN"
            | "+.NaN"
            | "-.NaN"
            | ".NAN"
            | "+.NAN"
            | "-.NAN"
    )
}

/// Strip one layer of single/double quotes from a scalar (for `!!str "x"`).
fn unquote_scalar(raw: &str) -> String {
    let raw = raw.trim();
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        unescape_double(&raw[1..raw.len() - 1])
            .unwrap_or_else(|_| raw[1..raw.len() - 1].to_string())
    } else if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        raw[1..raw.len() - 1].replace("''", "'")
    } else {
        raw.to_string()
    }
}

/// Merge a `<<:` source mapping into `map`; keys already present win, per
/// the YAML merge-key extension (`<<: *anchor`, `<<: {inline}`).
fn merge_map(map: &mut Map, source: Value) -> Result<()> {
    match source {
        Value::Object(src) => {
            for (k, v) in src {
                if !map.contains_key(&k) {
                    map.insert(k, v);
                }
            }
            Ok(())
        }
        _ => Err(Error::custom("yaml: `<<` merge value must be a mapping")),
    }
}

/// Whether a line contains a `key: value` colon (not a `:` inside quotes).
///
/// Byte scan (the quote characters are ASCII, and a `:` is always an ASCII
/// byte at a char boundary), so no per-line `Vec<char>` allocation.
fn contains_colon_separator(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b':' if !in_single && !in_double && (i + 1 >= bytes.len() || bytes[i + 1] == b' ') => {
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

fn split_key_value(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b':' if !in_single && !in_double => {
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
    // Standard tags (`!!str 123`, `!!int "42"`, ...) force a scalar type.
    let (raw, tag) = split_tag(raw);
    if let Some(tag) = tag {
        return parse_tagged(raw, tag);
    }
    // Quoted strings.
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return Ok(Value::from(unescape_double(&raw[1..raw.len() - 1])?));
    }
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return Ok(Value::from(raw[1..raw.len() - 1].replace("''", "'")));
    }
    // Anchors / aliases are resolved in block context by the parser; inside
    // flow collections they are rejected rather than silently read as plain
    // scalars.
    if raw.starts_with('*') || raw.starts_with('&') {
        return Err(Error::custom(
            "yaml: anchor/alias is only supported in block context",
        ));
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
    // YAML special floats `.inf` / `-.inf` / `.nan` are non-finite; the
    // library rejects non-finite floats everywhere, so they error here.
    if is_yaml_non_finite(raw) {
        return Err(Error::custom(
            "yaml: non-finite float values are not supported",
        ));
    }
    // Numbers.
    if let Ok(v) = raw.parse::<i64>() {
        return Ok(Value::from(v));
    }
    if let Ok(v) = raw.parse::<u64>() {
        return Ok(Value::from(v));
    }
    if let Ok(v) = raw.parse::<f64>() {
        if !v.is_finite() {
            return Err(Error::custom("yaml: non-finite float"));
        }
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
            Some('u') => {
                let hi = read_hex_escape(&mut chars)?;
                if (0xD800..=0xDBFF).contains(&hi) {
                    // High surrogate: combine with a following `\uXXXX` low.
                    if chars.clone().next() == Some('\\') {
                        chars.next();
                        if chars.clone().next() == Some('u') {
                            chars.next();
                            let lo = read_hex_escape(&mut chars)?;
                            if (0xDC00..=0xDFFF).contains(&lo) {
                                let cp =
                                    0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            } else {
                                out.push('\u{FFFD}');
                            }
                        } else {
                            out.push('\u{FFFD}');
                        }
                    } else {
                        out.push('\u{FFFD}');
                    }
                } else if (0xDC00..=0xDFFF).contains(&hi) {
                    out.push('\u{FFFD}');
                } else {
                    out.push(char::from_u32(hi as u32).unwrap_or('\u{FFFD}'));
                }
            }
            Some(other) => out.push(other),
            None => return Err(Error::custom("yaml: unterminated escape")),
        }
    }
    Ok(out)
}

/// Read four hexadecimal digits from a char iterator into a `u16`.
fn read_hex_escape(chars: &mut core::str::Chars<'_>) -> Result<u16> {
    let mut v: u16 = 0;
    for _ in 0..4 {
        let c = chars
            .next()
            .ok_or_else(|| Error::custom("yaml: truncated hex escape"))?;
        let d = c
            .to_digit(16)
            .ok_or_else(|| Error::custom("yaml: invalid hex escape"))?;
        v = v * 16 + d as u16;
    }
    Ok(v)
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
            // A Number key can only be finite after the decoder rejects
            // non-finite floats; the fallback is defensive only.
            Value::Number(n) => tree::number_string(n).unwrap_or_default(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".to_string(),
            _ => alloc::format!("{self}"),
        }
    }
}

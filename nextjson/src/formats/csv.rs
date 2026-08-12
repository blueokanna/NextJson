//! CSV codec (RFC 4180).
//!
//! Encodes `Vec<Vec<T>>` as rows of comma-separated fields and
//! `Vec<Map>` / rows of objects with a header row. The decoder parses the
//! whole table up front: if the target requests objects, the first row is
//! used as the header; if it requests arrays, every row is data.

use alloc::borrow::Cow;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::tree;
use crate::formats::Format;
use crate::number::Number;
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::write::Write;

/// CSV format marker.
#[derive(Clone, Copy, Debug)]
pub struct Csv;

impl Format for Csv {
    const NAME: &'static str = "csv";
    const MIME: &'static str = "text/csv";
    const EXTENSIONS: &'static [&'static str] = &["csv"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = CsvEncoder::new(Vec::new());
        let mut checked = crate::ser::CheckedEncoder::new(&mut encoder);
        T::nextencode(value, &mut checked)?;
        checked.finish()?;
        Ok(encoder.finish_vec())
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let rows = parse_csv(input)?;
        let mut decoder = CsvDecoder::new(rows);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

/// Write one CSV field, quoting when needed.
fn write_field(out: &mut Vec<u8>, field: &str) {
    let needs_quotes = field.is_empty()
        || field.contains(',')
        || field.contains('"')
        || field.contains('\n')
        || field.contains('\r');
    if !needs_quotes {
        out.extend_from_slice(field.as_bytes());
        return;
    }
    out.push(b'"');
    for &b in field.as_bytes() {
        if b == b'"' {
            out.push(b'"');
        }
        out.push(b);
    }
    out.push(b'"');
}

/// Append one UTF-8 scalar starting at `input[i]` to `field`; returns the
/// number of bytes consumed.
fn push_utf8(input: &[u8], i: usize, field: &mut String) -> Result<usize> {
    let len = utf8_len(input[i]).ok_or_else(|| Error::custom("csv: invalid utf-8"))?;
    let chunk = input
        .get(i..i + len)
        .ok_or_else(|| Error::custom("csv: truncated utf-8"))?;
    let s = core::str::from_utf8(chunk).map_err(|_| Error::custom("csv: invalid utf-8"))?;
    field.push_str(s);
    Ok(len)
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

/// Parse RFC 4180 CSV into rows of fields.
fn parse_csv(input: &[u8]) -> Result<Vec<Vec<String>>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut i = 0;
    let mut in_quotes = false;
    let mut after_quote = false;
    let mut at_field_start = true;
    let mut row_started = false;
    while i < input.len() {
        let b = input[i];
        if in_quotes {
            if b == b'"' {
                if input.get(i + 1) == Some(&b'"') {
                    field.push('"');
                    i += 2;
                    continue;
                }
                in_quotes = false;
                after_quote = true;
                i += 1;
                continue;
            }
            i += push_utf8(input, i, &mut field)?;
        } else if after_quote {
            match b {
                b',' => {
                    row.push(core::mem::take(&mut field));
                    after_quote = false;
                    at_field_start = true;
                    row_started = true;
                    i += 1;
                }
                b'\r' => {
                    if input.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    row.push(core::mem::take(&mut field));
                    rows.push(core::mem::take(&mut row));
                    after_quote = false;
                    at_field_start = true;
                    row_started = false;
                    i += 1;
                }
                b'\n' => {
                    row.push(core::mem::take(&mut field));
                    rows.push(core::mem::take(&mut row));
                    after_quote = false;
                    at_field_start = true;
                    row_started = false;
                    i += 1;
                }
                _ => {
                    return Err(Error::custom(
                        "csv: only a delimiter or line ending may follow a closing quote",
                    ));
                }
            }
        } else {
            match b {
                b'"' if at_field_start => {
                    in_quotes = true;
                    at_field_start = false;
                    i += 1;
                }
                b'"' => return Err(Error::custom("csv: quote inside an unquoted field")),
                b',' => {
                    row.push(core::mem::take(&mut field));
                    at_field_start = true;
                    row_started = true;
                    i += 1;
                }
                b'\r' => {
                    if input.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    row.push(core::mem::take(&mut field));
                    rows.push(core::mem::take(&mut row));
                    at_field_start = true;
                    row_started = false;
                    i += 1;
                }
                b'\n' => {
                    row.push(core::mem::take(&mut field));
                    rows.push(core::mem::take(&mut row));
                    at_field_start = true;
                    row_started = false;
                    i += 1;
                }
                _ => {
                    i += push_utf8(input, i, &mut field)?;
                    at_field_start = false;
                }
            }
        }
    }
    if in_quotes {
        return Err(Error::custom("csv: unterminated quoted field"));
    }
    if !at_field_start || row_started || !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Streaming CSV encoder.
pub struct CsvEncoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    /// Current nesting depth (0 = before root, 1 = inside rows, 2 = inside a row).
    depth: usize,
    /// Header keys captured from the first object row.
    header_keys: Vec<String>,
    header_index: BTreeMap<String, usize>,
    header_written: bool,
    object_open: bool,
    pending_column: Option<usize>,
    /// Values of the current row.
    row_values: Vec<Option<String>>,
}

impl<W: Write> CsvEncoder<W> {
    /// Create a CSV encoder over `writer`.
    pub fn new(writer: W) -> Self {
        CsvEncoder {
            writer,
            buf: Vec::with_capacity(512),
            depth: 0,
            header_keys: Vec::new(),
            header_index: BTreeMap::new(),
            header_written: false,
            object_open: false,
            pending_column: None,
            row_values: Vec::new(),
        }
    }

    fn write_row(&mut self, fields: &[String]) {
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                self.buf.push(b',');
            }
            write_field(&mut self.buf, field);
        }
        self.buf.push(b'\n');
    }

    fn flush_object_row(&mut self) -> Result<()> {
        if self.pending_column.is_some() {
            return Err(Error::custom("csv: object key has no value"));
        }
        if self.header_keys.is_empty() {
            return Err(Error::custom("csv: object rows must contain a field"));
        }
        if self.row_values.iter().any(Option::is_none) {
            return Err(Error::custom(
                "csv: object row does not contain every header field",
            ));
        }
        if !self.header_written {
            let header = self.header_keys.clone();
            self.write_row(&header);
            self.header_written = true;
        }
        let values = core::mem::take(&mut self.row_values)
            .into_iter()
            .map(|value| value.expect("checked above"))
            .collect::<Vec<_>>();
        self.write_row(&values);
        Ok(())
    }

    fn push_cell(&mut self, value: String) -> Result<()> {
        if self.object_open {
            let column = self
                .pending_column
                .take()
                .ok_or_else(|| Error::custom("csv: object value has no key"))?;
            let slot = self
                .row_values
                .get_mut(column)
                .ok_or_else(|| Error::custom("csv: invalid object column"))?;
            if slot.is_some() {
                return Err(Error::custom("csv: duplicate object key"));
            }
            *slot = Some(value);
            return Ok(());
        }
        if self.depth != 2 {
            return Err(Error::custom(
                "csv: root must be an array of array or object rows",
            ));
        }
        self.row_values.push(Some(value));
        Ok(())
    }

    fn finish_vec(mut self) -> Vec<u8> {
        core::mem::take(&mut self.buf)
    }

    /// Flush and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }
}

impl<W: Write> FormatEncoder for CsvEncoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        match self.depth {
            0 => {
                // Root: the rows container.
                self.depth = 1;
                Ok(())
            }
            1 => {
                // A row of cells.
                self.depth = 2;
                self.row_values.clear();
                self.pending_column = None;
                Ok(())
            }
            _ => Err(Error::custom(
                "csv: nested containers are not representable",
            )),
        }
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        match self.depth {
            2 => {
                if self.object_open {
                    return Err(Error::custom("csv: array end inside object row"));
                }
                self.depth = 1;
                let row = core::mem::take(&mut self.row_values)
                    .into_iter()
                    .map(|value| value.expect("array cells are initialized"))
                    .collect::<Vec<_>>();
                self.write_row(&row);
                Ok(())
            }
            1 => {
                if self.object_open {
                    return Err(Error::custom("csv: array ended inside object row"));
                }
                self.depth = 0;
                Ok(())
            }
            _ => Err(Error::custom("csv: array end without start")),
        }
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        if self.depth != 1 {
            return Err(Error::custom("csv: object must be a row"));
        }
        if self.object_open {
            return Err(Error::custom("csv: nested object row"));
        }
        self.object_open = true;
        self.pending_column = None;
        if self.header_written {
            self.row_values.clear();
            self.row_values.resize_with(self.header_keys.len(), || None);
        } else {
            self.row_values.clear();
        }
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        if !self.object_open {
            return Err(Error::custom("csv: key outside object row"));
        }
        if self.pending_column.is_some() {
            return Err(Error::custom("csv: previous object key has no value"));
        }
        let column = if self.header_written {
            self.header_index
                .get(key)
                .copied()
                .ok_or_else(|| Error::custom("csv: object row contains an unknown field"))?
        } else {
            let column = self.header_keys.len();
            if self.header_index.insert(key.to_string(), column).is_some() {
                return Err(Error::custom("csv: duplicate object key"));
            }
            self.header_keys.push(key.to_string());
            self.row_values.push(None);
            column
        };
        if self.row_values[column].is_some() {
            return Err(Error::custom("csv: duplicate object key"));
        }
        self.pending_column = Some(column);
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        if !self.object_open {
            return Err(Error::custom("csv: object end without start"));
        }
        self.flush_object_row()?;
        self.object_open = false;
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.push_cell("null".to_string())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.push_cell(if value { "true".into() } else { "false".into() })
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }

    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error> {
        self.push_cell(tree::number_string(value))
    }

    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        self.push_cell(value.to_string())
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Undecided,
    Arrays,
    Objects,
}

/// Streaming CSV decoder over pre-parsed rows.
pub struct CsvDecoder {
    rows: Vec<Vec<String>>,
    row_index: usize,
    cell_index: usize,
    depth: usize,
    mode: Mode,
    header: Vec<String>,
    lookahead: Option<Token<'static>>,
}

impl CsvDecoder {
    /// Create a decoder over pre-parsed rows.
    pub fn new(rows: Vec<Vec<String>>) -> Self {
        CsvDecoder {
            rows,
            row_index: 0,
            cell_index: 0,
            depth: 0,
            mode: Mode::Undecided,
            header: Vec::new(),
            lookahead: None,
        }
    }

    /// Validate that all rows were consumed.
    pub fn end(&mut self) -> Result<()> {
        self.expect_end()
    }

    fn expect_end(&mut self) -> Result<()> {
        let done = match self.mode {
            // Object rows: one header row plus data rows, all consumed.
            Mode::Objects => self.row_index >= self.rows.len().saturating_sub(1),
            // Array rows: every row consumed.
            Mode::Arrays => self.row_index >= self.rows.len(),
            // Bare scalar: a single-row input whose one cell was read.
            Mode::Undecided => self.rows.len() <= 1 && self.cell_index >= self.row(0).len(),
        };
        if done {
            Ok(())
        } else {
            Err(Error::custom("csv: unconsumed rows"))
        }
    }

    fn row(&self, idx: usize) -> &[String] {
        self.rows.get(idx).map(|r| r.as_slice()).unwrap_or(&[])
    }
}

impl FormatDecoder<'_> for CsvDecoder {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        if self.depth != 1 {
            return Err(Error::custom("csv: object must be a row"));
        }
        if self.mode == Mode::Undecided {
            self.mode = Mode::Objects;
            if self.rows.is_empty() {
                return Err(Error::custom("csv: no header row"));
            }
            self.header = self.rows[0].clone();
            let mut keys = BTreeSet::new();
            if self.header.iter().any(|key| !keys.insert(key.as_str())) {
                return Err(Error::custom("csv: duplicate header field"));
            }
            self.row_index = 1;
        }
        if self.mode != Mode::Objects {
            return Err(Error::custom("csv: row requested as array, not object"));
        }
        let row = self
            .rows
            .get(self.row_index)
            .ok_or_else(|| Error::custom("csv: missing object row"))?;
        if row.len() != self.header.len() {
            return Err(Error::custom("csv: object row width does not match header"));
        }
        self.depth += 1;
        self.cell_index = 0;
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        if self.depth != 2 {
            return Err(Error::custom("csv: object end without start"));
        }
        if self.cell_index != self.header.len() {
            return Err(Error::custom("csv: object row was not fully consumed"));
        }
        self.depth -= 1;
        self.row_index += 1;
        Ok(())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'static, str>>, Self::Error> {
        if self.cell_index >= self.header.len() {
            return Ok(None);
        }
        let key = self.header[self.cell_index].clone();
        Ok(Some(Cow::Owned(key)))
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        Ok(self.cell_index < self.header.len())
    }

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        if self.depth == 0 {
            // Root: the rows container.
            self.depth = 1;
            return Ok(());
        }
        if self.depth == 1 {
            if self.mode == Mode::Undecided {
                self.mode = Mode::Arrays;
            }
            if self.mode != Mode::Arrays {
                return Err(Error::custom("csv: row requested as object, not array"));
            }
            self.depth = 2;
            self.cell_index = 0;
            return Ok(());
        }
        Err(Error::custom(
            "csv: nested containers are not representable",
        ))
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        match self.depth {
            2 => {
                self.row_index += 1;
                self.depth = 1;
                Ok(())
            }
            1 => {
                self.depth = 0;
                Ok(())
            }
            _ => Err(Error::custom("csv: array end without start")),
        }
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        if self.depth == 1 {
            Ok(self.row_index < self.rows.len())
        } else {
            Ok(self.cell_index < self.row(self.row_index).len())
        }
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        if self.depth == 1 {
            Ok(self.row_index < self.rows.len())
        } else {
            Ok(self.cell_index < self.row(self.row_index).len())
        }
    }

    fn unit(&mut self) -> Result<(), Self::Error> {
        let cell = self.scalar_cell()?;
        if cell != "null" {
            return Err(Error::custom("csv: expected null cell"));
        }
        Ok(())
    }

    fn bool(&mut self) -> Result<bool, Self::Error> {
        let cell = self.scalar_cell()?;
        match cell.as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            other => Err(Error::custom(alloc::format!("csv: invalid bool {other:?}"))),
        }
    }

    fn number(&mut self) -> Result<Number, Self::Error> {
        let cell = self.scalar_cell()?;
        parse_number(&cell)
    }

    fn string(&mut self) -> Result<Cow<'static, str>, Self::Error> {
        let cell = self.scalar_cell()?;
        Ok(Cow::Owned(cell))
    }

    fn char(&mut self) -> Result<char, Self::Error> {
        let s = self.scalar_cell()?;
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(Error::invalid_type("a single-character string", "string")),
        }
    }

    fn skip_value(&mut self) -> Result<(), Self::Error> {
        self.next_cell()?;
        Ok(())
    }

    fn peek_token(&mut self) -> Result<Token<'static>, Self::Error> {
        if self.lookahead.is_none() {
            let cell = self.next_cell()?;
            self.lookahead = Some(classify_cell(&cell));
        }
        Ok(self.lookahead.as_ref().expect("set").clone())
    }

    fn next_token(&mut self) -> Result<Token<'static>, Self::Error> {
        if let Some(t) = self.lookahead.take() {
            return Ok(t);
        }
        let cell = self.next_cell()?;
        Ok(classify_cell(&cell))
    }

    fn save(&self) -> Mark {
        Mark {
            pos: self.row_index,
            depth: 0,
            frame_len: self.cell_index,
        }
    }

    fn restore(&mut self, mark: Mark) {
        self.row_index = mark.pos;
        self.cell_index = mark.frame_len;
    }
}

impl CsvDecoder {
    fn next_cell(&mut self) -> Result<String> {
        let row = self.row(self.row_index);
        let cell = row
            .get(self.cell_index)
            .cloned()
            .ok_or_else(|| Error::custom("csv: missing cell"))?;
        self.cell_index += 1;
        Ok(cell)
    }

    fn scalar_cell(&mut self) -> Result<String> {
        if let Some(t) = self.lookahead.take() {
            return Ok(match t {
                Token::Str(s) => s.into_owned(),
                Token::Number(n) => tree::number_string(&n),
                Token::Bool(b) => b.to_string(),
                Token::Null => "null".to_string(),
                _ => return Err(Error::custom("csv: unexpected token in cell")),
            });
        }
        self.next_cell()
    }
}

/// Classify a CSV cell into a typed token.
fn classify_cell(cell: &str) -> Token<'static> {
    if let Ok(v) = cell.parse::<i64>() {
        return Token::Number(Number::from(v));
    }
    if let Ok(v) = cell.parse::<u64>() {
        return Token::Number(Number::from(v));
    }
    if let Ok(v) = cell.parse::<f64>() {
        return Token::Number(Number::F64(v));
    }
    match cell {
        "true" | "1" => Token::Bool(true),
        "false" | "0" => Token::Bool(false),
        "null" => Token::Null,
        _ => Token::Str(Cow::Owned(cell.to_string())),
    }
}

fn parse_number(s: &str) -> Result<Number> {
    if let Ok(v) = s.parse::<i64>() {
        return Ok(Number::from(v));
    }
    if let Ok(v) = s.parse::<u64>() {
        return Ok(Number::from(v));
    }
    if let Ok(v) = s.parse::<f64>() {
        return Ok(Number::F64(v));
    }
    Err(Error::custom(alloc::format!("csv: invalid number {s:?}")))
}

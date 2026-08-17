//! NDJSON / JSONL codec (newline-delimited JSON).
//!
//! Each record is one complete JSON value on its own line, terminated by
//! `\n` (a trailing `\r` is tolerated). This is the de-facto streaming
//! format for logs, event streams and bulk imports.
//!
//! - **Encode**: a top-level array is written as one JSON value per line
//!   (no enclosing brackets); any other value is written as a single line.
//! - **Decode**: `decode::<Vec<T>>` reads the input as a line stream and
//!   returns one element per line; `decode::<T>` parses the first
//!   non-empty line. Blank lines are skipped.
//!
//! Interop: line-oriented output is accepted by `jq --slurp`, `ndjson-cli`,
//! `jsonl` tooling and every JSON parser that reads one value per line.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::de::{Decoder, FormatDecoder, Mark, NsonDeserialize, Token};
use crate::error::{Error, Result};
use crate::formats::tree::CollectEncoder;
use crate::formats::Format;
use crate::number::Number;
use crate::ser::NsonSerialize;
use crate::value::Value;

/// NDJSON format marker.
#[derive(Clone, Copy, Debug)]
pub struct Ndjson;

impl Format for Ndjson {
    const NAME: &'static str = "ndjson";
    const MIME: &'static str = "application/x-ndjson";
    const EXTENSIONS: &'static [&'static str] = &["ndjson", "jsonl"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut collector = CollectEncoder::new();
        T::nextencode(value, &mut collector)?;
        let root = collector.take_root()?;
        let mut out = Vec::new();
        match root {
            Value::Array(items) => {
                for item in items {
                    let line = crate::nextencode(&item)?;
                    out.extend_from_slice(&line);
                    out.push(b'\n');
                }
            }
            other => {
                let line = crate::nextencode(&other)?;
                out.extend_from_slice(&line);
                out.push(b'\n');
            }
        }
        Ok(out)
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = NdjsonDecoder::new(input);
        let value = T::nextdecode(&mut decoder)?;
        decoder.expect_end()?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Streaming NDJSON decoder.
///
/// Two modes:
/// - **Line stream**: entered when the first protocol call is
///   `begin_array` (i.e. the target is `Vec<T>` or another collection).
///   Every non-empty line becomes one element.
/// - **Single value**: any other first call parses the first non-empty line
///   as one JSON value and requires no further non-empty lines.
///
/// The decoder holds a persistent inner [`Decoder`] over the current line so
/// multi-step container reads (begin/object_key/…/end) all advance the same
/// position.
///
/// Two modes, decided by the *first* protocol call:
/// - `begin_array` first → **line stream** (the target is a collection such
///   as `Vec<T>`). Every non-empty line becomes one element; the stream root
///   array itself has no brackets.
/// - anything else first (`begin_object`, `peek_token`, scalars) →
///   **single value**: the first non-empty line is one JSON value.
pub struct NdjsonDecoder<'de> {
    input: &'de [u8],
    /// Start offset of the next unconsumed line.
    line_start: usize,
    /// Persistent decoder over the current line (or `None` when no line is
    /// loaded).
    current: Option<Decoder<'de>>,
    /// Line-stream mode (root `begin_array` was the first call).
    stream_mode: bool,
    /// Container nesting inside the current element; 0 is the stream root.
    depth: u32,
    /// Whether the first protocol call happened (single-value mode only).
    started: bool,
}

impl<'de> NdjsonDecoder<'de> {
    /// Create an NDJSON decoder over `input`.
    pub fn new(input: &'de [u8]) -> Self {
        NdjsonDecoder {
            input,
            line_start: 0,
            current: None,
            stream_mode: false,
            depth: 0,
            started: false,
        }
    }

    /// Validate that the whole input was consumed.
    pub fn end(&mut self) -> Result<()> {
        self.expect_end()
    }

    fn expect_end(&mut self) -> Result<()> {
        if self.stream_mode {
            // Stream mode consumes the whole input line by line.
            if self.current.is_some() {
                return Err(Error::custom("ndjson: trailing data after array"));
            }
            Ok(())
        } else if self.started {
            // The root was parsed from the first line; any further non-empty
            // lines are trailing garbage.
            self.current = None;
            let has_more = self.load_next_line()?;
            if has_more {
                Err(Error::custom("ndjson: trailing data after value"))
            } else {
                Ok(())
            }
        } else {
            Err(Error::custom("ndjson: no value"))
        }
    }

    /// Advance to the next non-empty line and load a fresh inner decoder.
    /// Returns `false` at EOF (leaving `current = None`).
    fn load_next_line(&mut self) -> Result<bool> {
        'lines: loop {
            let input = self.input;
            let line_begin = self.line_start;
            let mut i = line_begin;
            let mut saw_content = false;
            while i < input.len() {
                if input[i] == b'\n' {
                    let mut line = &input[line_begin..i];
                    if line.last() == Some(&b'\r') {
                        line = &line[..line.len() - 1];
                    }
                    self.line_start = i + 1;
                    if !saw_content {
                        continue 'lines; // blank line, keep scanning
                    }
                    self.current = Some(Decoder::new(line));
                    return Ok(true);
                }
                match input[i] {
                    b' ' | b'\t' | b'\r' => {}
                    _ => saw_content = true,
                }
                i += 1;
            }
            // EOF: the final line may lack a trailing newline.
            if saw_content {
                let line = &input[line_begin..];
                self.line_start = input.len();
                self.current = Some(Decoder::new(line));
                return Ok(true);
            }
            self.line_start = input.len();
            self.current = None;
            return Ok(false);
        }
    }

    /// Ensure a line is loaded (single-value mode entry).
    fn ensure_line(&mut self) -> Result<()> {
        if !self.started {
            self.started = true;
            if self.current.is_none() && !self.load_next_line()? {
                return Err(Error::custom("ndjson: empty input"));
            }
        }
        Ok(())
    }

    /// Forward a value call to the current line's decoder.
    fn with_current<T>(&mut self, f: impl FnOnce(&mut Decoder<'de>) -> Result<T>) -> Result<T> {
        self.ensure_line()?;
        let decoder = self
            .current
            .as_mut()
            .ok_or_else(|| Error::custom("ndjson: no line loaded"))?;
        f(decoder)
    }

    /// Whether this call is at the stream root level (stream mode only).
    fn at_stream_root(&self) -> bool {
        self.stream_mode && self.depth == 0
    }
}

impl<'de> FormatDecoder<'de> for NdjsonDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        if !self.started {
            // First protocol call: enter line-stream mode.
            self.stream_mode = true;
            self.started = true;
            self.current = None;
            self.depth = 0;
            return Ok(());
        }
        self.depth += 1;
        self.with_current(|d| d.begin_array())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        if self.at_stream_root() {
            // Root stream array closed; all lines consumed as elements.
            self.current = None;
            return Ok(());
        }
        self.depth = self.depth.saturating_sub(1);
        self.with_current(|d| d.end_array())
    }

    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        if self.at_stream_root() {
            if self.current.is_none() {
                self.load_next_line()?;
            }
            return Ok(self.current.is_some());
        }
        self.with_current(|d| d.array_has_more())
    }

    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        if self.at_stream_root() {
            // The element (one line) is complete; move to the next line.
            self.current = None;
            return self.load_next_line();
        }
        self.with_current(|d| d.array_entry_sep())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.depth += 1;
        self.with_current(|d| d.begin_object())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.depth = self.depth.saturating_sub(1);
        self.with_current(|d| d.end_object())
    }

    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        self.with_current(|d| d.object_key())
    }

    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.with_current(|d| d.object_entry_sep())
    }

    fn unit(&mut self) -> Result<(), Self::Error> {
        self.with_current(|d| d.unit())
    }

    fn bool(&mut self) -> Result<bool, Self::Error> {
        self.with_current(|d| d.bool())
    }

    fn number(&mut self) -> Result<Number, Self::Error> {
        self.with_current(|d| d.number())
    }

    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error> {
        self.with_current(|d| d.string())
    }

    fn char(&mut self) -> Result<char, Self::Error> {
        self.with_current(|d| d.char())
    }

    fn skip_value(&mut self) -> Result<(), Self::Error> {
        self.with_current(|d| d.skip_value())
    }

    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error> {
        self.with_current(|d| d.peek_token())
    }

    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        self.with_current(|d| d.next_token())
    }

    fn save(&self) -> Mark {
        Mark::new(self.line_start, 0)
    }

    fn restore(&mut self, mark: Mark) {
        self.line_start = mark.pos;
        self.current = None;
        self.started = true;
        // Reload the current line so subsequent reads continue from the mark.
        let _ = self.load_next_line();
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

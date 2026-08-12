//! Serialization: buffered [`Encoder`], the [`NsonSerialize`] trait, and
//! standard-library implementations.
//!
//! Design points:
//! - hand-written stack-buffer integer and float output;
//! - single-pass string escaping with bulk copy for unescaped input;
//! - per-container first-element flag stack for zero-cost separators.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, BinaryHeap, LinkedList, VecDeque};
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::fmt::{self, Write as _};
use core::marker::PhantomData;
use core::ops::{Range, RangeFrom, RangeInclusive, RangeTo, RangeToInclusive};
use core::time::Duration;

use crate::error::{Error, ErrorKind, FormatError, Result};
use crate::map::Map;
use crate::number::Number;
use crate::schema::{NsonSchema, TypeSchema};
use crate::value::Value;
use crate::write::Write;

/// Serialization configuration.
#[derive(Clone, Debug)]
pub struct EncodeConfig {
    /// Pretty-print output (indentation + newlines). Default `false`.
    pub pretty: bool,
    /// Indentation string used by pretty printing. Default two spaces.
    pub indent: &'static str,
    /// Escape all non-ASCII characters as `\uXXXX`. Default `false`.
    pub escape_non_ascii: bool,
}

impl Default for EncodeConfig {
    fn default() -> Self {
        EncodeConfig {
            pretty: false,
            indent: "  ",
            escape_non_ascii: false,
        }
    }
}

impl EncodeConfig {
    /// Compact output config (default).
    pub fn compact() -> Self {
        EncodeConfig::default()
    }
    /// Pretty-printed output config.
    pub fn pretty() -> Self {
        EncodeConfig {
            pretty: true,
            ..EncodeConfig::default()
        }
    }
    /// Set the indentation string.
    pub fn indent(mut self, indent: &'static str) -> Self {
        self.indent = indent;
        self
    }
    /// Set whether to escape non-ASCII.
    pub fn escape_non_ascii(mut self, on: bool) -> Self {
        self.escape_non_ascii = on;
        self
    }
}

/// Format-neutral emission contract implemented by every destination codec.
///
/// `NsonSerialize::nextencode` is generic over this trait, so one type
/// implementation can target every codec whose data model represents the
/// emitted values. Binary codecs can use [`separator`] and [`key`] as counting
/// signals and patch length prefixes when a container closes. Some
/// document-oriented codecs collect the event stream before emission.
///
/// [`separator`]: FormatEncoder::separator
/// [`key`]: FormatEncoder::key
pub trait FormatEncoder {
    /// The error type produced by this format's methods.
    ///
    /// External codecs may use their own error type; it only needs to wrap
    /// [`crate::error::Error`] so generic serialization code can propagate
    /// format failures. The built-in formats all use
    /// [`crate::error::Error`].
    type Error: FormatError;

    /// Begin an array container.
    fn begin_array(&mut self) -> Result<(), Self::Error>;
    /// Emit the separator preceding a subsequent array element.
    ///
    /// Called once per element, including the first, so binary codecs can use
    /// it as an element counter.
    fn separator(&mut self) -> Result<(), Self::Error>;
    /// End the current array container.
    fn end_array(&mut self) -> Result<(), Self::Error>;
    /// Begin an object container.
    fn begin_object(&mut self) -> Result<(), Self::Error>;
    /// Emit an object key.
    ///
    /// Called once per entry, including the first, so binary codecs can use it
    /// as an entry counter.
    fn key(&mut self, key: &str) -> Result<(), Self::Error>;
    /// End the current object container.
    fn end_object(&mut self) -> Result<(), Self::Error>;
    /// Emit `null`.
    fn write_null(&mut self) -> Result<(), Self::Error>;
    /// Emit a boolean.
    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error>;
    /// Emit a UTF-8 string.
    fn write_str(&mut self, value: &str) -> Result<(), Self::Error>;
    /// Emit a single character (a one-scalar string).
    fn write_char(&mut self, value: char) -> Result<(), Self::Error>;
    /// Emit a number preserving its exact internal kind.
    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error>;
    /// Emit an `i64`.
    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error>;
    /// Emit a `u64`.
    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error>;
    /// Emit an `i128`.
    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error>;
    /// Emit a `u128`.
    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error>;
    /// Emit an `f64` (shortest round-trip).
    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error>;
    /// Emit an `f32` (shortest round-trip).
    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error>;

    /// Emit an `i8` (default: widen to [`write_i64`](FormatEncoder::write_i64)).
    ///
    /// Binary codecs that preserve source width on the wire override this.
    fn write_i8(&mut self, value: i8) -> Result<(), Self::Error> {
        self.write_i64(value as i64)
    }
    /// Emit an `i16` (default: widen to [`write_i64`](FormatEncoder::write_i64)).
    fn write_i16(&mut self, value: i16) -> Result<(), Self::Error> {
        self.write_i64(value as i64)
    }
    /// Emit an `i32` (default: widen to [`write_i64`](FormatEncoder::write_i64)).
    fn write_i32(&mut self, value: i32) -> Result<(), Self::Error> {
        self.write_i64(value as i64)
    }
    /// Emit a `u8` (default: widen to [`write_u64`](FormatEncoder::write_u64)).
    fn write_u8(&mut self, value: u8) -> Result<(), Self::Error> {
        self.write_u64(value as u64)
    }
    /// Emit a `u16` (default: widen to [`write_u64`](FormatEncoder::write_u64)).
    fn write_u16(&mut self, value: u16) -> Result<(), Self::Error> {
        self.write_u64(value as u64)
    }
    /// Emit a `u32` (default: widen to [`write_u64`](FormatEncoder::write_u64)).
    fn write_u32(&mut self, value: u32) -> Result<(), Self::Error> {
        self.write_u64(value as u64)
    }

    /// Emit a byte sequence.
    ///
    /// The default emits a sequence of `u8` values, which is lossless in every
    /// self-describing format (JSON text becomes `[1, 2, 3]`, matching
    /// serde_json). Binary codecs override this to emit a native byte-string
    /// wire type (length prefix + raw bytes), which is far more compact.
    fn write_bytes(&mut self, value: &[u8]) -> Result<(), Self::Error> {
        self.begin_array()?;
        for &byte in value {
            self.separator()?;
            self.write_u8(byte)?;
        }
        self.end_array()
    }

    /// Emit `Option::None`.
    ///
    /// The default maps to [`write_null`](FormatEncoder::write_null), which is
    /// exactly the JSON shape. Binary codecs override this to emit a
    /// distinguishing tag so `None` stays distinct from a `Some` payload.
    fn write_none(&mut self) -> Result<(), Self::Error> {
        self.write_null()
    }
    /// Emit the marker that the following value is `Option::Some`.
    ///
    /// The default emits nothing (the payload follows immediately), which is
    /// the JSON shape. Binary codecs override this to emit a distinguishing
    /// tag.
    fn write_some(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Emit a map key.
    ///
    /// The default serializes the key to a string and emits it with
    /// [`key`](FormatEncoder::key) (the JSON shape). Binary codecs override
    /// this to write the key as a plain value, which supports non-string keys
    /// such as `BTreeMap<u8, V>` without string round-tripping.
    fn map_key<K: NsonSerialize>(&mut self, key: &K) -> Result<(), Self::Error> {
        let string = key_to_str(key)?;
        self.key(&string)
    }

    /// Whether this format produces human-readable output.
    ///
    /// Text formats return `true`; binary codecs return `false`. Types that
    /// encode differently for humans (timestamps, identifiers, byte strings)
    /// branch on this, mirroring serde's `Serializer::is_human_readable`.
    fn is_human_readable(&self) -> bool {
        true
    }
}

enum EventFrame {
    Array { ready: bool },
    Object { pending_value: bool },
}

/// Validates the format-neutral serialization event protocol before events
/// reach a concrete codec.
pub(crate) struct CheckedEncoder<'a, E: FormatEncoder + ?Sized> {
    inner: &'a mut E,
    frames: Vec<EventFrame>,
    root_written: bool,
}

impl<'a, E: FormatEncoder + ?Sized> CheckedEncoder<'a, E> {
    pub(crate) fn new(inner: &'a mut E) -> Self {
        CheckedEncoder {
            inner,
            frames: Vec::new(),
            root_written: false,
        }
    }

    fn value(&mut self) -> Result<(), E::Error> {
        match self.frames.last_mut() {
            Some(EventFrame::Array { ready }) if *ready => {
                *ready = false;
                Ok(())
            }
            Some(EventFrame::Array { .. }) => {
                Err(Error::custom("array separator required before value").into())
            }
            Some(EventFrame::Object { pending_value }) if *pending_value => {
                *pending_value = false;
                Ok(())
            }
            Some(EventFrame::Object { .. }) => {
                Err(Error::custom("object key required before value").into())
            }
            None if self.root_written => Err(Error::custom("multiple root values").into()),
            None => {
                self.root_written = true;
                Ok(())
            }
        }
    }

    pub(crate) fn finish(self) -> Result<(), E::Error> {
        if !self.root_written {
            return Err(Error::custom("encoder did not receive a root value").into());
        }
        if !self.frames.is_empty() {
            return Err(Error::custom("encoder finished inside a container").into());
        }
        Ok(())
    }
}

impl<E: FormatEncoder + ?Sized> FormatEncoder for CheckedEncoder<'_, E> {
    type Error = E::Error;

    fn begin_array(&mut self) -> Result<(), E::Error> {
        self.value()?;
        self.inner.begin_array()?;
        self.frames.push(EventFrame::Array { ready: false });
        Ok(())
    }

    fn separator(&mut self) -> Result<(), E::Error> {
        match self.frames.last_mut() {
            Some(EventFrame::Array { ready }) if !*ready => *ready = true,
            Some(EventFrame::Array { .. }) => {
                return Err(Error::custom("array value required after separator").into());
            }
            _ => return Err(Error::custom("array separator outside array").into()),
        }
        self.inner.separator()
    }

    fn end_array(&mut self) -> Result<(), E::Error> {
        match self.frames.last() {
            Some(EventFrame::Array { ready: false }) => {}
            Some(EventFrame::Array { ready: true }) => {
                return Err(Error::custom("array ended after separator without value").into());
            }
            Some(EventFrame::Object { .. }) => {
                return Err(Error::custom("mismatched array end inside object").into());
            }
            None => return Err(Error::custom("array end without matching start").into()),
        }
        self.inner.end_array()?;
        self.frames.pop();
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), E::Error> {
        self.value()?;
        self.inner.begin_object()?;
        self.frames.push(EventFrame::Object {
            pending_value: false,
        });
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), E::Error> {
        match self.frames.last_mut() {
            Some(EventFrame::Object { pending_value }) if !*pending_value => {
                *pending_value = true;
            }
            Some(EventFrame::Object { .. }) => {
                return Err(Error::custom("object value required after key").into());
            }
            _ => return Err(Error::custom("object key outside object").into()),
        }
        self.inner.key(key)
    }

    fn end_object(&mut self) -> Result<(), E::Error> {
        match self.frames.last() {
            Some(EventFrame::Object {
                pending_value: false,
            }) => {}
            Some(EventFrame::Object {
                pending_value: true,
            }) => return Err(Error::custom("object ended before keyed value").into()),
            Some(EventFrame::Array { .. }) => {
                return Err(Error::custom("mismatched object end inside array").into());
            }
            None => return Err(Error::custom("object end without matching start").into()),
        }
        self.inner.end_object()?;
        self.frames.pop();
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_null()
    }

    fn write_bool(&mut self, value: bool) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_bool(value)
    }

    fn write_str(&mut self, value: &str) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_str(value)
    }

    fn write_char(&mut self, value: char) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_char(value)
    }

    fn write_number(&mut self, value: &Number) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_number(value)
    }

    fn write_i64(&mut self, value: i64) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_i64(value)
    }

    fn write_u64(&mut self, value: u64) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_u64(value)
    }

    fn write_i128(&mut self, value: i128) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_i128(value)
    }

    fn write_u128(&mut self, value: u128) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_u128(value)
    }

    fn write_f64(&mut self, value: f64) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_f64(value)
    }

    fn write_f32(&mut self, value: f32) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_f32(value)
    }

    fn write_i8(&mut self, value: i8) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_i8(value)
    }

    fn write_i16(&mut self, value: i16) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_i16(value)
    }

    fn write_i32(&mut self, value: i32) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_i32(value)
    }

    fn write_u8(&mut self, value: u8) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_u8(value)
    }

    fn write_u16(&mut self, value: u16) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_u16(value)
    }

    fn write_u32(&mut self, value: u32) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_u32(value)
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_bytes(value)
    }

    fn write_none(&mut self) -> Result<(), E::Error> {
        self.value()?;
        self.inner.write_none()
    }

    fn write_some(&mut self) -> Result<(), E::Error> {
        // A `Some` payload is not itself a value: the following value consumes
        // the value slot, so no protocol transition happens here.
        self.inner.write_some()
    }

    fn map_key<K: NsonSerialize>(&mut self, key: &K) -> Result<(), E::Error> {
        match self.frames.last_mut() {
            Some(EventFrame::Object { pending_value }) if !*pending_value => {
                *pending_value = true;
            }
            Some(EventFrame::Object { .. }) => {
                return Err(Error::custom("object value required after key").into());
            }
            _ => return Err(Error::custom("object key outside object").into()),
        }
        self.inner.map_key(key)
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

impl<W: Write> FormatEncoder for Encoder<W> {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        Encoder::begin_array(self)
    }
    fn separator(&mut self) -> Result<(), Self::Error> {
        Encoder::separator(self)
    }
    fn end_array(&mut self) -> Result<(), Self::Error> {
        Encoder::end_array(self)
    }
    fn begin_object(&mut self) -> Result<(), Self::Error> {
        Encoder::begin_object(self)
    }
    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        Encoder::key(self, key)
    }
    fn end_object(&mut self) -> Result<(), Self::Error> {
        Encoder::end_object(self)
    }
    fn write_null(&mut self) -> Result<(), Self::Error> {
        Encoder::write_null(self)
    }
    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        Encoder::write_bool(self, value)
    }
    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        Encoder::write_str(self, value)
    }
    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        Encoder::write_char(self, value)
    }
    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error> {
        Encoder::write_number(self, value)
    }
    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        Encoder::write_i64(self, value)
    }
    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        Encoder::write_u64(self, value)
    }
    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        Encoder::write_i128(self, value)
    }
    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        Encoder::write_u128(self, value)
    }
    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        Encoder::write_f64(self, value)
    }
    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        Encoder::write_f32(self, value)
    }
    fn write_i8(&mut self, value: i8) -> Result<(), Self::Error> {
        Encoder::write_i64(self, value as i64)
    }
    fn write_i16(&mut self, value: i16) -> Result<(), Self::Error> {
        Encoder::write_i64(self, value as i64)
    }
    fn write_i32(&mut self, value: i32) -> Result<(), Self::Error> {
        Encoder::write_i64(self, value as i64)
    }
    fn write_u8(&mut self, value: u8) -> Result<(), Self::Error> {
        Encoder::write_u64(self, value as u64)
    }
    fn write_u16(&mut self, value: u16) -> Result<(), Self::Error> {
        Encoder::write_u64(self, value as u64)
    }
    fn write_u32(&mut self, value: u32) -> Result<(), Self::Error> {
        Encoder::write_u64(self, value as u64)
    }
    fn write_none(&mut self) -> Result<(), Self::Error> {
        Encoder::write_null(self)
    }
}

/// Serialization trait: nextencode `Self` into any [`FormatEncoder`].
///
/// The format is a generic parameter, so method bodies are monomorphized at
/// compile time with no dynamic dispatch. JSON, CBOR, and every codec in
/// [`crate::formats`] implement [`FormatEncoder`].
pub trait NsonSerialize: NsonSchema {
    /// Next-encode `self` into `encoder`.
    ///
    /// Errors are produced in the target format's own error type
    /// ([`FormatEncoder::Error`]).
    fn nextencode<E: FormatEncoder>(&self, encoder: &mut E) -> Result<(), E::Error>;
}

/// JSON encoder with internal buffering and indentation state.
///
/// All bytes are buffered in an internal `Vec<u8>` and flushed to `W` once a
/// threshold is crossed, keeping memory bounded.
pub struct Encoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    depth: usize,
    frames: Vec<EncodeFrame>,
    root_written: bool,
    pretty: bool,
    indent: &'static str,
    escape_non_ascii: bool,
    flush_threshold: usize,
}

enum EncodeFrame {
    Array { first: bool, ready: bool },
    Object { first: bool, pending_value: bool },
}

const FLUSH_THRESHOLD: usize = 8192;

impl<W: Write> Encoder<W> {
    /// Create an encoder with the default (compact) config.
    pub fn new(writer: W) -> Self {
        Encoder::with_config(writer, EncodeConfig::default())
    }

    /// Create an encoder with the given config.
    pub fn with_config(writer: W, config: EncodeConfig) -> Self {
        Encoder {
            writer,
            buf: Vec::with_capacity(1024),
            depth: 0,
            frames: Vec::new(),
            root_written: false,
            pretty: config.pretty,
            indent: config.indent,
            escape_non_ascii: config.escape_non_ascii,
            flush_threshold: FLUSH_THRESHOLD,
        }
    }

    /// Flush the internal buffer and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        self.validate_finished()?;
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }

    /// Flush the internal buffer without consuming self.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.write_all(&self.buf)?;
        self.buf.clear();
        self.writer.flush()
    }

    #[inline]
    fn maybe_flush(&mut self) -> Result<()> {
        if self.buf.len() >= self.flush_threshold {
            self.writer.write_all(&self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // container primitives
    // -----------------------------------------------------------------

    /// Open an object: write `{`.
    pub fn begin_object(&mut self) -> Result<()> {
        self.start_value()?;
        self.buf.push(b'{');
        self.frames.push(EncodeFrame::Object {
            first: true,
            pending_value: false,
        });
        self.depth += 1;
        if self.pretty {
            self.buf.push(b'\n');
            self.write_indent();
        }
        self.maybe_flush()
    }

    /// Close an object: write `}`.
    pub fn end_object(&mut self) -> Result<()> {
        match self.frames.last() {
            Some(EncodeFrame::Object {
                pending_value: false,
                ..
            }) => {}
            Some(EncodeFrame::Object {
                pending_value: true,
                ..
            }) => return Err(Error::custom("object ended before keyed value")),
            Some(EncodeFrame::Array { .. }) => {
                return Err(Error::custom("mismatched object end inside array"));
            }
            None => return Err(Error::custom("object end without matching start")),
        }
        self.frames.pop();
        self.depth -= 1;
        if self.pretty {
            self.buf.push(b'\n');
            self.write_indent();
        }
        self.buf.push(b'}');
        self.maybe_flush()
    }

    /// Open an array: write `[`.
    pub fn begin_array(&mut self) -> Result<()> {
        self.start_value()?;
        self.buf.push(b'[');
        self.frames.push(EncodeFrame::Array {
            first: true,
            ready: false,
        });
        self.depth += 1;
        if self.pretty {
            self.buf.push(b'\n');
            self.write_indent();
        }
        self.maybe_flush()
    }

    /// Close an array: write `]`.
    pub fn end_array(&mut self) -> Result<()> {
        match self.frames.last() {
            Some(EncodeFrame::Array { ready: false, .. }) => {}
            Some(EncodeFrame::Array { ready: true, .. }) => {
                return Err(Error::custom("array ended after separator without value"));
            }
            Some(EncodeFrame::Object { .. }) => {
                return Err(Error::custom("mismatched array end inside object"));
            }
            None => return Err(Error::custom("array end without matching start")),
        }
        self.frames.pop();
        self.depth -= 1;
        if self.pretty {
            self.buf.push(b'\n');
            self.write_indent();
        }
        self.buf.push(b']');
        self.maybe_flush()
    }

    /// Write an object key: separator + `"key":`.
    pub fn key(&mut self, key: &str) -> Result<()> {
        let first = match self.frames.last_mut() {
            Some(EncodeFrame::Object {
                first,
                pending_value,
            }) if !*pending_value => {
                *pending_value = true;
                core::mem::replace(first, false)
            }
            Some(EncodeFrame::Object { .. }) => {
                return Err(Error::custom("object value required after key"));
            }
            _ => return Err(Error::custom("object key outside object")),
        };
        self.write_separator(first);
        self.write_str_inner(key)?;
        self.buf.push(b':');
        if self.pretty {
            self.buf.push(b' ');
        }
        Ok(())
    }

    /// Write an element / key separator.
    ///
    /// The first entry of a container produces nothing; subsequent entries
    /// produce `,` (plus newline and indent in pretty mode).
    pub fn separator(&mut self) -> Result<()> {
        let first = match self.frames.last_mut() {
            Some(EncodeFrame::Array { first, ready }) if !*ready => {
                *ready = true;
                core::mem::replace(first, false)
            }
            Some(EncodeFrame::Array { .. }) => {
                return Err(Error::custom("array value required after separator"));
            }
            _ => return Err(Error::custom("array separator outside array")),
        };
        self.write_separator(first);
        Ok(())
    }

    fn write_separator(&mut self, first: bool) {
        if !first {
            self.buf.push(b',');
            if self.pretty {
                self.buf.push(b'\n');
                self.write_indent();
            }
        }
    }

    fn start_value(&mut self) -> Result<()> {
        match self.frames.last_mut() {
            Some(EncodeFrame::Array { ready, .. }) if *ready => {
                *ready = false;
                Ok(())
            }
            Some(EncodeFrame::Array { .. }) => {
                Err(Error::custom("array separator required before value"))
            }
            Some(EncodeFrame::Object { pending_value, .. }) if *pending_value => {
                *pending_value = false;
                Ok(())
            }
            Some(EncodeFrame::Object { .. }) => {
                Err(Error::custom("object key required before value"))
            }
            None if self.root_written => Err(Error::custom("multiple root values")),
            None => {
                self.root_written = true;
                Ok(())
            }
        }
    }

    fn validate_finished(&self) -> Result<()> {
        if !self.root_written {
            return Err(Error::custom("encoder did not receive a root value"));
        }
        if !self.frames.is_empty() {
            return Err(Error::custom("encoder finished inside a container"));
        }
        Ok(())
    }

    #[inline]
    fn write_indent(&mut self) {
        for _ in 0..self.depth {
            self.buf.extend_from_slice(self.indent.as_bytes());
        }
    }

    // -----------------------------------------------------------------
    // scalar primitives
    // -----------------------------------------------------------------

    /// Write `null`.
    pub fn write_null(&mut self) -> Result<()> {
        self.start_value()?;
        self.buf.extend_from_slice(b"null");
        self.maybe_flush()
    }

    /// Write a boolean.
    pub fn write_bool(&mut self, v: bool) -> Result<()> {
        self.start_value()?;
        self.buf
            .extend_from_slice(if v { b"true" } else { b"false" });
        self.maybe_flush()
    }

    /// Write a string (auto-escaped).
    pub fn write_str(&mut self, s: &str) -> Result<()> {
        self.start_value()?;
        self.write_str_inner(s)?;
        self.maybe_flush()
    }

    /// Write a character.
    pub fn write_char(&mut self, c: char) -> Result<()> {
        let mut buf = [0u8; 4];
        self.write_str(c.encode_utf8(&mut buf))
    }

    /// Write a number.
    pub fn write_number(&mut self, n: &Number) -> Result<()> {
        match *n {
            Number::I64(v) => self.write_i64(v),
            Number::U64(v) => self.write_u64(v),
            Number::I128(v) => self.write_i128(v),
            Number::U128(v) => self.write_u128(v),
            Number::F64(v) => self.write_f64(v),
        }
    }

    /// Write an `i64`.
    pub fn write_i64(&mut self, v: i64) -> Result<()> {
        self.start_value()?;
        write_signed_integer_into(&mut self.buf, v as i128);
        self.maybe_flush()
    }

    /// Write a `u64`.
    pub fn write_u64(&mut self, v: u64) -> Result<()> {
        self.start_value()?;
        write_unsigned_integer_into(&mut self.buf, v as u128);
        self.maybe_flush()
    }

    /// Write an `i128`.
    pub fn write_i128(&mut self, v: i128) -> Result<()> {
        self.start_value()?;
        write_signed_integer_into(&mut self.buf, v);
        self.maybe_flush()
    }

    /// Write a `u128`.
    pub fn write_u128(&mut self, v: u128) -> Result<()> {
        self.start_value()?;
        write_unsigned_integer_into(&mut self.buf, v);
        self.maybe_flush()
    }

    /// Write an `f64` (shortest round-trip; non-finite values error).
    ///
    /// Integral floats are written as `1.0` rather than `1` so float-ness
    /// survives round-trips.
    pub fn write_f64(&mut self, v: f64) -> Result<()> {
        if !v.is_finite() {
            return Err(Error::new(ErrorKind::NonFiniteFloat, None, None, 0));
        }
        self.start_value()?;
        write_float_into(&mut self.buf, v)?;
        self.maybe_flush()
    }

    /// Write an `f32` using its shortest round-trip representation.
    pub fn write_f32(&mut self, v: f32) -> Result<()> {
        if !v.is_finite() {
            return Err(Error::new(ErrorKind::NonFiniteFloat, None, None, 0));
        }
        self.start_value()?;
        write_float_into(&mut self.buf, v)?;
        self.maybe_flush()
    }

    fn write_str_inner(&mut self, s: &str) -> Result<()> {
        self.buf.push(b'"');
        let bytes = s.as_bytes();
        if bytes.iter().all(|&byte| {
            byte >= 0x20 && byte != b'"' && byte != b'\\' && (!self.escape_non_ascii || byte < 0x80)
        }) {
            self.buf.extend_from_slice(bytes);
            self.buf.push(b'"');
            return Ok(());
        }
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b'"' => self.buf.extend_from_slice(b"\\\""),
                b'\\' => self.buf.extend_from_slice(b"\\\\"),
                0x08 => self.buf.extend_from_slice(b"\\b"),
                0x0C => self.buf.extend_from_slice(b"\\f"),
                b'\n' => self.buf.extend_from_slice(b"\\n"),
                b'\r' => self.buf.extend_from_slice(b"\\r"),
                b'\t' => self.buf.extend_from_slice(b"\\t"),
                0x00..=0x1F => {
                    self.buf.extend_from_slice(b"\\u00");
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    self.buf.push(HEX[(b >> 4) as usize]);
                    self.buf.push(HEX[(b & 0xF) as usize]);
                }
                _ if self.escape_non_ascii && b >= 0x80 => {
                    let ch = s[i..].chars().next().expect("valid utf-8");
                    write_unicode_escape(&mut self.buf, ch);
                    i += ch.len_utf8();
                    continue;
                }
                _ => self.buf.push(b),
            }
            i += 1;
        }
        self.buf.push(b'"');
        Ok(())
    }
}

impl Encoder<Vec<u8>> {
    pub(crate) fn for_vec(config: EncodeConfig) -> Self {
        let mut encoder = Encoder::with_config(Vec::new(), config);
        encoder.flush_threshold = usize::MAX;
        encoder
    }

    pub(crate) fn finish_vec(mut self) -> Result<Vec<u8>> {
        self.validate_finished()?;
        debug_assert!(self.writer.is_empty());
        Ok(core::mem::take(&mut self.buf))
    }
}

/// Write a char as `\uXXXX` (surrogate pair when needed).
fn write_unicode_escape(buf: &mut Vec<u8>, ch: char) {
    fn hex4(buf: &mut Vec<u8>, cp: u32) {
        buf.extend_from_slice(b"\\u");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        buf.push(HEX[((cp >> 12) & 0xF) as usize]);
        buf.push(HEX[((cp >> 8) & 0xF) as usize]);
        buf.push(HEX[((cp >> 4) & 0xF) as usize]);
        buf.push(HEX[(cp & 0xF) as usize]);
    }
    let cp = ch as u32;
    if cp <= 0xFFFF {
        hex4(buf, cp);
    } else {
        let v = cp - 0x10000;
        hex4(buf, 0xD800 + (v >> 10));
        hex4(buf, 0xDC00 + (v & 0x3FF));
    }
}

/// Integer output using a stack buffer (no allocation).
fn write_unsigned_integer_into(buf: &mut Vec<u8>, mut value: u128) {
    let mut digits = [0_u8; 39];
    let mut cursor = digits.len();
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    buf.extend_from_slice(&digits[cursor..]);
}

fn write_signed_integer_into(buf: &mut Vec<u8>, value: i128) {
    if value < 0 {
        buf.push(b'-');
        write_unsigned_integer_into(buf, value.wrapping_neg() as u128);
    } else {
        write_unsigned_integer_into(buf, value as u128);
    }
}

struct FloatBuffer {
    bytes: [u8; 64],
    len: usize,
}

impl FloatBuffer {
    fn new() -> Self {
        FloatBuffer {
            bytes: [0; 64],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl fmt::Write for FloatBuffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        let output = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        output.copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn write_float_into<T: fmt::Display>(buf: &mut Vec<u8>, value: T) -> Result<()> {
    let mut formatted = FloatBuffer::new();
    core::write!(&mut formatted, "{value}")
        .map_err(|_| Error::custom("internal float formatting buffer exhausted"))?;
    let bytes = formatted.as_bytes();
    buf.extend_from_slice(bytes);
    if !bytes.iter().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        buf.extend_from_slice(b".0");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// std / alloc NsonSchema + NsonSerialize implementations
// ---------------------------------------------------------------------------

macro_rules! impl_scalar {
    ($($t:ty => $schema:expr => $write:ident => $cast_to:ty),* $(,)?) => {$(
        impl NsonSchema for $t {
            const SCHEMA: TypeSchema = $schema;
        }
        impl NsonSerialize for $t {
            fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
                e.$write(*self as $cast_to)
            }
        }
    )*};
}

impl_scalar! {
    bool => TypeSchema::Bool => write_bool => bool,
    i8 => TypeSchema::I8 => write_i8 => i8,
    i16 => TypeSchema::I16 => write_i16 => i16,
    i32 => TypeSchema::I32 => write_i32 => i32,
    i64 => TypeSchema::I64 => write_i64 => i64,
    i128 => TypeSchema::I128 => write_i128 => i128,
    isize => TypeSchema::Isize => write_i64 => i64,
    u8 => TypeSchema::U8 => write_u8 => u8,
    u16 => TypeSchema::U16 => write_u16 => u16,
    u32 => TypeSchema::U32 => write_u32 => u32,
    u64 => TypeSchema::U64 => write_u64 => u64,
    u128 => TypeSchema::U128 => write_u128 => u128,
    usize => TypeSchema::Usize => write_u64 => u64,
    f32 => TypeSchema::F32 => write_f32 => f32,
    f64 => TypeSchema::F64 => write_f64 => f64,
}

impl NsonSchema for char {
    const SCHEMA: TypeSchema = TypeSchema::Char;
}
impl NsonSerialize for char {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_char(*self)
    }
}

impl NsonSchema for str {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
impl NsonSerialize for str {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(self)
    }
}

impl NsonSchema for String {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
impl NsonSerialize for String {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(self)
    }
}

impl<'a> NsonSchema for Cow<'a, str> {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
impl<'a> NsonSerialize for Cow<'a, str> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(self)
    }
}

impl<T: NsonSerialize + ?Sized> NsonSchema for Box<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + ?Sized> NsonSerialize for Box<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        T::nextencode(self, e)
    }
}

impl<T: NsonSerialize + ?Sized> NsonSchema for &T {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + ?Sized> NsonSerialize for &T {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        T::nextencode(*self, e)
    }
}

impl<T: NsonSerialize + ?Sized> NsonSchema for &mut T {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + ?Sized> NsonSerialize for &mut T {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        T::nextencode(&**self, e)
    }
}

impl<T: NsonSerialize> NsonSchema for Rc<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize> NsonSerialize for Rc<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        T::nextencode(self, e)
    }
}

impl<T: NsonSerialize> NsonSchema for Arc<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize> NsonSerialize for Arc<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        T::nextencode(self, e)
    }
}

impl<T: NsonSerialize + Copy> NsonSchema for Cell<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + Copy> NsonSerialize for Cell<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        T::nextencode(&self.get(), e)
    }
}

impl<T: NsonSerialize> NsonSchema for RefCell<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize> NsonSerialize for RefCell<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        T::nextencode(&self.borrow(), e)
    }
}

impl<T: NsonSerialize> NsonSchema for Option<T> {
    const SCHEMA: TypeSchema = TypeSchema::Optional(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for Option<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        match self {
            Some(v) => {
                e.write_some()?;
                T::nextencode(v, e)
            }
            None => e.write_none(),
        }
    }
}

impl<T: NsonSerialize, E: NsonSerialize> NsonSchema for core::result::Result<T, E> {
    const SCHEMA: TypeSchema = TypeSchema::Enum(&crate::schema::EnumSchema {
        name: "Result",
        tag: None,
        content: None,
        untagged: false,
        default_tag: "type",
        variants: &[
            crate::schema::VariantSchema {
                name: "Ok",
                orig: "Ok",
                ty: T::SCHEMA,
            },
            crate::schema::VariantSchema {
                name: "Err",
                orig: "Err",
                ty: E::SCHEMA,
            },
        ],
    });
}
impl<T: NsonSerialize, E: NsonSerialize> NsonSerialize for core::result::Result<T, E> {
    fn nextencode<__E: FormatEncoder>(&self, e: &mut __E) -> Result<(), __E::Error> {
        e.begin_object()?;
        match self {
            Ok(v) => {
                e.key("Ok")?;
                T::nextencode(v, e)?;
            }
            Err(v) => {
                e.key("Err")?;
                E::nextencode(v, e)?;
            }
        }
        e.end_object()
    }
}

impl<T: NsonSerialize> NsonSchema for Vec<T> {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for Vec<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for [T] {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for [T] {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

impl<T: NsonSerialize, const N: usize> NsonSchema for [T; N] {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize, const N: usize> NsonSerialize for [T; N] {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

// ---------------------------------------------------------------------------
// byte sequences use the dedicated `write_bytes` / `bytes()` event primitives.
//
// As in serde, `Vec<u8>` / `&[u8]` / `[u8; N]` deliberately keep the generic
// sequence implementation (an array of `u8`), so no conflicting impls arise.
// Types that want a native compact byte string on the wire use the
// [`crate::Bytes`] wrapper, which routes through `write_bytes`.
// ---------------------------------------------------------------------------

impl<T: NsonSerialize> NsonSchema for VecDeque<T> {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for VecDeque<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for LinkedList<T> {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for LinkedList<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for BTreeSet<T> {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for BTreeSet<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

impl<T: NsonSerialize + Ord> NsonSchema for BinaryHeap<T> {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize + Ord> NsonSerialize for BinaryHeap<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

impl<K: NsonSerialize, V: NsonSerialize> NsonSchema for BTreeMap<K, V> {
    const SCHEMA: TypeSchema = TypeSchema::Map(&V::SCHEMA);
}
impl<K: NsonSerialize, V: NsonSerialize> NsonSerialize for BTreeMap<K, V> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_object()?;
        for (k, v) in self {
            e.map_key(k)?;
            V::nextencode(v, e)?;
        }
        e.end_object()
    }
}

#[cfg(feature = "std")]
impl<K: NsonSerialize + core::hash::Hash + Eq, V: NsonSerialize> NsonSchema
    for std::collections::HashMap<K, V>
{
    const SCHEMA: TypeSchema = TypeSchema::Map(&V::SCHEMA);
}
#[cfg(feature = "std")]
impl<K: NsonSerialize + core::hash::Hash + Eq, V: NsonSerialize> NsonSerialize
    for std::collections::HashMap<K, V>
{
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_object()?;
        for (k, v) in self {
            e.map_key(k)?;
            V::nextencode(v, e)?;
        }
        e.end_object()
    }
}

#[cfg(feature = "std")]
impl<T: NsonSerialize + core::hash::Hash + Eq> NsonSchema for std::collections::HashSet<T> {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
#[cfg(feature = "std")]
impl<T: NsonSerialize + core::hash::Hash + Eq> NsonSerialize for std::collections::HashSet<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

/// Convert a map key to a string.
///
/// String-like keys keep their text; scalar keys (numbers, booleans) use
/// their JSON spelling as the key text (matching serde_json). The default
/// [`FormatEncoder::map_key`] implementation routes through this.
fn key_to_str<K: NsonSerialize>(k: &K) -> Result<String> {
    let mut encoder = Encoder::new(Vec::new());
    K::nextencode(k, &mut encoder)?;
    let bytes = encoder.finish()?;
    if bytes.first() == Some(&b'"') {
        let mut d = crate::de::Decoder::new(&bytes);
        match d.string()? {
            Cow::Borrowed(s) => Ok(s.to_string()),
            Cow::Owned(s) => Ok(s),
        }
    } else {
        // A scalar key: `1`, `true`, ... becomes the string `"1"`, `"true"`.
        String::from_utf8(bytes)
            .map_err(|_| Error::custom("map key must serialize to a string or scalar"))
    }
}

// ---------------------------------------------------------------------------
// tuples / unit / PhantomData / common types
// ---------------------------------------------------------------------------

impl NsonSchema for () {
    const SCHEMA: TypeSchema = TypeSchema::Unit;
}
impl NsonSerialize for () {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_null()
    }
}

impl<T: ?Sized> NsonSchema for PhantomData<T> {
    const SCHEMA: TypeSchema = TypeSchema::Unit;
}
impl<T: ?Sized> NsonSerialize for PhantomData<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_null()
    }
}

impl NsonSchema for Duration {
    const SCHEMA: TypeSchema = TypeSchema::U128;
}
impl NsonSerialize for Duration {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_u128(self.as_nanos())
    }
}

#[cfg(feature = "std")]
impl NsonSchema for std::path::Path {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::path::Path {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(&self.to_string_lossy())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::path::PathBuf {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::path::PathBuf {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        self.as_path().nextencode(e)
    }
}

#[cfg(feature = "std")]
impl NsonSchema for std::net::IpAddr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::IpAddr {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(&self.to_string())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::net::Ipv4Addr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::Ipv4Addr {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(&self.to_string())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::net::Ipv6Addr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::Ipv6Addr {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(&self.to_string())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::net::SocketAddr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::SocketAddr {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_str(&self.to_string())
    }
}

impl<T: NsonSerialize> NsonSchema for Range<T> {
    const SCHEMA: TypeSchema = TypeSchema::Tuple(&[T::SCHEMA, T::SCHEMA]);
}
impl<T: NsonSerialize> NsonSerialize for Range<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        e.separator()?;
        T::nextencode(&self.start, e)?;
        e.separator()?;
        T::nextencode(&self.end, e)?;
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for RangeInclusive<T> {
    const SCHEMA: TypeSchema = TypeSchema::Tuple(&[T::SCHEMA, T::SCHEMA]);
}
impl<T: NsonSerialize> NsonSerialize for RangeInclusive<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        e.separator()?;
        T::nextencode(self.start(), e)?;
        e.separator()?;
        T::nextencode(self.end(), e)?;
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for RangeFrom<T> {
    const SCHEMA: TypeSchema = TypeSchema::Tuple(&[T::SCHEMA]);
}
impl<T: NsonSerialize> NsonSerialize for RangeFrom<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        e.separator()?;
        T::nextencode(&self.start, e)?;
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for RangeTo<T> {
    const SCHEMA: TypeSchema = TypeSchema::Tuple(&[T::SCHEMA]);
}
impl<T: NsonSerialize> NsonSerialize for RangeTo<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        e.separator()?;
        T::nextencode(&self.end, e)?;
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for RangeToInclusive<T> {
    const SCHEMA: TypeSchema = TypeSchema::Tuple(&[T::SCHEMA]);
}
impl<T: NsonSerialize> NsonSerialize for RangeToInclusive<T> {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_array()?;
        e.separator()?;
        T::nextencode(&self.end, e)?;
        e.end_array()
    }
}

macro_rules! impl_atomic {
    ($($t:ty => $inner:ty),* $(,)?) => {$(
        impl NsonSchema for $t {
            const SCHEMA: TypeSchema = <$inner as NsonSchema>::SCHEMA;
        }
        impl NsonSerialize for $t {
            fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
                let v = self.load(core::sync::atomic::Ordering::Relaxed);
                <$inner as NsonSerialize>::nextencode(&v, e)
            }
        }
    )*};
}
impl_atomic! {
    core::sync::atomic::AtomicBool => bool,
    core::sync::atomic::AtomicI8 => i8,
    core::sync::atomic::AtomicI16 => i16,
    core::sync::atomic::AtomicI32 => i32,
    core::sync::atomic::AtomicI64 => i64,
    core::sync::atomic::AtomicIsize => isize,
    core::sync::atomic::AtomicU8 => u8,
    core::sync::atomic::AtomicU16 => u16,
    core::sync::atomic::AtomicU32 => u32,
    core::sync::atomic::AtomicU64 => u64,
    core::sync::atomic::AtomicUsize => usize,
}

macro_rules! impl_tuple_ser {
    ($(($first:ident : $First:ident $(, $i:ident : $T:ident)*)),* $(,)?) => {$(
        impl<$First: NsonSerialize $(, $T: NsonSerialize)*> NsonSchema for ($First, $( $T, )*) {
            const SCHEMA: TypeSchema = TypeSchema::Tuple(&[$First::SCHEMA, $( $T::SCHEMA, )*]);
        }
        impl<$First: NsonSerialize $(, $T: NsonSerialize)*> NsonSerialize for ($First, $( $T, )*) {
            #[allow(non_snake_case)]
            fn nextencode<__E: FormatEncoder>(&self, e: &mut __E) -> Result<(), __E::Error> {
                let ($first, $( $i, )*) = self;
                e.begin_array()?;
                e.separator()?;
                $First::nextencode($first, e)?;
                $(
                    e.separator()?;
                    $T::nextencode($i, e)?;
                )*
                e.end_array()
            }
        }
    )*};
}

impl_tuple_ser! {
    (a: A),
    (a: A, b: B),
    (a: A, b: B, c: C),
    (a: A, b: B, c: C, d: D),
    (a: A, b: B, c: C, d: D, e: E),
    (a: A, b: B, c: C, d: D, e: E, f: F),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K),
    (a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K, l: L),
}

// ---------------------------------------------------------------------------
// Number / Map / Value
// ---------------------------------------------------------------------------

impl NsonSchema for Number {
    const SCHEMA: TypeSchema = TypeSchema::Opaque;
}
impl NsonSerialize for Number {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.write_number(self)
    }
}

impl NsonSchema for Map {
    const SCHEMA: TypeSchema = TypeSchema::Map(&TypeSchema::Opaque);
}
impl NsonSerialize for Map {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        e.begin_object()?;
        for (k, v) in self.iter() {
            e.key(k)?;
            NsonSerialize::nextencode(v, e)?;
        }
        e.end_object()
    }
}

impl NsonSchema for Value {
    const SCHEMA: TypeSchema = TypeSchema::Opaque;
}
impl NsonSerialize for Value {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        match self {
            Value::Null => e.write_null(),
            Value::Bool(b) => e.write_bool(*b),
            Value::Number(n) => e.write_number(n),
            Value::String(s) => e.write_str(s),
            Value::Array(a) => {
                e.begin_array()?;
                for v in a {
                    e.separator()?;
                    NsonSerialize::nextencode(v, e)?;
                }
                e.end_array()
            }
            Value::Object(m) => NsonSerialize::nextencode(m, e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_formatting_basics() {
        let mut buf = Vec::new();
        write_unsigned_integer_into(&mut buf, 0);
        assert_eq!(buf, b"0");
        let mut buf = Vec::new();
        write_unsigned_integer_into(&mut buf, 12345);
        assert_eq!(buf, b"12345");
        let mut buf = Vec::new();
        write_unsigned_integer_into(&mut buf, u64::MAX as u128);
        assert_eq!(buf, b"18446744073709551615");
        let mut buf = Vec::new();
        write_signed_integer_into(&mut buf, i128::MIN);
        assert_eq!(buf, b"-170141183460469231731687303715884105728");
    }

    #[test]
    fn string_escaping() {
        let mut e = Encoder::new(Vec::new());
        e.write_str("\"\\\n\t\u{1}\u{1f4a9}").unwrap();
        let out = e.finish().unwrap();
        assert_eq!(out, b"\"\\\"\\\\\\n\\t\\u0001\xf0\x9f\x92\xa9\"");
    }

    #[test]
    fn escape_non_ascii() {
        let mut e =
            Encoder::with_config(Vec::new(), EncodeConfig::default().escape_non_ascii(true));
        e.write_str("\u{e9}\u{1f4a9}").unwrap();
        let out = e.finish().unwrap();
        assert_eq!(out, b"\"\\u00e9\\ud83d\\udca9\"");
    }

    #[test]
    fn non_finite_errors() {
        let mut e = Encoder::new(Vec::new());
        assert!(e.write_f64(f64::NAN).is_err());
        let mut e = Encoder::new(Vec::new());
        assert!(e.write_f64(f64::INFINITY).is_err());
    }

    #[test]
    fn f32_uses_its_own_shortest_representation() {
        let mut encoder = Encoder::new(Vec::new());
        encoder.write_f32(1.2_f32).unwrap();
        assert_eq!(encoder.finish().unwrap(), b"1.2");
    }

    #[test]
    fn pretty_roundtrip() {
        let mut e = Encoder::with_config(Vec::new(), EncodeConfig::pretty());
        e.begin_object().unwrap();
        e.key("a").unwrap();
        e.write_i64(1).unwrap();
        e.key("b").unwrap();
        e.begin_array().unwrap();
        e.separator().unwrap();
        e.write_null().unwrap();
        e.end_array().unwrap();
        e.end_object().unwrap();
        let out = String::from_utf8(e.finish().unwrap()).unwrap();
        assert_eq!(out, "{\n  \"a\": 1,\n  \"b\": [\n    null\n  ]\n}");
    }

    #[test]
    fn rejects_invalid_encoding_event_order() {
        let mut encoder = Encoder::new(Vec::new());
        assert!(encoder.end_array().is_err());

        let mut encoder = Encoder::new(Vec::new());
        encoder.begin_array().unwrap();
        assert!(encoder.write_null().is_err());
        assert!(encoder.end_object().is_err());

        let mut encoder = Encoder::new(Vec::new());
        encoder.begin_object().unwrap();
        encoder.key("pending").unwrap();
        assert!(encoder.end_object().is_err());

        let mut encoder = Encoder::new(Vec::new());
        encoder.write_null().unwrap();
        assert!(encoder.write_bool(true).is_err());

        let encoder = Encoder::new(Vec::new());
        assert!(encoder.finish().is_err());
    }
}

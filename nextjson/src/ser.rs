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

use crate::error::{Error, ErrorKind, Result};
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

/// Serialization trait: nextencode `Self` into an [`Encoder`].
///
/// The writer is a generic parameter, so method bodies are monomorphized at
/// compile time with no dynamic dispatch.
pub trait NsonSerialize: NsonSchema {
    /// Next-encode `self` into `encoder`.
    fn nextencode<W: Write>(&self, encoder: &mut Encoder<W>) -> Result<()>;
}

/// JSON encoder with internal buffering and indentation state.
///
/// All bytes are buffered in an internal `Vec<u8>` and flushed to `W` once a
/// threshold is crossed, keeping memory bounded.
pub struct Encoder<W: Write> {
    writer: W,
    buf: Vec<u8>,
    depth: usize,
    first: Vec<bool>,
    pretty: bool,
    indent: &'static str,
    escape_non_ascii: bool,
    flush_threshold: usize,
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
            first: Vec::new(),
            pretty: config.pretty,
            indent: config.indent,
            escape_non_ascii: config.escape_non_ascii,
            flush_threshold: FLUSH_THRESHOLD,
        }
    }

    /// Flush the internal buffer and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
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
        self.buf.push(b'{');
        self.first.push(true);
        self.depth += 1;
        if self.pretty {
            self.buf.push(b'\n');
            self.write_indent();
        }
        self.maybe_flush()
    }

    /// Close an object: write `}`.
    pub fn end_object(&mut self) -> Result<()> {
        self.first.pop();
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
        self.buf.push(b'[');
        self.first.push(true);
        self.depth += 1;
        if self.pretty {
            self.buf.push(b'\n');
            self.write_indent();
        }
        self.maybe_flush()
    }

    /// Close an array: write `]`.
    pub fn end_array(&mut self) -> Result<()> {
        self.first.pop();
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
        self.separator()?;
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
        if let Some(top) = self.first.last_mut() {
            if *top {
                *top = false;
            } else {
                self.buf.push(b',');
                if self.pretty {
                    self.buf.push(b'\n');
                    self.write_indent();
                }
            }
        }
        Ok(())
    }

    /// Write raw bytes (used by `flatten` splicing).
    pub fn write_raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.buf.extend_from_slice(bytes);
        self.maybe_flush()
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
        self.buf.extend_from_slice(b"null");
        self.maybe_flush()
    }

    /// Write a boolean.
    pub fn write_bool(&mut self, v: bool) -> Result<()> {
        self.buf
            .extend_from_slice(if v { b"true" } else { b"false" });
        self.maybe_flush()
    }

    /// Write a string (auto-escaped).
    pub fn write_str(&mut self, s: &str) -> Result<()> {
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
        write_signed_integer_into(&mut self.buf, v as i128);
        self.maybe_flush()
    }

    /// Write a `u64`.
    pub fn write_u64(&mut self, v: u64) -> Result<()> {
        write_unsigned_integer_into(&mut self.buf, v as u128);
        self.maybe_flush()
    }

    /// Write an `i128`.
    pub fn write_i128(&mut self, v: i128) -> Result<()> {
        write_signed_integer_into(&mut self.buf, v);
        self.maybe_flush()
    }

    /// Write a `u128`.
    pub fn write_u128(&mut self, v: u128) -> Result<()> {
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
        write_float_into(&mut self.buf, v)?;
        self.maybe_flush()
    }

    /// Write an `f32` using its shortest round-trip representation.
    pub fn write_f32(&mut self, v: f32) -> Result<()> {
        if !v.is_finite() {
            return Err(Error::new(ErrorKind::NonFiniteFloat, None, None, 0));
        }
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

    pub(crate) fn finish_vec(mut self) -> Vec<u8> {
        debug_assert!(self.writer.is_empty());
        core::mem::take(&mut self.buf)
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
            fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
                e.$write(*self as $cast_to)
            }
        }
    )*};
}

impl_scalar! {
    bool => TypeSchema::Bool => write_bool => bool,
    i8 => TypeSchema::I8 => write_i64 => i64,
    i16 => TypeSchema::I16 => write_i64 => i64,
    i32 => TypeSchema::I32 => write_i64 => i64,
    i64 => TypeSchema::I64 => write_i64 => i64,
    i128 => TypeSchema::I128 => write_i128 => i128,
    isize => TypeSchema::Isize => write_i64 => i64,
    u8 => TypeSchema::U8 => write_u64 => u64,
    u16 => TypeSchema::U16 => write_u64 => u64,
    u32 => TypeSchema::U32 => write_u64 => u64,
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_char(*self)
    }
}

impl NsonSchema for str {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
impl NsonSerialize for str {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(self)
    }
}

impl NsonSchema for String {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
impl NsonSerialize for String {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(self)
    }
}

impl<'a> NsonSchema for Cow<'a, str> {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
impl<'a> NsonSerialize for Cow<'a, str> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(self)
    }
}

impl<T: NsonSerialize + ?Sized> NsonSchema for Box<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + ?Sized> NsonSerialize for Box<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        T::nextencode(self, e)
    }
}

impl<T: NsonSerialize + ?Sized> NsonSchema for &T {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + ?Sized> NsonSerialize for &T {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        T::nextencode(*self, e)
    }
}

impl<T: NsonSerialize + ?Sized> NsonSchema for &mut T {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + ?Sized> NsonSerialize for &mut T {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        T::nextencode(&**self, e)
    }
}

impl<T: NsonSerialize> NsonSchema for Rc<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize> NsonSerialize for Rc<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        T::nextencode(self, e)
    }
}

impl<T: NsonSerialize> NsonSchema for Arc<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize> NsonSerialize for Arc<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        T::nextencode(self, e)
    }
}

impl<T: NsonSerialize + Copy> NsonSchema for Cell<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize + Copy> NsonSerialize for Cell<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        T::nextencode(&self.get(), e)
    }
}

impl<T: NsonSerialize> NsonSchema for RefCell<T> {
    const SCHEMA: TypeSchema = T::SCHEMA;
}
impl<T: NsonSerialize> NsonSerialize for RefCell<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        T::nextencode(&self.borrow(), e)
    }
}

impl<T: NsonSerialize> NsonSchema for Option<T> {
    const SCHEMA: TypeSchema = TypeSchema::Optional(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for Option<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        match self {
            Some(v) => T::nextencode(v, e),
            None => e.write_null(),
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

impl<T: NsonSerialize> NsonSchema for VecDeque<T> {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&T::SCHEMA);
}
impl<T: NsonSerialize> NsonSerialize for VecDeque<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.begin_object()?;
        for (k, v) in self {
            let key = key_to_str(k)?;
            e.key(&key)?;
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.begin_object()?;
        for (k, v) in self {
            let key = key_to_str(k)?;
            e.key(&key)?;
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.begin_array()?;
        for item in self {
            e.separator()?;
            T::nextencode(item, e)?;
        }
        e.end_array()
    }
}

/// Convert a map key to a string (only string keys are supported).
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
        Err(Error::custom("map key must serialize to a JSON string"))
    }
}

// ---------------------------------------------------------------------------
// tuples / unit / PhantomData / common types
// ---------------------------------------------------------------------------

impl NsonSchema for () {
    const SCHEMA: TypeSchema = TypeSchema::Unit;
}
impl NsonSerialize for () {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_null()
    }
}

impl<T: ?Sized> NsonSchema for PhantomData<T> {
    const SCHEMA: TypeSchema = TypeSchema::Unit;
}
impl<T: ?Sized> NsonSerialize for PhantomData<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_null()
    }
}

impl NsonSchema for Duration {
    const SCHEMA: TypeSchema = TypeSchema::U64;
}
impl NsonSerialize for Duration {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_u64(self.as_nanos() as u64)
    }
}

#[cfg(feature = "std")]
impl NsonSchema for std::path::Path {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::path::Path {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(&self.to_string_lossy())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::path::PathBuf {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::path::PathBuf {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        self.as_path().nextencode(e)
    }
}

#[cfg(feature = "std")]
impl NsonSchema for std::net::IpAddr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::IpAddr {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(&self.to_string())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::net::Ipv4Addr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::Ipv4Addr {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(&self.to_string())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::net::Ipv6Addr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::Ipv6Addr {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(&self.to_string())
    }
}
#[cfg(feature = "std")]
impl NsonSchema for std::net::SocketAddr {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}
#[cfg(feature = "std")]
impl NsonSerialize for std::net::SocketAddr {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_str(&self.to_string())
    }
}

impl<T: NsonSerialize> NsonSchema for Range<T> {
    const SCHEMA: TypeSchema = TypeSchema::Tuple(&[T::SCHEMA, T::SCHEMA]);
}
impl<T: NsonSerialize> NsonSerialize for Range<T> {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
            fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
                let v = self.load(core::sync::atomic::Ordering::SeqCst);
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
            fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
        e.write_number(self)
    }
}

impl NsonSchema for Map {
    const SCHEMA: TypeSchema = TypeSchema::Map(&TypeSchema::Opaque);
}
impl NsonSerialize for Map {
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
    fn nextencode<W: Write>(&self, e: &mut Encoder<W>) -> Result<()> {
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
}

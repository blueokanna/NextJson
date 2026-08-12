//! Shared relay between parsed [`Value`] trees and the unified token stream.
//!
//! Document-oriented formats (CBOR, envy, pickle, TOML, YAML) parse their
//! input into a [`Value`] tree first, then replay that tree through the
//! crate's token stream so the generic `NsonDeserialize` machinery can drive
//! it. The tree→token conversion and the delegating [`FormatDecoder`] wrapper
//! are identical for every such codec, so they live here once instead of
//! being duplicated per format.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::de::{Decoder, FormatDecoder, Mark, Token};
use crate::error::{Error, Result};
use crate::map::Map;
use crate::number::Number;
use crate::ser::FormatEncoder;
use crate::value::Value;

/// Format a [`Number`] as a plain decimal string (the shared scalar
/// representation used by the text-emitting codecs).
pub(crate) fn number_string(n: &Number) -> String {
    match n {
        Number::I64(v) => v.to_string(),
        Number::U64(v) => v.to_string(),
        Number::I128(v) => v.to_string(),
        Number::U128(v) => v.to_string(),
        Number::F64(v) => v.to_string(),
    }
}

enum Builder {
    Array(Vec<Value>),
    Object {
        map: Map,
        pending_key: Option<String>,
    },
    Root,
}

/// Streaming encoder that buffers the event stream into a [`Value`] tree.
///
/// Document-shaped formats (TOML, YAML) must emit scalars before their
/// subtables, so they collect the whole event stream into a tree and
/// serialize it when the root closes. The collection itself is
/// format-neutral and lives here once; each format keeps only its emitter.
///
/// `null` is collected like any other value; codecs without a null type
/// (TOML) reject it when emitting.
pub(crate) struct CollectEncoder {
    stack: Vec<Builder>,
    root: Option<Value>,
}

impl CollectEncoder {
    /// Create an empty event collector.
    pub(crate) fn new() -> Self {
        CollectEncoder {
            stack: vec![Builder::Root],
            root: None,
        }
    }

    /// Take the collected root tree (called by the format's `encode`).
    pub(crate) fn take_root(&mut self) -> Result<Value> {
        if self.stack.len() != 1 || !matches!(self.stack.last(), Some(Builder::Root)) {
            return Err(Error::custom("collector: unfinished container"));
        }
        self.root
            .take()
            .ok_or_else(|| Error::custom("collector: no root value"))
    }

    fn attach(&mut self, value: Value) -> Result<()> {
        match self.stack.last_mut() {
            Some(Builder::Array(items)) => items.push(value),
            Some(Builder::Object { map, pending_key }) => {
                let key = pending_key
                    .take()
                    .ok_or_else(|| Error::custom("collector: object value has no key"))?;
                if map.insert(key, value).is_some() {
                    return Err(Error::custom("collector: duplicate object key"));
                }
            }
            Some(Builder::Root) if self.root.is_none() => self.root = Some(value),
            Some(Builder::Root) => return Err(Error::custom("collector: multiple root values")),
            None => return Err(Error::custom("collector: value outside root")),
        }
        Ok(())
    }
}

impl FormatEncoder for CollectEncoder {
    type Error = crate::error::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.stack.push(Builder::Array(Vec::new()));
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        match self.stack.pop() {
            Some(Builder::Array(items)) => self.attach(Value::Array(items)),
            _ => Err(Error::custom("collector: array end without start")),
        }
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.stack.push(Builder::Object {
            map: Map::new(),
            pending_key: None,
        });
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        match self.stack.last_mut() {
            Some(Builder::Object { map, pending_key }) => {
                if pending_key.is_some() {
                    return Err(Error::custom("collector: previous key has no value"));
                }
                if map.contains_key(key) {
                    return Err(Error::custom("collector: duplicate object key"));
                }
                *pending_key = Some(key.to_string());
                Ok(())
            }
            _ => Err(Error::custom("collector: key outside object")),
        }
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        match self.stack.pop() {
            Some(Builder::Object { map, pending_key }) if pending_key.is_none() => {
                self.attach(Value::Object(map))
            }
            Some(Builder::Object { .. }) => {
                Err(Error::custom("collector: object key has no value"))
            }
            _ => Err(Error::custom("collector: object end without start")),
        }
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.attach(Value::Null)
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_number(&mut self, value: &Number) -> Result<(), Self::Error> {
        self.attach(Value::from(*value))
    }

    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        self.attach(Value::from(value))
    }
}

macro_rules! impl_collecting_format_encoder {
    ($encoder:ident) => {
        impl<W: $crate::write::Write> $crate::ser::FormatEncoder for $encoder<W> {
            type Error = $crate::error::Error;

            fn begin_array(&mut self) -> $crate::Result<(), Self::Error> {
                self.collector.begin_array()
            }
            fn separator(&mut self) -> $crate::Result<(), Self::Error> {
                self.collector.separator()
            }
            fn end_array(&mut self) -> $crate::Result<(), Self::Error> {
                self.collector.end_array()
            }
            fn begin_object(&mut self) -> $crate::Result<(), Self::Error> {
                self.collector.begin_object()
            }
            fn key(&mut self, key: &str) -> $crate::Result<(), Self::Error> {
                self.collector.key(key)
            }
            fn end_object(&mut self) -> $crate::Result<(), Self::Error> {
                self.collector.end_object()
            }
            fn write_null(&mut self) -> $crate::Result<(), Self::Error> {
                self.collector.write_null()
            }
            fn write_bool(&mut self, value: bool) -> $crate::Result<(), Self::Error> {
                self.collector.write_bool(value)
            }
            fn write_str(&mut self, value: &str) -> $crate::Result<(), Self::Error> {
                self.collector.write_str(value)
            }
            fn write_char(&mut self, value: char) -> $crate::Result<(), Self::Error> {
                self.collector.write_char(value)
            }
            fn write_number(&mut self, value: &$crate::Number) -> $crate::Result<(), Self::Error> {
                self.collector.write_number(value)
            }
            fn write_i64(&mut self, value: i64) -> $crate::Result<(), Self::Error> {
                self.collector.write_i64(value)
            }
            fn write_u64(&mut self, value: u64) -> $crate::Result<(), Self::Error> {
                self.collector.write_u64(value)
            }
            fn write_i128(&mut self, value: i128) -> $crate::Result<(), Self::Error> {
                self.collector.write_i128(value)
            }
            fn write_u128(&mut self, value: u128) -> $crate::Result<(), Self::Error> {
                self.collector.write_u128(value)
            }
            fn write_f64(&mut self, value: f64) -> $crate::Result<(), Self::Error> {
                self.collector.write_f64(value)
            }
            fn write_f32(&mut self, value: f32) -> $crate::Result<(), Self::Error> {
                self.collector.write_f32(value)
            }
        }
    };
}

pub(crate) use impl_collecting_format_encoder;

/// Convert a [`Value`] into an owned token stream (the unified replay path).
pub(crate) fn value_to_tokens(v: &Value) -> Vec<Token<'static>> {
    let mut out = Vec::new();
    value_to_tokens_inner(v, &mut out);
    out
}

fn value_to_tokens_inner(v: &Value, out: &mut Vec<Token<'static>>) {
    match v {
        Value::Null => out.push(Token::Null),
        Value::Bool(b) => out.push(Token::Bool(*b)),
        Value::Number(n) => out.push(Token::Number(*n)),
        Value::String(s) => out.push(Token::Str(Cow::Owned(s.clone()))),
        Value::Array(a) => {
            out.push(Token::BeginArray);
            for x in a {
                value_to_tokens_inner(x, out);
            }
            out.push(Token::EndArray);
        }
        Value::Object(m) => {
            out.push(Token::BeginObject);
            for (k, val) in m.iter() {
                out.push(Token::Str(Cow::Owned(k.to_string())));
                value_to_tokens_inner(val, out);
            }
            out.push(Token::EndObject);
        }
    }
}

/// A [`FormatDecoder`] that replays an owned token stream produced from a
/// [`Value`] tree.
///
/// Formats that parse their input into a [`Value`] first wrap that stream in
/// this type instead of re-implementing the full [`FormatDecoder`] contract.
/// The inner decoder borrows the owned stream for the lifetime of the
/// wrapper; `object_key` / `string` are re-lifetimed to the caller's `'de`.
pub struct TreeDecoder<'de> {
    inner: Decoder<'static>,
    _marker: core::marker::PhantomData<&'de ()>,
}

impl<'de> TreeDecoder<'de> {
    /// Wrap an owned token stream produced by `value_to_tokens`.
    pub fn new(tokens: Vec<Token<'static>>) -> Self {
        TreeDecoder {
            inner: Decoder::from_tokens(tokens),
            _marker: core::marker::PhantomData,
        }
    }

    /// Validate that the whole token stream was consumed.
    pub fn end(&mut self) -> Result<()> {
        self.inner.end()
    }
}

impl<'de> FormatDecoder<'de> for TreeDecoder<'de> {
    type Error = crate::error::Error;

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        self.inner.begin_object()
    }
    fn end_object(&mut self) -> Result<(), Self::Error> {
        self.inner.end_object()
    }
    fn object_key(&mut self) -> Result<Option<Cow<'de, str>>, Self::Error> {
        Ok(self.inner.object_key()?.map(|k| match k {
            Cow::Borrowed(s) => Cow::Borrowed(s),
            Cow::Owned(s) => Cow::Owned(s),
        }))
    }
    fn object_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.inner.object_entry_sep()
    }
    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.inner.begin_array()
    }
    fn end_array(&mut self) -> Result<(), Self::Error> {
        self.inner.end_array()
    }
    fn array_has_more(&mut self) -> Result<bool, Self::Error> {
        self.inner.array_has_more()
    }
    fn array_entry_sep(&mut self) -> Result<bool, Self::Error> {
        self.inner.array_entry_sep()
    }
    fn unit(&mut self) -> Result<(), Self::Error> {
        self.inner.unit()
    }
    fn bool(&mut self) -> Result<bool, Self::Error> {
        self.inner.bool()
    }
    fn number(&mut self) -> Result<Number, Self::Error> {
        self.inner.number()
    }
    fn string(&mut self) -> Result<Cow<'de, str>, Self::Error> {
        Ok(match self.inner.string()? {
            Cow::Borrowed(s) => Cow::Borrowed(s),
            Cow::Owned(s) => Cow::Owned(s),
        })
    }
    fn char(&mut self) -> Result<char, Self::Error> {
        self.inner.char()
    }
    fn skip_value(&mut self) -> Result<(), Self::Error> {
        self.inner.skip_value()
    }
    fn peek_token(&mut self) -> Result<Token<'de>, Self::Error> {
        self.inner.peek_token()
    }
    fn next_token(&mut self) -> Result<Token<'de>, Self::Error> {
        self.inner.next_token()
    }
    fn save(&self) -> Mark {
        self.inner.save()
    }
    fn restore(&mut self, mark: Mark) {
        self.inner.restore(mark)
    }
}

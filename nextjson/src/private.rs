//! Runtime helpers used by macro-generated code (`#[doc(hidden)]`).
//!
//! All complex logic (content replay, token splicing, `Value`-driven nextdecode,
//! internal-tag merging) is consolidated here so the derive only emits glue.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::de::{read_token_tree, DecodeSlot, Decoder, NsonDeserialize};
use crate::error::{FormatError, Result};
use crate::ser::{FormatEncoder, NsonSerialize};
use crate::value::Value;

pub use crate::de::{
    Decoder as DecoderReexport, NsonDeserialize as NsonDeserializeReexport, Token,
    Token as TokenReexport,
};
pub use crate::encoding::Encoder as EncoderReexport;
pub use crate::ser::NsonSerialize as NsonSerializeReexport;

/// RAII storage used by derive-generated decoders for one struct field.
///
/// A successfully decoded value is dropped automatically unless it is moved
/// into the completed parent value with [`take`](InitSlot::take). This gives
/// partially decoded structs and duplicate fields normal Rust drop semantics.
///
/// The slot *is* a [`DecodeSlot`](crate::de::DecodeSlot): typed fields decode
/// directly into their own storage via `nextdecode_into` instead of building
/// an intermediate value and moving it, so the generic field path costs one
/// `Option` (the slot) rather than two.
#[doc(hidden)]
pub struct InitSlot<T> {
    slot: DecodeSlot<T>,
}

impl<T> InitSlot<T> {
    /// Create an empty field slot.
    pub const fn new() -> Self {
        InitSlot {
            slot: DecodeSlot::new(),
        }
    }

    /// Next-decode a field directly into this slot.
    pub fn nextdecode<'de, D: crate::de::FormatDecoder<'de>>(
        &mut self,
        decoder: &mut D,
    ) -> Result<(), D::Error>
    where
        T: NsonDeserialize<'de>,
    {
        T::nextdecode_into(decoder, &mut self.slot)?;
        if !self.slot.is_initialized() {
            return Err(FormatError::custom(
                "NsonDeserialize::nextdecode_into returned success without writing a value",
            ));
        }
        Ok(())
    }

    /// Replace the slot with an already constructed value.
    pub fn write(&mut self, value: T) {
        self.slot.write(value);
    }

    /// Move the initialized value out of the slot.
    ///
    /// # Panics
    /// Panics if the generated decoder did not initialize this field.
    pub fn take(&mut self) -> T {
        self.slot
            .take()
            .expect("nextjson derive: uninitialized field slot")
    }
}

impl<T> Default for InitSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a decoder from an in-memory token stream.
pub fn from_tokens<'de>(tokens: Vec<Token<'de>>) -> Decoder<'de> {
    Decoder::from_tokens(tokens)
}

/// A whole object read as `(key, value token subtree)` pairs.
type ObjectMap<'de> = Vec<(Cow<'de, str>, Vec<Token<'de>>)>;

/// Read a whole object as `(key, value token subtree)` pairs.
pub fn read_object_map<'de, D: crate::de::FormatDecoder<'de>>(
    decoder: &mut D,
) -> Result<ObjectMap<'de>, D::Error> {
    let mut entries = Vec::new();
    decoder.begin_object()?;
    while let Some(key) = decoder.object_key()? {
        let value = read_token_tree(decoder)?;
        entries.push((key, value));
        if !decoder.object_entry_sep()? {
            break;
        }
    }
    decoder.end_object()?;
    Ok(entries)
}

/// Spliced an entry list into an object token stream.
pub fn tokens_to_object<'de>(
    mut entries: Vec<(Cow<'de, str>, Vec<Token<'de>>)>,
) -> Vec<Token<'de>> {
    let mut out = Vec::with_capacity(entries.len() * 2 + 2);
    out.push(Token::BeginObject);
    for (k, v) in entries.drain(..) {
        out.push(Token::Str(k));
        out.extend(v);
    }
    out.push(Token::EndObject);
    out
}

/// Extract a string from a single-value token subtree (for enum tags).
pub fn token_to_string<'de>(tokens: &[Token<'de>]) -> Result<String> {
    match tokens {
        [Token::Str(s)] => Ok(s.to_string()),
        _ => Err(crate::error::Error::invalid_type(
            "a string",
            "a non-string value",
        )),
    }
}

/// Decode any type from a [`Value`] (owned, any lifetime).
pub fn nextdecode_value<T: for<'de> NsonDeserialize<'de>>(value: Value) -> Result<T> {
    let tokens = value_to_tokens(&value)?;
    let mut decoder = Decoder::from_tokens(tokens);
    let decoded = T::nextdecode(&mut decoder)?;
    decoder.end()?;
    Ok(decoded)
}

/// Convert a [`Value`] into an owned token stream, bounded to the same
/// nesting limit the decoders enforce so a hand-built deep tree cannot
/// overflow the stack.
fn value_to_tokens(v: &Value) -> Result<Vec<Token<'static>>> {
    let mut out = Vec::new();
    value_to_tokens_inner(v, &mut out, 0)?;
    Ok(out)
}

fn value_to_tokens_inner(v: &Value, out: &mut Vec<Token<'static>>, depth: u32) -> Result<()> {
    if depth > 128 {
        return Err(crate::error::Error::custom(
            "value nesting exceeds the maximum depth (128)",
        ));
    }
    match v {
        Value::Null => out.push(Token::Null),
        Value::Bool(b) => out.push(Token::Bool(*b)),
        Value::Number(n) => out.push(Token::Number(*n)),
        Value::String(s) => out.push(Token::Str(Cow::Owned(s.clone()))),
        Value::Array(a) => {
            out.push(Token::BeginArray);
            for x in a {
                value_to_tokens_inner(x, out, depth + 1)?;
            }
            out.push(Token::EndArray);
        }
        Value::Object(m) => {
            out.push(Token::BeginObject);
            for (k, val) in m.iter() {
                out.push(Token::Str(Cow::Owned(k.to_string())));
                value_to_tokens_inner(val, out, depth + 1)?;
            }
            out.push(Token::EndObject);
        }
    }
    Ok(())
}

/// Internal-tag serialize: write `{ "<tag>": "<variant>", ...content }`.
pub fn write_tagged_object<E: crate::ser::FormatEncoder>(
    encoder: &mut E,
    tag: &str,
    variant_name: &str,
    value: Value,
) -> Result<(), E::Error> {
    match value {
        Value::Object(m) => {
            encoder.begin_object()?;
            encoder.key(tag)?;
            encoder.write_str(variant_name)?;
            for (k, v) in m.iter() {
                encoder.key(k)?;
                NsonSerialize::nextencode(v, encoder)?;
            }
            encoder.end_object()
        }
        _ => Err(crate::error::Error::custom(
            "internally tagged newtype variant must serialize to an object",
        )
        .into()),
    }
}

/// Forward a serialized object's entries into an already-open outer object.
///
/// The adapter suppresses exactly the flattened value's root object markers
/// and forwards every nested event unchanged. Its small root state machine is
/// intentionally independent from a concrete encoder so derive-generated
/// flattening stays allocation-free for every output format.
pub fn flatten_serialize<T, E>(value: &T, encoder: &mut E) -> Result<(), E::Error>
where
    T: NsonSerialize + ?Sized,
    E: FormatEncoder,
{
    let mut flat = FlattenEncoder::new(encoder);
    value.nextencode(&mut flat)?;
    flat.finish()
}

struct FlattenEncoder<'a, E: FormatEncoder> {
    inner: &'a mut E,
    depth: usize,
    started: bool,
    finished: bool,
    root_pending_value: bool,
}

impl<'a, E: FormatEncoder> FlattenEncoder<'a, E> {
    fn new(inner: &'a mut E) -> Self {
        FlattenEncoder {
            inner,
            depth: 0,
            started: false,
            finished: false,
            root_pending_value: false,
        }
    }

    fn error(message: &'static str) -> E::Error {
        crate::error::Error::custom(message).into()
    }

    fn start_value(&mut self) -> Result<(), E::Error> {
        if !self.started || self.finished {
            return Err(Self::error("flatten: expected an object root"));
        }
        if self.depth == 1 {
            if !self.root_pending_value {
                return Err(Self::error("flatten: object value has no key"));
            }
            self.root_pending_value = false;
        }
        Ok(())
    }

    fn finish(self) -> Result<(), E::Error> {
        if !self.started || !self.finished || self.depth != 0 {
            return Err(Self::error("flatten: object was not closed"));
        }
        if self.root_pending_value {
            return Err(Self::error("flatten: object ended before keyed value"));
        }
        Ok(())
    }
}

impl<E: FormatEncoder> FormatEncoder for FlattenEncoder<'_, E> {
    type Error = E::Error;

    fn begin_array(&mut self) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.begin_array()?;
        self.depth += 1;
        Ok(())
    }

    fn separator(&mut self) -> Result<(), Self::Error> {
        if self.depth <= 1 {
            return Err(Self::error("flatten: array separator at object root"));
        }
        self.inner.separator()
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        if self.depth <= 1 {
            return Err(Self::error("flatten: array end without matching start"));
        }
        self.inner.end_array()?;
        self.depth -= 1;
        Ok(())
    }

    fn begin_object(&mut self) -> Result<(), Self::Error> {
        if !self.started {
            self.started = true;
            self.depth = 1;
            return Ok(());
        }
        self.start_value()?;
        self.inner.begin_object()?;
        self.depth += 1;
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), Self::Error> {
        if !self.started || self.finished || self.depth == 0 {
            return Err(Self::error("flatten: object key outside object"));
        }
        if self.depth == 1 {
            if self.root_pending_value {
                return Err(Self::error("flatten: object value required after key"));
            }
            self.inner.key(key)?;
            self.root_pending_value = true;
            return Ok(());
        }
        self.inner.key(key)
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        if self.depth == 0 {
            return Err(Self::error("flatten: object end without matching start"));
        }
        if self.depth == 1 {
            if self.root_pending_value {
                return Err(Self::error("flatten: object ended before keyed value"));
            }
            self.depth = 0;
            self.finished = true;
            return Ok(());
        }
        self.inner.end_object()?;
        self.depth -= 1;
        Ok(())
    }

    fn write_null(&mut self) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_null()
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_bool(value)
    }

    fn write_str(&mut self, value: &str) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_str(value)
    }

    fn write_char(&mut self, value: char) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_char(value)
    }

    fn write_number(&mut self, value: &crate::Number) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_number(value)
    }

    fn write_i64(&mut self, value: i64) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_i64(value)
    }

    fn write_u64(&mut self, value: u64) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_u64(value)
    }

    fn write_i128(&mut self, value: i128) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_i128(value)
    }

    fn write_u128(&mut self, value: u128) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_u128(value)
    }

    fn write_f64(&mut self, value: f64) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_f64(value)
    }

    fn write_f32(&mut self, value: f32) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_f32(value)
    }

    fn write_i8(&mut self, value: i8) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_i8(value)
    }

    fn write_i16(&mut self, value: i16) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_i16(value)
    }

    fn write_i32(&mut self, value: i32) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_i32(value)
    }

    fn write_u8(&mut self, value: u8) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_u8(value)
    }

    fn write_u16(&mut self, value: u16) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_u16(value)
    }

    fn write_u32(&mut self, value: u32) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_u32(value)
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_bytes(value)
    }

    fn write_none(&mut self) -> Result<(), Self::Error> {
        self.start_value()?;
        self.inner.write_none()
    }

    fn write_some(&mut self) -> Result<(), Self::Error> {
        if self.finished {
            return Err(Self::error("flatten: expected an object root"));
        }
        // Flattening is defined by the JSON object shape. At the root, Some
        // is transparent (including nested Option wrappers), matching the
        // previous JSON-mediated implementation. Option values inside the
        // flattened object still forward their native format marker.
        if !self.started {
            return Ok(());
        }
        if self.depth == 1 && !self.root_pending_value {
            return Err(Self::error("flatten: object value has no key"));
        }
        self.inner.write_some()
    }

    fn map_key<K: NsonSerialize>(&mut self, key: &K) -> Result<(), Self::Error> {
        if !self.started || self.finished || self.depth == 0 {
            return Err(Self::error("flatten: map key outside object"));
        }
        if self.depth == 1 {
            if self.root_pending_value {
                return Err(Self::error("flatten: object value required after key"));
            }
            self.inner.map_key(key)?;
            self.root_pending_value = true;
            return Ok(());
        }
        self.inner.map_key(key)
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

/// Re-emit one `Value` through any format encoder.
///
/// Used by `flatten` so the flattened sub-object's fields merge into the
/// enclosing container regardless of the destination format.
fn emit_value<E: crate::ser::FormatEncoder>(
    value: &Value,
    encoder: &mut E,
) -> Result<(), E::Error> {
    match value {
        Value::Null => encoder.write_null(),
        Value::Bool(b) => encoder.write_bool(*b),
        Value::Number(n) => encoder.write_number(n),
        Value::String(s) => encoder.write_str(s),
        Value::Array(a) => {
            encoder.begin_array()?;
            for item in a {
                encoder.separator()?;
                emit_value(item, encoder)?;
            }
            encoder.end_array()
        }
        Value::Object(m) => {
            encoder.begin_object()?;
            for (k, v) in m.iter() {
                encoder.key(k)?;
                emit_value(v, encoder)?;
            }
            encoder.end_object()
        }
    }
}

/// Splice the fields of a JSON object value into any format encoder.
///
/// `json` must be the compact JSON text of an object (`{...}`). Its entries
/// are re-emitted as key/value events so flattened structs work in every
/// supported format, not just JSON.
pub fn flatten_into<E: crate::ser::FormatEncoder>(
    json: &[u8],
    encoder: &mut E,
) -> Result<(), E::Error> {
    let mut decoder = Decoder::new(json);
    let value = Value::nextdecode(&mut decoder)?;
    decoder.end()?;
    match value {
        Value::Object(m) => {
            for (k, v) in m.iter() {
                encoder.key(k)?;
                emit_value(v, encoder)?;
            }
            Ok(())
        }
        _ => Err(crate::error::Error::custom("flatten: expected an object or map").into()),
    }
}

pub use crate::error::Error as ErrorReexport;
pub use crate::error::Result as ResultReexport;
pub use crate::map::Map as MapReexport;
pub use crate::value::Value as ValueReexport;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Map;
    use alloc::vec;

    #[test]
    fn value_tokens_roundtrip() {
        let v = Value::Object(Map::from_iter(vec![
            (
                "a".to_string(),
                Value::Array(vec![Value::Number(1.into()), Value::Null]),
            ),
            ("b".to_string(), Value::Bool(true)),
        ]));
        let tokens = value_to_tokens(&v).unwrap();
        let mut d = Decoder::from_tokens(tokens);
        let back: Map = NsonDeserialize::nextdecode(&mut d).unwrap();
        assert_eq!(
            back.get("a").unwrap(),
            &Value::Array(vec![Value::Number(1.into()), Value::Null])
        );
    }

    #[test]
    fn read_object_map_works() {
        let mut d = Decoder::new(br#"{"type":"A","x":1,"y":[2,3]}"#);
        let entries = read_object_map(&mut d).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].0.as_ref(), "type");
        assert_eq!(token_to_string(&entries[0].1).unwrap(), "A");
    }
}

//! Runtime helpers used by macro-generated code (`#[doc(hidden)]`).
//!
//! All complex logic (content replay, token splicing, `Value`-driven decode,
//! internal-tag merging) is consolidated here so the derive only emits glue.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::mem::MaybeUninit;

use crate::de::{read_token_tree, Decoder, NsonDeserialize};
use crate::encode::Encoder;
use crate::error::Result;
use crate::ser::NsonSerialize;
use crate::value::Value;
use crate::write::Write;

pub use crate::de::{Decoder as DecoderReexport, NsonDeserialize as NsonDeserializeReexport, Token, Token as TokenReexport};
pub use crate::encode::Encoder as EncoderReexport;
pub use crate::ser::NsonSerialize as NsonSerializeReexport;

/// RAII storage used by derive-generated decoders for one struct field.
///
/// A successfully decoded value is dropped automatically unless it is moved
/// into the completed parent value with [`take`](InitSlot::take). This gives
/// partially decoded structs and duplicate fields normal Rust drop semantics.
#[doc(hidden)]
pub struct InitSlot<T> {
    value: MaybeUninit<T>,
    initialized: bool,
}

impl<T> InitSlot<T> {
    /// Create an empty field slot.
    pub const fn new() -> Self {
        InitSlot {
            value: MaybeUninit::uninit(),
            initialized: false,
        }
    }

    /// Decode a field directly into this slot.
    pub fn decode<'de>(&mut self, decoder: &mut Decoder<'de>) -> Result<()>
    where
        T: NsonDeserialize<'de>,
    {
        self.clear();
        T::decode_into(decoder, &mut self.value)?;
        self.initialized = true;
        Ok(())
    }

    /// Replace the slot with an already constructed value.
    pub fn write(&mut self, value: T) {
        self.clear();
        self.value.write(value);
        self.initialized = true;
    }

    /// Move the initialized value out of the slot.
    ///
    /// # Panics
    /// Panics if the generated decoder did not initialize this field.
    pub fn take(&mut self) -> T {
        assert!(self.initialized, "nextjson derive: uninitialized field slot");
        self.initialized = false;
        // SAFETY: the flag is set only after `write` or a successful
        // `decode_into`, both of which fully initialize `value`.
        #[allow(unsafe_code)]
        unsafe {
            self.value.assume_init_read()
        }
    }

    fn clear(&mut self) {
        if self.initialized {
            self.initialized = false;
            // SAFETY: the flag is set only after successful initialization.
            #[allow(unsafe_code)]
            unsafe {
                self.value.assume_init_drop();
            }
        }
    }
}

impl<T> Default for InitSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for InitSlot<T> {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Create a decoder from an in-memory token stream.
pub fn from_tokens<'de>(tokens: Vec<Token<'de>>) -> Decoder<'de> {
    Decoder::from_tokens(tokens)
}

/// Read a whole object as `(key, value token subtree)` pairs.
pub fn read_object_map<'de>(
    decoder: &mut Decoder<'de>,
) -> Result<Vec<(Cow<'de, str>, Vec<Token<'de>>)>> {
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
pub fn tokens_to_object<'de>(mut entries: Vec<(Cow<'de, str>, Vec<Token<'de>>)>) -> Vec<Token<'de>> {
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
        _ => Err(crate::error::Error::invalid_type("a string", "a non-string value")),
    }
}

/// Decode any type from a [`Value`] (owned, any lifetime).
pub fn decode_value<T: for<'de> NsonDeserialize<'de>>(value: Value) -> Result<T> {
    let tokens = value_to_tokens(&value);
    let mut decoder = Decoder::from_tokens(tokens);
    let decoded = T::decode(&mut decoder)?;
    decoder.end()?;
    Ok(decoded)
}

fn value_to_tokens(v: &Value) -> Vec<Token<'static>> {
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

/// Internal-tag serialize: write `{ "<tag>": "<variant>", ...content }`.
pub fn write_tagged_object<W: Write>(
    encoder: &mut Encoder<W>,
    tag: &str,
    variant_name: &str,
    value: Value,
) -> Result<()> {
    match value {
        Value::Object(m) => {
            encoder.begin_object()?;
            encoder.key(tag)?;
            encoder.write_str(variant_name)?;
            for (k, v) in m.iter() {
                encoder.key(k)?;
                NsonSerialize::encode(v, encoder)?;
            }
            encoder.end_object()
        }
        _ => Err(crate::error::Error::custom(
            "internally tagged newtype variant must serialize to an object",
        )),
    }
}

pub use crate::map::Map as MapReexport;
pub use crate::value::Value as ValueReexport;
pub use crate::error::Error as ErrorReexport;
pub use crate::error::Result as ResultReexport;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::map::Map;

    #[test]
    fn value_tokens_roundtrip() {
        let v = Value::Object(Map::from_iter(vec![
            ("a".to_string(), Value::Array(vec![Value::Number(1.into()), Value::Null])),
            ("b".to_string(), Value::Bool(true)),
        ]));
        let tokens = value_to_tokens(&v);
        let mut d = Decoder::from_tokens(tokens);
        let back: Map = NsonDeserialize::decode(&mut d).unwrap();
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

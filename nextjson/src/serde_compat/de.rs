use alloc::borrow::Cow;

use serde::de::{DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};

use crate::de::{DecodeConfig, Decoder, Token};
use crate::error::{Error, Result};
use crate::number::Number;

/// A Serde deserializer backed directly by NextJson's zero-copy decoder.
pub struct Deserializer<'de> {
    decoder: Decoder<'de>,
}

impl<'de> Deserializer<'de> {
    /// Create a deserializer with the default NextJson nextdecode configuration.
    pub fn new(input: &'de [u8]) -> Self {
        Deserializer {
            decoder: Decoder::new(input),
        }
    }

    /// Create a deserializer with an explicit nextdecode configuration.
    pub fn with_config(input: &'de [u8], config: DecodeConfig) -> Self {
        Deserializer {
            decoder: Decoder::with_config(input, config),
        }
    }

    /// Verify that no non-whitespace input remains.
    pub fn end(&mut self) -> Result<()> {
        self.decoder.end()
    }
}

impl<'de> serde::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.decoder.peek_token()? {
            Token::Null => {
                self.decoder.unit()?;
                visitor.visit_unit()
            }
            Token::Bool(_) => visitor.visit_bool(self.decoder.bool()?),
            Token::Number(_) => visit_number(self.decoder.number()?, visitor),
            Token::Str(_) => visit_cow_str(self.decoder.string()?, visitor),
            Token::BeginArray => self.deserialize_seq(visitor),
            Token::BeginObject => self.deserialize_map(visitor),
            Token::EndObject | Token::EndArray => {
                Err(Error::custom("unexpected end of JSON container"))
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if matches!(self.decoder.peek_token()?, Token::Null) {
            self.decoder.unit()?;
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.decoder.unit()?;
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.decoder.begin_array()?;
        let mut access = SequenceAccess {
            deserializer: self,
            first: true,
            finished: false,
        };
        let value = visitor.visit_seq(&mut access)?;
        access.finish()?;
        Ok(value)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.decoder.begin_object()?;
        let mut access = ObjectAccess {
            deserializer: self,
            first: true,
            value_pending: false,
            finished: false,
        };
        let value = visitor.visit_map(&mut access)?;
        access.finish()?;
        Ok(value)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        let (variant, object) = match self.decoder.peek_token()? {
            Token::Str(_) => (self.decoder.string()?, false),
            Token::BeginObject => {
                self.decoder.begin_object()?;
                let variant = self
                    .decoder
                    .object_key()?
                    .ok_or_else(|| Error::custom("expected a single-key enum object"))?;
                (variant, true)
            }
            _ => {
                return Err(Error::invalid_type(
                    "a string or single-key object",
                    "JSON value",
                ))
            }
        };
        visitor.visit_enum(JsonEnumAccess {
            deserializer: self,
            variant,
            object,
        })
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visit_cow_str(self.decoder.string()?, visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visit_cow_str(self.decoder.string()?, visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visit_cow_str(self.decoder.string()?, visitor)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_char(self.decoder.char()?)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if matches!(self.decoder.peek_token()?, Token::Str(_)) {
            match self.decoder.string()? {
                Cow::Borrowed(value) => visitor.visit_borrowed_bytes(value.as_bytes()),
                Cow::Owned(value) => visitor.visit_byte_buf(value.into_bytes()),
            }
        } else {
            self.deserialize_seq(visitor)
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.decoder.skip_value()?;
        visitor.visit_unit()
    }

    fn is_human_readable(&self) -> bool {
        true
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64
    }
}

fn visit_number<'de, V: Visitor<'de>>(number: Number, visitor: V) -> Result<V::Value> {
    match number {
        Number::I64(value) => visitor.visit_i64(value),
        Number::U64(value) => visitor.visit_u64(value),
        Number::I128(value) => visitor.visit_i128(value),
        Number::U128(value) => visitor.visit_u128(value),
        Number::F64(value) => visitor.visit_f64(value),
    }
}

fn visit_cow_str<'de, V: Visitor<'de>>(value: Cow<'de, str>, visitor: V) -> Result<V::Value> {
    match value {
        Cow::Borrowed(value) => visitor.visit_borrowed_str(value),
        Cow::Owned(value) => visitor.visit_string(value),
    }
}

struct SequenceAccess<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
    first: bool,
    finished: bool,
}

impl<'de> SequenceAccess<'_, 'de> {
    fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if self.first {
            if self.deserializer.decoder.array_has_more()? {
                return Err(Error::custom(
                    "sequence visitor did not consume all elements",
                ));
            }
        } else if self.deserializer.decoder.array_entry_sep()? {
            return Err(Error::custom(
                "sequence visitor did not consume all elements",
            ));
        }
        self.deserializer.decoder.end_array()?;
        self.finished = true;
        Ok(())
    }
}

impl<'de> SeqAccess<'de> for &mut SequenceAccess<'_, 'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.finished {
            return Ok(None);
        }
        if self.first {
            self.first = false;
            if !self.deserializer.decoder.array_has_more()? {
                self.deserializer.decoder.end_array()?;
                self.finished = true;
                return Ok(None);
            }
        } else if !self.deserializer.decoder.array_entry_sep()? {
            self.deserializer.decoder.end_array()?;
            self.finished = true;
            return Ok(None);
        }
        seed.deserialize(&mut *self.deserializer).map(Some)
    }
}

struct ObjectAccess<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
    first: bool,
    value_pending: bool,
    finished: bool,
}

impl<'de> ObjectAccess<'_, 'de> {
    fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if self.value_pending {
            return Err(Error::custom("map visitor did not consume a value"));
        }
        if !self.first && self.deserializer.decoder.object_entry_sep()? {
            return Err(Error::custom("map visitor did not consume all entries"));
        }
        if self.first && self.deserializer.decoder.object_key()?.is_some() {
            return Err(Error::custom("map visitor did not consume all entries"));
        }
        self.deserializer.decoder.end_object()?;
        self.finished = true;
        Ok(())
    }
}

impl<'de> MapAccess<'de> for &mut ObjectAccess<'_, 'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.finished {
            return Ok(None);
        }
        if self.value_pending {
            return Err(Error::custom("next_key_seed called before next_value_seed"));
        }
        if self.first {
            self.first = false;
        } else if !self.deserializer.decoder.object_entry_sep()? {
            self.deserializer.decoder.end_object()?;
            self.finished = true;
            return Ok(None);
        }
        let Some(key) = self.deserializer.decoder.object_key()? else {
            self.deserializer.decoder.end_object()?;
            self.finished = true;
            return Ok(None);
        };
        self.value_pending = true;
        seed.deserialize(MapKeyDeserializer { key }).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        if !self.value_pending {
            return Err(Error::custom("next_value_seed called before next_key_seed"));
        }
        let value = seed.deserialize(&mut *self.deserializer)?;
        self.value_pending = false;
        Ok(value)
    }
}

struct JsonEnumAccess<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
    variant: Cow<'de, str>,
    object: bool,
}

impl<'a, 'de> EnumAccess<'de> for JsonEnumAccess<'a, 'de> {
    type Error = Error;
    type Variant = JsonVariantAccess<'a, 'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(MapKeyDeserializer { key: self.variant })?;
        Ok((
            variant,
            JsonVariantAccess {
                deserializer: self.deserializer,
                object: self.object,
            },
        ))
    }
}

struct JsonVariantAccess<'a, 'de> {
    deserializer: &'a mut Deserializer<'de>,
    object: bool,
}

impl JsonVariantAccess<'_, '_> {
    fn require_content(&self) -> Result<()> {
        if self.object {
            Ok(())
        } else {
            Err(Error::custom(
                "expected enum content in a single-key object",
            ))
        }
    }

    fn finish_object(&mut self) -> Result<()> {
        if self.deserializer.decoder.object_entry_sep()? {
            return Err(Error::custom("expected a single-key enum object"));
        }
        self.deserializer.decoder.end_object()
    }
}

impl<'de> VariantAccess<'de> for JsonVariantAccess<'_, 'de> {
    type Error = Error;

    fn unit_variant(mut self) -> Result<()> {
        if self.object {
            self.deserializer.decoder.unit()?;
            self.finish_object()
        } else {
            Ok(())
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(mut self, seed: T) -> Result<T::Value> {
        self.require_content()?;
        let value = seed.deserialize(&mut *self.deserializer)?;
        self.finish_object()?;
        Ok(value)
    }

    fn tuple_variant<V: Visitor<'de>>(mut self, len: usize, visitor: V) -> Result<V::Value> {
        self.require_content()?;
        let value = serde::Deserializer::deserialize_tuple(&mut *self.deserializer, len, visitor)?;
        self.finish_object()?;
        Ok(value)
    }

    fn struct_variant<V: Visitor<'de>>(
        mut self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.require_content()?;
        let value =
            serde::Deserializer::deserialize_struct(&mut *self.deserializer, "", fields, visitor)?;
        self.finish_object()?;
        Ok(value)
    }
}

struct MapKeyDeserializer<'de> {
    key: Cow<'de, str>,
}

macro_rules! deserialize_key_number {
    ($method:ident, $visit:ident, $ty:ty) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            let value = self
                .key
                .parse::<$ty>()
                .map_err(|_| Error::custom("invalid numeric JSON object key"))?;
            visitor.$visit(value)
        }
    };
}

impl<'de> serde::Deserializer<'de> for MapKeyDeserializer<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visit_cow_str(self.key, visitor)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visit_cow_str(self.key, visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visit_cow_str(self.key, visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visit_cow_str(self.key, visitor)
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.key.as_ref() {
            "true" => visitor.visit_bool(true),
            "false" => visitor.visit_bool(false),
            _ => Err(Error::custom("invalid boolean JSON object key")),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let mut chars = self.key.chars();
        match (chars.next(), chars.next()) {
            (Some(value), None) => visitor.visit_char(value),
            _ => Err(Error::custom("invalid char JSON object key")),
        }
    }

    deserialize_key_number!(deserialize_i8, visit_i8, i8);
    deserialize_key_number!(deserialize_i16, visit_i16, i16);
    deserialize_key_number!(deserialize_i32, visit_i32, i32);
    deserialize_key_number!(deserialize_i64, visit_i64, i64);
    deserialize_key_number!(deserialize_i128, visit_i128, i128);
    deserialize_key_number!(deserialize_u8, visit_u8, u8);
    deserialize_key_number!(deserialize_u16, visit_u16, u16);
    deserialize_key_number!(deserialize_u32, visit_u32, u32);
    deserialize_key_number!(deserialize_u64, visit_u64, u64);
    deserialize_key_number!(deserialize_u128, visit_u128, u128);

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_enum(KeyEnumAccess { key: self.key })
    }

    fn is_human_readable(&self) -> bool {
        true
    }

    serde::forward_to_deserialize_any! {
        f32 f64 bytes byte_buf option unit unit_struct seq tuple tuple_struct map struct
        ignored_any
    }
}

struct KeyEnumAccess<'de> {
    key: Cow<'de, str>,
}

impl<'de> EnumAccess<'de> for KeyEnumAccess<'de> {
    type Error = Error;
    type Variant = UnitKeyVariant;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        let value = seed.deserialize(MapKeyDeserializer { key: self.key })?;
        Ok((value, UnitKeyVariant))
    }
}

struct UnitKeyVariant;

impl<'de> VariantAccess<'de> for UnitKeyVariant {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, _seed: T) -> Result<T::Value> {
        Err(Error::custom("map key enum variants must be unit variants"))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, _visitor: V) -> Result<V::Value> {
        Err(Error::custom("map key enum variants must be unit variants"))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value> {
        Err(Error::custom("map key enum variants must be unit variants"))
    }
}

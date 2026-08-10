use alloc::string::{String, ToString};

use serde::ser::{
    Impossible, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant,
};

use crate::encoding::{EncodeConfig, Encoder};
use crate::error::{Error, Result};
use crate::write::Write;

/// A Serde serializer backed directly by NextJson's streaming encoder.
pub struct Serializer<W: Write> {
    encoder: Encoder<W>,
}

impl<W: Write> Serializer<W> {
    /// Create a compact serializer over `writer`.
    pub fn new(writer: W) -> Self {
        Serializer {
            encoder: Encoder::new(writer),
        }
    }

    /// Create a serializer with an explicit NextJson encoder configuration.
    pub fn with_config(writer: W, config: EncodeConfig) -> Self {
        Serializer {
            encoder: Encoder::with_config(writer, config),
        }
    }

    /// Flush buffered JSON and return the underlying writer.
    pub fn finish(self) -> Result<W> {
        self.encoder.finish()
    }
}

impl Serializer<alloc::vec::Vec<u8>> {
    pub(crate) fn for_vec(config: EncodeConfig) -> Self {
        Serializer {
            encoder: Encoder::for_vec(config),
        }
    }

    pub(crate) fn finish_vec(self) -> alloc::vec::Vec<u8> {
        self.encoder.finish_vec()
    }
}

impl<'a, W: Write> serde::Serializer for &'a mut Serializer<W> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Sequence<'a, W>;
    type SerializeTuple = Sequence<'a, W>;
    type SerializeTupleStruct = Sequence<'a, W>;
    type SerializeTupleVariant = TupleVariant<'a, W>;
    type SerializeMap = Object<'a, W>;
    type SerializeStruct = Object<'a, W>;
    type SerializeStructVariant = StructVariant<'a, W>;

    fn serialize_bool(self, value: bool) -> Result<()> {
        self.encoder.write_bool(value)
    }

    fn serialize_i8(self, value: i8) -> Result<()> {
        self.serialize_i64(value as i64)
    }

    fn serialize_i16(self, value: i16) -> Result<()> {
        self.serialize_i64(value as i64)
    }

    fn serialize_i32(self, value: i32) -> Result<()> {
        self.serialize_i64(value as i64)
    }

    fn serialize_i64(self, value: i64) -> Result<()> {
        self.encoder.write_i64(value)
    }

    fn serialize_i128(self, value: i128) -> Result<()> {
        self.encoder.write_i128(value)
    }

    fn serialize_u8(self, value: u8) -> Result<()> {
        self.serialize_u64(value as u64)
    }

    fn serialize_u16(self, value: u16) -> Result<()> {
        self.serialize_u64(value as u64)
    }

    fn serialize_u32(self, value: u32) -> Result<()> {
        self.serialize_u64(value as u64)
    }

    fn serialize_u64(self, value: u64) -> Result<()> {
        self.encoder.write_u64(value)
    }

    fn serialize_u128(self, value: u128) -> Result<()> {
        self.encoder.write_u128(value)
    }

    fn serialize_f32(self, value: f32) -> Result<()> {
        self.encoder.write_f32(value)
    }

    fn serialize_f64(self, value: f64) -> Result<()> {
        self.encoder.write_f64(value)
    }

    fn serialize_char(self, value: char) -> Result<()> {
        self.encoder.write_char(value)
    }

    fn serialize_str(self, value: &str) -> Result<()> {
        self.encoder.write_str(value)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<()> {
        self.encoder.begin_array()?;
        for byte in value {
            self.encoder.separator()?;
            self.encoder.write_u64(*byte as u64)?;
        }
        self.encoder.end_array()
    }

    fn serialize_none(self) -> Result<()> {
        self.encoder.write_null()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<()> {
        self.encoder.write_null()
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<()> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<()> {
        self.encoder.write_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<()> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()> {
        self.encoder.begin_object()?;
        self.encoder.key(variant)?;
        value.serialize(&mut *self)?;
        self.encoder.end_object()
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq> {
        self.encoder.begin_array()?;
        Ok(Sequence { serializer: self })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        self.encoder.begin_object()?;
        self.encoder.key(variant)?;
        self.encoder.begin_array()?;
        Ok(TupleVariant { serializer: self })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap> {
        self.encoder.begin_object()?;
        Ok(Object {
            serializer: self,
            key_pending: false,
        })
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Self::SerializeStruct> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        self.encoder.begin_object()?;
        self.encoder.key(variant)?;
        self.encoder.begin_object()?;
        Ok(StructVariant { serializer: self })
    }

    fn collect_str<T: core::fmt::Display + ?Sized>(self, value: &T) -> Result<()> {
        self.encoder.write_str(&value.to_string())
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

/// Serde sequence state.
pub struct Sequence<'a, W: Write> {
    serializer: &'a mut Serializer<W>,
}

impl<W: Write> SerializeSeq for Sequence<'_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.serializer.encoder.separator()?;
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<()> {
        self.serializer.encoder.end_array()
    }
}

impl<W: Write> SerializeTuple for Sequence<'_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<()> {
        SerializeSeq::end(self)
    }
}

impl<W: Write> SerializeTupleStruct for Sequence<'_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<()> {
        SerializeSeq::end(self)
    }
}

/// Serde object state.
pub struct Object<'a, W: Write> {
    serializer: &'a mut Serializer<W>,
    key_pending: bool,
}

impl<W: Write> SerializeMap for Object<'_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        if self.key_pending {
            return Err(Error::custom("serialize_key called before serialize_value"));
        }
        let key = key.serialize(MapKeySerializer)?;
        self.serializer.encoder.key(&key)?;
        self.key_pending = true;
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        if !self.key_pending {
            return Err(Error::custom("serialize_value called before serialize_key"));
        }
        value.serialize(&mut *self.serializer)?;
        self.key_pending = false;
        Ok(())
    }

    fn end(self) -> Result<()> {
        if self.key_pending {
            return Err(Error::custom("map ended without a value for its last key"));
        }
        self.serializer.encoder.end_object()
    }
}

impl<W: Write> SerializeStruct for Object<'_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.serializer.encoder.key(key)?;
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<()> {
        self.serializer.encoder.end_object()
    }
}

/// Serde tuple-variant state.
pub struct TupleVariant<'a, W: Write> {
    serializer: &'a mut Serializer<W>,
}

impl<W: Write> SerializeTupleVariant for TupleVariant<'_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.serializer.encoder.separator()?;
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<()> {
        self.serializer.encoder.end_array()?;
        self.serializer.encoder.end_object()
    }
}

/// Serde struct-variant state.
pub struct StructVariant<'a, W: Write> {
    serializer: &'a mut Serializer<W>,
}

impl<W: Write> SerializeStructVariant for StructVariant<'_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.serializer.encoder.key(key)?;
        value.serialize(&mut *self.serializer)
    }

    fn end(self) -> Result<()> {
        self.serializer.encoder.end_object()?;
        self.serializer.encoder.end_object()
    }
}

struct MapKeySerializer;

impl serde::Serializer for MapKeySerializer {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    fn serialize_bool(self, value: bool) -> Result<String> {
        Ok(if value { "true" } else { "false" }.into())
    }

    fn serialize_i8(self, value: i8) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_i16(self, value: i16) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_i32(self, value: i32) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_i64(self, value: i64) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_i128(self, value: i128) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_u8(self, value: u8) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_u16(self, value: u16) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_u32(self, value: u32) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_u64(self, value: u64) -> Result<String> {
        Ok(value.to_string())
    }
    fn serialize_u128(self, value: u128) -> Result<String> {
        Ok(value.to_string())
    }

    fn serialize_f32(self, _value: f32) -> Result<String> {
        invalid_map_key()
    }
    fn serialize_f64(self, _value: f64) -> Result<String> {
        invalid_map_key()
    }

    fn serialize_char(self, value: char) -> Result<String> {
        Ok(value.into())
    }
    fn serialize_str(self, value: &str) -> Result<String> {
        Ok(value.into())
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<String> {
        invalid_map_key()
    }

    fn serialize_none(self) -> Result<String> {
        invalid_map_key()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<String> {
        invalid_map_key()
    }
    fn serialize_unit(self) -> Result<String> {
        invalid_map_key()
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<String> {
        invalid_map_key()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<String> {
        Ok(variant.into())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<String> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<String> {
        invalid_map_key()
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq> {
        invalid_map_key()
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple> {
        invalid_map_key()
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        invalid_map_key()
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        invalid_map_key()
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap> {
        invalid_map_key()
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        invalid_map_key()
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        invalid_map_key()
    }

    fn collect_str<T: core::fmt::Display + ?Sized>(self, value: &T) -> Result<String> {
        Ok(value.to_string())
    }

    fn is_human_readable(&self) -> bool {
        true
    }
}

fn invalid_map_key<T>() -> Result<T> {
    Err(Error::custom(
        "JSON object keys must be strings, integers, booleans, or chars",
    ))
}

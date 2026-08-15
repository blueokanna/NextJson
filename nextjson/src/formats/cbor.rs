//! CBOR codec (RFC 8949) exposed through the unified [`Format`] interface.
//!
//! The codec relays through the crate's streaming [`crate::cross_format`]
//! engine: `T` is encoded to compact JSON events and streamed into a
//! [`CborSink`], and CBOR bytes are streamed into a [`JsonSink`] before
//! decoding. This reuses the single validated relay implementation rather
//! than duplicating a third tokenizer.

use alloc::vec::Vec;

use crate::cross_format;
use crate::de::{Decoder, NsonDeserialize};
use crate::encoding::{EncodeConfig, Encoder};
use crate::error::Result;
use crate::formats::tree;
use crate::formats::Format;
use crate::ser::NsonSerialize;

/// CBOR format marker.
#[derive(Clone, Copy, Debug)]
pub struct Cbor;

impl Format for Cbor {
    const NAME: &'static str = "cbor";
    const MIME: &'static str = "application/cbor";
    const EXTENSIONS: &'static [&'static str] = &["cbor"];
    const BINARY: bool = true;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = Encoder::<Vec<u8>>::for_vec(EncodeConfig::compact());
        T::nextencode(value, &mut encoder)?;
        let json = encoder.finish_vec()?;
        cross_format::json_to_cbor(&json)
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let json = cross_format::cbor_to_json(input)?;
        let mut parser = Decoder::new(&json);
        let value = crate::Value::nextdecode(&mut parser)?;
        parser.end()?;
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value)?);
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

/// The CBOR encoder writes through the JSON relay.
pub type CborEncoder<W> = Encoder<W>;

/// CBOR decoder serving the unified interface over a JSON relay.
///
/// CBOR bytes are streamed to JSON and parsed into a [`crate::Value`], which
/// is then replayed through the shared [`crate::formats::TreeDecoder`].
pub type CborDecoder<'de> = tree::TreeDecoder<'de>;

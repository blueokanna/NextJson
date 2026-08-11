//! JSON codec (RFC 8259) exposed through the unified [`Format`] interface.
//!
//! This is the crate's native codec: encoding drives the hand-rolled
//! [`Encoder`] and decoding drives the lazy single-token [`Decoder`], so the
//! `json` format marker is a thin alias over the highest-performance path in
//! the library.

use alloc::vec::Vec;

use crate::de::{Decoder, NsonDeserialize};
use crate::encoding::{EncodeConfig, Encoder};
use crate::error::Result;
use crate::formats::Format;
use crate::ser::NsonSerialize;

/// JSON format marker.
#[derive(Clone, Copy, Debug)]
pub struct Json;

impl Format for Json {
    const NAME: &'static str = "json";
    const MIME: &'static str = "application/json";
    const EXTENSIONS: &'static [&'static str] = &["json"];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, value: &T) -> Result<Vec<u8>> {
        let mut encoder = Encoder::for_vec(EncodeConfig::compact());
        T::nextencode(value, &mut encoder)?;
        encoder.finish_vec()
    }

    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let mut decoder = JsonDecoder::new(input);
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

/// The JSON encoder is the native buffered encoder.
pub type JsonEncoder<W> = Encoder<W>;

/// JSON decoder serving the unified interface (the native [`Decoder`], which
/// already implements [`crate::de::FormatDecoder`]).
pub type JsonDecoder<'de> = Decoder<'de>;

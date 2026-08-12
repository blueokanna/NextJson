//! Explicit byte-string wrapper for compact binary wire types.
//!
//! As in serde, plain `Vec<u8>` / `&[u8]` / `[u8; N]` keep the generic
//! sequence representation (an array of `u8`), which is lossless everywhere.
//! A type that wants a *native* byte string on the wire (length prefix + raw
//! bytes in binary formats) wraps its slice in [`Bytes`]; the encoder then
//! calls [`FormatEncoder::write_bytes`](crate::ser::FormatEncoder::write_bytes)
//! and the decoder
//! [`FormatDecoder::bytes`](crate::de::FormatDecoder::bytes).

use alloc::borrow::Cow;
use core::ops::Deref;

use crate::de::{DecodeSlot, FormatDecoder, NsonDeserialize};
use crate::error::{Error, Result};
use crate::schema::{NsonSchema, TypeSchema};
use crate::ser::{FormatEncoder, NsonSerialize};

/// A borrowed byte string that round-trips through the dedicated bytes path.
///
/// ```rust
/// use nextjson::Bytes;
///
/// let value = Bytes(b"\x00\x01binary");
/// let json = nextjson::nextencode(&value)?;
/// // JSON keeps the array spelling for compatibility with plain `Vec<u8>`:
/// assert_eq!(json, b"[0,1,98,105,110,97,114,121]");
/// # Ok::<(), nextjson::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Bytes<'a>(pub &'a [u8]);

impl<'a> Bytes<'a> {
    /// The wrapped byte slice.
    pub fn as_bytes(&self) -> &'a [u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for Bytes<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        Bytes(bytes)
    }
}

impl<'a> From<&'a str> for Bytes<'a> {
    fn from(text: &'a str) -> Self {
        Bytes(text.as_bytes())
    }
}

impl<'a> Deref for Bytes<'a> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> AsRef<[u8]> for Bytes<'a> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl NsonSchema for Bytes<'_> {
    const SCHEMA: TypeSchema = TypeSchema::Bytes;
}

impl NsonSerialize for Bytes<'_> {
    fn nextencode<E: FormatEncoder>(&self, encoder: &mut E) -> Result<(), E::Error> {
        encoder.write_bytes(self.0)
    }
}

impl<'de, 'a> NsonDeserialize<'de> for Bytes<'a>
where
    'de: 'a,
{
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        match decoder.bytes()? {
            Cow::Borrowed(b) => {
                out.write(Bytes(b));
                Ok(())
            }
            Cow::Owned(_) => Err(Error::invalid_type(
                "a borrowed byte string (no escape sequences)",
                "bytes",
            )
            .into()),
        }
    }
}

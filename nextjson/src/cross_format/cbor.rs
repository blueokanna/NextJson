use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use crate::cross_format::EventSink;
use crate::error::{Error, ErrorKind, Result};
use crate::event_state::{EventState, Kind};
use crate::number::Number;
use crate::write::Write;

const FLUSH_THRESHOLD: usize = 8192;

/// Streaming RFC 8949 CBOR destination for JSON-compatible events.
///
/// Arrays and maps use CBOR's indefinite-length representation so values can
/// be relayed without pre-counting or buffering a tree. Integer values through
/// `u128`/`i128` use standard major types or bignum tags 2 and 3. Raw byte
/// strings and non-string map keys are outside the JSON-compatible profile.
pub struct CborSink<W: Write> {
    writer: W,
    buffer: Vec<u8>,
    structure: EventState,
}

impl<W: Write> CborSink<W> {
    /// Create a CBOR event sink over `writer`.
    pub fn new(writer: W) -> Self {
        CborSink {
            writer,
            buffer: Vec::with_capacity(1024),
            structure: EventState::new(false),
        }
    }

    /// Validate completion, flush output, and return the writer.
    pub fn finish(mut self) -> Result<W> {
        self.structure.finish()?;
        self.writer.write_all(&self.buffer)?;
        self.buffer.clear();
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn push(&mut self, byte: u8) -> Result<()> {
        self.buffer.push(byte);
        self.maybe_flush()
    }

    fn extend(&mut self, bytes: &[u8]) -> Result<()> {
        self.buffer.extend_from_slice(bytes);
        self.maybe_flush()
    }

    fn maybe_flush(&mut self) -> Result<()> {
        if self.buffer.len() >= FLUSH_THRESHOLD {
            self.writer.write_all(&self.buffer)?;
            self.buffer.clear();
        }
        Ok(())
    }

    fn type_and_argument(&mut self, major: u8, argument: u64) -> Result<()> {
        let prefix = major << 5;
        if argument < 24 {
            self.push(prefix | argument as u8)
        } else if argument <= u8::MAX as u64 {
            self.extend(&[prefix | 24, argument as u8])
        } else if argument <= u16::MAX as u64 {
            self.extend(&[prefix | 25])?;
            self.extend(&(argument as u16).to_be_bytes())
        } else if argument <= u32::MAX as u64 {
            self.extend(&[prefix | 26])?;
            self.extend(&(argument as u32).to_be_bytes())
        } else {
            self.extend(&[prefix | 27])?;
            self.extend(&argument.to_be_bytes())
        }
    }

    fn unsigned(&mut self, value: u128) -> Result<()> {
        if let Ok(value) = u64::try_from(value) {
            self.type_and_argument(0, value)
        } else {
            self.bignum(2, value)
        }
    }

    fn signed(&mut self, value: i128) -> Result<()> {
        if value >= 0 {
            return self.unsigned(value as u128);
        }
        let argument = (-1 - value) as u128;
        if let Ok(argument) = u64::try_from(argument) {
            self.type_and_argument(1, argument)
        } else {
            self.bignum(3, argument)
        }
    }

    fn bignum(&mut self, tag: u64, value: u128) -> Result<()> {
        self.type_and_argument(6, tag)?;
        let bytes = value.to_be_bytes();
        let first = bytes
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(bytes.len() - 1);
        let magnitude = &bytes[first..];
        self.type_and_argument(2, magnitude.len() as u64)?;
        self.extend(magnitude)
    }
}

impl<W: Write> EventSink for CborSink<W> {
    fn null(&mut self) -> Result<()> {
        self.structure.value()?;
        self.push(0xf6)
    }

    fn boolean(&mut self, value: bool) -> Result<()> {
        self.structure.value()?;
        self.push(if value { 0xf5 } else { 0xf4 })
    }

    fn number(&mut self, value: Number) -> Result<()> {
        self.structure.value()?;
        match value {
            Number::I64(value) => self.signed(value as i128),
            Number::U64(value) => self.unsigned(value as u128),
            Number::I128(value) => self.signed(value),
            Number::U128(value) => self.unsigned(value),
            Number::F64(value) if value.is_finite() => {
                self.push(0xfb)?;
                self.extend(&value.to_bits().to_be_bytes())
            }
            Number::F64(_) => Err(Error::custom("CBOR profile rejects non-finite floats")),
        }
    }

    fn string(&mut self, value: &str) -> Result<()> {
        self.structure.value()?;
        self.type_and_argument(3, value.len() as u64)?;
        self.extend(value.as_bytes())
    }

    fn begin_array(&mut self) -> Result<()> {
        self.structure.begin(Kind::Array)?;
        self.push(0x9f)
    }

    fn end_array(&mut self) -> Result<()> {
        self.structure.end(Kind::Array)?;
        self.push(0xff)
    }

    fn begin_object(&mut self) -> Result<()> {
        self.structure.begin(Kind::Object)?;
        self.push(0xbf)
    }

    fn object_key(&mut self, key: &str) -> Result<()> {
        self.structure.key()?;
        self.type_and_argument(3, key.len() as u64)?;
        self.extend(key.as_bytes())
    }

    fn end_object(&mut self) -> Result<()> {
        self.structure.end(Kind::Object)?;
        self.push(0xff)
    }
}

pub(super) fn cbor_into_with_max_depth<S: EventSink + ?Sized>(
    input: &[u8],
    max_depth: u32,
    sink: &mut S,
) -> Result<()> {
    let mut reader = CborReader {
        input,
        position: 0,
        depth: 0,
        max_depth,
    };
    reader.item(sink)?;
    if reader.position != input.len() {
        return Err(reader.error("trailing data after CBOR root value"));
    }
    Ok(())
}

struct CborReader<'de> {
    input: &'de [u8],
    position: usize,
    depth: u32,
    max_depth: u32,
}

impl<'de> CborReader<'de> {
    fn item<S: EventSink + ?Sized>(&mut self, sink: &mut S) -> Result<()> {
        let initial = self.byte()?;
        let major = initial >> 5;
        let additional = initial & 0x1f;
        match major {
            0 => {
                let value = self.required_argument(additional)?;
                sink.number(Number::U64(value))
            }
            1 => {
                let argument = self.required_argument(additional)?;
                let value = if argument <= i64::MAX as u64 {
                    Number::I64(-1 - argument as i64)
                } else {
                    Number::I128(-1 - argument as i128)
                };
                sink.number(value)
            }
            2 => Err(self.error("CBOR byte strings are not representable in JSON")),
            3 => {
                let value = self.text(additional)?;
                sink.string(value.as_ref())
            }
            4 => self.array(additional, sink),
            5 => self.map(additional, sink),
            6 => self.tag(additional, sink),
            7 => self.simple(additional, sink),
            _ => Err(self.error("invalid CBOR major type")),
        }
    }

    fn array<S: EventSink + ?Sized>(&mut self, additional: u8, sink: &mut S) -> Result<()> {
        self.enter()?;
        sink.begin_array()?;
        match self.argument(additional)? {
            Some(length) => {
                for _ in 0..length {
                    self.item(sink)?;
                }
            }
            None => {
                while !self.take_break()? {
                    self.item(sink)?;
                }
            }
        }
        self.leave();
        sink.end_array()
    }

    fn map<S: EventSink + ?Sized>(&mut self, additional: u8, sink: &mut S) -> Result<()> {
        self.enter()?;
        sink.begin_object()?;
        match self.argument(additional)? {
            Some(length) => {
                for _ in 0..length {
                    let key = self.text_item()?;
                    sink.object_key(key.as_ref())?;
                    self.item(sink)?;
                }
            }
            None => loop {
                if self.take_break()? {
                    break;
                }
                let key = self.text_item()?;
                sink.object_key(key.as_ref())?;
                self.item(sink)?;
            },
        }
        self.leave();
        sink.end_object()
    }

    fn tag<S: EventSink + ?Sized>(&mut self, additional: u8, sink: &mut S) -> Result<()> {
        let tag = self.required_argument(additional)?;
        if tag != 2 && tag != 3 {
            return Err(self.error("unsupported CBOR semantic tag"));
        }
        let magnitude = self.bignum_magnitude()?;
        let number = if tag == 2 {
            if magnitude <= u64::MAX as u128 {
                Number::U64(magnitude as u64)
            } else {
                Number::U128(magnitude)
            }
        } else if magnitude <= i64::MAX as u128 {
            Number::I64(-1 - magnitude as i64)
        } else if magnitude <= i128::MAX as u128 {
            Number::I128(-1 - magnitude as i128)
        } else {
            return Err(self.error("negative CBOR bignum exceeds i128"));
        };
        sink.number(number)
    }

    fn simple<S: EventSink + ?Sized>(&mut self, additional: u8, sink: &mut S) -> Result<()> {
        match additional {
            20 => sink.boolean(false),
            21 => sink.boolean(true),
            22 => sink.null(),
            25 => {
                let bits = self.u16()?;
                let value = half_to_f32(bits) as f64;
                self.finite_float(value, sink)
            }
            26 => {
                let value = f32::from_bits(self.u32()?) as f64;
                self.finite_float(value, sink)
            }
            27 => {
                let value = f64::from_bits(self.u64()?);
                self.finite_float(value, sink)
            }
            31 => Err(self.error("unexpected CBOR break marker")),
            _ => Err(self.error("unsupported CBOR simple value")),
        }
    }

    fn finite_float<S: EventSink + ?Sized>(&self, value: f64, sink: &mut S) -> Result<()> {
        if !value.is_finite() {
            return Err(self.error("non-finite CBOR float is not representable in JSON"));
        }
        sink.number(Number::F64(value))
    }

    fn text_item(&mut self) -> Result<Cow<'de, str>> {
        let initial = self.byte()?;
        if initial >> 5 != 3 {
            return Err(self.error("CBOR map key must be a text string"));
        }
        self.text(initial & 0x1f)
    }

    fn text(&mut self, additional: u8) -> Result<Cow<'de, str>> {
        match self.argument(additional)? {
            Some(length) => Ok(Cow::Borrowed(self.definite_text(length)?)),
            None => {
                let mut output = String::new();
                loop {
                    if self.take_break()? {
                        break;
                    }
                    let initial = self.byte()?;
                    if initial >> 5 != 3 || initial & 0x1f == 31 {
                        return Err(self.error("invalid indefinite CBOR text chunk"));
                    }
                    let length = self.required_argument(initial & 0x1f)?;
                    output.push_str(self.definite_text(length)?);
                }
                Ok(Cow::Owned(output))
            }
        }
    }

    fn definite_text(&mut self, length: u64) -> Result<&'de str> {
        let length = usize::try_from(length).map_err(|_| self.error("CBOR text too large"))?;
        let bytes = self.bytes(length)?;
        core::str::from_utf8(bytes).map_err(|_| self.error("invalid UTF-8 in CBOR text"))
    }

    fn bignum_magnitude(&mut self) -> Result<u128> {
        let initial = self.byte()?;
        if initial >> 5 != 2 || initial & 0x1f == 31 {
            return Err(self.error("CBOR bignum tag must contain a definite byte string"));
        }
        let length = self.required_argument(initial & 0x1f)?;
        if length > 16 {
            return Err(self.error("CBOR bignum exceeds 128 bits"));
        }
        let bytes = self.bytes(length as usize)?;
        let mut value = 0_u128;
        for byte in bytes {
            value = (value << 8) | *byte as u128;
        }
        Ok(value)
    }

    fn argument(&mut self, additional: u8) -> Result<Option<u64>> {
        match additional {
            0..=23 => Ok(Some(additional as u64)),
            24 => Ok(Some(self.byte()? as u64)),
            25 => Ok(Some(self.u16()? as u64)),
            26 => Ok(Some(self.u32()? as u64)),
            27 => Ok(Some(self.u64()?)),
            31 => Ok(None),
            _ => Err(self.error("reserved CBOR additional information")),
        }
    }

    fn required_argument(&mut self, additional: u8) -> Result<u64> {
        self.argument(additional)?
            .ok_or_else(|| self.error("indefinite length is invalid for this CBOR type"))
    }

    fn enter(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(self.error("CBOR nesting limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn take_break(&mut self) -> Result<bool> {
        if self.input.get(self.position) == Some(&0xff) {
            self.position += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn byte(&mut self) -> Result<u8> {
        let byte = self
            .input
            .get(self.position)
            .copied()
            .ok_or_else(|| self.error("unexpected end of CBOR input"))?;
        self.position += 1;
        Ok(byte)
    }

    fn bytes(&mut self, length: usize) -> Result<&'de [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| self.error("CBOR length overflow"))?;
        let bytes = self
            .input
            .get(self.position..end)
            .ok_or_else(|| self.error("unexpected end of CBOR input"))?;
        self.position = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes: [u8; 2] = self
            .bytes(2)?
            .try_into()
            .map_err(|_| self.error("invalid CBOR u16"))?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes: [u8; 4] = self
            .bytes(4)?
            .try_into()
            .map_err(|_| self.error("invalid CBOR u32"))?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64> {
        let bytes: [u8; 8] = self
            .bytes(8)?
            .try_into()
            .map_err(|_| self.error("invalid CBOR u64"))?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn error(&self, message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Custom(message.into()), None, None, self.position)
    }
}

/// Convert an IEEE 754 half-precision bit pattern to `f32`.
///
/// Shared with the native `formats::Cbor` codec so the two CBOR paths use the
/// exact same conversion.
pub(crate) fn half_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let mut fraction = (bits & 0x03ff) as u32;
    let output = if exponent == 0 {
        if fraction == 0 {
            sign
        } else {
            let mut unbiased = -14;
            while fraction & 0x0400 == 0 {
                fraction <<= 1;
                unbiased -= 1;
            }
            fraction &= 0x03ff;
            sign | (((unbiased + 127) as u32) << 23) | (fraction << 13)
        }
    } else if exponent == 0x1f {
        sign | 0x7f80_0000 | (fraction << 13)
    } else {
        sign | (((exponent - 15 + 127) as u32) << 23) | (fraction << 13)
    };
    f32::from_bits(output)
}

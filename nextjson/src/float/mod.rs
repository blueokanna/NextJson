//! Finite IEEE-754 to shortest decimal conversion, shared by text encoders.
//!
//! Directed integer intervals cover normal magnitudes; core's shortest
//! scientific formatter handles subnormals and ambiguous boundaries.
//! Codecs own framing and errors. Conversion has no shared mutable state.

mod decimal;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use core::fmt::{self, Write};

use crate::error::{Error, ErrorKind, Result};

pub(crate) const BATCH_SIZE: usize = 64;
const MAX_FORMATTED_LEN: usize = 24;

pub(crate) trait Float: Copy {
    /// Sign, decimal mantissa (possibly with trailing zeros), and exponent.
    fn decimal(self) -> Result<(bool, u64, i32)>;
    /// Exact multiples of 1/8 within the source width's fixed-notation bound.
    fn exact_eighth(self) -> Option<(bool, u64)>;
}

macro_rules! impl_float {
    ($ty:ty, $bits:expr, $mask:expr, $convert:path) => {
        impl Float for $ty {
            #[inline(always)]
            fn exact_eighth(self) -> Option<(bool, u64)> {
                let bits = self.to_bits();
                // Every accepted value has at least this many zero low bits.
                // Reject ordinary decimal fractions before exponent decoding.
                let low_bits = if $bits == 52 { 10 } else { 7 };
                if bits & ((1 << low_bits) - 1) != 0 {
                    return None;
                }
                let fraction = bits & ((1 << $bits) - 1);
                let exponent = ((bits >> $bits) & $mask) as u32;
                if exponent == 0 && fraction == 0 {
                    return Some((self.is_sign_negative(), 0));
                }
                let bias = $mask >> 1;
                let upper = bias + if $bits == 23 { 13 } else { 39 };
                if !(bias - 3..=upper).contains(&exponent) {
                    return None;
                }
                let shift = $bits + bias - 3 - exponent;
                let significand = fraction | (1 << $bits);
                if significand & ((1 << shift) - 1) != 0 {
                    return None;
                }
                let scaled = (significand >> shift) as u64;
                if $bits == 52 && scaled >= 8_000_000_000_000 {
                    return None;
                }
                Some((self.is_sign_negative(), scaled))
            }

            #[inline(always)]
            fn decimal(self) -> Result<(bool, u64, i32)> {
                if let Some((negative, eighths)) = self.exact_eighth() {
                    return Ok((negative, eighths * 125, -3));
                }
                if !self.is_finite() {
                    return Err(Error::new(ErrorKind::NonFiniteFloat, None, None, 0));
                }
                let bits = self.to_bits();
                let mantissa = bits & ((1 << $bits) - 1);
                let exponent = ((bits >> $bits) & $mask) as u32;
                let negative = self.is_sign_negative();
                let decimal = match $convert(mantissa as u64, exponent) {
                    Some(decimal) => decimal,
                    None => fallback(self),
                };
                Ok((negative, decimal.mantissa, decimal.exponent))
            }
        }
    };
}

impl_float!(f64, 52, 0x7ff, decimal::convert::<52>);
impl_float!(f32, 23, 0xff, decimal::convert::<23>);

fn fallback(value: impl fmt::LowerExp) -> decimal::Decimal {
    struct Scientific {
        bytes: [u8; 24],
        len: usize,
    }
    impl Write for Scientific {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let end = self.len + text.len();
            let destination = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
            destination.copy_from_slice(text.as_bytes());
            self.len = end;
            Ok(())
        }
    }
    let mut output = Scientific {
        bytes: [0; 24],
        len: 0,
    };
    write!(output, "{value:e}").expect("finite IEEE scientific notation fits 24 bytes");
    let bytes = &output.bytes[..output.len];
    let mut mantissa = 0;
    let mut digits = 0;
    let mut i = usize::from(bytes[0] == b'-');
    while bytes[i] != b'e' {
        if bytes[i] != b'.' {
            mantissa = mantissa * 10 + u64::from(bytes[i] - b'0');
            digits += 1;
        }
        i += 1;
    }
    i += 1;
    let negative = bytes[i] == b'-';
    i += usize::from(negative || bytes[i] == b'+');
    let mut exponent = 0;
    while i < bytes.len() {
        exponent = exponent * 10 + i32::from(bytes[i] - b'0');
        i += 1;
    }
    decimal::Decimal {
        mantissa,
        exponent: (if negative { -exponent } else { exponent }) - digits + 1,
    }
}

/// The wire representation needs at most 24 bytes. Extra scratch space
/// lets fixed and scientific notation share digits without copying them
/// into a second buffer. All populated ranges contain ASCII.
pub(crate) struct Buffer {
    bytes: [u8; 32],
    start: usize,
    end: usize,
}

impl Buffer {
    #[inline(always)]
    pub(crate) fn format(value: impl Float) -> Result<Self> {
        Ok(Self::from_decimal(value.decimal()?))
    }

    #[inline(always)]
    fn from_decimal((negative, mantissa, exponent): (bool, u64, i32)) -> Self {
        let mut output = Self {
            bytes: [0; 32],
            start: 0,
            end: 0,
        };
        if mantissa == 0 {
            output.bytes[..4].copy_from_slice(b"-0.0");
            output.start = usize::from(!negative);
            output.end = 4;
            return output;
        }
        let (mut start, mut end) = decimal_digits(mantissa, &mut output.bytes);
        let original_exponent = exponent;
        let exponent = exponent + (24 - end) as i32;
        let digits = end - start;
        let point = digits as i32 + exponent;
        if exponent >= 0 && point <= 16 {
            output.bytes.copy_within(start..end, 1);
            start = 1;
            end = 1 + point as usize;
            output.bytes[1 + digits..end].fill(b'0');
            output.bytes[end..end + 2].copy_from_slice(b".0");
            end += 2;
        } else if point > 0 && point <= 16 {
            insert_dot(&mut output.bytes, (-original_exponent) as u32);
            start -= 1;
        } else if point > -5 && point <= 0 {
            let before = start;
            start -= (2 - point) as usize;
            output.bytes[start..start + 2].copy_from_slice(b"0.");
            output.bytes[start + 2..before].fill(b'0');
        } else {
            if digits > 1 {
                output.bytes[start - 1] = output.bytes[start];
                output.bytes[start] = b'.';
                start -= 1;
            }
            output.bytes[end] = b'e';
            end += 1;
            let mut exponent = point - 1;
            if exponent < 0 {
                output.bytes[end] = b'-';
                end += 1;
                exponent = -exponent;
            }
            if exponent >= 100 {
                output.bytes[end] = b'0' + (exponent / 100) as u8;
                end += 1;
                exponent %= 100;
                let pair = exponent as usize * 2;
                output.bytes[end..end + 2].copy_from_slice(&DIGITS[pair..pair + 2]);
                end += 2;
            } else if exponent >= 10 {
                let pair = exponent as usize * 2;
                output.bytes[end..end + 2].copy_from_slice(&DIGITS[pair..pair + 2]);
                end += 2;
            } else {
                output.bytes[end] = b'0' + exponent as u8;
                end += 1;
            }
        }
        if negative {
            start -= 1;
            output.bytes[start] = b'-';
        }
        output.start = start;
        output.end = end;
        debug_assert!(end - start <= MAX_FORMATTED_LEN);
        output
    }

    #[inline]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[self.start..self.end]
    }

    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).expect("decimal output is ASCII")
    }
}

const DIGITS: &[u8; 200] = b"00010203040506070809101112131415161718192021222324252627282930313233343536373839404142434445464748495051525354555657585960616263646566676869707172737475767778798081828384858687888990919293949596979899";

#[inline]
pub(crate) fn write_float_into<T: Float>(buf: &mut Vec<u8>, value: T) -> Result<()> {
    // Preserve the existing exact-eighth path: records and telemetry often
    // contain integers/halves/quarters. The general converter should not
    // make those values pay for an interval calculation or digit packing.
    // Bit classification rejects other exponent ranges immediately, without
    // doing float arithmetic or a float-to-integer cast on ordinary decimals.
    // Bounds keep adjacent three-place decimals distinct at the source width.
    if let Some((negative, integer)) = value.exact_eighth() {
        write_eighth(buf, negative, integer);
        return Ok(());
    }
    write_general_float(buf, value)
}

#[inline]
fn write_eighth(buf: &mut Vec<u8>, negative: bool, eighths: u64) {
    if negative {
        buf.push(b'-');
    }
    crate::ser::write_u64_into(buf, eighths / 8);
    const FRACTIONS: [&[u8]; 8] = [
        b".0", b".125", b".25", b".375", b".5", b".625", b".75", b".875",
    ];
    buf.extend_from_slice(FRACTIONS[(eighths % 8) as usize]);
}

#[inline(always)]
fn write_general_float(buf: &mut Vec<u8>, value: impl Float) -> Result<()> {
    buf.extend_from_slice(Buffer::format(value)?.as_bytes());
    Ok(())
}

pub(crate) fn write_many<T: Float>(buf: &mut Vec<u8>, values: &[T], separator: u8) -> Result<()> {
    let Some((&first, values)) = values.split_first() else {
        return Ok(());
    };
    write_float_into(buf, first)?;
    for chunk in values.chunks(BATCH_SIZE) {
        // Stage conversion separately so digit formatting does not extend
        // each value's arithmetic dependency chain. Scratch space is bounded
        // independently of the input length and belongs to this call.
        let mut decimals = [(false, 0, 0); BATCH_SIZE];
        for (decimal, &value) in decimals.iter_mut().zip(chunk) {
            *decimal = value.decimal()?;
        }
        let decimals = &decimals[..chunk.len()];
        // Integer/quarter/eighth telemetry keeps the direct integer writer.
        // This test uses the prepared decimal itself, so no input is rounded
        // or classified twice. m * 10^-3 = (m / 125) / 8 exactly.
        if decimals.iter().all(|&(_, m, e)| e == -3 && m % 125 == 0) {
            for &(negative, mantissa, _) in decimals {
                buf.push(separator);
                write_eighth(buf, negative, mantissa / 125);
            }
            continue;
        }
        buf.reserve(chunk.len() * (MAX_FORMATTED_LEN + 1));
        for &decimal in decimals {
            let number = Buffer::from_decimal(decimal);
            buf.push(separator);
            buf.extend_from_slice(number.as_bytes());
        }
    }
    Ok(())
}

const fn digit_quads() -> [u32; 10_000] {
    let mut result = [0; 10_000];
    let mut i = 0;
    while i < result.len() {
        result[i] = u32::from_be_bytes([
            b'0' + (i / 1000) as u8,
            b'0' + (i / 100 % 10) as u8,
            b'0' + (i / 10 % 10) as u8,
            b'0' + (i % 10) as u8,
        ]);
        i += 1;
    }
    result
}

static QUADS: [u32; 10_000] = digit_quads();

#[inline(always)]
fn four_digits(value: u32) -> u32 {
    QUADS[value as usize]
}

#[inline(always)]
fn eight_digits(value: u32) -> u64 {
    (u64::from(four_digits(value / 10_000)) << 32) | u64::from(four_digits(value % 10_000))
}

#[inline(always)]
fn decimal_digits(value: u64, bytes: &mut [u8; 32]) -> (usize, usize) {
    if value < 100 {
        if value < 10 {
            bytes[23] = b'0' + value as u8;
            return (23, 24);
        }
        let pair = value as usize * 2;
        bytes[22..24].copy_from_slice(&DIGITS[pair..pair + 2]);
        return (22, if value % 10 == 0 { 23 } else { 24 });
    }
    let low = eight_digits((value % 100_000_000) as u32);
    bytes[16..24].copy_from_slice(&low.to_be_bytes());
    let low_digits = low ^ 0x3030_3030_3030_3030;
    let top = value / 100_000_000;
    if top == 0 {
        (
            16 + low_digits.leading_zeros() as usize / 8,
            24 - low_digits.trailing_zeros() as usize / 8,
        )
    } else {
        let high = eight_digits((top % 100_000_000) as u32);
        bytes[8..16].copy_from_slice(&high.to_be_bytes());
        let high_digits = high ^ 0x3030_3030_3030_3030;
        let prefix = top / 100_000_000;
        let start = if prefix != 0 {
            bytes[7] = b'0' + prefix as u8;
            7
        } else {
            8 + high_digits.leading_zeros() as usize / 8
        };
        let end = if low_digits != 0 {
            24 - low_digits.trailing_zeros() as usize / 8
        } else if high_digits != 0 {
            16 - high_digits.trailing_zeros() as usize / 8
        } else {
            8
        };
        (start, end)
    }
}

#[inline(always)]
fn insert_dot(bytes: &mut [u8; 32], fractional_digits: u32) {
    let low = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
    let high = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
    bytes[6] = bytes[7];
    if fractional_digits < 8 {
        let shift = fractional_digits * 8;
        let suffix = (1u64 << shift) - 1;
        bytes[7] = (high >> 56) as u8;
        let lower = (low & suffix) | ((low & !suffix) << 8) | (u64::from(b'.') << shift);
        let upper = (high << 8) | (low >> 56);
        bytes[16..24].copy_from_slice(&lower.to_be_bytes());
        bytes[8..16].copy_from_slice(&upper.to_be_bytes());
    } else if fractional_digits < 16 {
        let shift = (fractional_digits - 8) * 8;
        let suffix = (1u64 << shift) - 1;
        bytes[7] = (high >> 56) as u8;
        let upper = (high & suffix) | ((high & !suffix) << 8) | (u64::from(b'.') << shift);
        bytes[8..16].copy_from_slice(&upper.to_be_bytes());
    } else {
        bytes[7] = b'.';
    }
}

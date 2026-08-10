//! JSON number type with lossless Rust integer and finite `f64` storage.

use core::fmt;

use crate::error::{Error, ErrorKind, Result};

/// A JSON number.
///
/// Kept as an enum rather than a bare `f64` so every Rust integer round-trips
/// losslessly. Integer literals beyond `u128` are rejected instead of silently
/// losing precision.
#[derive(Clone, Copy, PartialEq)]
pub enum Number {
    /// Signed 64-bit integer.
    I64(i64),
    /// Unsigned 64-bit integer. All non-negative integers use this variant so
    /// that parsed and constructed numbers compare equal.
    U64(u64),
    /// Signed 128-bit integer outside the `i64` range.
    I128(i128),
    /// Unsigned 128-bit integer outside the `u64` range.
    U128(u128),
    /// 64-bit float (including `-0.0` and extremes).
    F64(f64),
}

impl Number {
    /// Whether this is an `i64`.
    pub fn is_i64(&self) -> bool {
        matches!(self, Number::I64(_))
    }
    /// Whether this is a `u64`.
    pub fn is_u64(&self) -> bool {
        matches!(self, Number::U64(_))
    }
    /// Whether this is an `i128`-backed value outside the `i64` range.
    pub fn is_i128(&self) -> bool {
        matches!(self, Number::I128(_))
    }
    /// Whether this is a `u128`-backed value outside the `u64` range.
    pub fn is_u128(&self) -> bool {
        matches!(self, Number::U128(_))
    }
    /// Whether this is an `f64`.
    pub fn is_f64(&self) -> bool {
        matches!(self, Number::F64(_))
    }

    /// Best-effort conversion to `i64` (fractional floats are truncated).
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Number::I64(v) => Some(v),
            Number::U64(v) => i64::try_from(v).ok(),
            Number::I128(v) => i64::try_from(v).ok(),
            Number::U128(v) => i64::try_from(v).ok(),
            Number::F64(v) => {
                if v >= i64::MIN as f64 && v < i64::MAX as f64 {
                    Some(v as i64)
                } else {
                    None
                }
            }
        }
    }

    /// Best-effort conversion to `u64` (fractional floats are truncated).
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Number::U64(v) => Some(v),
            Number::I64(v) => u64::try_from(v).ok(),
            Number::I128(v) => u64::try_from(v).ok(),
            Number::U128(v) => u64::try_from(v).ok(),
            Number::F64(v) => {
                if v >= 0.0 && v < u64::MAX as f64 {
                    Some(v as u64)
                } else {
                    None
                }
            }
        }
    }

    /// Exact conversion to `i128` when the value is integral and in range.
    pub fn as_i128(&self) -> Option<i128> {
        match *self {
            Number::I64(v) => Some(v as i128),
            Number::U64(v) => Some(v as i128),
            Number::I128(v) => Some(v),
            Number::U128(v) => i128::try_from(v).ok(),
            Number::F64(v) if v.is_finite() && v % 1.0 == 0.0 => {
                if v >= i128::MIN as f64 && v < i128::MAX as f64 {
                    Some(v as i128)
                } else {
                    None
                }
            }
            Number::F64(_) => None,
        }
    }

    /// Exact conversion to `u128` when the value is integral and in range.
    pub fn as_u128(&self) -> Option<u128> {
        match *self {
            Number::I64(v) => u128::try_from(v).ok(),
            Number::U64(v) => Some(v as u128),
            Number::I128(v) => u128::try_from(v).ok(),
            Number::U128(v) => Some(v),
            Number::F64(v) if v.is_finite() && v % 1.0 == 0.0 => {
                if v >= 0.0 && v < u128::MAX as f64 {
                    Some(v as u128)
                } else {
                    None
                }
            }
            Number::F64(_) => None,
        }
    }

    /// Convert to `f64` (may lose precision, never fails).
    pub fn as_f64(&self) -> f64 {
        match *self {
            Number::I64(v) => v as f64,
            Number::U64(v) => v as f64,
            Number::I128(v) => v as f64,
            Number::U128(v) => v as f64,
            Number::F64(v) => v,
        }
    }

    /// Build from an `f64`; returns `None` for non-finite values.
    pub fn from_f64(v: f64) -> Option<Number> {
        if v.is_finite() {
            Some(Number::F64(v))
        } else {
            None
        }
    }

    /// Whether this represents an integer value.
    pub fn is_integer(&self) -> bool {
        match *self {
            Number::I64(_) | Number::U64(_) | Number::I128(_) | Number::U128(_) => true,
            Number::F64(v) => v.is_finite() && v % 1.0 == 0.0,
        }
    }

    /// Whether this is finite.
    pub fn is_finite(&self) -> bool {
        match *self {
            Number::F64(v) => v.is_finite(),
            _ => true,
        }
    }

    /// Parse a JSON number byte slice (no surrounding whitespace).
    pub(crate) fn parse(raw: &[u8], is_float: bool) -> Result<Number> {
        if is_float {
            let s = core::str::from_utf8(raw)
                .map_err(|_| Error::new(ErrorKind::InvalidNumber, None, None, 0))?;
            let v: f64 = s
                .parse()
                .map_err(|_| Error::new(ErrorKind::InvalidNumber, None, None, 0))?;
            if !v.is_finite() {
                return Err(Error::new(ErrorKind::NumberOutOfRange, None, None, 0));
            }
            return Ok(Number::F64(v));
        }

        if raw.first() == Some(&b'-') {
            let value = parse_i128(raw)
                .ok_or_else(|| Error::new(ErrorKind::NumberOutOfRange, None, None, 0))?;
            if value >= i64::MIN as i128 {
                Ok(Number::I64(value as i64))
            } else {
                Ok(Number::I128(value))
            }
        } else {
            let value = parse_u128(raw)
                .ok_or_else(|| Error::new(ErrorKind::NumberOutOfRange, None, None, 0))?;
            if value <= u64::MAX as u128 {
                Ok(Number::U64(value as u64))
            } else {
                Ok(Number::U128(value))
            }
        }
    }
}

/// Hand-rolled overflow-checked signed integer parsing (`i128`).
fn parse_i128(raw: &[u8]) -> Option<i128> {
    debug_assert_eq!(raw[0], b'-');
    let mut magnitude = 0_u128;
    for &byte in &raw[1..] {
        let digit = (byte - b'0') as u128;
        magnitude = magnitude.checked_mul(10)?.checked_add(digit)?;
    }
    let min_magnitude = (i128::MAX as u128) + 1;
    if magnitude == min_magnitude {
        Some(i128::MIN)
    } else if magnitude <= i128::MAX as u128 {
        Some(-(magnitude as i128))
    } else {
        None
    }
}

/// Hand-rolled overflow-checked unsigned integer parsing (`u128`).
fn parse_u128(raw: &[u8]) -> Option<u128> {
    let mut value = 0_u128;
    for &byte in raw {
        let digit = (byte - b'0') as u128;
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Number::I64(v) => write!(f, "{v}"),
            Number::U64(v) => write!(f, "{v}"),
            Number::I128(v) => write!(f, "{v}"),
            Number::U128(v) => write!(f, "{v}"),
            Number::F64(v) => {
                if v == 0.0 && v.is_sign_negative() {
                    write!(f, "-0.0")
                } else {
                    write!(f, "{v}")
                }
            }
        }
    }
}

impl fmt::Debug for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl From<i8> for Number {
    fn from(v: i8) -> Self {
        if v < 0 {
            Number::I64(v as i64)
        } else {
            Number::U64(v as u64)
        }
    }
}
impl From<i16> for Number {
    fn from(v: i16) -> Self {
        if v < 0 {
            Number::I64(v as i64)
        } else {
            Number::U64(v as u64)
        }
    }
}
impl From<i32> for Number {
    fn from(v: i32) -> Self {
        if v < 0 {
            Number::I64(v as i64)
        } else {
            Number::U64(v as u64)
        }
    }
}
impl From<i64> for Number {
    fn from(v: i64) -> Self {
        if v < 0 {
            Number::I64(v)
        } else {
            Number::U64(v as u64)
        }
    }
}
impl From<i128> for Number {
    fn from(v: i128) -> Self {
        if v < i64::MIN as i128 {
            Number::I128(v)
        } else if v < 0 {
            Number::I64(v as i64)
        } else if v <= u64::MAX as i128 {
            Number::U64(v as u64)
        } else {
            Number::U128(v as u128)
        }
    }
}
impl From<isize> for Number {
    fn from(v: isize) -> Self {
        if v < 0 {
            Number::I64(v as i64)
        } else {
            Number::U64(v as u64)
        }
    }
}
impl From<u8> for Number {
    fn from(v: u8) -> Self {
        Number::U64(v as u64)
    }
}
impl From<u16> for Number {
    fn from(v: u16) -> Self {
        Number::U64(v as u64)
    }
}
impl From<u32> for Number {
    fn from(v: u32) -> Self {
        Number::U64(v as u64)
    }
}
impl From<u64> for Number {
    fn from(v: u64) -> Self {
        Number::U64(v)
    }
}
impl From<u128> for Number {
    fn from(v: u128) -> Self {
        if v <= u64::MAX as u128 {
            Number::U64(v as u64)
        } else {
            Number::U128(v)
        }
    }
}
impl From<usize> for Number {
    fn from(v: usize) -> Self {
        Number::U64(v as u64)
    }
}
impl From<f32> for Number {
    fn from(v: f32) -> Self {
        Number::F64(v as f64)
    }
}
impl From<f64> for Number {
    fn from(v: f64) -> Self {
        Number::F64(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_integers() {
        assert_eq!(Number::parse(b"0", false).unwrap(), Number::U64(0));
        assert_eq!(Number::parse(b"42", false).unwrap(), Number::U64(42));
        assert_eq!(Number::parse(b"-1", false).unwrap(), Number::I64(-1));
        assert_eq!(
            Number::parse(b"9223372036854775807", false).unwrap(),
            Number::U64(9223372036854775807)
        );
        assert_eq!(
            Number::parse(b"-9223372036854775808", false).unwrap(),
            Number::I64(i64::MIN)
        );
        assert_eq!(
            Number::parse(b"18446744073709551615", false).unwrap(),
            Number::U64(u64::MAX)
        );
        assert_eq!(
            Number::parse(b"18446744073709551616", false).unwrap(),
            Number::U128(18_446_744_073_709_551_616)
        );
        assert_eq!(
            Number::parse(b"-170141183460469231731687303715884105728", false).unwrap(),
            Number::I128(i128::MIN)
        );
        assert_eq!(
            Number::parse(b"340282366920938463463374607431768211455", false).unwrap(),
            Number::U128(u128::MAX)
        );
        assert!(Number::parse(b"340282366920938463463374607431768211456", false).is_err());
    }

    #[test]
    fn parses_floats() {
        assert_eq!(Number::parse(b"1.5", true).unwrap(), Number::F64(1.5));
        assert_eq!(Number::parse(b"-0.0", true).unwrap(), Number::F64(-0.0));
        assert_eq!(Number::parse(b"1e3", true).unwrap(), Number::F64(1000.0));
        assert_eq!(Number::parse(b"1.5E-2", true).unwrap(), Number::F64(0.015));
        assert!(Number::parse(b"1e400", true).is_err());
    }

    #[test]
    fn non_negative_unified_as_u64() {
        assert!(Number::from(1i64).is_u64());
        assert!(Number::from(-1i64).is_i64());
    }
}

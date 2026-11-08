//! Directed fixed-point intervals for finite IEEE-754 values.
//!
//! Multipliers approximate x * 10^places with an explicit error bound.
//! Ambiguous boundaries fall back to core formatting; they are never guessed.
#[path = "powers.rs"]
mod powers;

pub(super) struct Decimal {
    pub mantissa: u64,
    pub exponent: i32,
}

struct Power {
    low: u64,
    high: u16,
    places: i16,
    exact: bool,
}

const SHIFT: u32 = 70;
const MASK: u128 = (1u128 << SHIFT) - 1;
const HALF: u128 = 1u128 << (SHIFT - 1);

#[inline(always)]
pub(super) fn convert<const BITS: u32>(fraction: u64, raw_exponent: u32) -> Option<Decimal> {
    if raw_exponent == 0 {
        return None;
    }
    let power = if BITS == 52 {
        &powers::BINARY64[raw_exponent as usize - 1]
    } else {
        &powers::BINARY32[raw_exponent as usize - 1]
    };
    let factor = u128::from(power.low) | (u128::from(power.high) << 64);
    let significand = (1u64 << BITS) | fraction;
    let center_units = significand * 4;
    let lower_units = center_units
        - if fraction == 0 && raw_exponent > 1 {
            1
        } else {
            2
        };
    let upper_units = center_units + 2;
    let center = u128::from(center_units) * factor;
    let lower = center - u128::from(center_units - lower_units) * factor;
    let upper = center + factor * 2;
    let odd = significand & 1 != 0;
    let (lo, hi, error) = if power.exact {
        (
            (lower >> SHIFT) as u64 + u64::from(lower & MASK != 0 || odd),
            ((upper - u128::from(odd)) >> SHIFT) as u64,
            0,
        )
    } else {
        let lower_top = lower + u128::from(lower_units) - 1;
        let upper_top = upper + u128::from(upper_units) - 1;
        if lower >> SHIFT != lower_top >> SHIFT || upper >> SHIFT != upper_top >> SHIFT {
            return None;
        }
        (
            (lower >> SHIFT) as u64 + 1,
            (upper >> SHIFT) as u64,
            u128::from(center_units),
        )
    };
    debug_assert!(lo <= hi);
    let unique = if BITS == 52 { 100 } else { 1000 };
    debug_assert!(hi - lo < unique);
    let candidate = hi / unique;
    if candidate * unique >= lo {
        return Some(Decimal {
            mantissa: candidate,
            exponent: (if BITS == 52 { 2 } else { 3 }) - i32::from(power.places),
        });
    }
    let fractional = center & MASK;
    if fractional + error > MASK {
        return None;
    }
    let integer = (center >> SHIFT) as u64;
    let has_fraction = fractional != 0 || error != 0;
    if BITS == 23 {
        if let Some(mantissa) = nearest::<100>(lo, hi, integer, has_fraction) {
            return Some(Decimal {
                mantissa,
                exponent: 2 - i32::from(power.places),
            });
        }
    }
    if let Some(mantissa) = nearest::<10>(lo, hi, integer, has_fraction) {
        return Some(Decimal {
            mantissa,
            exponent: 1 - i32::from(power.places),
        });
    }
    let round_up = fractional > HALF || (fractional == HALF && (error != 0 || integer & 1 != 0));
    let top = fractional + error;
    let round_top = top > HALF || (top == HALF && error == 0 && integer & 1 != 0);
    if round_up != round_top {
        return None;
    }
    Some(Decimal {
        mantissa: (integer + u64::from(round_up)).clamp(lo, hi),
        exponent: -i32::from(power.places),
    })
}

#[inline(always)]
fn nearest<const DIVISOR: u64>(lo: u64, hi: u64, integer: u64, has_fraction: bool) -> Option<u64> {
    let lo = lo.div_ceil(DIVISOR);
    let hi = hi / DIVISOR;
    if lo > hi {
        return None;
    }
    let candidate = integer / DIVISOR;
    let remainder = integer % DIVISOR;
    let up = remainder > DIVISOR / 2
        || (remainder == DIVISOR / 2 && (has_fraction || candidate & 1 != 0));
    Some((candidate + u64::from(up)).clamp(lo, hi))
}

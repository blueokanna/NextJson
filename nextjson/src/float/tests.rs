use super::Buffer;
use alloc::format;

fn significant_digits(text: &str) -> usize {
    let mantissa = text.split('e').next().unwrap();
    let digits: alloc::vec::Vec<_> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    let start = digits.iter().position(|&c| c != b'0').unwrap_or(0);
    let end = digits.iter().rposition(|&c| c != b'0').map_or(1, |i| i + 1);
    end - start
}

fn check64(bits: u64) {
    let value = f64::from_bits(bits);
    if !value.is_finite() {
        assert!(Buffer::format(value).is_err());
        return;
    }
    let buffer = Buffer::format(value).unwrap();
    let text = buffer.as_str();
    assert_eq!(text.parse::<f64>().unwrap().to_bits(), bits, "{text}");
    assert_eq!(
        significant_digits(text),
        significant_digits(&format!("{value:e}")),
        "not shortest: {text} for {bits:016x}"
    );
    assert!(text.contains(['.', 'e']));
}

fn check32(bits: u32) {
    let value = f32::from_bits(bits);
    if !value.is_finite() {
        assert!(Buffer::format(value).is_err());
        return;
    }
    let buffer = Buffer::format(value).unwrap();
    let text = buffer.as_str();
    assert_eq!(text.parse::<f32>().unwrap().to_bits(), bits, "{text}");
    assert_eq!(
        significant_digits(text),
        significant_digits(&format!("{value:e}")),
        "not shortest: {text} for {bits:08x}"
    );
    assert!(text.contains(['.', 'e']));
}

#[test]
fn all_exponents_and_rounding_boundaries() {
    for exponent in 0..=0x7ffu64 {
        for fraction in [
            0,
            1,
            2,
            3,
            (1 << 51) - 1,
            1 << 51,
            (1 << 52) - 2,
            (1 << 52) - 1,
        ] {
            for sign in [0, 1 << 63] {
                check64(sign | (exponent << 52) | fraction);
            }
        }
    }
    for exponent in 0..=0xffu32 {
        for fraction in [
            0,
            1,
            2,
            3,
            (1 << 22) - 1,
            1 << 22,
            (1 << 23) - 2,
            (1 << 23) - 1,
        ] {
            for sign in [0, 1 << 31] {
                check32(sign | (exponent << 23) | fraction);
            }
        }
    }
}

#[test]
fn decimal_boundaries_and_ties() {
    for exponent in -323..=308 {
        let value = format!("1e{exponent}").parse::<f64>().unwrap();
        for bits in value.to_bits() - 1..=value.to_bits() + 1 {
            check64(bits);
            check64(bits | (1 << 63));
        }
    }
    for value in [
        0.1,
        1.2345678,
        0.000244140625,
        0.0000001,
        8388608.0,
        33554450.0,
    ] {
        for bits in (value as f32).to_bits() - 1..=(value as f32).to_bits() + 1 {
            check32(bits);
            check32(bits | (1 << 31));
        }
    }
}

#[test]
fn deterministic_bit_patterns_round_trip_with_shortest_significands() {
    let mut state = 0x243f_6a88_85a3_08d3u64;
    for _ in 0..100_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        check64(state);
        check32(state as u32);
    }
}

#[test]
fn notation_and_source_precision_are_explicit() {
    for (value, text) in [
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (1.0, "1.0"),
        (1e-5, "0.00001"),
        (1e-6, "1e-6"),
        (1e16, "1e16"),
        (f64::from_bits(1), "5e-324"),
        (f64::MAX, "1.7976931348623157e308"),
    ] {
        assert_eq!(Buffer::format(value).unwrap().as_str(), text);
    }
    assert_eq!(Buffer::format(1.2f32).unwrap().as_str(), "1.2");
    assert_eq!(
        Buffer::format(1.2f32 as f64).unwrap().as_str(),
        "1.2000000476837158"
    );
}

#[test]
fn decimal_grid_neighbors_and_binary_halfway_values() {
    for exponent in -20..=20 {
        for coefficient in [1, 3, 7, 9, 11, 99, 1234567, 9999999] {
            let value = format!("{coefficient}e{exponent}").parse::<f64>().unwrap();
            for bits in value.to_bits() - 2..=value.to_bits() + 2 {
                check64(bits);
                check64(bits | (1 << 63));
            }
            let value = value as f32;
            for bits in value.to_bits() - 2..=value.to_bits() + 2 {
                check32(bits);
                check32(bits | (1 << 31));
            }
        }
    }
    for exponent in 0..52 {
        let value = (1u64 << exponent) as f64 + 0.5;
        for bits in value.to_bits() - 2..=value.to_bits() + 2 {
            check64(bits);
        }
    }
}

#[test]
fn ordinary_exponents_cover_both_source_widths() {
    let mut state = 0x7e57_93d8_528c_62a9u64;
    for exponent in 1004..=1074u64 {
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let bits = (state & ((1 << 52) - 1)) | (exponent << 52) | (state & (1 << 63));
            check64(bits);
            let raw32 = (exponent - 896) as u32;
            check32((state as u32 & ((1 << 23) - 1)) | (raw32 << 23) | (state as u32 & (1 << 31)));
        }
    }
}

#[test]
fn exact_fraction_shortcut_matches_full_conversion_at_boundaries() {
    use alloc::vec::Vec;
    let mut bytes = Vec::new();
    for base in [
        0.0f64,
        0.125,
        1.0,
        16_383.875,
        16_384.0,
        999_999_999_999.875,
        1e12,
    ] {
        for bits in base.to_bits().saturating_sub(2)..=base.to_bits() + 2 {
            for sign in [0, 1 << 63] {
                let value = f64::from_bits(bits | sign);
                bytes.clear();
                super::write_float_into(&mut bytes, value).unwrap();
                assert_eq!(bytes, Buffer::format(value).unwrap().as_bytes());
                let value = value as f32;
                bytes.clear();
                super::write_float_into(&mut bytes, value).unwrap();
                assert_eq!(bytes, Buffer::format(value).unwrap().as_bytes());
            }
        }
    }
}

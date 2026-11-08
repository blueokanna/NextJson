//! Differential comparison with Serde/Zmij and an independent Ryū oracle.
//! Workspace unit tests additionally compare with core's flt2dec.
use nextjson::formats::{Format, Ndjson};

fn decimal(text: &str) -> (bool, u64, i32) {
    let negative = text.starts_with('-');
    let unsigned = text.trim_start_matches('-');
    let (mantissa, exp) = unsigned.split_once('e').unwrap_or((unsigned, "0"));
    let mut exponent: i32 = exp.parse().unwrap();
    if let Some((_, fraction)) = mantissa.split_once('.') {
        exponent -= fraction.len() as i32;
    }
    let mut digits: u64 = mantissa.replace('.', "").parse().unwrap();
    if digits == 0 {
        return (negative, 0, 0);
    }
    while digits.is_multiple_of(10) {
        digits /= 10;
        exponent += 1;
    }
    (negative, digits, exponent)
}

#[test]
fn million_patterns_match_independent_f64_and_f32_formatters() {
    let mut state = 0x1319_8a2e_0370_7344u64;
    let mut oracle = ryu::Buffer::new();
    for _ in 0..1_000_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = f64::from_bits(state);
        if value.is_finite() {
            let actual = nextjson::to_string(&value).unwrap();
            let reference = serde_json::to_string(&value).unwrap();
            assert_eq!(decimal(&actual), decimal(&reference), "f64 {state:016x}");
            assert_eq!(
                decimal(&actual),
                decimal(oracle.format_finite(value)),
                "ryu f64 {state:016x}"
            );
            assert_eq!(actual.parse::<f64>().unwrap().to_bits(), state);
        }
        let value = f32::from_bits(state as u32);
        if value.is_finite() {
            let actual = nextjson::to_string(&value).unwrap();
            let reference = serde_json::to_string(&value).unwrap();
            assert_eq!(
                decimal(&actual),
                decimal(&reference),
                "f32 {:08x}",
                state as u32
            );
            assert_eq!(
                decimal(&actual),
                decimal(oracle.format_finite(value)),
                "ryu f32 {:08x}",
                state as u32
            );
            assert_eq!(actual.parse::<f32>().unwrap().to_bits(), state as u32);
        }
    }
}

#[test]
fn ordinary_range_matches_both_reference_formatters() {
    let mut state = 0xa438_83df_21be_6719u64;
    let mut oracle = ryu::Buffer::new();
    for i in 0..400_000u64 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let bits = (state & ((1 << 52) - 1)) | ((1004 + i % 71) << 52) | (state & (1 << 63));
        let value = f64::from_bits(bits);
        let actual = nextjson::to_string(&value).unwrap();
        assert_eq!(
            decimal(&actual),
            decimal(oracle.format_finite(value)),
            "{bits:016x}"
        );
        assert_eq!(
            decimal(&actual),
            decimal(&serde_json::to_string(&value).unwrap())
        );
        assert_eq!(actual.parse::<f64>().unwrap().to_bits(), bits);
        let bits = (state as u32 & ((1 << 23) - 1))
            | (((108 + i % 71) as u32) << 23)
            | (state as u32 & (1 << 31));
        let value = f32::from_bits(bits);
        let actual = nextjson::to_string(&value).unwrap();
        assert_eq!(
            decimal(&actual),
            decimal(oracle.format_finite(value)),
            "{bits:08x}"
        );
        assert_eq!(
            decimal(&actual),
            decimal(&serde_json::to_string(&value).unwrap())
        );
        assert_eq!(actual.parse::<f32>().unwrap().to_bits(), bits);
    }
}

#[test]
fn concurrent_encoders_have_independent_state() {
    std::thread::scope(|scope| {
        for thread in 0..8 {
            scope.spawn(move || {
                let values: Vec<_> = (0..2048)
                    .map(|i| (i + thread * 2048) as f64 / 37.0)
                    .collect();
                for _ in 0..32 {
                    let wire = Ndjson.encode(&values).unwrap();
                    for (line, value) in wire
                        .split(|&c| c == b'\n')
                        .filter(|line| !line.is_empty())
                        .zip(&values)
                    {
                        let actual: f64 = std::str::from_utf8(line).unwrap().parse().unwrap();
                        assert_eq!(actual.to_bits(), value.to_bits());
                    }
                    assert_eq!(wire.iter().filter(|&&c| c == b'\n').count(), values.len());
                }
            });
        }
    });
}

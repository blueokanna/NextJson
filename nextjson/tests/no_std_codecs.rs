//! Correctness boundaries for the optimized no_std codecs.
#![no_std]
extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use nextjson::formats::{Format, Json, MsgPack, Ndjson, Ron};
use nextjson::{FormatEncoder, NsonSchema, NsonSerialize, TypeSchema};

#[test]
fn finite_float_extremes_round_trip_without_fixed_buffer_failure() {
    for value in [
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        -0.0,
        1e100,
        1e-100,
    ] {
        let json = Json.encode(&value).unwrap();
        assert_eq!(
            Json.decode::<f64>(&json).unwrap().to_bits(),
            value.to_bits()
        );
        let ron = Ron.encode(&value).unwrap();
        assert_eq!(Ron.decode::<f64>(&ron).unwrap().to_bits(), value.to_bits());
        let lines = Ndjson.encode(&vec![value]).unwrap();
        assert_eq!(
            Ndjson.decode::<Vec<f64>>(&lines).unwrap()[0].to_bits(),
            value.to_bits()
        );
    }
}

#[test]
fn float_fast_path_and_fallback_preserve_bits() {
    for n in -2048..2048 {
        let value = n as f64 / 8.0;
        for bits in [
            value.to_bits(),
            value.to_bits().wrapping_add(1),
            value.to_bits().wrapping_sub(1),
        ] {
            let value = f64::from_bits(bits);
            if !value.is_finite() {
                continue;
            }
            let bytes = nextjson::to_vec(&value).unwrap();
            assert_eq!(nextjson::from_slice::<f64>(&bytes).unwrap().to_bits(), bits);
        }
    }
    let mut bits = 0x1234_5678_9abc_def0u64;
    for _ in 0..10_000 {
        bits ^= bits << 13;
        bits ^= bits >> 7;
        bits ^= bits << 17;
        let value = f64::from_bits(bits);
        if value.is_finite() {
            assert_eq!(
                nextjson::from_slice::<f64>(&nextjson::to_vec(&value).unwrap())
                    .unwrap()
                    .to_bits(),
                bits
            );
        }
        let value = f32::from_bits(bits as u32);
        if value.is_finite() {
            assert_eq!(
                nextjson::from_slice::<f32>(&nextjson::to_vec(&value).unwrap())
                    .unwrap()
                    .to_bits(),
                value.to_bits()
            );
        }
    }
}

#[test]
fn ron_borrows_clean_strings_and_validates_escapes() {
    for invalid in [
        b"''".as_slice(),
        b"'ab'",
        b"'\\x80'",
        b"'\\u{d800}'",
        b"'\xff'",
        b"'\xf0\x9f'",
    ] {
        assert!(Ron.decode::<char>(invalid).is_err());
    }
    let input = br#"["hello", "world"]"#;
    let values: Vec<&str> = Ron.decode(input).unwrap();
    assert_eq!(values, vec!["hello", "world"]);
    assert_eq!(values[0].as_ptr(), input[2..].as_ptr());
    let control = "\0\u{8}\u{c}\n\t\r\"\\中文🙂";
    assert_eq!(
        Ron.decode::<String>(&Ron.encode(control).unwrap()).unwrap(),
        control
    );
    assert_eq!(
        Ron.decode::<String>(br#""\u{1f642}\x41\0""#).unwrap(),
        "🙂A\0"
    );
    for invalid in [
        br#""\q""#.as_slice(),
        br#""\u{}""#,
        br#""\u{d800}""#,
        br#""\x80""#,
        br#""\u{110000}""#,
    ] {
        assert!(Ron.decode::<String>(invalid).is_err());
    }
}

#[test]
fn ron_requires_separators_and_balanced_some_wrappers() {
    for input in [
        b"[1 2]".as_slice(),
        b"{\"a\" 1}",
        b"{\"a\":1 \"b\":2}",
        b"Some(1",
        b"Some([1]",
    ] {
        assert!(Ron.decode::<nextjson::Value>(input).is_err());
    }
    let value: Vec<Vec<u64>> = Ron.decode(b"Some([[1],[2]])").unwrap();
    assert_eq!(value, vec![vec![1], vec![2]]);
    let value: Vec<u64> = Ron
        .decode(b"/* outer /* inner */ */ Some(Some([1,2,])) // tail")
        .unwrap();
    assert_eq!(value, vec![1, 2]);
    assert!(Ron.decode::<Vec<u64>>(b"[1] /* unclosed").is_err());
}

#[test]
fn ron_preserves_wide_integers_and_rejects_integer_overflow() {
    for value in [0, u64::MAX as u128 + 1, u128::MAX] {
        assert_eq!(
            Ron.decode::<u128>(&Ron.encode(&value).unwrap()).unwrap(),
            value
        );
    }
    assert_eq!(
        Ron.decode::<i128>(&Ron.encode(&i128::MIN).unwrap())
            .unwrap(),
        i128::MIN
    );
    assert!(Ron
        .decode::<u128>(b"340282366920938463463374607431768211456")
        .is_err());
}

#[test]
fn ron_nested_options_keep_none_and_some_unit_distinct() {
    for (value, wire) in [
        (None, "None"),
        (Some(None), "Some(None)"),
        (Some(Some(())), "Some(Some(()))"),
    ] {
        let bytes = Ron.encode(&value).unwrap();
        assert_eq!(bytes, wire.as_bytes());
        assert_eq!(Ron.decode::<Option<Option<()>>>(&bytes).unwrap(), value);
    }
    let values = vec![Some(vec![Some(1u64), None]), None, Some(vec![])];
    let bytes = Ron.encode(&values).unwrap();
    assert_eq!(
        Ron.decode::<Vec<Option<Vec<Option<u64>>>>>(&bytes).unwrap(),
        values
    );
    for bytes in [
        b"Some(None".as_slice(),
        b"Some(()) trailing",
        b"Some(Some(())",
    ] {
        assert!(Ron.decode::<Option<Option<()>>>(bytes).is_err());
    }
}

#[test]
fn ndjson_direct_framing_preserves_empty_and_nested_values() {
    assert_eq!(Ndjson.encode(&Vec::<u64>::new()).unwrap(), b"");
    assert_eq!(
        Ndjson.encode(&vec![vec![], vec![1u64, 2], vec![]]).unwrap(),
        b"[]\n[1,2]\n[]\n"
    );
    assert_eq!(Ndjson.encode(&"line\ntext").unwrap(), b"\"line\\ntext\"\n");
    assert_eq!(Ndjson.encode(&false).unwrap(), b"false\n");
}

struct ClaimedLength(usize, usize);
impl NsonSchema for ClaimedLength {
    const SCHEMA: TypeSchema = TypeSchema::Seq(&TypeSchema::U64);
}
impl NsonSerialize for ClaimedLength {
    fn nextencode<E: FormatEncoder>(&self, encoder: &mut E) -> nextjson::Result<(), E::Error> {
        encoder.begin_array_len(self.0)?;
        for _ in 0..self.1 {
            encoder.separator()?;
            encoder.write_u64(1)?;
        }
        encoder.end_array()
    }
}

#[test]
fn messagepack_length_hints_are_checked_not_trusted() {
    for (hint, count) in [(0, 1), (1, 0), (2, 3), (3, 2), (usize::MAX, 0)] {
        assert!(MsgPack.encode(&ClaimedLength(hint, count)).is_err());
    }
    for len in [0, 15, 16, 65535, 65536] {
        let values = vec![0u8; len];
        let bytes = MsgPack.encode(&values).unwrap();
        let prefix = if len <= 15 {
            0x90 | len as u8
        } else if len <= 65535 {
            0xdc
        } else {
            0xdd
        };
        assert_eq!(bytes[0], prefix);
        assert_eq!(MsgPack.decode::<Vec<u8>>(&bytes).unwrap(), values);
    }
}

#[test]
fn json_integer_fast_path_keeps_the_wide_boundary() {
    for value in [
        9_999_999_999_999_999_999u128,
        10_000_000_000_000_000_000,
        u64::MAX as u128,
        u64::MAX as u128 + 1,
        u128::MAX,
    ] {
        assert_eq!(
            nextjson::from_str::<u128>(&value.to_string()).unwrap(),
            value
        );
    }
    for value in [i64::MIN as i128, i64::MIN as i128 - 1, i128::MIN] {
        assert_eq!(
            nextjson::from_str::<i128>(&value.to_string()).unwrap(),
            value
        );
    }
}

struct Scalar(u64);
impl NsonSchema for Scalar {
    const SCHEMA: TypeSchema = TypeSchema::U64;
}
impl NsonSerialize for Scalar {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> nextjson::Result<(), E::Error> {
        e.write_u64(self.0)
    }
}

#[test]
fn batched_arrays_match_scalar_events_in_all_framing_modes() {
    let numbers = vec![0u64, 1, 127, 128, 65535, 65536, u64::MAX];
    let scalar: Vec<_> = numbers.iter().copied().map(Scalar).collect();
    assert_eq!(
        Json.encode(&numbers).unwrap(),
        Json.encode(&scalar).unwrap()
    );
    assert_eq!(
        MsgPack.encode(&numbers).unwrap(),
        MsgPack.encode(&scalar).unwrap()
    );
    assert_eq!(Ron.encode(&numbers).unwrap(), Ron.encode(&scalar).unwrap());
    assert_eq!(
        Ndjson.encode(&numbers).unwrap(),
        Ndjson.encode(&scalar).unwrap()
    );
    assert_eq!(
        nextjson::to_vec_pretty(&numbers).unwrap(),
        nextjson::to_vec_pretty(&scalar).unwrap()
    );
    assert_eq!(
        Ndjson.encode(&vec![numbers]).unwrap(),
        Ndjson.encode(&vec![scalar]).unwrap()
    );
    for values in [
        vec![f64::NAN],
        vec![1.0, f64::INFINITY],
        vec![f64::NEG_INFINITY],
    ] {
        assert!(Json.encode(&values).is_err());
        assert!(Ndjson.encode(&values).is_err());
        assert!(Ron.encode(&values).is_err());
    }
}

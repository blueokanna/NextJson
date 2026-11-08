//! Exercise shared decimal conversion through framing and writer boundaries.
#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use nextjson::formats::{Format, Json, Ndjson, Ron};

#[test]
fn float_arrays_keep_bits_in_json_ndjson_and_ron() {
    let values = [
        0.0,
        -0.0,
        0.1,
        -12.34567,
        core::f64::consts::PI,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        1e-6,
        1e16,
    ];
    let expected: Vec<_> = values.iter().map(|v| v.to_bits()).collect();
    for (wire, decoded) in [
        {
            let wire = Json.encode(&values).unwrap();
            let decoded = Json.decode::<Vec<f64>>(&wire).unwrap();
            (wire, decoded)
        },
        {
            let wire = Ndjson.encode(&values).unwrap();
            let decoded = Ndjson.decode::<Vec<f64>>(&wire).unwrap();
            (wire, decoded)
        },
        {
            let wire = Ron.encode(&values).unwrap();
            let decoded = Ron.decode::<Vec<f64>>(&wire).unwrap();
            (wire, decoded)
        },
    ] {
        assert_eq!(
            decoded.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            expected,
            "{wire:?}"
        );
    }
    let lines = Ndjson.encode(&values).unwrap();
    assert_eq!(lines.iter().filter(|&&c| c == b'\n').count(), values.len());
}

#[test]
fn streaming_and_pretty_output_use_the_same_numbers() {
    struct ShortWrites(Vec<u8>, usize);
    impl nextjson::Write for ShortWrites {
        fn write_all(&mut self, bytes: &[u8]) -> nextjson::Result<()> {
            self.1 += 1;
            self.0.extend_from_slice(bytes);
            Ok(())
        }
        fn flush(&mut self) -> nextjson::Result<()> {
            Ok(())
        }
    }
    let values: Vec<_> = (0..4096).map(|i| i as f64 / 13.0).collect();
    let mut writer = ShortWrites(Vec::new(), 0);
    nextjson::to_writer(&mut writer, &values).unwrap();
    assert_eq!(writer.0, nextjson::to_vec(&values).unwrap());
    assert!(writer.1 > 1, "exercise multiple encoder flushes");
    let pretty = nextjson::to_string_pretty(&values).unwrap();
    assert_eq!(nextjson::from_str::<Vec<f64>>(&pretty).unwrap(), values);
}

#[test]
fn non_finite_values_are_rejected_on_all_entry_paths() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Json.encode(&value).is_err());
        assert!(Json.encode(&[0.1, value]).is_err());
        assert!(Ndjson.encode(&[0.1, value]).is_err());
        assert!(Ron.encode(&value).is_err());
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(Json.encode(&value).is_err());
        assert!(Ndjson.encode(&[0.1, value]).is_err());
        assert!(Ron.encode(&value).is_err());
    }
}

#[test]
fn binary32_batch_keeps_source_precision_and_nested_framing() {
    let values = [0.0f32, -0.0, 1.2, f32::from_bits(1), f32::MAX];
    let expected: Vec<_> = values.iter().map(|v| v.to_bits()).collect();
    let json = Json.encode(&values).unwrap();
    let decoded: Vec<f32> = Json.decode(&json).unwrap();
    assert_eq!(
        decoded.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        expected
    );
    let lines = Ndjson.encode(&[values, values]).unwrap();
    assert_eq!(lines.iter().filter(|&&c| c == b'\n').count(), 2);
    for row in Ndjson.decode::<Vec<Vec<f32>>>(&lines).unwrap() {
        assert_eq!(
            row.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn batch_boundaries_match_scalar_and_streaming_encoders() {
    for count in [0, 1, 31, 32, 33, 63, 64, 65, 127, 128, 129, 257] {
        let values: Vec<_> = (0..count)
            .map(|i| match i % 7 {
                0 => -0.0,
                1 => f64::MAX,
                2 => f64::from_bits(1),
                3 => i as f64 * 0.125,
                _ => i as f64 / 97.0,
            })
            .collect();
        let memory = Json.encode(&values).unwrap();
        let mut streamed = Vec::new();
        nextjson::to_writer(&mut streamed, &values).unwrap();
        assert_eq!(memory, streamed);
        let lines = Ndjson.encode(&values).unwrap();
        let mut expected = Vec::new();
        for value in &values {
            expected.extend_from_slice(&Json.encode(value).unwrap());
            expected.push(b'\n');
        }
        assert_eq!(lines, expected);
        let nested = Ndjson.encode(&[&values, &values]).unwrap();
        let mut expected = memory.clone();
        expected.push(b'\n');
        expected.extend_from_slice(&memory);
        expected.push(b'\n');
        assert_eq!(nested, expected);
        let values: Vec<f32> = values
            .iter()
            .map(|v| *v as f32)
            .filter(|v| v.is_finite())
            .collect();
        let mut streamed = Vec::new();
        nextjson::to_writer(&mut streamed, &values).unwrap();
        assert_eq!(Json.encode(&values).unwrap(), streamed);
    }
    for index in [0, 31, 32, 63, 64] {
        let mut values = alloc::vec![0.1; 65];
        values[index] = f64::INFINITY;
        assert!(Json.encode(&values).is_err());
        assert!(Ndjson.encode(&values).is_err());
    }
}

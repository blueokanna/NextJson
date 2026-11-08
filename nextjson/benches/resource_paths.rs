//! Focused measurements for buffered strings and text-cell decoding.
//! Each sample includes allocation and destruction of the decoded value.

use nextjson::formats::{Csv, Format, Ndjson};
use std::hint::black_box;
use std::time::{Duration, Instant};

fn measure(name: &str, bytes: usize, mut decode: impl FnMut()) {
    for _ in 0..20 {
        decode();
    }
    for sample in 1..=5 {
        let start = Instant::now();
        let mut operations = 0u64;
        while start.elapsed() < Duration::from_millis(200) {
            decode();
            operations += 1;
        }
        let seconds = start.elapsed().as_secs_f64();
        let ops = operations as f64 / seconds;
        println!(
            "{name},{sample},{bytes},{ops:.3},{:.3}",
            ops * bytes as f64 / 1e6
        );
    }
}

fn main() {
    println!("case,sample,input_bytes,decode_ops,decode_MBps");
    for (name, text) in [
        ("stream_ascii", "abcdef0123456789".repeat(16_384)),
        ("stream_unicode_escaped", "中🙂abc\n\"\\".repeat(16_384)),
    ] {
        let input = nextjson::to_vec(&text).unwrap();
        assert_eq!(
            nextjson::from_reader::<_, String>(input.as_slice()).unwrap(),
            text
        );
        measure(name, input.len(), || {
            black_box(nextjson::from_reader::<_, String>(black_box(input.as_slice())).unwrap());
        });
    }
    let rows: Vec<Vec<u64>> = (0..256)
        .map(|row| (0..16).map(|col| row * 17 + col).collect())
        .collect();
    let csv = Csv.encode(&rows).unwrap();
    assert_eq!(Csv.decode::<Vec<Vec<u64>>>(&csv).unwrap(), rows);
    measure("csv_numbers", csv.len(), || {
        black_box(Csv.decode::<Vec<Vec<u64>>>(black_box(&csv)).unwrap());
    });
    let ndjson = Ndjson.encode(&rows).unwrap();
    assert_eq!(Ndjson.decode::<Vec<Vec<u64>>>(&ndjson).unwrap(), rows);
    measure("ndjson_numbers", ndjson.len(), || {
        black_box(Ndjson.decode::<Vec<Vec<u64>>>(black_box(&ndjson)).unwrap());
    });
}

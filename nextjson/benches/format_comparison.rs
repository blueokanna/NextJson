use std::hint::black_box;
use std::time::{Duration, Instant};

use nextjson::{NsonDeserialize, NsonSerialize};

#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Record {
    id: u64,
    active: bool,
    score: f64,
    name: String,
    tags: Vec<String>,
    samples: Vec<i64>,
}

fn fixture() -> Vec<Record> {
    (0..128)
        .map(|index| Record {
            id: index,
            active: index % 3 != 0,
            score: index as f64 * 1.25 - 17.5,
            name: format!("record-{index:04}"),
            tags: vec![
                "json".into(),
                "zero-copy".into(),
                format!("group-{}", index % 8),
            ],
            samples: (0..16).map(|sample| index as i64 * 31 - sample).collect(),
        })
        .collect()
}

fn measure(mut operation: impl FnMut(), duration: Duration) -> (u64, f64) {
    let start = Instant::now();
    let mut iterations = 0_u64;
    while start.elapsed() < duration {
        operation();
        iterations += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    (iterations, iterations as f64 / elapsed)
}

fn run(name: &str, duration: Duration, operation: impl FnMut()) {
    let (iterations, operations_per_second) = measure(operation, duration);
    println!("{name},{iterations},{operations_per_second:.2}");
}

fn main() {
    let duration_ms = std::env::var("NEXTJSON_BENCH_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(2_000);
    let duration = Duration::from_millis(duration_ms.max(100));
    let records = fixture();
    let json = nextjson::nextencode(&records).expect("fixture JSON encoding");
    let cbor = nextjson::cross_format::json_to_cbor(&json).expect("fixture JSON to CBOR");
    let json_from_cbor = nextjson::cross_format::cbor_to_json(&cbor).expect("fixture CBOR to JSON");
    let decoded: Vec<Record> =
        nextjson::nextdecode(&json_from_cbor).expect("fixture round-trip decoding");
    assert_eq!(decoded, records);

    for _ in 0..100 {
        black_box(nextjson::nextencode(black_box(&records)).unwrap());
        black_box(nextjson::nextdecode::<Vec<Record>>(black_box(&json)).unwrap());
        black_box(nextjson::cross_format::json_to_cbor(black_box(&json)).unwrap());
        black_box(nextjson::cross_format::cbor_to_json(black_box(&cbor)).unwrap());
    }

    println!("case,iterations,operations_per_second");
    run("nextjson_native_nextencode", duration, || {
        black_box(nextjson::nextencode(black_box(&records)).unwrap());
    });
    run("nextjson_native_nextdecode", duration, || {
        black_box(nextjson::nextdecode::<Vec<Record>>(black_box(&json)).unwrap());
    });
    run("nextjson_json_to_cbor", duration, || {
        black_box(nextjson::cross_format::json_to_cbor(black_box(&json)).unwrap());
    });
    run("nextjson_cbor_to_json", duration, || {
        black_box(nextjson::cross_format::cbor_to_json(black_box(&cbor)).unwrap());
    });
}

use std::hint::black_box;
use std::time::{Duration, Instant};

use nextjson::{NsonDeserialize, NsonSerialize};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize, Serialize, Deserialize)]
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

    let nextjson_bytes = nextjson::to_vec(&records).expect("NextJson fixture serialization");
    let serde_bytes = serde_json::to_vec(&records).expect("serde_json fixture serialization");
    let nextjson_semantics: serde_json::Value =
        serde_json::from_slice(&nextjson_bytes).expect("NextJson output is valid JSON");
    let serde_semantics: serde_json::Value =
        serde_json::from_slice(&serde_bytes).expect("serde_json output is valid JSON");
    assert_eq!(nextjson_semantics, serde_semantics);

    for _ in 0..100 {
        black_box(nextjson::to_vec(black_box(&records)).unwrap());
        black_box(nextjson::serde_compat::to_vec(black_box(&records)).unwrap());
        black_box(serde_json::to_vec(black_box(&records)).unwrap());
        black_box(nextjson::from_slice::<Vec<Record>>(black_box(&nextjson_bytes)).unwrap());
        black_box(
            nextjson::serde_compat::from_slice::<Vec<Record>>(black_box(&nextjson_bytes)).unwrap(),
        );
        black_box(serde_json::from_slice::<Vec<Record>>(black_box(&serde_bytes)).unwrap());
    }

    println!("case,iterations,operations_per_second");
    run("nextjson_native_nextencode", duration, || {
        black_box(nextjson::to_vec(black_box(&records)).unwrap());
    });
    run("nextjson_serde_nextencode", duration, || {
        black_box(nextjson::serde_compat::to_vec(black_box(&records)).unwrap());
    });
    run("serde_json_nextencode", duration, || {
        black_box(serde_json::to_vec(black_box(&records)).unwrap());
    });
    run("nextjson_native_nextdecode", duration, || {
        black_box(nextjson::from_slice::<Vec<Record>>(black_box(&nextjson_bytes)).unwrap());
    });
    run("nextjson_serde_nextdecode", duration, || {
        black_box(
            nextjson::serde_compat::from_slice::<Vec<Record>>(black_box(&nextjson_bytes)).unwrap(),
        );
    });
    run("serde_json_nextdecode", duration, || {
        black_box(serde_json::from_slice::<Vec<Record>>(black_box(&serde_bytes)).unwrap());
    });
}

//! Production-grade nextjson vs serde/serde_json comparison benchmark.
//!
//! This binary lives in a standalone crate (`benchmarks/serde-comparison/`)
//! that is intentionally outside the root workspace: benchmarking against the
//! mature serde ecosystem requires third-party crates, which the repository's
//! dependency-audit gate forbids in the workspace Cargo.lock. The standalone
//! crate keeps its own Cargo.lock, so the main library stays zero-dependency.
//!
//! The same fixture, the same warm-up, and the same measurement loop are used
//! for both libraries, on the same machine and process. Output is CSV:
//!
//! ```text
//! case,size_bytes,ops,MBps
//! ```
//!
//! Run:
//! ```text
//! cd benchmarks/serde-comparison
//! cargo run --release
//! ```
//! Tune the per-case window with `NEXTJSON_BENCH_MS` (default 2000 ms).

use std::hint::black_box;
use std::time::{Duration, Instant};

use nextjson::{NsonDeserialize, NsonSerialize};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct Record {
    id: u64,
    active: bool,
    score: f64,
    name: String,
    tags: Vec<String>,
    samples: Vec<i64>,
}

fn fixture() -> Vec<Record> {
    (0..256)
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

fn measure(duration: Duration, mut operation: impl FnMut()) -> f64 {
    let start = Instant::now();
    let mut iterations = 0_u64;
    while start.elapsed() < duration {
        operation();
        iterations += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    iterations as f64 / elapsed
}

/// Measure a named case and print one CSV row. `encode` produces the bytes,
/// `decode` consumes them; both are dyn calls so the compiler cannot elide them.
fn bench(name: &str, duration: Duration, encode: &dyn Fn() -> Vec<u8>, decode: &dyn Fn(&[u8])) {
    let bytes = encode();
    let size = bytes.len();
    for _ in 0..500 {
        black_box(encode());
        decode(black_box(&bytes));
    }
    let encode_ops = measure(duration, || {
        black_box(encode());
    });
    let decode_ops = measure(duration, || {
        decode(black_box(&bytes));
    });
    let enc_mbps = encode_ops * size as f64 / 1_000_000.0;
    let dec_mbps = decode_ops * size as f64 / 1_000_000.0;
    println!("{name},{size},{encode_ops},{enc_mbps:.2},{decode_ops},{dec_mbps:.2}");
}

fn main() {
    let duration_ms = std::env::var("NEXTJSON_BENCH_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(2_000);
    let duration = Duration::from_millis(duration_ms.max(100));

    let records = fixture();

    // Correctness self-check: both libraries round-trip the fixture exactly.
    let njson_json = nextjson::nextencode(&records).unwrap();
    assert_eq!(
        nextjson::nextdecode::<Vec<Record>>(&njson_json).unwrap(),
        records
    );
    let serde_json_bytes = serde_json::to_vec(&records).unwrap();
    let serde_back: Vec<Record> = serde_json::from_slice(&serde_json_bytes).unwrap();
    assert_eq!(serde_back, records);

    println!("case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps");
    bench(
        "nextjson_encode",
        duration,
        &|| nextjson::nextencode(&records).unwrap(),
        &|b| {
            black_box(nextjson::nextdecode::<Vec<Record>>(b).unwrap());
        },
    );
    bench(
        "serde_json_encode",
        duration,
        &|| serde_json::to_vec(&records).unwrap(),
        &|b| {
            black_box(serde_json::from_slice::<Vec<Record>>(b).unwrap());
        },
    );

    // Float-free control: quantifies how much of the encode gap is float
    // formatting (nextjson uses `core::fmt::Display`; serde_json uses ryu).
    run_float_free_comparison();
}

/// Float-free fixture: isolates the float-formatting cost. nextjson formats
/// floats through `core::fmt::Display` (flt2dec); serde_json uses the ryu
/// crate. Removing floats shows how much of the encode gap is exactly that.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct IntRecord {
    id: u64,
    active: bool,
    name: String,
    tags: Vec<String>,
    samples: Vec<i64>,
}

fn int_fixture() -> Vec<IntRecord> {
    (0..256)
        .map(|index| IntRecord {
            id: index,
            active: index % 3 != 0,
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

fn run_float_free_comparison() {
    let records = int_fixture();
    let njson_json = nextjson::nextencode(&records).unwrap();
    assert_eq!(nextjson::nextdecode::<Vec<IntRecord>>(&njson_json).unwrap(), records);
    let serde_json_bytes = serde_json::to_vec(&records).unwrap();
    let serde_back: Vec<IntRecord> = serde_json::from_slice(&serde_json_bytes).unwrap();
    assert_eq!(serde_back, records);

    println!("case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps");
    bench(
        "nextjson_encode_intonly",
        Duration::from_millis(2_000),
        &|| nextjson::nextencode(&records).unwrap(),
        &|b| {
            black_box(nextjson::nextdecode::<Vec<IntRecord>>(b).unwrap());
        },
    );
    bench(
        "serde_json_encode_intonly",
        Duration::from_millis(2_000),
        &|| serde_json::to_vec(&records).unwrap(),
        &|b| {
            black_box(serde_json::from_slice::<Vec<IntRecord>>(b).unwrap());
        },
    );
}


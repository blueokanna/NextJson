//! Production-grade format comparison benchmark (zero dependencies).
//!
//! Compares every nextjson format that can represent the fixture on encode and
//! decode throughput plus encoded size. Output is a CSV table:
//!
//! ```text
//! format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps
//! ```
//!
//! Run with `cargo bench -p nextjson --bench format_comparison`. Tune the
//! per-case measurement window with `NEXTJSON_BENCH_MS` (default 2000 ms). A
//! warm-up pass runs before measurement so the allocator and instruction cache
//! are steady.
//!
//! The comparison is in-process and dependency-free: serde ecosystem crates
//! cannot be benchmarked in-tree without adding third-party dependencies,
//! which the repository's dependency audit forbids. See `docs/BENCHMARKS.md`
//! for the honest methodology and external-comparison notes.

use std::hint::black_box;
use std::time::{Duration, Instant};

use nextjson::formats::Format;
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

/// Float-free record: every format (including bencode) can represent it.
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct IntRecord {
    id: u64,
    count: i64,
    name: String,
    tags: Vec<String>,
}

/// Unsigned-only record: postcard rejects signed scalars as non-self-describing.
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct UintRecord {
    id: u64,
    count: u64,
    name: String,
    tags: Vec<String>,
}

/// Document root for TOML/BSON: both require a top-level table, not an array.
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Doc<T> {
    records: Vec<T>,
}

fn record_fixture() -> Vec<Record> {
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

fn int_record_fixture() -> Vec<IntRecord> {
    (0..128)
        .map(|index| IntRecord {
            id: index,
            count: index as i64 * 7 - 3,
            name: format!("item-{index:04}"),
            tags: vec!["a".into(), "b".into(), format!("g{}", index % 5)],
        })
        .collect()
}

/// Benchmark one encode/decode pair and print a CSV row.
fn benchmark_pair(
    name: &str,
    duration: Duration,
    encode: &dyn Fn() -> Vec<u8>,
    decode: &dyn Fn(&[u8]),
) {
    let bytes = encode();
    let size = bytes.len();

    // Warm-up so the allocator and caches are steady.
    for _ in 0..200 {
        black_box(encode());
        decode(black_box(&bytes));
    }

    let (_, encode_ops) = measure(duration, || {
        black_box(encode());
    });
    // decode 是 dyn 间接调用，编译器不会消除；对参数做 black_box 防提升。
    let (_, decode_ops) = measure(duration, || {
        decode(black_box(&bytes));
    });

    let encode_mbps = encode_ops * size as f64 / 1_000_000.0;
    let decode_mbps = decode_ops * size as f64 / 1_000_000.0;
    println!("{name},{size},{encode_ops},{encode_mbps:.2},{decode_ops},{decode_mbps:.2}");
}

fn measure(duration: Duration, mut operation: impl FnMut()) -> (u64, f64) {
    let start = Instant::now();
    let mut iterations = 0_u64;
    while start.elapsed() < duration {
        operation();
        iterations += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    (iterations, iterations as f64 / elapsed)
}

fn main() {
    let duration_ms = std::env::var("NEXTJSON_BENCH_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(2_000);
    let duration = Duration::from_millis(duration_ms.max(100));

    let records = record_fixture();
    let int_records = int_record_fixture();

    // Self-check: the native path round-trips exactly.
    assert_eq!(
        nextjson::nextdecode::<Vec<Record>>(&nextjson::nextencode(&records).unwrap()).unwrap(),
        records
    );

    println!("format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps");

    // Full-model formats (float + bool + nested containers).
    macro_rules! bench_full {
        ($($mod:ident),* $(,)?) => {
            $(
                let enc = |r: &Vec<Record>| nextjson::formats::$mod.encode(r).unwrap();
                let dec = |b: &[u8]| nextjson::formats::$mod.decode::<Vec<Record>>(b).unwrap();
                benchmark_pair(
                    <nextjson::formats::$mod as Format>::NAME,
                    duration,
                    &|| enc(&records),
                    &|b| {
                        black_box(dec(b));
                    },
                );
            )*
        };
    }
    bench_full!(Json, Json5, Hjson, Yaml, Ron, Sexpr, Cbor, MsgPack, Pickle);

    // TOML / BSON: document-shaped, wrap the array in a table root.
    macro_rules! bench_doc {
        ($($mod:ident),* $(,)?) => {
            $(
                let doc = Doc { records: records.clone() };
                let enc = |d: &Doc<Record>| nextjson::formats::$mod.encode(d).unwrap();
                let dec = |b: &[u8]| nextjson::formats::$mod.decode::<Doc<Record>>(b).unwrap();
                benchmark_pair(
                    <nextjson::formats::$mod as Format>::NAME,
                    duration,
                    &|| enc(&doc),
                    &|b| {
                        black_box(dec(b));
                    },
                );
            )*
        };
    }
    bench_doc!(Toml, Bson);

    // Bencode / Postcard: no float on the wire; use the int/uint fixtures.
    let uint_records: Vec<UintRecord> = int_records
        .iter()
        .map(|r| UintRecord {
            id: r.id,
            count: r.count.max(0) as u64,
            name: r.name.clone(),
            tags: r.tags.clone(),
        })
        .collect();
    let bencode_enc = |r: &Vec<IntRecord>| nextjson::formats::Bencode.encode(r).unwrap();
    let bencode_dec = |b: &[u8]| {
        nextjson::formats::Bencode
            .decode::<Vec<IntRecord>>(b)
            .unwrap()
    };
    benchmark_pair(
        <nextjson::formats::Bencode as Format>::NAME,
        duration,
        &|| bencode_enc(&int_records),
        &|b| {
            black_box(bencode_dec(b));
        },
    );
    let postcard_enc = |r: &Vec<UintRecord>| nextjson::formats::Postcard.encode(r).unwrap();
    let postcard_dec = |b: &[u8]| {
        nextjson::formats::Postcard
            .decode::<Vec<UintRecord>>(b)
            .unwrap()
    };
    benchmark_pair(
        <nextjson::formats::Postcard as Format>::NAME,
        duration,
        &|| postcard_enc(&uint_records),
        &|b| {
            black_box(postcard_dec(b));
        },
    );

    // Row-shaped text format: flat rows only.
    #[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
    struct FlatRow {
        id: u64,
        active: bool,
        score: f64,
        name: String,
    }
    let rows: Vec<FlatRow> = (0..128)
        .map(|i| FlatRow {
            id: i,
            active: i % 2 == 0,
            score: i as f64 * 0.5,
            name: format!("row-{i:04}"),
        })
        .collect();
    let csv_enc = |r: &Vec<FlatRow>| nextjson::formats::Csv.encode(r).unwrap();
    let csv_dec = |b: &[u8]| nextjson::formats::Csv.decode::<Vec<FlatRow>>(b).unwrap();
    benchmark_pair(
        <nextjson::formats::Csv as Format>::NAME,
        duration,
        &|| csv_enc(&rows),
        &|b| {
            black_box(csv_dec(b));
        },
    );
}

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

/// A document-shaped, unsigned-only fixture.
///
/// TOML and BSON require a document (table / BSON document) root, and
/// nextjson's postcard codec rejects signed integers and floats, so these
/// formats are measured on a `Config` value instead of `Vec<Record>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct Config {
    title: String,
    owner: Owner,
    tags: Vec<String>,
    retries: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct Owner {
    name: String,
    id: u64,
}

fn config_fixture() -> Config {
    Config {
        title: "NextJson benchmark".into(),
        owner: Owner {
            name: "blueokanna".into(),
            id: 42,
        },
        tags: vec!["json".into(), "zero-copy".into(), "benchmark".into()],
        retries: 3,
    }
}

/// Round-trip a nextjson format and a serde format on the same value, then
/// measure both encode and decode throughput.
fn pair_bench(
    nextjson_name: &str,
    serde_name: &str,
    duration: Duration,
    nextjson_encode: &dyn Fn() -> Vec<u8>,
    nextjson_decode: &dyn Fn(&[u8]),
    serde_encode: &dyn Fn() -> Vec<u8>,
    serde_decode: &dyn Fn(&[u8]),
) {
    bench(nextjson_name, duration, nextjson_encode, nextjson_decode);
    bench(serde_name, duration, serde_encode, serde_decode);
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
    let config = config_fixture();

    // Correctness self-check: every format round-trips its fixture exactly.
    check_nextjson(&records, nextjson::formats::Json);
    check_serde(&records, |b| serde_json::from_slice(b), || serde_json::to_vec(&records).unwrap());
    check_nextjson(&records, nextjson::formats::Yaml);
    check_serde(&records, |b| serde_yaml::from_slice(b), || serde_yaml::to_string(&records).unwrap().into_bytes());
    check_nextjson(&records, nextjson::formats::Ron);
    check_serde(&records, |b| ron::from_str(std::str::from_utf8(b).unwrap()), || ron::to_string(&records).unwrap().into_bytes());
    check_nextjson(&records, nextjson::formats::MsgPack);
    check_serde(&records, |b| rmp_serde::from_slice(b), || rmp_serde::to_vec(&records).unwrap());
    check_nextjson(&records, nextjson::formats::Cbor);
    check_serde(&records, |b| ciborium::from_reader::<Vec<Record>, _>(&b[..]), || ciborium_serialize(&records));
    check_serde(&records, |b| bincode::deserialize(b), || bincode::serialize(&records).unwrap());
    check_nextjson(&config, nextjson::formats::Toml);
    check_serde(&config, |b| toml::from_str(std::str::from_utf8(b).unwrap()), || toml::to_string(&config).unwrap().into_bytes());
    check_nextjson(&config, nextjson::formats::Bson);
    check_serde(&config, |b| bson::from_slice(b), || bson::to_vec(&config).unwrap());
    check_nextjson(&config, nextjson::formats::Postcard);
    check_serde(&config, |b| postcard::from_bytes(b), || postcard::to_allocvec(&config).unwrap());

    println!("case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps");

    // JSON (both)
    pair_bench(
        "nextjson_json",
        "serde_json",
        duration,
        &|| nextjson::nextencode(&records).unwrap(),
        &|b| {
            black_box(nextjson::nextdecode::<Vec<Record>>(b).unwrap());
        },
        &|| serde_json::to_vec(&records).unwrap(),
        &|b| {
            black_box(serde_json::from_slice::<Vec<Record>>(b).unwrap());
        },
    );
    // YAML (both)
    pair_bench(
        "nextjson_yaml",
        "serde_yaml",
        duration,
        &|| nextjson::formats::encode_with(&records, nextjson::formats::Yaml).unwrap(),
        &|b| {
            black_box(
                nextjson::formats::decode_with::<Vec<Record>, _>(b, nextjson::formats::Yaml)
                    .unwrap(),
            );
        },
        &|| serde_yaml::to_string(&records).unwrap().into_bytes(),
        &|b| {
            black_box(serde_yaml::from_slice::<Vec<Record>>(b).unwrap());
        },
    );
    // RON (both)
    pair_bench(
        "nextjson_ron",
        "serde_ron",
        duration,
        &|| nextjson::formats::encode_with(&records, nextjson::formats::Ron).unwrap(),
        &|b| {
            black_box(
                nextjson::formats::decode_with::<Vec<Record>, _>(b, nextjson::formats::Ron)
                    .unwrap(),
            );
        },
        &|| ron::to_string(&records).unwrap().into_bytes(),
        &|b| {
            black_box(ron::from_str::<Vec<Record>>(std::str::from_utf8(b).unwrap()).unwrap());
        },
    );
    // MessagePack (both)
    pair_bench(
        "nextjson_msgpack",
        "rmp_serde",
        duration,
        &|| nextjson::formats::encode_with(&records, nextjson::formats::MsgPack).unwrap(),
        &|b| {
            black_box(
                nextjson::formats::decode_with::<Vec<Record>, _>(b, nextjson::formats::MsgPack)
                    .unwrap(),
            );
        },
        &|| rmp_serde::to_vec(&records).unwrap(),
        &|b| {
            black_box(rmp_serde::from_slice::<Vec<Record>>(b).unwrap());
        },
    );
    // CBOR (both)
    pair_bench(
        "nextjson_cbor",
        "ciborium",
        duration,
        &|| nextjson::formats::encode_with(&records, nextjson::formats::Cbor).unwrap(),
        &|b| {
            black_box(
                nextjson::formats::decode_with::<Vec<Record>, _>(b, nextjson::formats::Cbor)
                    .unwrap(),
            );
        },
        &|| ciborium_serialize(&records),
        &|b| {
            black_box(ciborium::from_reader::<Vec<Record>, _>(&b[..]).unwrap());
        },
    );
    // Bincode (serde only: nextjson has no bincode codec)
    bench(
        "nextjson_bincode(na)",
        duration,
        &|| Vec::new(),
        &|_| {},
    );
    bench(
        "serde_bincode",
        duration,
        &|| bincode::serialize(&records).unwrap(),
        &|b| {
            black_box(bincode::deserialize::<Vec<Record>>(b).unwrap());
        },
    );
    // TOML (both, document-shaped Config fixture)
    pair_bench(
        "nextjson_toml",
        "serde_toml",
        duration,
        &|| nextjson::formats::encode_with(&config, nextjson::formats::Toml).unwrap(),
        &|b| {
            black_box(
                nextjson::formats::decode_with::<Config, _>(b, nextjson::formats::Toml).unwrap(),
            );
        },
        &|| toml::to_string(&config).unwrap().into_bytes(),
        &|b| {
            black_box(toml::from_str::<Config>(std::str::from_utf8(b).unwrap()).unwrap());
        },
    );
    // BSON (both, document-shaped Config fixture)
    pair_bench(
        "nextjson_bson",
        "serde_bson",
        duration,
        &|| nextjson::formats::encode_with(&config, nextjson::formats::Bson).unwrap(),
        &|b| {
            black_box(
                nextjson::formats::decode_with::<Config, _>(b, nextjson::formats::Bson).unwrap(),
            );
        },
        &|| bson::to_vec(&config).unwrap(),
        &|b| {
            black_box(bson::from_slice::<Config>(b).unwrap());
        },
    );
    // Postcard (both, unsigned Config fixture)
    pair_bench(
        "nextjson_postcard",
        "serde_postcard",
        duration,
        &|| nextjson::formats::encode_with(&config, nextjson::formats::Postcard).unwrap(),
        &|b| {
            black_box(
                nextjson::formats::decode_with::<Config, _>(b, nextjson::formats::Postcard)
                    .unwrap(),
            );
        },
        &|| postcard::to_allocvec(&config).unwrap(),
        &|b| {
            black_box(postcard::from_bytes::<Config>(b).unwrap());
        },
    );

    // Float-free control: quantifies how much of the encode gap is float
    // formatting (nextjson uses `core::fmt::Display`; serde_json uses ryu).
    // Uses the same `NEXTJSON_BENCH_MS` window as the main table.
    run_float_free_comparison(duration);
}

/// Assert that a nextjson format round-trips `value` exactly.
fn check_nextjson<T>(value: &T, format: impl nextjson::formats::Format)
where
    T: nextjson::NsonSerialize + for<'de> nextjson::NsonDeserialize<'de> + PartialEq + std::fmt::Debug,
{
    let bytes = nextjson::formats::encode_with(value, format).unwrap();
    let back = nextjson::formats::decode_with::<T, _>(&bytes, format).unwrap();
    assert_eq!(&back, value);
}

/// Assert that a serde format round-trips `value` exactly.
fn check_serde<T, E>(
    value: &T,
    decode: impl Fn(&[u8]) -> Result<T, E>,
    encode: impl Fn() -> Vec<u8>,
) where
    T: PartialEq + std::fmt::Debug,
    E: std::fmt::Debug,
{
    let bytes = encode();
    let back = decode(&bytes).unwrap();
    assert_eq!(&back, value);
}

/// Serialize through ciborium's writer API (ciborium 0.2 has no `into_vec`).
fn ciborium_serialize<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).unwrap();
    out
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

fn run_float_free_comparison(duration: Duration) {
    let records = int_fixture();
    let njson_json = nextjson::nextencode(&records).unwrap();
    assert_eq!(nextjson::nextdecode::<Vec<IntRecord>>(&njson_json).unwrap(), records);
    let serde_json_bytes = serde_json::to_vec(&records).unwrap();
    let serde_back: Vec<IntRecord> = serde_json::from_slice(&serde_json_bytes).unwrap();
    assert_eq!(serde_back, records);

    println!("case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps");
    bench(
        "nextjson_encode_intonly",
        duration,
        &|| nextjson::nextencode(&records).unwrap(),
        &|b| {
            black_box(nextjson::nextdecode::<Vec<IntRecord>>(b).unwrap());
        },
    );
    bench(
        "serde_json_encode_intonly",
        duration,
        &|| serde_json::to_vec(&records).unwrap(),
        &|b| {
            black_box(serde_json::from_slice::<Vec<IntRecord>>(b).unwrap());
        },
    );
}


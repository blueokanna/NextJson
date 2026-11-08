//! Typed, checked measurements of the four primary no_std codecs.
//! The host timer and Serde reference implementations belong only to this benchmark.

use nextjson::formats::{Format, MsgPack, Ndjson, Ron};
use nextjson::{NsonDeserialize, NsonSerialize};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::hint::black_box;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct Record {
    id: u64,
    active: bool,
    name: String,
    scores: Vec<f64>,
    tags: Vec<String>,
}

fn measure(name: &str, operation: &str, bytes: usize, mut work: impl FnMut()) {
    let warmup = Instant::now();
    while warmup.elapsed() < Duration::from_millis(100) {
        work();
    }
    for sample in 1..=5 {
        let start = Instant::now();
        let mut count = 0u64;
        while start.elapsed() < Duration::from_millis(250) {
            work();
            count += 1;
        }
        let seconds = start.elapsed().as_secs_f64();
        println!(
            "{name},{operation},{sample},{bytes},{count},{:.9},{:.3}",
            seconds,
            seconds * 1e9 / count as f64
        );
    }
}

fn bench<T: PartialEq + Debug>(
    name: &str,
    value: &T,
    encode: impl Fn(&T) -> Vec<u8>,
    decode: impl Fn(&[u8]) -> T,
) {
    let bytes = encode(value);
    assert_eq!(decode(&bytes), *value, "round-trip: {name}");
    measure(name, "encode", bytes.len(), || {
        black_box(encode(black_box(value)));
    });
    measure(name, "decode", bytes.len(), || {
        black_box(decode(black_box(&bytes)));
    });
}

fn run<T>(fixture: &str, value: Vec<T>)
where
    T: Serialize
        + for<'de> Deserialize<'de>
        + NsonSerialize
        + for<'de> NsonDeserialize<'de>
        + PartialEq
        + Debug,
{
    bench(
        &format!("nextjson_{fixture}_json"),
        &value,
        |v| nextjson::to_vec(v).unwrap(),
        |b| nextjson::from_slice(b).unwrap(),
    );
    bench(
        &format!("serde_{fixture}_json"),
        &value,
        |v| serde_json::to_vec(v).unwrap(),
        |b| serde_json::from_slice(b).unwrap(),
    );
    bench(
        &format!("nextjson_{fixture}_msgpack"),
        &value,
        |v| MsgPack.encode(v).unwrap(),
        |b| MsgPack.decode(b).unwrap(),
    );
    bench(
        &format!("serde_{fixture}_msgpack"),
        &value,
        |v| rmp_serde::to_vec_named(v).unwrap(),
        |b| rmp_serde::from_slice(b).unwrap(),
    );
    bench(
        &format!("nextjson_{fixture}_ron"),
        &value,
        |v| Ron.encode(v).unwrap(),
        |b| Ron.decode(b).unwrap(),
    );
    bench(
        &format!("serde_{fixture}_ron"),
        &value,
        |v| ron::to_string(v).unwrap().into_bytes(),
        |b| ron::de::from_bytes(b).unwrap(),
    );
    bench(
        &format!("nextjson_{fixture}_ndjson"),
        &value,
        |v| Ndjson.encode(v).unwrap(),
        |b| Ndjson.decode(b).unwrap(),
    );
    bench(
        &format!("serde_{fixture}_ndjson"),
        &value,
        |v| {
            let mut out = Vec::new();
            for item in v {
                serde_json::to_writer(&mut out, item).unwrap();
                out.push(b'\n');
            }
            out
        },
        |b| {
            b.split(|&c| c == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice(line).unwrap())
                .collect()
        },
    );
}

fn main() {
    println!("case,operation,sample,input_bytes,operations,seconds,ns_per_op");
    run(
        "records",
        (0..256u64)
            .map(|id| Record {
                id,
                active: id % 2 == 0,
                name: format!("record-{id}-中文"),
                scores: vec![id as f64 * 0.5, -0.125, 42.25],
                tags: vec!["alpha".into(), "line\nquoted\"".into()],
            })
            .collect(),
    );
    run("integers", (0..10_000u64).collect());
    // Exercise the general decimal formatter, independently of the exact
    // binary fractions in records.scores.
    run(
        "decimals",
        (0..2048)
            .map(|index| [0.1f64, -12.34567, core::f64::consts::PI, 12345.6789][index % 4])
            .collect(),
    );
}

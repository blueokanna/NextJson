//! Allocation-inclusive float encoding throughput with paired samples.
//! --millis=250 --samples=5 --threads=1,4 [--fixture=ordinary]
//! CSV ns_per_op is aggregate wall time / completed operations, NOT request latency.

use nextjson::formats::{Format, Ndjson};
use nextjson::NsonSerialize;
use serde::Serialize;
use std::hint::black_box;
use std::sync::Barrier;
use std::time::{Duration, Instant};

fn encode<T: Serialize + NsonSerialize>(values: &[T], library: &str, format: &str) -> Vec<u8> {
    match (library, format) {
        ("nextjson", "json") => nextjson::to_vec(values).unwrap(),
        ("nextjson", "ndjson") => Ndjson.encode(values).unwrap(),
        ("serde", "json") => serde_json::to_vec(values).unwrap(),
        ("serde", "ndjson") => {
            let mut bytes = Vec::new();
            for value in values {
                serde_json::to_writer(&mut bytes, value).unwrap();
                bytes.push(b'\n');
            }
            bytes
        }
        _ => unreachable!(),
    }
}

fn measure(workers: usize, duration: Duration, work: impl Fn() + Sync) -> (u64, f64) {
    let barrier = Barrier::new(workers + 1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let work = &work;
                let barrier = &barrier;
                scope.spawn(move || {
                    let warmup = Instant::now();
                    while warmup.elapsed() < Duration::from_millis(50) {
                        work();
                    }
                    barrier.wait();
                    let start = Instant::now();
                    let mut operations = 0u64;
                    while start.elapsed() < duration {
                        work();
                        operations += 1;
                    }
                    operations
                })
            })
            .collect();
        barrier.wait();
        let start = Instant::now();
        let operations = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .sum();
        (operations, start.elapsed().as_secs_f64())
    })
}

fn check_wire<T: Copy + Into<f64>>(wire: &[u8], values: &[T], format: &str) {
    // Use core parsing so the reference parser's optional rounding behavior
    // is not part of an encoding measurement. Check signed zero by bits.
    let text = std::str::from_utf8(wire).unwrap();
    let parts: Vec<_> = if format == "json" {
        text.strip_prefix('[')
            .unwrap()
            .strip_suffix(']')
            .unwrap()
            .split(',')
            .collect()
    } else {
        text.lines().collect()
    };
    assert_eq!(parts.len(), values.len());
    for (part, &value) in parts.iter().zip(values) {
        let actual = if std::mem::size_of::<T>() == 4 {
            part.parse::<f32>().unwrap() as f64
        } else {
            part.parse::<f64>().unwrap()
        };
        assert_eq!(actual.to_bits(), value.into().to_bits());
    }
}

struct Config {
    duration: Duration,
    samples: usize,
    threads: Vec<usize>,
    fixture: Option<String>,
}

fn run<T: Serialize + NsonSerialize + Copy + Into<f64> + Sync>(
    name: &str,
    values: &[T],
    config: &Config,
) {
    if config
        .fixture
        .as_deref()
        .is_some_and(|filter| filter != name)
    {
        return;
    }
    for format in ["json", "ndjson"] {
        for library in ["nextjson", "serde"] {
            check_wire(&encode(values, library, format), values, format);
        }
        for &threads in &config.threads {
            for sample in 1..=config.samples {
                let order = if sample % 2 == 1 {
                    ["nextjson", "serde"]
                } else {
                    ["serde", "nextjson"]
                };
                for library in order {
                    let bytes = encode(values, library, format).len();
                    let (operations, seconds) = measure(threads, config.duration, || {
                        black_box(encode(black_box(values), library, format));
                    });
                    println!("{name},{library},{format},{threads},{sample},{},{bytes},{operations},{seconds:.9},{:.3}", values.len(), seconds * 1e9 / operations as f64);
                }
            }
        }
    }
}

fn main() {
    let mut config = Config {
        duration: Duration::from_millis(250),
        samples: 5,
        threads: vec![1, 4],
        fixture: None,
    };
    for arg in std::env::args().skip(1) {
        let (key, value) = arg.split_once('=').expect("expected --name=value");
        match key {
            "--millis" => config.duration = Duration::from_millis(value.parse().unwrap()),
            "--samples" => config.samples = value.parse().unwrap(),
            "--threads" => config.threads = value.split(',').map(|s| s.parse().unwrap()).collect(),
            "--fixture" => config.fixture = Some(value.into()),
            _ => panic!("unknown argument: {key}"),
        }
    }
    assert!(!config.duration.is_zero() && config.samples > 0);
    assert!(!config.threads.is_empty() && config.threads.iter().all(|&n| n > 0));
    if let Some(fixture) = &config.fixture {
        assert!(
            ["ordinary", "varied", "wide", "f32", "exact", "dense"].contains(&fixture.as_str()),
            "unknown fixture"
        );
    }
    println!(
        "fixture,library,format,threads,sample,values,output_bytes,operations,seconds,ns_per_op"
    );
    let ordinary: Vec<_> = (0..2048)
        .map(|i| [0.1, -12.34567, std::f64::consts::PI, 12345.6789][i % 4])
        .collect();
    run("ordinary", &ordinary, &config);
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let varied: Vec<_> = (0..2048)
        .map(|_| ((next() % 2_000_000_000_000) as i64 - 1_000_000_000_000) as f64 / 100_000.0)
        .collect();
    run("varied", &varied, &config);
    let wide: Vec<_> = std::iter::from_fn(|| Some(f64::from_bits(next())))
        .filter(|v| v.is_finite())
        .take(2048)
        .collect();
    run("wide", &wide, &config);
    let single: Vec<_> = varied.iter().map(|&v| v as f32).collect();
    run("f32", &single, &config);
    let exact: Vec<_> = (0..2048).map(|i| (i as f64 - 1024.0) * 0.125).collect();
    run("exact", &exact, &config);
    let dense: Vec<_> = (0..2048)
        .map(|i| {
            let bits = next();
            f64::from_bits((bits & ((1 << 52) - 1)) | ((1009 + i % 38) << 52) | (bits & (1 << 63)))
        })
        .collect();
    run("dense", &dense, &config);
}

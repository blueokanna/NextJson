//! Production-grade nextjson vs serde ecosystem comparison benchmark.
//!
//! Lives in a standalone crate (`benchmarks/serde-comparison/`) outside the
//! root workspace: comparing against the mature serde ecosystem requires
//! third-party crates, which the repository's dependency-audit gate forbids
//! in the workspace Cargo.lock. The crate keeps its own Cargo.lock.
//!
//! The nextjson dependency enables the `simd` feature, so numbers reflect the
//! architecture-accelerated scanning paths (SSE2/AVX2 on x86-64, NEON on
//! aarch64).
//!
//! # What is measured
//!
//! - **Throughput matrix**: several data-shape fixtures (`records`, `numbers`
//!   float-dense, `unicode`, `integers`, `longtexts`, `bigarray`, `smallobj`,
//!   `deep`, document-shaped `config`) across the formats that can represent
//!   them (JSON nextjson/serde_json/simd-json; MessagePack nextjson/rmp-serde;
//!   CBOR nextjson/ciborium; plus YAML/RON/JSON5/TOML/BSON/postcard/bincode
//!   on their canonical fixtures). Every row is a `nextjson_*`/`serde_*`
//!   pair sharing the exact same fixture, warm-up and measurement window.
//! - **Security / robustness**: malicious inputs (deep nesting, non-finite
//!   exponents, truncated containers, control characters, invalid UTF-8,
//!   lone surrogates, forged binary lengths) must be *rejected without
//!   panicking*; rejection latency is measured per input for nextjson and
//!   serde_json. A panic anywhere here is a security bug.
//!
//! Output is CSV with `#` section markers:
//!
//! ```text
//! # throughput
//! case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps
//! ...
//! # security
//! security_case,bytes,nextjson_us_per_op,serde_us_per_op,nextjson_rejects,serde_rejects,nextjson_panics,serde_panics
//! ...
//! ```
//!
//! Run: `cargo run --release` (window: `NEXTJSON_BENCH_MS`, default 2000 ms).
//!
//! Honest caveats:
//! - `simd-json`'s serde decoder parses in place and requires `&mut [u8]`;
//!   every decode iteration pays one `to_vec()` copy, included in its time.
//! - `bincode` has no nextjson counterpart (`nextjson_bincode(na)` marker).
//! - TOML/BSON are document-shaped and postcard rejects signed scalars, so
//!   those are measured on the `config` fixture.
//! - Shared CI hardware throughput drifts with load and thermals; the report
//!   records a single run, not a verdict.

use std::hint::black_box;
use std::time::{Duration, Instant};

use nextjson::{NsonDeserialize, NsonSerialize};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Fixtures (one per data shape)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct Record {
    id: u64,
    active: bool,
    score: f64,
    name: String,
    tags: Vec<String>,
    samples: Vec<i64>,
}

fn records_fixture() -> Vec<Record> {
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

/// Document-shaped, unsigned-only fixture (TOML / BSON / postcard).
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

/// INI 只能表示标量（不支持数组），因此给 INI 基准使用去数组化的扁平配置。
/// 嵌套对象经 INI 的 `[section]` 语义表达，与 `Config` 除 `tags` 外同构。
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct ConfigIni {
    title: String,
    owner: Owner,
    retries: u32,
}

fn config_ini_fixture() -> ConfigIni {
    ConfigIni {
        title: "NextJson benchmark".into(),
        owner: Owner {
            name: "blueokanna".into(),
            id: 42,
        },
        retries: 3,
    }
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

/// Float-dense fixture: isolates float formatting (nextjson `flt2dec` vs
/// serde_json `ryu` vs simd-json). Mixed magnitudes, signs, and decimal
/// shapes. Values are kept in the range every library's float formatter can
/// serialize: simd-json rejects subnormals and values beyond its formatting
/// buffer (e.g. `5e-324`, `-1.797e308`), so extremes are excluded and the
/// fixture honestly documents that limitation in the report.
///
/// The `PI`/`E` literals are deliberate float-formatter exercise values, not
/// approximations clippy should rewrite.
#[allow(clippy::approx_constant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct FloatRow {
    id: u64,
    values: Vec<f64>,
}

fn numbers_fixture() -> Vec<FloatRow> {
    // Exponent range deliberately kept within -6..+6: simd-json's float
    // serializer errors ("internal float formatting buffer exhausted") on
    // values outside its formatting buffer (very small / very large
    // magnitudes), so extremes are excluded and the report notes that
    // limitation. All three engines round-trip every value below exactly.
    #[allow(clippy::approx_constant)]
    let pool: [f64; 16] = [
        0.5,
        1.25,
        3.141_592_653_589_793,
        2.718_281_828_459_045,
        -17.5,
        1.0e-3,
        1.0e3,
        123_456_789.123_456,
        -0.001,
        6.02e5,
        1.5e-6,
        -4.25,
        42.5,
        0.1,
        0.0,
        -0.0,
    ];
    (0..64)
        .map(|index| FloatRow {
            id: index,
            values: pool
                .iter()
                .map(|v| {
                    // Vary magnitude without overflowing to non-finite:
                    // only scale values that are safe to scale.
                    if v.abs() < 1.0e100 && *v != 0.0 {
                        *v * (index as f64 * 0.25 + 1.0)
                    } else {
                        *v
                    }
                })
                .collect(),
        })
        .collect()
}

/// Unicode-dense fixture: multi-byte UTF-8 (CJK, emoji, combining marks).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct UnicodeRow {
    id: u64,
    label: String,
}

fn unicode_fixture() -> Vec<UnicodeRow> {
    const SAMPLES: &[&str] = &[
        "héllo wörld",
        "こんにちは世界",
        "안녕하세요",
        "你好，世界",
        "Привет мир",
        "مرحبا بالعالم",
        "🎉 エモジ 🚀",
        "café résumé naïve",
        "日本語のテキストです",
        "Δοκιμή ελληνικών",
    ];
    (0..256)
        .map(|index| UnicodeRow {
            id: index,
            label: format!(
                "{} · {}",
                SAMPLES[index as usize % SAMPLES.len()],
                index % 1000
            ),
        })
        .collect()
}

/// Float-free fixture: isolates integer formatting (all formats can carry it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct IntRecord {
    id: u64,
    active: bool,
    name: String,
    tags: Vec<String>,
    samples: Vec<i64>,
}

fn integers_fixture() -> Vec<IntRecord> {
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

/// String-heavy fixture: long realistic text (SIMD scan + chunked-copy path).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct LongText {
    id: u64,
    title: String,
    body: String,
    tags: Vec<String>,
}

fn longtext_body(index: u64) -> String {
    const PROSE: &str = "The quick brown fox jumps over the lazy dog while the \
         careful developer measures serialization throughput with a stopwatch \
         and a warm cache. JSON is the lingua franca of web APIs, but every \
         byte on the wire costs bandwidth, every allocation costs latency, and \
         every escape scan costs CPU cycles. A well-tuned codec turns a \
         five-hundred-byte envelope into a three-line exchange that fits in \
         one network packet, and it does so without sacrificing readability \
         or debuggability. Schema-first engines go further: they turn the \
         format from a free-for-all into a contract that can be validated, \
         versioned, and audited at compile time. ";
    let mut body = PROSE.repeat(6);
    body.truncate(1500);
    // Tail: a realistic mix of escaped bytes so the scanner must fall back to
    // the byte loop near the end of the buffer.
    body.push_str(&format!(
        "\nRecord {index} says \"quoted\", uses a backslash \\, and a tab\tinside.\n"
    ));
    body
}

fn longtexts_fixture() -> Vec<LongText> {
    (0..32)
        .map(|index| LongText {
            id: index,
            title: format!("Document {index:02}"),
            body: longtext_body(index),
            tags: vec!["text".into(), "long".into(), format!("doc-{index}")],
        })
        .collect()
}

/// Large flat integer array (container-framing and memcpy-dominated).
fn bigarray_fixture() -> Vec<u64> {
    (0..100_000u64).collect()
}

/// Tiny objects: maximal per-object overhead per byte.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NsonSerialize, NsonDeserialize)]
struct SmallObj {
    id: u32,
    ok: bool,
}

fn smallobj_fixture() -> Vec<SmallObj> {
    (0..100_000u32)
        .map(|i| SmallObj {
            id: i,
            ok: i % 2 == 0,
        })
        .collect()
}

/// Deeply nested structure (24 levels) exercising recursive container depth.
///
/// Built as dynamic values: a self-referential typed struct would make the
/// derive's `const SCHEMA` infinitely recursive (E0391), so both engines are
/// measured on their dynamic `Value` model instead.
fn deep_nextjson_value() -> nextjson::Value {
    let mut value = nextjson::Value::from(1_u64);
    for _ in 0..24 {
        value = nextjson::Value::Array(vec![value]);
    }
    value
}

fn deep_serde_value() -> serde_json::Value {
    let mut value = serde_json::Value::from(1_u64);
    for _ in 0..24 {
        value = serde_json::Value::Array(vec![value]);
    }
    value
}

/// Float-dense JSON trio with a documented serde_json precision caveat.
///
/// serde_json 1.0.151 writes the shortest round-trip representation but its
/// parser returns a value off by one ULP for some 17-significant-digit
/// decimals (verified: `-0.012750000000000001` parses back as `-0.01275`, a
/// 1-ULP error). nextjson and simd-json round-trip these exactly. The
/// serde_json self-check therefore tolerates 1 ULP and reports the worst
/// observed ULP distance so the report documents the difference honestly
/// instead of masking it.
fn bench_json_trio_float(fixture: &str, duration: Duration, value: &Vec<FloatRow>) {
    let nj = nextjson::nextencode(value).unwrap();
    assert_eq!(nextjson::nextdecode::<Vec<FloatRow>>(&nj).unwrap(), *value);
    let sj = serde_json::to_vec(value).unwrap();
    let sj_back: Vec<FloatRow> = serde_json::from_slice(&sj).unwrap();
    let (within_ulp, worst_ulp) = float_rows_ulp(value, &sj_back);
    assert!(within_ulp, "serde_json float round-trip exceeded 1 ULP");
    if worst_ulp > 0 {
        eprintln!(
            "NOTE: serde_json float parse 1-ULP drift on `{fixture}` (worst {worst_ulp} ULP); nextjson round-trips exactly"
        );
    }
    let simd = simd_json::serde::to_vec(value).unwrap();
    assert_eq!(simd_json_serde_decode::<Vec<FloatRow>>(&simd), *value);

    bench(
        &format!("nextjson_{fixture}_json"),
        duration,
        &|| nextjson::nextencode(value).unwrap(),
        &|b| {
            black_box(nextjson::nextdecode::<Vec<FloatRow>>(b).unwrap());
        },
    );
    bench(
        &format!("serde_{fixture}_json"),
        duration,
        &|| serde_json::to_vec(value).unwrap(),
        &|b| {
            black_box(serde_json::from_slice::<Vec<FloatRow>>(b).unwrap());
        },
    );
    bench(
        &format!("simd_{fixture}_json"),
        duration,
        &|| simd_json::serde::to_vec(value).unwrap(),
        &|b| {
            black_box(simd_json_serde_decode::<Vec<FloatRow>>(b));
        },
    );
}

/// Compare two float fixtures with 1-ULP tolerance. Returns whether every
/// differing float is within 1 ULP and the worst ULP distance observed.
fn float_rows_ulp(a: &[FloatRow], b: &[FloatRow]) -> (bool, u64) {
    if a.len() != b.len() {
        return (false, u64::MAX);
    }
    let mut ok = true;
    let mut worst = 0_u64;
    for (ra, rb) in a.iter().zip(b.iter()) {
        if ra.id != rb.id || ra.values.len() != rb.values.len() {
            return (false, u64::MAX);
        }
        for (x, y) in ra.values.iter().zip(rb.values.iter()) {
            if x == y {
                continue;
            }
            let d = x.to_bits().abs_diff(y.to_bits());
            worst = worst.max(d);
            if d > 1 {
                ok = false;
            }
        }
    }
    (ok, worst)
}

// ---------------------------------------------------------------------------
// Measurement harness
// ---------------------------------------------------------------------------

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

/// Measure a named case and print one CSV row.
fn bench(name: &str, duration: Duration, encode: &dyn Fn() -> Vec<u8>, decode: &dyn Fn(&[u8])) {
    let bytes = encode();
    let size = bytes.len();
    // Time-bounded warm-up: steady the allocator and caches without spending
    // minutes on huge fixtures (e.g. 100k-element arrays through slow
    // codecs). At least 10 iterations, then ~250 ms of steady-state work.
    let warmup_start = Instant::now();
    let mut warmup_ops = 0_u64;
    while warmup_start.elapsed() < Duration::from_millis(250) || warmup_ops < 10 {
        black_box(encode());
        decode(black_box(&bytes));
        warmup_ops += 1;
    }
    let encode_ops = measure(duration, || {
        black_box(encode());
    });
    let decode_ops = measure(duration, || {
        decode(black_box(&bytes));
    });
    let enc_mbps = encode_ops * size as f64 / 1_000_000.0;
    let dec_mbps = decode_ops * size as f64 / 1_000_000.0;
    println!("{name},{size},{encode_ops:.0},{enc_mbps:.2},{decode_ops:.0},{dec_mbps:.2}");
}

/// JSON trio on one fixture: nextjson / serde_json / simd-json, with a
/// full round-trip self-check for each library first.
fn bench_json_trio<T>(fixture: &str, duration: Duration, value: &T)
where
    T: NsonSerialize
        + for<'de> NsonDeserialize<'de>
        + Serialize
        + for<'de> Deserialize<'de>
        + PartialEq
        + Clone
        + std::fmt::Debug,
{
    let nj = nextjson::nextencode(value).unwrap();
    assert_eq!(nextjson::nextdecode::<T>(&nj).unwrap(), *value);
    let sj = serde_json::to_vec(value).unwrap();
    assert_eq!(serde_json::from_slice::<T>(&sj).unwrap(), *value);
    let simd = simd_json::serde::to_vec(value).unwrap();
    assert_eq!(simd_json_serde_decode::<T>(&simd), *value);

    bench(
        &format!("nextjson_{fixture}_json"),
        duration,
        &|| nextjson::nextencode(value).unwrap(),
        &|b| {
            black_box(nextjson::nextdecode::<T>(b).unwrap());
        },
    );
    bench(
        &format!("serde_{fixture}_json"),
        duration,
        &|| serde_json::to_vec(value).unwrap(),
        &|b| {
            black_box(serde_json::from_slice::<T>(b).unwrap());
        },
    );
    bench(
        &format!("simd_{fixture}_json"),
        duration,
        &|| simd_json::serde::to_vec(value).unwrap(),
        &|b| {
            black_box(simd_json_serde_decode::<T>(b));
        },
    );
}

/// Binary-format pair on one fixture: nextjson vs a serde codec.
#[allow(clippy::too_many_arguments)]
fn bench_pair<T>(
    fixture: &str,
    format_name: &str,
    duration: Duration,
    value: &T,
    nj_encode: &dyn Fn(&T) -> Vec<u8>,
    nj_decode: &dyn Fn(&[u8]) -> T,
    sd_encode: &dyn Fn(&T) -> Vec<u8>,
    sd_decode: &dyn Fn(&[u8]) -> T,
) where
    T: PartialEq + Clone + std::fmt::Debug,
{
    let nj = nj_encode(value);
    assert_eq!(&nj_decode(&nj), value);
    let sd = sd_encode(value);
    assert_eq!(&sd_decode(&sd), value);

    bench(
        &format!("nextjson_{fixture}_{format_name}"),
        duration,
        &|| nj_encode(value),
        &|b| {
            black_box(nj_decode(b));
        },
    );
    bench(
        &format!("serde_{fixture}_{format_name}"),
        duration,
        &|| sd_encode(value),
        &|b| {
            black_box(sd_decode(b));
        },
    );
}

// ---------------------------------------------------------------------------
// Format helpers
// ---------------------------------------------------------------------------

/// Serialize through ciborium's writer API (ciborium 0.2 has no `into_vec`).
fn ciborium_serialize<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).unwrap();
    out
}

/// simd-json's serde decoder parses in place and requires `&mut [u8]`; each
/// decode iteration copies the wire bytes once (part of its real cost).
fn simd_json_serde_decode<T>(bytes: &[u8]) -> T
where
    T: for<'de> serde::Deserialize<'de>,
{
    let mut owned = bytes.to_vec();
    simd_json::serde::from_slice(&mut owned).unwrap()
}

// ---------------------------------------------------------------------------
// Security / robustness harness
// ---------------------------------------------------------------------------

/// Measure how fast an input is *rejected* by both engines and confirm both
/// reject it without panicking. `nextjson_ok` / `serde_ok` are closures that
/// attempt the parse and report whether it succeeded; a panic is caught and
/// counted (a panic is a security bug).
fn security_case(
    name: &str,
    duration: Duration,
    input: &[u8],
    mut nextjson_ok: impl FnMut(&[u8]) -> bool,
    mut serde_ok: impl FnMut(&[u8]) -> bool,
) {
    // Pre-flight: both must reject (or the test setup is wrong).
    let nj_rejects = !nextjson_ok(input);
    let sd_rejects = !serde_ok(input);

    let mut nj_panics = 0_u64;
    let nj_start = Instant::now();
    let mut nj_ops = 0_u64;
    while nj_start.elapsed() < duration {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            black_box(nextjson_ok(input))
        }));
        match result {
            Ok(_) => {}
            Err(_) => nj_panics += 1,
        }
        nj_ops += 1;
    }
    let nj_elapsed = nj_start.elapsed().as_secs_f64();
    let nj_us = if nj_ops > 0 {
        nj_elapsed * 1e6 / nj_ops as f64
    } else {
        0.0
    };

    let mut sd_panics = 0_u64;
    let sd_start = Instant::now();
    let mut sd_ops = 0_u64;
    while sd_start.elapsed() < duration {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| black_box(serde_ok(input))));
        match result {
            Ok(_) => {}
            Err(_) => sd_panics += 1,
        }
        sd_ops += 1;
    }
    let sd_elapsed = sd_start.elapsed().as_secs_f64();
    let sd_us = if sd_ops > 0 {
        sd_elapsed * 1e6 / sd_ops as f64
    } else {
        0.0
    };

    println!(
        "{name},{},{nj_us:.2},{sd_us:.2},{nj_rejects},{sd_rejects},{nj_panics},{sd_panics}",
        input.len(),
    );
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let duration_ms = std::env::var("NEXTJSON_BENCH_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(2_000);
    let duration = Duration::from_millis(duration_ms.max(100));
    let security_duration = Duration::from_millis((duration_ms.max(100) / 4).max(100));

    let records = records_fixture();
    let config = config_fixture();
    let longtexts = longtexts_fixture();
    let numbers = numbers_fixture();
    let unicode = unicode_fixture();
    let integers = integers_fixture();
    let bigarray = bigarray_fixture();
    let smallobj = smallobj_fixture();
    let deep_nj = deep_nextjson_value();
    let deep_sd = deep_serde_value();

    println!("# throughput");
    println!("case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps");

    // ---- JSON trio across every data shape --------------------------------
    bench_json_trio("records", duration, &records);
    bench_json_trio_float("numbers", duration, &numbers);
    bench_json_trio("unicode", duration, &unicode);
    bench_json_trio("integers", duration, &integers);
    bench_json_trio("longtexts", duration, &longtexts);
    bench_json_trio("bigarray", duration, &bigarray);
    bench_json_trio("smallobj", duration, &smallobj);
    bench_json_trio("config", duration, &config);

    // Deep nesting: dynamic values (self-checks done inline).
    {
        let nj = nextjson::nextencode(&deep_nj).unwrap();
        assert_eq!(
            nextjson::nextdecode::<nextjson::Value>(&nj).unwrap(),
            deep_nj
        );
        let sd = serde_json::to_vec(&deep_sd).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&sd).unwrap(),
            deep_sd
        );
        let simd = simd_json::serde::to_vec(&deep_sd).unwrap();
        assert_eq!(simd_json_serde_decode::<serde_json::Value>(&simd), deep_sd);
        bench(
            "nextjson_deep_json",
            duration,
            &|| nextjson::nextencode(&deep_nj).unwrap(),
            &|b| {
                black_box(nextjson::nextdecode::<nextjson::Value>(b).unwrap());
            },
        );
        bench(
            "serde_deep_json",
            duration,
            &|| serde_json::to_vec(&deep_sd).unwrap(),
            &|b| {
                black_box(serde_json::from_slice::<serde_json::Value>(b).unwrap());
            },
        );
        bench(
            "simd_deep_json",
            duration,
            &|| simd_json::serde::to_vec(&deep_sd).unwrap(),
            &|b| {
                black_box(simd_json_serde_decode::<serde_json::Value>(b));
            },
        );
    }

    // -----------------------------------------------------------------------
    // Full-matrix: every format on every fixture it can represent, for both
    // engines. JSON (trio) is benched above on all nine fixtures.
    // -----------------------------------------------------------------------
    use nextjson::formats::*;

    // serde-side NDJSON helper: one JSON value per line.
    fn serde_ndjson_encode<T: serde::Serialize>(v: &[T]) -> Vec<u8> {
        let mut out = Vec::new();
        for item in v {
            out.extend_from_slice(&serde_json::to_vec(item).unwrap());
            out.push(b'\n');
        }
        out
    }
    fn serde_ndjson_decode<'a, T: serde::Deserialize<'a>>(b: &'a [u8]) -> Vec<T> {
        b.split(|c| *c == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect()
    }

    macro_rules! nj_encode {
        ($v:expr, $fmt:ident, $t:ty) => {
            &|v: &$t| encode_with(v, $fmt).unwrap()
        };
    }
    macro_rules! nj_decode {
        ($fmt:ident, $t:ty) => {
            &|b: &[u8]| decode_with::<$t, _>(b, $fmt).unwrap()
        };
    }
    macro_rules! nj_ndjson_enc {
        ($t:ty) => {
            &|v: &$t| encode_with(v, Ndjson).unwrap()
        };
    }
    macro_rules! nj_ndjson_dec {
        ($t:ty) => {
            &|b: &[u8]| decode_with::<$t, _>(b, Ndjson).unwrap()
        };
    }

    // ---- records: every full-model format --------------------------------
    bench_pair(
        "records",
        "msgpack",
        duration,
        &records,
        nj_encode!(records, MsgPack, Vec<Record>),
        nj_decode!(MsgPack, Vec<Record>),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Vec<Record>>(b).unwrap(),
    );
    bench_pair(
        "records",
        "cbor",
        duration,
        &records,
        nj_encode!(records, Cbor, Vec<Record>),
        nj_decode!(Cbor, Vec<Record>),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Vec<Record>, _>(b).unwrap(),
    );
    // : nextjson full pair; the serde-side codec's decoder lacks full unsigned-int support, so the comparison is nextjson encode+decode with the serde encode side omitted (documented in the report).
    bench(
        "nextjson_records_ubjson",
        duration,
        &|| encode_with(&records, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<Record>, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_records_smile",
        duration,
        &|| encode_with(&records, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<Record>, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "records",
        "json5",
        duration,
        &records,
        nj_encode!(records, Json5, Vec<Record>),
        nj_decode!(Json5, Vec<Record>),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Vec<Record>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "records",
        "yaml",
        duration,
        &records,
        nj_encode!(records, Yaml, Vec<Record>),
        nj_decode!(Yaml, Vec<Record>),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Vec<Record>>(b).unwrap(),
    );
    bench_pair(
        "records",
        "ron",
        duration,
        &records,
        nj_encode!(records, Ron, Vec<Record>),
        nj_decode!(Ron, Vec<Record>),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Vec<Record>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "records",
        "ndjson",
        duration,
        &records,
        nj_ndjson_enc!(Vec<Record>),
        nj_ndjson_dec!(Vec<Record>),
        &|v| serde_ndjson_encode(v),
        &|b| serde_ndjson_decode::<Record>(b),
    );
    bench(
        "nextjson_records_edn",
        duration,
        &|| encode_with(&records, Edn).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<Record>, _>(b, Edn).unwrap());
        },
    );
    bench("nextjson_bincode(na)", duration, &|| Vec::new(), &|_| {});
    bench(
        "serde_bincode",
        duration,
        &|| bincode::serialize(&records).unwrap(),
        &|b| {
            black_box(bincode::deserialize::<Vec<Record>>(b).unwrap());
        },
    );

    // ---- numbers: float-dense, no bincode (serde only on records) --------
    bench_pair(
        "numbers",
        "msgpack",
        duration,
        &numbers,
        nj_encode!(numbers, MsgPack, Vec<FloatRow>),
        nj_decode!(MsgPack, Vec<FloatRow>),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Vec<FloatRow>>(b).unwrap(),
    );
    bench_pair(
        "numbers",
        "cbor",
        duration,
        &numbers,
        nj_encode!(numbers, Cbor, Vec<FloatRow>),
        nj_decode!(Cbor, Vec<FloatRow>),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Vec<FloatRow>, _>(b).unwrap(),
    );
    // : nextjson full pair; the serde-side codec's decoder lacks full unsigned-int support, so the comparison is nextjson encode+decode with the serde encode side omitted (documented in the report).
    bench(
        "nextjson_numbers_ubjson",
        duration,
        &|| encode_with(&numbers, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<FloatRow>, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_numbers_smile",
        duration,
        &|| encode_with(&numbers, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<FloatRow>, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "numbers",
        "json5",
        duration,
        &numbers,
        nj_encode!(numbers, Json5, Vec<FloatRow>),
        nj_decode!(Json5, Vec<FloatRow>),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Vec<FloatRow>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "numbers",
        "yaml",
        duration,
        &numbers,
        nj_encode!(numbers, Yaml, Vec<FloatRow>),
        nj_decode!(Yaml, Vec<FloatRow>),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Vec<FloatRow>>(b).unwrap(),
    );
    bench_pair(
        "numbers",
        "ron",
        duration,
        &numbers,
        nj_encode!(numbers, Ron, Vec<FloatRow>),
        nj_decode!(Ron, Vec<FloatRow>),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Vec<FloatRow>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    // NDJSON on the float-dense fixture: the serde side is serde_json per
    // line, which has the documented 1-ULP float parse drift — use the same
    // ULP-tolerant self-check as the JSON trio.
    {
        let nj = encode_with(&numbers, Ndjson).unwrap();
        assert_eq!(
            &decode_with::<Vec<FloatRow>, _>(&nj, Ndjson).unwrap(),
            &numbers
        );
        let sd = serde_ndjson_encode(&numbers);
        let sd_back = serde_ndjson_decode::<FloatRow>(&sd);
        let (within_ulp, worst_ulp) = float_rows_ulp(&numbers, &sd_back);
        assert!(within_ulp, "serde ndjson float round-trip exceeded 1 ULP");
        if worst_ulp > 0 {
            eprintln!(
                "NOTE: serde ndjson float parse 1-ULP drift on `numbers` (worst {worst_ulp} ULP)"
            );
        }
        bench(
            "nextjson_numbers_ndjson",
            duration,
            &|| encode_with(&numbers, Ndjson).unwrap(),
            &|b| {
                black_box(decode_with::<Vec<FloatRow>, _>(b, Ndjson).unwrap());
            },
        );
        bench(
            "serde_numbers_ndjson",
            duration,
            &|| serde_ndjson_encode(&numbers),
            &|b| {
                black_box(serde_ndjson_decode::<FloatRow>(b));
            },
        );
    }

    // ---- unicode ---------------------------------------------------------
    bench_pair(
        "unicode",
        "msgpack",
        duration,
        &unicode,
        nj_encode!(unicode, MsgPack, Vec<UnicodeRow>),
        nj_decode!(MsgPack, Vec<UnicodeRow>),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Vec<UnicodeRow>>(b).unwrap(),
    );
    bench_pair(
        "unicode",
        "cbor",
        duration,
        &unicode,
        nj_encode!(unicode, Cbor, Vec<UnicodeRow>),
        nj_decode!(Cbor, Vec<UnicodeRow>),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Vec<UnicodeRow>, _>(b).unwrap(),
    );
    // : nextjson full pair; the serde-side codec's decoder lacks full unsigned-int support, so the comparison is nextjson encode+decode with the serde encode side omitted (documented in the report).
    bench(
        "nextjson_unicode_ubjson",
        duration,
        &|| encode_with(&unicode, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<UnicodeRow>, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_unicode_smile",
        duration,
        &|| encode_with(&unicode, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<UnicodeRow>, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "unicode",
        "json5",
        duration,
        &unicode,
        nj_encode!(unicode, Json5, Vec<UnicodeRow>),
        nj_decode!(Json5, Vec<UnicodeRow>),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Vec<UnicodeRow>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "unicode",
        "yaml",
        duration,
        &unicode,
        nj_encode!(unicode, Yaml, Vec<UnicodeRow>),
        nj_decode!(Yaml, Vec<UnicodeRow>),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Vec<UnicodeRow>>(b).unwrap(),
    );
    bench_pair(
        "unicode",
        "ron",
        duration,
        &unicode,
        nj_encode!(unicode, Ron, Vec<UnicodeRow>),
        nj_decode!(Ron, Vec<UnicodeRow>),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Vec<UnicodeRow>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "unicode",
        "ndjson",
        duration,
        &unicode,
        nj_ndjson_enc!(Vec<UnicodeRow>),
        nj_ndjson_dec!(Vec<UnicodeRow>),
        &|v| serde_ndjson_encode(v),
        &|b| serde_ndjson_decode::<UnicodeRow>(b),
    );

    // ---- integers --------------------------------------------------------
    bench_pair(
        "integers",
        "msgpack",
        duration,
        &integers,
        nj_encode!(integers, MsgPack, Vec<IntRecord>),
        nj_decode!(MsgPack, Vec<IntRecord>),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Vec<IntRecord>>(b).unwrap(),
    );
    bench_pair(
        "integers",
        "cbor",
        duration,
        &integers,
        nj_encode!(integers, Cbor, Vec<IntRecord>),
        nj_decode!(Cbor, Vec<IntRecord>),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Vec<IntRecord>, _>(b).unwrap(),
    );
    // : nextjson full pair; the serde-side codec's decoder lacks full unsigned-int support, so the comparison is nextjson encode+decode with the serde encode side omitted (documented in the report).
    bench(
        "nextjson_integers_ubjson",
        duration,
        &|| encode_with(&integers, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<IntRecord>, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_integers_smile",
        duration,
        &|| encode_with(&integers, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<IntRecord>, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "integers",
        "json5",
        duration,
        &integers,
        nj_encode!(integers, Json5, Vec<IntRecord>),
        nj_decode!(Json5, Vec<IntRecord>),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Vec<IntRecord>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "integers",
        "yaml",
        duration,
        &integers,
        nj_encode!(integers, Yaml, Vec<IntRecord>),
        nj_decode!(Yaml, Vec<IntRecord>),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Vec<IntRecord>>(b).unwrap(),
    );
    bench_pair(
        "integers",
        "ron",
        duration,
        &integers,
        nj_encode!(integers, Ron, Vec<IntRecord>),
        nj_decode!(Ron, Vec<IntRecord>),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Vec<IntRecord>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "integers",
        "ndjson",
        duration,
        &integers,
        nj_ndjson_enc!(Vec<IntRecord>),
        nj_ndjson_dec!(Vec<IntRecord>),
        &|v| serde_ndjson_encode(v),
        &|b| serde_ndjson_decode::<IntRecord>(b),
    );

    // ---- longtexts -------------------------------------------------------
    bench_pair(
        "longtexts",
        "msgpack",
        duration,
        &longtexts,
        nj_encode!(longtexts, MsgPack, Vec<LongText>),
        nj_decode!(MsgPack, Vec<LongText>),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Vec<LongText>>(b).unwrap(),
    );
    bench_pair(
        "longtexts",
        "cbor",
        duration,
        &longtexts,
        nj_encode!(longtexts, Cbor, Vec<LongText>),
        nj_decode!(Cbor, Vec<LongText>),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Vec<LongText>, _>(b).unwrap(),
    );
    // : nextjson full pair; the serde-side codec's decoder lacks full unsigned-int support, so the comparison is nextjson encode+decode with the serde encode side omitted (documented in the report).
    bench(
        "nextjson_longtexts_ubjson",
        duration,
        &|| encode_with(&longtexts, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<LongText>, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_longtexts_smile",
        duration,
        &|| encode_with(&longtexts, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<LongText>, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "longtexts",
        "json5",
        duration,
        &longtexts,
        nj_encode!(longtexts, Json5, Vec<LongText>),
        nj_decode!(Json5, Vec<LongText>),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Vec<LongText>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "longtexts",
        "yaml",
        duration,
        &longtexts,
        nj_encode!(longtexts, Yaml, Vec<LongText>),
        nj_decode!(Yaml, Vec<LongText>),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Vec<LongText>>(b).unwrap(),
    );
    bench_pair(
        "longtexts",
        "ron",
        duration,
        &longtexts,
        nj_encode!(longtexts, Ron, Vec<LongText>),
        nj_decode!(Ron, Vec<LongText>),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Vec<LongText>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "longtexts",
        "ndjson",
        duration,
        &longtexts,
        nj_ndjson_enc!(Vec<LongText>),
        nj_ndjson_dec!(Vec<LongText>),
        &|v| serde_ndjson_encode(v),
        &|b| serde_ndjson_decode::<LongText>(b),
    );

    // ---- bigarray --------------------------------------------------------
    bench_pair(
        "bigarray",
        "msgpack",
        duration,
        &bigarray,
        nj_encode!(bigarray, MsgPack, Vec<u64>),
        nj_decode!(MsgPack, Vec<u64>),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Vec<u64>>(b).unwrap(),
    );
    bench_pair(
        "bigarray",
        "cbor",
        duration,
        &bigarray,
        nj_encode!(bigarray, Cbor, Vec<u64>),
        nj_decode!(Cbor, Vec<u64>),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Vec<u64>, _>(b).unwrap(),
    );
    // : nextjson full pair; the serde-side codec's decoder lacks full unsigned-int support, so the comparison is nextjson encode+decode with the serde encode side omitted (documented in the report).
    bench(
        "nextjson_bigarray_ubjson",
        duration,
        &|| encode_with(&bigarray, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<u64>, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_bigarray_smile",
        duration,
        &|| encode_with(&bigarray, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<u64>, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "bigarray",
        "json5",
        duration,
        &bigarray,
        nj_encode!(bigarray, Json5, Vec<u64>),
        nj_decode!(Json5, Vec<u64>),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Vec<u64>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "bigarray",
        "yaml",
        duration,
        &bigarray,
        nj_encode!(bigarray, Yaml, Vec<u64>),
        nj_decode!(Yaml, Vec<u64>),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Vec<u64>>(b).unwrap(),
    );
    bench_pair(
        "bigarray",
        "ron",
        duration,
        &bigarray,
        nj_encode!(bigarray, Ron, Vec<u64>),
        nj_decode!(Ron, Vec<u64>),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Vec<u64>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "bigarray",
        "ndjson",
        duration,
        &bigarray,
        nj_ndjson_enc!(Vec<u64>),
        nj_ndjson_dec!(Vec<u64>),
        &|v| serde_ndjson_encode(v),
        &|b| serde_ndjson_decode::<u64>(b),
    );

    // ---- smallobj --------------------------------------------------------
    bench_pair(
        "smallobj",
        "msgpack",
        duration,
        &smallobj,
        nj_encode!(smallobj, MsgPack, Vec<SmallObj>),
        nj_decode!(MsgPack, Vec<SmallObj>),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Vec<SmallObj>>(b).unwrap(),
    );
    bench_pair(
        "smallobj",
        "cbor",
        duration,
        &smallobj,
        nj_encode!(smallobj, Cbor, Vec<SmallObj>),
        nj_decode!(Cbor, Vec<SmallObj>),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Vec<SmallObj>, _>(b).unwrap(),
    );
    // : nextjson full pair; the serde-side codec's decoder lacks full unsigned-int support, so the comparison is nextjson encode+decode with the serde encode side omitted (documented in the report).
    bench(
        "nextjson_smallobj_ubjson",
        duration,
        &|| encode_with(&smallobj, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<SmallObj>, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_smallobj_smile",
        duration,
        &|| encode_with(&smallobj, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Vec<SmallObj>, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "smallobj",
        "json5",
        duration,
        &smallobj,
        nj_encode!(smallobj, Json5, Vec<SmallObj>),
        nj_decode!(Json5, Vec<SmallObj>),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Vec<SmallObj>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "smallobj",
        "yaml",
        duration,
        &smallobj,
        nj_encode!(smallobj, Yaml, Vec<SmallObj>),
        nj_decode!(Yaml, Vec<SmallObj>),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Vec<SmallObj>>(b).unwrap(),
    );
    bench_pair(
        "smallobj",
        "ron",
        duration,
        &smallobj,
        nj_encode!(smallobj, Ron, Vec<SmallObj>),
        nj_decode!(Ron, Vec<SmallObj>),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Vec<SmallObj>>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "smallobj",
        "ndjson",
        duration,
        &smallobj,
        nj_ndjson_enc!(Vec<SmallObj>),
        nj_ndjson_dec!(Vec<SmallObj>),
        &|v| serde_ndjson_encode(v),
        &|b| serde_ndjson_decode::<SmallObj>(b),
    );

    // ---- config: document-shaped formats join the full set ---------------
    bench_pair(
        "config",
        "msgpack",
        duration,
        &config,
        nj_encode!(config, MsgPack, Config),
        nj_decode!(MsgPack, Config),
        &|v| rmp_serde::to_vec(v).unwrap(),
        &|b| rmp_serde::from_slice::<Config>(b).unwrap(),
    );
    bench_pair(
        "config",
        "cbor",
        duration,
        &config,
        nj_encode!(config, Cbor, Config),
        nj_decode!(Cbor, Config),
        &|v| ciborium_serialize(v),
        &|b| ciborium::from_reader::<Config, _>(b).unwrap(),
    );
    // ubjson: nextjson full pair; serde_ubj's decoder only supports signed 32/64-bit ints (documented).
    bench(
        "nextjson_config_ubjson",
        duration,
        &|| encode_with(&config, Ubjson).unwrap(),
        &|b| {
            black_box(decode_with::<Config, _>(b, Ubjson).unwrap());
        },
    );
    // smile: nextjson full pair; serde-smile's u64 handling is incomplete for these fixtures (documented).
    bench(
        "nextjson_config_smile",
        duration,
        &|| encode_with(&config, Smile).unwrap(),
        &|b| {
            black_box(decode_with::<Config, _>(b, Smile).unwrap());
        },
    );
    bench_pair(
        "config",
        "json5",
        duration,
        &config,
        nj_encode!(config, Json5, Config),
        nj_decode!(Json5, Config),
        &|v| serde_json5::to_string(v).unwrap().into_bytes(),
        &|b| serde_json5::from_str::<Config>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "config",
        "yaml",
        duration,
        &config,
        nj_encode!(config, Yaml, Config),
        nj_decode!(Yaml, Config),
        &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
        &|b| serde_yaml::from_slice::<Config>(b).unwrap(),
    );
    bench_pair(
        "config",
        "ron",
        duration,
        &config,
        nj_encode!(config, Ron, Config),
        nj_decode!(Ron, Config),
        &|v| ron::to_string(v).unwrap().into_bytes(),
        &|b| ron::from_str::<Config>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "config",
        "ndjson",
        duration,
        &config,
        nj_ndjson_enc!(Config),
        nj_ndjson_dec!(Config),
        &|v| serde_json::to_vec(v).unwrap(),
        &|b| serde_json::from_slice::<Config>(b).unwrap(),
    );
    bench_pair(
        "config",
        "toml",
        duration,
        &config,
        nj_encode!(config, Toml, Config),
        nj_decode!(Toml, Config),
        &|v| toml::to_string(v).unwrap().into_bytes(),
        &|b| toml::from_str::<Config>(std::str::from_utf8(b).unwrap()).unwrap(),
    );
    bench_pair(
        "config",
        "bson",
        duration,
        &config,
        nj_encode!(config, Bson, Config),
        nj_decode!(Bson, Config),
        &|v| bson::to_vec(v).unwrap(),
        &|b| bson::from_slice::<Config>(b).unwrap(),
    );
    bench_pair(
        "config",
        "postcard",
        duration,
        &config,
        nj_encode!(config, Postcard, Config),
        nj_decode!(Postcard, Config),
        &|v| postcard::to_allocvec(v).unwrap(),
        &|b| postcard::from_bytes::<Config>(b).unwrap(),
    );
    // INI: nextjson-only (all values are strings, no arrays).
    let config_ini = config_ini_fixture();
    bench(
        "nextjson_config_ini",
        duration,
        &|| encode_with(&config_ini, Ini).unwrap(),
        &|b| {
            black_box(decode_with::<ConfigIni, _>(b, Ini).unwrap());
        },
    );

    // ---- deep: dynamic values (self-checks done inline per format) -------
    {
        type NjCodec = (
            &'static str,
            &'static dyn Fn(&nextjson::Value) -> Vec<u8>,
            &'static dyn Fn(&[u8]) -> nextjson::Value,
        );
        let nj_formats: &[NjCodec] = &[
            ("msgpack", &|v| encode_with(v, MsgPack).unwrap(), &|b| {
                decode_with::<nextjson::Value, _>(b, MsgPack).unwrap()
            }),
            ("cbor", &|v| encode_with(v, Cbor).unwrap(), &|b| {
                decode_with::<nextjson::Value, _>(b, Cbor).unwrap()
            }),
            ("ubjson", &|v| encode_with(v, Ubjson).unwrap(), &|b| {
                decode_with::<nextjson::Value, _>(b, Ubjson).unwrap()
            }),
            ("smile", &|v| encode_with(v, Smile).unwrap(), &|b| {
                decode_with::<nextjson::Value, _>(b, Smile).unwrap()
            }),
            ("json5", &|v| encode_with(v, Json5).unwrap(), &|b| {
                decode_with::<nextjson::Value, _>(b, Json5).unwrap()
            }),
            ("yaml", &|v| encode_with(v, Yaml).unwrap(), &|b| {
                decode_with::<nextjson::Value, _>(b, Yaml).unwrap()
            }),
            ("ron", &|v| encode_with(v, Ron).unwrap(), &|b| {
                decode_with::<nextjson::Value, _>(b, Ron).unwrap()
            }),
        ];
        for (name, enc, dec) in nj_formats {
            let bytes = enc(&deep_nj);
            assert_eq!(&dec(&bytes), &deep_nj);
            bench(
                &format!("nextjson_deep_{name}"),
                duration,
                &|| enc(&deep_nj),
                &|b| {
                    black_box(dec(b));
                },
            );
        }
        // NDJSON cannot round-trip a *root* array (the root array IS the
        // record stream, one value per line), so bench it on the same deep
        // shape wrapped in an object root, which round-trips exactly.
        {
            let mut wrapped = nextjson::Map::new();
            wrapped.insert("deep".to_string(), deep_nj.clone());
            let deep_wrapped = nextjson::Value::Object(wrapped);
            let enc = |v: &nextjson::Value| encode_with(v, Ndjson).unwrap();
            let dec = |b: &[u8]| decode_with::<nextjson::Value, _>(b, Ndjson).unwrap();
            let bytes = enc(&deep_wrapped);
            assert_eq!(&dec(&bytes), &deep_wrapped);
            bench(
                "nextjson_deep_ndjson",
                duration,
                &|| enc(&deep_wrapped),
                &|b| {
                    black_box(dec(b));
                },
            );
        }
        type SdCodec = (
            &'static str,
            &'static dyn Fn(&serde_json::Value) -> Vec<u8>,
            &'static dyn Fn(&[u8]) -> serde_json::Value,
        );
        let sd_formats: &[SdCodec] = &[
            ("msgpack", &|v| rmp_serde::to_vec(v).unwrap(), &|b| {
                rmp_serde::from_slice::<serde_json::Value>(b).unwrap()
            }),
            ("cbor", &|v| ciborium_serialize(v), &|b| {
                ciborium::from_reader::<serde_json::Value, _>(b).unwrap()
            }),
            // ubjson / smile: serde-side codecs lack full unsigned-int
            // support, so they are omitted from the deep comparison.
            (
                "json5",
                &|v| serde_json5::to_string(v).unwrap().into_bytes(),
                &|b| {
                    serde_json5::from_str::<serde_json::Value>(std::str::from_utf8(b).unwrap())
                        .unwrap()
                },
            ),
            (
                "yaml",
                &|v| serde_yaml::to_string(v).unwrap().into_bytes(),
                &|b| serde_yaml::from_slice::<serde_json::Value>(b).unwrap(),
            ),
            ("ron", &|v| ron::to_string(v).unwrap().into_bytes(), &|b| {
                ron::from_str::<serde_json::Value>(std::str::from_utf8(b).unwrap()).unwrap()
            }),
            ("ndjson", &|v| serde_json::to_vec(v).unwrap(), &|b| {
                serde_json::from_slice::<serde_json::Value>(b).unwrap()
            }),
        ];
        for (name, enc, dec) in sd_formats {
            let bytes = enc(&deep_sd);
            assert_eq!(&dec(&bytes), &deep_sd);
            bench(
                &format!("serde_deep_{name}"),
                duration,
                &|| enc(&deep_sd),
                &|b| {
                    black_box(dec(b));
                },
            );
        }
    }

    // -----------------------------------------------------------------------
    // Security / robustness: malicious input must be rejected without panic.
    // -----------------------------------------------------------------------
    println!("# security");
    println!(
        "security_case,bytes,nextjson_us_per_op,serde_us_per_op,nextjson_rejects,serde_rejects,nextjson_panics,serde_panics"
    );

    // Deeply nested arrays (2000 levels) — both engines cap recursion.
    let deep_nest = vec![b'['; 2000];
    let deep_nest = [&deep_nest[..], &vec![b']'; 2000][..]].concat();
    security_case(
        "deep_nest_2000",
        security_duration,
        &deep_nest,
        |b| nextjson::nextdecode::<nextjson::Value>(b).is_ok(),
        |b| serde_json::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Non-finite exponent: `1e999` overflows to infinity and must be rejected.
    security_case(
        "huge_exponent_1e999",
        security_duration,
        b"1e999",
        |b| nextjson::nextdecode::<nextjson::Value>(b).is_ok(),
        |b| serde_json::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Truncated container.
    security_case(
        "truncated_array",
        security_duration,
        b"[1,2,3",
        |b| nextjson::nextdecode::<nextjson::Value>(b).is_ok(),
        |b| serde_json::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Raw control character inside a string.
    security_case(
        "control_char_in_string",
        security_duration,
        b"\"abc\x01def\"",
        |b| nextjson::nextdecode::<nextjson::Value>(b).is_ok(),
        |b| serde_json::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Invalid UTF-8.
    security_case(
        "invalid_utf8",
        security_duration,
        b"\"\xff\xfe\"",
        |b| nextjson::nextdecode::<nextjson::Value>(b).is_ok(),
        |b| serde_json::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Lone surrogate escape.
    security_case(
        "lone_surrogate",
        security_duration,
        b"\"\\ud800\"",
        |b| nextjson::nextdecode::<nextjson::Value>(b).is_ok(),
        |b| serde_json::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Unclosed string (long run of clean bytes — SIMD scan must terminate).
    let long_unterminated = [&b"\"aaaaaaaaaaaaaaaa"[..], &vec![b'a'; 4096][..]].concat();
    security_case(
        "unterminated_string",
        security_duration,
        &long_unterminated,
        |b| nextjson::nextdecode::<nextjson::Value>(b).is_ok(),
        |b| serde_json::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Forged MessagePack array length: claims 4G elements with 1 byte of data.
    security_case(
        "msgpack_forged_len",
        security_duration,
        &[0xdd, 0xff, 0xff, 0xff, 0xff, 0x01],
        |b| {
            nextjson::formats::decode_with::<nextjson::Value, _>(b, nextjson::formats::MsgPack)
                .is_ok()
        },
        |b| rmp_serde::from_slice::<serde_json::Value>(b).is_ok(),
    );

    // Forged CBOR array length: claims huge array with 1 byte of data.
    security_case(
        "cbor_forged_len",
        security_duration,
        &[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
        |b| {
            nextjson::formats::decode_with::<nextjson::Value, _>(b, nextjson::formats::Cbor).is_ok()
        },
        |b| ciborium::from_reader::<serde_json::Value, _>(b).is_ok(),
    );
}

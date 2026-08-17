//! GitHub Actions benchmark report generator.
//!
//! Merges the two benchmark CSVs produced by the `benchmark.yml` workflow —
//! the in-workspace format matrix (`format_comparison`) and the standalone
//! serde comparison (`serde-comparison`) — into a single
//! `Github_Action_Benchmark.md` report with environment metadata.
//!
//! Usage:
//! ```text
//! cargo run --release --bin bench_report -- \
//!   --format-csv format.csv \
//!   --serde-csv serde.csv \
//!   [--out Github_Action_Benchmark.md]
//! ```
//!
//! Environment metadata comes from standard variables (`GITHUB_SHA`,
//! `GITHUB_REF_NAME`, `GITHUB_RUN_ID`, `RUNNER_OS`) plus `rustc --version`
//! and, on Linux, the CPU model from `/proc/cpuinfo`. Missing values are
//! reported as `n/a`; the tool never fails on missing metadata.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
struct Row {
    name: String,
    size: u64,
    encode_mbps: f64,
    decode_mbps: f64,
}

/// Parse a `case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps`
/// CSV (single header row; extra columns are ignored).
fn parse_csv(path: &str) -> Vec<Row> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path));
    let mut rows = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("case,") || line.starts_with("format,") {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            eprintln!(
                "warning: {path}:{} malformed row ({} cols): {}",
                lineno + 1,
                cols.len(),
                line
            );
            continue;
        }
        let name = cols[0].trim().to_string();
        let size = cols[1].trim().parse::<u64>().unwrap_or(0);
        let encode_mbps = cols[3].trim().parse::<f64>().unwrap_or(0.0);
        let decode_mbps = cols[5].trim().parse::<f64>().unwrap_or(0.0);
        rows.push(Row {
            name,
            size,
            encode_mbps,
            decode_mbps,
        });
    }
    rows
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

/// Run `git <args...>` and return trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
}

/// Best-effort commit SHA: `GITHUB_SHA` in Actions, `git rev-parse HEAD`
/// locally.
fn commit_sha() -> String {
    env::var("GITHUB_SHA")
        .ok()
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "n/a".to_string())
}

/// Best-effort branch/ref name.
fn ref_name() -> String {
    env::var("GITHUB_REF_NAME")
        .ok()
        .or_else(|| git(&["branch", "--show-current"]))
        .unwrap_or_else(|| "n/a".to_string())
}

/// Best-effort timestamp: in Actions the run page has the wall-clock time;
/// locally the last-commit committer date is the closest stable reference.
fn generated_at() -> String {
    if env::var("GITHUB_ACTIONS").is_ok() {
        return env::var("GITHUB_RUN_ID")
            .map(|id| format!("see workflow run #{id}"))
            .unwrap_or_else(|_| "see workflow run".to_string());
    }
    git(&["log", "-1", "--format=%cI"]).unwrap_or_else(|| "n/a".to_string())
}

fn cpu_model() -> String {
    if let Ok(text) = fs::read_to_string("/proc/cpuinfo") {
        for line in text.lines() {
            if let Some(model) = line.strip_prefix("model name") {
                return model.trim_start_matches(':').trim().to_string();
            }
        }
    }
    env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "n/a".to_string())
}

/// Split a case name into (implementation, format-suffix), e.g.
/// `nextjson_longtext_json` -> (`nextjson`, `longtext_json`),
/// `serde_bincode` -> (`serde`, `bincode`).
fn split_case(name: &str) -> (&str, &str) {
    match name.split_once('_') {
        Some((impl_, rest)) => (impl_, rest),
        None => (name, ""),
    }
}

const FORMAT_ORDER: &[&str] = &[
    "json",
    "longtext_json",
    "json5",
    "yaml",
    "ron",
    "msgpack",
    "cbor",
    "bincode",
    "toml",
    "bson",
    "postcard",
    "intonly",
];

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut format_csv = None;
    let mut serde_csv = None;
    let mut out = "Github_Action_Benchmark.md".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--format-csv" => {
                i += 1;
                format_csv = args.get(i).cloned();
            }
            "--serde-csv" => {
                i += 1;
                serde_csv = args.get(i).cloned();
            }
            "--out" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    out = v.clone();
                }
            }
            other => {
                eprintln!("warning: unknown argument {other}");
            }
        }
        i += 1;
    }
    let format_csv = format_csv.expect("--format-csv is required");
    let serde_csv = serde_csv.expect("--serde-csv is required");

    let format_rows = parse_csv(&format_csv);
    let serde_rows = parse_csv(&serde_csv);

    // Group serde-comparison rows by format suffix, keeping impl order.
    let mut by_format: BTreeMap<String, Vec<(String, Row)>> = BTreeMap::new();
    for row in &serde_rows {
        let (impl_, fmt) = split_case(&row.name);
        if impl_ == "nextjson" && fmt == "bincode(na)" {
            continue; // placeholder row, never measured
        }
        by_format
            .entry(fmt.to_string())
            .or_default()
            .push((impl_.to_string(), row.clone()));
    }

    let mut md = String::new();
    md.push_str("# GitHub Actions 基准报告 — NextJson vs Serde 生态\n\n");
    md.push_str(
        "> 本报告由 `.github/workflows/benchmark.yml` 自动生成，原始 CSV 随工作流产物一起上传。\n",
    );
    md.push_str("> 所有数字均为 **release** 构建（`opt-level=3`, `lto=thin`, `codegen-units=1`），预热后按固定时间窗测量。\n\n");

    md.push_str("| 元数据 | 值 |\n|---|---|\n");
    md.push_str(&format!(
        "| 运行器 OS | `{}` |\n",
        env::var("RUNNER_OS").unwrap_or_else(|_| env::consts::OS.to_string())
    ));
    md.push_str(&format!("| CPU | `{}` |\n", cpu_model()));
    md.push_str(&format!("| 工具链 | `{}` |\n", rustc_version()));
    md.push_str(&format!(
        "| 提交 | `{}` (`{}`) |\n",
        commit_sha(),
        ref_name()
    ));
    md.push_str(&format!(
        "| 工作流运行 | `{}` |\n",
        env::var("GITHUB_RUN_ID").unwrap_or_else(|_| "local run".to_string())
    ));
    md.push_str(&format!("| 生成时间 | `{}` |\n", generated_at()));
    md.push('\n');

    // ---- Section 1: serde ecosystem comparison ---------------------------
    md.push_str("## 1. Serde 生态对比（encode / decode，MB/s）\n\n");
    md.push_str("| 格式 | nextjson 编码 | nextjson 解码 | serde 编码 | serde 解码 | 编码比(nj/serde) | 解码比(nj/serde) |\n");
    md.push_str("|---|---|---|---|---|---|---|\n");
    for fmt in FORMAT_ORDER {
        let Some(entries) = by_format.get(*fmt) else {
            continue;
        };
        let nj = entries.iter().find(|(i, _)| i == "nextjson");
        let serde = entries.iter().find(|(i, _)| i == "serde");
        let simd = entries.iter().find(|(i, _)| i == "simd");
        if nj.is_none() && serde.is_none() {
            continue;
        }
        let mut line = format!("| `{fmt}` ");
        let fmt_col = |entry: Option<&(String, Row)>| match entry {
            Some((_, r)) => format!("| {:.1} | {:.1} ", r.encode_mbps, r.decode_mbps),
            None => "| — | — ".to_string(),
        };
        line.push_str(&fmt_col(nj));
        line.push_str(&fmt_col(serde));
        match (nj, serde) {
            (Some((_, a)), Some((_, b))) if b.encode_mbps > 0.0 && b.decode_mbps > 0.0 => {
                line.push_str(&format!(
                    "| {:.2}x | {:.2}x |\n",
                    a.encode_mbps / b.encode_mbps,
                    a.decode_mbps / b.decode_mbps
                ));
            }
            _ => line.push_str("| — | — |\n"),
        }
        md.push_str(&line);
        // Note simd-json row when present (it has no direct pair column).
        if let Some((_, r)) = simd {
            md.push_str(&format!(
                "| `{fmt}` *(simd-json)* | — | — | {:.1} | {:.1} | — | — |\n",
                r.encode_mbps, r.decode_mbps
            ));
        }
    }
    md.push_str("\n> 比值为 nextjson ÷ serde（>1 表示 nextjson 更快）。`bincode` 仅 serde 侧有实现（`nextjson` 无 bincode codec），单独列出。\n");
    if let Some(entries) = by_format.get("bincode") {
        for (_, r) in entries {
            md.push_str(&format!(
                "> - `bincode`（serde only）：{} 字节；编码 {:.1} MB/s；解码 {:.1} MB/s\n",
                r.size, r.encode_mbps, r.decode_mbps
            ));
        }
    }
    md.push('\n');

    // ---- Section 2: nextjson full format matrix --------------------------
    md.push_str("## 2. NextJson 全格式吞吐矩阵（14 格式，encode / decode）\n\n");
    md.push_str("| 格式 | size(字节) | 编码 MB/s | 解码 MB/s |\n|---|---|---|---|\n");
    for row in &format_rows {
        md.push_str(&format!(
            "| `{}` | {} | {:.1} | {:.1} |\n",
            row.name, row.size, row.encode_mbps, row.decode_mbps
        ));
    }
    md.push('\n');

    // ---- Section 3: serde ecosystem raw rows -----------------------------
    md.push_str("## 3. Serde 对比原始行\n\n");
    md.push_str("| case | size(字节) | 编码 MB/s | 解码 MB/s |\n|---|---|---|---|\n");
    for row in &serde_rows {
        md.push_str(&format!(
            "| `{}` | {} | {:.1} | {:.1} |\n",
            row.name, row.size, row.encode_mbps, row.decode_mbps
        ));
    }
    md.push('\n');

    // ---- Section 4: methodology ------------------------------------------
    md.push_str(
        "## 4. 方法学\n\n\
- **构建**：`--release`，workspace `[profile.release]`（`opt-level=3`, `lto=\"thin\"`, `codegen-units=1`）。\n\
- **测量**：每个 case 先跑 500 次预热（含 decode），再按 `NEXTJSON_BENCH_MS`（默认 2000ms）时间窗计数 encode 与 decode 的 ops，取吞吐量（MB/s = ops × size / 1e6）。\n\
- **同一进程**：nextjson 与 serde 使用相同的 fixture、相同的预热与测量循环，直接可比。\n\
- **nextjson `simd` feature**：本报告中的 nextjson 数字启用了 `simd`（x86-64：SSE2 基线 + AVX2 运行时检测；aarch64：NEON）。`simd` 是 opt-in 特性，默认构建保持 `#![deny(unsafe_code)]` 零 unsafe。\n\
- **诚实声明**：\n\
  - `simd-json` 的 serde 解码要求可变输入缓冲（原地解析），因此其每次解码迭代包含一次 `to_vec()` 拷贝，该拷贝计入其耗时。\n\
  - 共享 CI 硬件的吞吐会随负载与散热漂移；本报告是单次运行的记录，不是严格的性能基准判决。\n\
  - `bincode` 无 nextjson 对应实现；TOML/BSON 需文档根、postcard 拒有符号标量，这三者使用 `Config` fixture 而非 `Vec<Record>`。\n",
    );

    let out_path = Path::new(&out);
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    fs::write(out_path, md).expect("write report");
    eprintln!("wrote {out}");
}

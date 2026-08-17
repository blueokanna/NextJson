//! GitHub Actions benchmark report generator.
//!
//! Merges the two benchmark outputs into one `Github_Action_Benchmark.md`:
//!
//! 1. `--serde-csv`: the standalone matrix benchmark output (sections
//!    `# throughput` and `# security`, CSV rows with a single header per
//!    section).
//! 2. `--format-csv`: the in-workspace 14-format matrix.
//!
//! The throughput section is grouped per data-shape fixture; each row shows
//! encode/decode MB/s for nextjson and the serde implementation, the ratio,
//! and per-operation latency (ns/op). The security section reports rejection
//! latency and whether every malicious input is rejected without panicking.
//!
//! Usage:
//! ```text
//! cargo run --release --bin bench_report -- \
//!   --serde-csv serde.csv --format-csv format.csv [--out Github_Action_Benchmark.md]
//! ```

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
    encode_ops: f64,
    decode_ops: f64,
}

#[derive(Clone, Debug)]
struct SecRow {
    name: String,
    bytes: u64,
    nj_us: f64,
    sd_us: f64,
    nj_rejects: bool,
    sd_rejects: bool,
    nj_panics: u64,
    sd_panics: u64,
}

fn parse_bool(s: &str) -> bool {
    matches!(s.trim(), "true" | "1")
}

/// Parse the standalone matrix CSV. Returns (throughput rows, security rows).
fn parse_sections(path: &str) -> (Vec<Row>, Vec<SecRow>) {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path));
    let mut throughput = Vec::new();
    let mut security = Vec::new();
    let mut section = "";
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('#') {
            section = name.trim();
            continue;
        }
        if line.starts_with("case,")
            || line.starts_with("format,")
            || line.starts_with("security_case,")
        {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        match section {
            "security" => {
                if cols.len() < 8 {
                    eprintln!(
                        "warning: {path}:{} malformed security row: {}",
                        lineno + 1,
                        line
                    );
                    continue;
                }
                security.push(SecRow {
                    name: cols[0].trim().to_string(),
                    bytes: cols[1].trim().parse().unwrap_or(0),
                    nj_us: cols[2].trim().parse().unwrap_or(0.0),
                    sd_us: cols[3].trim().parse().unwrap_or(0.0),
                    nj_rejects: parse_bool(cols[4]),
                    sd_rejects: parse_bool(cols[5]),
                    nj_panics: cols[6].trim().parse().unwrap_or(0),
                    sd_panics: cols[7].trim().parse().unwrap_or(0),
                });
            }
            _ => {
                if cols.len() < 6 {
                    eprintln!(
                        "warning: {path}:{} malformed row ({} cols): {}",
                        lineno + 1,
                        cols.len(),
                        line
                    );
                    continue;
                }
                throughput.push(Row {
                    name: cols[0].trim().to_string(),
                    size: cols[1].trim().parse().unwrap_or(0),
                    encode_ops: cols[2].trim().parse().unwrap_or(0.0),
                    encode_mbps: cols[3].trim().parse().unwrap_or(0.0),
                    decode_ops: cols[4].trim().parse().unwrap_or(0.0),
                    decode_mbps: cols[5].trim().parse().unwrap_or(0.0),
                });
            }
        }
    }
    (throughput, security)
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

fn git(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn commit_sha() -> String {
    env::var("GITHUB_SHA")
        .ok()
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "n/a".to_string())
}

fn ref_name() -> String {
    env::var("GITHUB_REF_NAME")
        .ok()
        .or_else(|| git(&["branch", "--show-current"]))
        .unwrap_or_else(|| "n/a".to_string())
}

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

/// Split `impl_fixture_format` (or `impl_format` for bincode) into parts.
fn split_case(name: &str) -> (String, String, String) {
    let parts: Vec<&str> = name.split('_').collect();
    if parts.is_empty() {
        return (String::new(), String::new(), name.to_string());
    }
    let impl_ = parts[0].to_string();
    if name == "nextjson_bincode(na)" {
        return (impl_, "bincode".to_string(), "na".to_string());
    }
    if name == "serde_bincode" {
        // bincode is only benched on the `records` fixture; report it there.
        return (impl_, "records".to_string(), "bincode".to_string());
    }
    match parts.len() {
        2 => (impl_, String::new(), parts[1].to_string()),
        _ => (impl_, parts[1].to_string(), parts[2..].join("_")),
    }
}

const FIXTURE_ORDER: &[&str] = &[
    "records",
    "numbers",
    "unicode",
    "integers",
    "longtexts",
    "bigarray",
    "smallobj",
    "deep",
    "config",
];

const FORMAT_ORDER: &[&str] = &[
    "json", "msgpack", "cbor", "ubjson", "smile", "json5", "yaml", "ron", "toml", "bson",
    "postcard", "ndjson", "ini", "edn", "bincode",
];

const FIXTURE_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "records",
        "混合类型记录 ×256（id/bool/f64/字符串/标签/样本数组）",
    ),
    (
        "numbers",
        "浮点密集：64 行 ×16 个 f64（多数量级/正负号/小数形态）",
    ),
    (
        "unicode",
        "Unicode 密集：256 条多字节 UTF-8（CJK/emoji/组合字符）",
    ),
    ("integers", "纯整数：256 条记录，无浮点（隔离整数格式化）"),
    (
        "longtexts",
        "长文本：32 条记录 ×~1.5KiB 正文（SIMD 扫描 + 分块拷贝路径）",
    ),
    ("bigarray", "大数组：100,000 个 u64（容器帧与 memcpy 主导）"),
    (
        "smallobj",
        "小对象：100,000 个 {id:u32, ok:bool}（每对象固定开销主导）",
    ),
    ("deep", "深层嵌套：24 层动态 Value 数组（递归容器深度）"),
    ("config", "文档形态：TOML/BSON/postcard 用的小配置对象"),
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

    let (throughput, security) = parse_sections(&serde_csv);
    let format_rows = parse_sections(&format_csv).0;

    let mut by_fixture: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for row in &throughput {
        let (impl_, fixture, format) = split_case(&row.name);
        if impl_ == "nextjson" && fixture == "bincode" && format == "na" {
            continue;
        }
        by_fixture.entry(fixture).or_default().push(row.clone());
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

    // ---- Section 1: serde comparison grouped by data shape ----------------
    md.push_str("## 1. Serde 生态对比（按数据特征分组）\n\n");
    for fixture in FIXTURE_ORDER {
        let Some(rows) = by_fixture.get(*fixture) else {
            continue;
        };
        let desc = FIXTURE_DESCRIPTIONS
            .iter()
            .find(|(f, _)| *f == *fixture)
            .map(|(_, d)| *d)
            .unwrap_or("");
        md.push_str(&format!("### `{fixture}` — {desc}\n\n"));
        md.push_str("| 格式 | nextjson 编 | nextjson 解 | serde 编 | serde 解 | simd-json 编 | simd-json 解 | 编码比 | 解码比 | nextjson ns/op(编) | serde ns/op(编) |\n");
        md.push_str("|---|---|---|---|---|---|---|---|---|---|---|\n");
        for format in FORMAT_ORDER {
            let find = |impl_: &str| {
                rows.iter()
                    .find(|r| split_case(&r.name).0 == impl_ && split_case(&r.name).2 == *format)
            };
            let nj = find("nextjson");
            let serde = find("serde");
            let simd = find("simd");
            if nj.is_none() && serde.is_none() && simd.is_none() {
                continue;
            }
            let cell = |r: Option<&Row>| match r {
                Some(row) => format!("| {:.1} | {:.1} ", row.encode_mbps, row.decode_mbps),
                None => "| — | — ".to_string(),
            };
            let mut line = format!("| `{format}` ");
            line.push_str(&cell(nj));
            line.push_str(&cell(serde));
            line.push_str(&cell(simd));
            match (nj, serde) {
                (Some(a), Some(b)) if b.encode_mbps > 0.0 && b.decode_mbps > 0.0 => {
                    line.push_str(&format!(
                        "| {:.2}x | {:.2}x ",
                        a.encode_mbps / b.encode_mbps,
                        a.decode_mbps / b.decode_mbps
                    ));
                }
                _ => line.push_str("| — | — "),
            }
            match nj {
                Some(row) if row.encode_ops > 0.0 => {
                    line.push_str(&format!("| {:.0} ", 1e9 / row.encode_ops));
                }
                _ => line.push_str("| — "),
            }
            match serde {
                Some(row) if row.encode_ops > 0.0 => {
                    line.push_str(&format!("| {:.0} |\n", 1e9 / row.encode_ops));
                }
                _ => line.push_str("| — |\n"),
            }
            md.push_str(&line);
        }
        md.push('\n');
    }
    md.push_str("> 比值 = nextjson ÷ serde（>1 表示 nextjson 更快）。`bincode` 仅 serde 侧有实现。`simd-json` 与 `serde_json` 同一行并排：**simd-json 的解码每次迭代含一次 `to_vec()` 拷贝（原地解析），该拷贝计入其耗时**。\n\n");

    // ---- Section 2: security / robustness ---------------------------------
    md.push_str("## 2. 安全与鲁棒性对比（恶意输入拒绝）\n\n");
    md.push_str("| 攻击向量 | 输入字节 | nextjson 拒绝 | serde 拒绝 | nextjson us/op | serde us/op | nextjson 加速 | nextjson panic | serde panic |\n");
    md.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for row in &security {
        let speedup = if row.sd_us > 0.0 {
            row.sd_us / row.nj_us
        } else {
            0.0
        };
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | {:.2} | {:.2} | {:.1}x | {} | {} |\n",
            row.name,
            row.bytes,
            mark(row.nj_rejects),
            mark(row.sd_rejects),
            row.nj_us,
            row.sd_us,
            speedup,
            mark(row.nj_panics == 0),
            mark(row.sd_panics == 0),
        ));
    }
    md.push_str("\n> 所有恶意输入必须被**拒绝且不 panic**（任何 panic 都是安全漏洞）。拒绝速度 = 每个输入的平均微秒数（越低越好）。\n");
    md.push_str("> **浮点精度对比**：serde_json 1.0.151 的解析器对部分 17 位有效数字小数存在 1-ULP 误差（写对读错，如 `-0.012750000000000001` → `-0.01275`）；nextjson 与 simd-json 均完全 round-trip。\n\n");

    // ---- Section 3: nextjson full format matrix ---------------------------
    md.push_str(&format!(
        "## 3. NextJson 全格式吞吐矩阵（{} 格式，encode / decode）\n\n",
        format_rows.len()
    ));
    md.push_str("| 格式 | size(字节) | 编码 MB/s | 解码 MB/s | 编码 ns/op | 解码 ns/op |\n|---|---|---|---|---|---|\n");
    for row in &format_rows {
        let enc_ns = if row.encode_ops > 0.0 {
            1e9 / row.encode_ops
        } else {
            0.0
        };
        let dec_ns = if row.decode_ops > 0.0 {
            1e9 / row.decode_ops
        } else {
            0.0
        };
        md.push_str(&format!(
            "| `{}` | {} | {:.1} | {:.1} | {:.0} | {:.0} |\n",
            row.name, row.size, row.encode_mbps, row.decode_mbps, enc_ns, dec_ns
        ));
    }
    md.push('\n');

    // ---- Section 4: methodology -------------------------------------------
    md.push_str(
        "## 4. 方法学\n\n\
- **构建**：`--release`，workspace `[profile.release]`（`opt-level=3`, `lto=\"thin\"`, `codegen-units=1`）。\n\
- **测量**：每个 case 先做时间有界的预热（至少 10 次迭代、再稳定 ~250ms，避免大 fixture 或慢 codec 的固定 500 次预热拖垮整轮），再按 `NEXTJSON_BENCH_MS`（默认 2000ms）时间窗计数 encode 与 decode 的 ops，取吞吐量（MB/s = ops × size / 1e6）与延迟（ns/op = 1e9 / ops）。\n\
- **同一进程**：nextjson 与 serde 使用相同的 fixture、相同的预热与测量循环，直接可比。\n\
- **全量矩阵**：每个数据形状 fixture 对所有能表示它的格式都做对比（JSON 三引擎 + MessagePack/CBOR/JSON5/YAML/RON/UBJSON/SMILE/NDJSON/TOML/BSON/postcard/bincode），不再是只对比 json/msgpack/cbor 三个格式。\n\
- **数据形状**：9 个 fixture 覆盖混合记录、浮点密集、Unicode、纯整数、长文本、大数组、小对象、深层嵌套、文档形态，避免单 fixture 偏差。\n\
- **nextjson `simd` feature**：本报告中的 nextjson 数字启用了 `simd`（x86-64：SSE2 基线 + AVX2 运行时检测；aarch64：NEON）。`simd` 是 opt-in 特性，默认构建保持 `#![deny(unsafe_code)]` 零 unsafe。\n\
- **诚实声明**：\n\
  - `simd-json` 的 serde 解码要求可变输入缓冲（原地解析），因此其每次解码迭代包含一次 `to_vec()` 拷贝，该拷贝计入其耗时。\n\
  - serde_json 1.0.151 对 17 位有效数字浮点的解析有 1-ULP 误差（见第 2 节）；其 `numbers` fixture 的 self-check 使用 1-ULP 容差并如实报告。\n\
  - simd-json 的浮点序列化器对超出其格式化缓冲的极值（如次正规数 `5e-324`）报错，故 `numbers` fixture 的指数范围限定在 -6..+6。\n\
  - **UBJSON / SMILE 的 serde 侧缺口**：`serde_ubj` 0.2.0 的**解码器只支持有符号 32/64 位整数**（`u8/u16/u32/u64/i128` 全部返回 `Unsupported`），`serde-smile` 0.2.2 对 `u64` 的处理同样不完整（`id: u64` 等字段无法往返），而本报告的所有 typed fixture 都含无符号字段——因此这两个格式只呈现 nextjson 的完整 encode+decode 行，不呈现 serde 对比行；这是生态 crate 的真实能力限制，不是测量省略。nextjson 的 UBJSON/SMILE 字节级互操作（计数式/强类型容器、共享字符串/键名引用等）由 `formats_new` 集成测试按规范字节验证。\n\
  - 共享 CI 硬件的吞吐会随负载与散热漂移；本报告是单次运行的记录，不是严格的性能基准判决。\n\
  - `bincode` 无 nextjson 对应实现；TOML/BSON 需文档根、postcard 拒有符号标量，这三者使用 `config` fixture。\n",
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

fn mark(b: bool) -> &'static str {
    if b {
        "✓"
    } else {
        "✗"
    }
}

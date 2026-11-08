//! GitHub Actions benchmark report generator.
//!
//! Merges the two benchmark outputs into one `Github_Action_Benchmark.md`:
//!
//! 1. `--serde-csv`: the standalone matrix benchmark output (sections
//!    `# throughput` and `# security`, CSV rows with a single header per
//!    section).
//! 2. `--format-csv`: the in-workspace format and API-path matrix.
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

/// Parse a complete benchmark CSV. Invalid or duplicate rows are errors:
/// a report must never replace missing measurements with invented zeroes.
fn parse_csv(text: &str, source: &str) -> Result<(Vec<Row>, Vec<SecRow>), String> {
    let mut throughput = Vec::new();
    let mut security = Vec::new();
    let mut section = "throughput";
    let mut seen = std::collections::BTreeSet::new();
    for (lineno, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let error = |message: &str| format!("{source}:{}: {message}", lineno + 1);
        if let Some(name) = line.strip_prefix('#') {
            section = match name.trim() {
                "throughput" => "throughput",
                "security" => "security",
                _ => return Err(error("unknown CSV section")),
            };
            continue;
        }
        if line == "case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps"
            || line == "format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps"
        {
            if section != "throughput" {
                return Err(error("throughput header inside security section"));
            }
            continue;
        }
        if line == "security_case,bytes,nextjson_us_per_op,serde_us_per_op,nextjson_rejects,serde_rejects,nextjson_panics,serde_panics" {
            if section != "security" { return Err(error("security header outside security section")); }
            continue;
        }
        let cols: Vec<_> = line.split(',').map(str::trim).collect();
        let expected = if section == "security" { 8 } else { 6 };
        if cols.len() != expected {
            return Err(error("wrong column count"));
        }
        if cols[0].is_empty()
            || !cols[0]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-()".contains(&b))
        {
            return Err(error("invalid case name"));
        }
        if !seen.insert((section, cols[0])) {
            return Err(error("duplicate case"));
        }
        let integer = |index: usize| -> Result<u64, String> {
            cols[index]
                .parse()
                .map_err(|_| error("invalid unsigned integer"))
        };
        let number = |index: usize, positive: bool| -> Result<f64, String> {
            let value: f64 = cols[index]
                .parse()
                .map_err(|_| error("invalid measurement"))?;
            if !value.is_finite() || value < 0.0 || (positive && value == 0.0) {
                return Err(error("measurement must be finite and in range"));
            }
            Ok(value)
        };
        let boolean = |index: usize| -> Result<bool, String> {
            match cols[index] {
                "true" | "1" => Ok(true),
                "false" | "0" => Ok(false),
                _ => Err(error("invalid boolean")),
            }
        };
        if section == "security" {
            security.push(SecRow {
                name: cols[0].into(),
                bytes: integer(1)?,
                nj_us: number(2, false)?,
                sd_us: number(3, false)?,
                nj_rejects: boolean(4)?,
                sd_rejects: boolean(5)?,
                nj_panics: integer(6)?,
                sd_panics: integer(7)?,
            });
        } else {
            throughput.push(Row {
                name: cols[0].into(),
                size: integer(1)?,
                encode_ops: number(2, true)?,
                encode_mbps: number(3, false)?,
                decode_ops: number(4, true)?,
                decode_mbps: number(5, false)?,
            });
        }
    }
    if throughput.is_empty() && security.is_empty() {
        return Err(format!("{source}: no measurements"));
    }
    Ok((throughput, security))
}

fn parse_sections(path: &str) -> Result<(Vec<Row>, Vec<SecRow>), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    parse_csv(&text, path)
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
    if let Ok(sha) = env::var("GITHUB_SHA") {
        return sha;
    }
    let mut sha = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "n/a".to_string());
    if git(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty()) {
        sha.push_str(" + working-tree changes");
    }
    sha
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| format!("Unix timestamp {} (UTC)", elapsed.as_secs()))
        .unwrap_or_else(|_| "unavailable".to_string())
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
    if let Err(error) = run() {
        eprintln!("benchmark report: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
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
                out = args.get(i).cloned().ok_or("--out requires a path")?;
            }
            other => {
                return Err(format!("unknown argument {other}"));
            }
        }
        i += 1;
    }
    let format_csv = format_csv.ok_or("--format-csv is required")?;
    let serde_csv = serde_csv.ok_or("--serde-csv is required")?;

    let (throughput, security) = parse_sections(&serde_csv)?;
    let (format_rows, unexpected_security) = parse_sections(&format_csv)?;
    if throughput.is_empty()
        || security.is_empty()
        || format_rows.is_empty()
        || !unexpected_security.is_empty()
    {
        return Err("missing or misplaced benchmark sections".into());
    }

    let mut by_fixture: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for row in &throughput {
        let (impl_, fixture, format) = split_case(&row.name);
        if !["nextjson", "serde", "simd"].contains(&impl_.as_str())
            || !FIXTURE_ORDER.contains(&fixture.as_str())
            || !FORMAT_ORDER.contains(&format.as_str())
        {
            return Err(format!("unrecognized throughput case: {}", row.name));
        }
        by_fixture.entry(fixture).or_default().push(row.clone());
    }

    let mut md = String::new();
    md.push_str("# 基准报告 — NextJson vs Serde 生态\n\n");
    md.push_str(
        "> 本报告由 `bench_report` 从原始 CSV 生成；GitHub Actions 运行时会上传 CSV 与报告。\n",
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
    md.push_str(&format!(
        "| 当前进程 NEXTJSON_BENCH_MS | `{}`（原始 CSV 不含时间窗，需与测量环境核对） |\n",
        env::var("NEXTJSON_BENCH_MS").unwrap_or_else(|_| "unset".to_string())
    ));
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
        let speedup = if row.nj_us > 0.0
            && row.sd_us > 0.0
            && row.nj_rejects
            && row.sd_rejects
            && row.nj_panics == 0
            && row.sd_panics == 0
        {
            format!("{:.1}x", row.sd_us / row.nj_us)
        } else {
            "—".to_string()
        };
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | {:.2} | {:.2} | {} | {} | {} |\n",
            row.name,
            row.bytes,
            mark(row.nj_rejects),
            mark(row.sd_rejects),
            row.nj_us,
            row.sd_us,
            speedup,
            row.nj_panics,
            row.sd_panics,
        ));
    }
    md.push_str("\n> 表中记录本次运行的拒绝结果和 panic 次数，不代表完整安全审计。只有双方均拒绝且未 panic 时才比较拒绝耗时；零耗时不计算比值。\n\n");

    // ---- Section 3: nextjson full format matrix ---------------------------
    md.push_str(&format!(
        "## 3. NextJson 吞吐矩阵（{} 测量路径，encode / decode）\n\n",
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
        "## 4. 方法与边界\n\n\
- release 构建，先预热，再按 NEXTJSON_BENCH_MS 分别测量编码与解码。原生矩阵固定预热 200 次；生态对照使用时间有界预热。\n\
- MB/s = 每秒操作数 × 编码字节数 / 1e6；ns/op = 1e9 / 每秒操作数。不同格式的编码体积不同，不能仅凭 MB/s 排名。\n\
- simd-json 的解码循环包含输入缓冲复制，其时间计入解码结果。\n\
- numbers 用例的 serde_json 与 Serde NDJSON 往返自检允许最多 1 ULP 浮点误差，NextJson 与 simd-json 使用相等检查；这些自检条件不同，不能据此宣称双方具有相同的数值保真度。实际误差诊断写入运行日志。\n\
- json 是默认校验路径；json-checked-explicit 与 json-trusted-explicit 使用相同 writer 缓冲方式比较策略；json-reader-limited 测量受输入配额约束的流式读取。\n\
- 预读、校验和表示能力均属于测量条件。缺少对照行表示本次没有相应测量，不把缺失值写成零，也不自动归因于第三方库缺陷。\n\
- 本报告只陈述输入 CSV 中的观测，不从吞吐数据推断浮点精度或安全结论。\n\
- 共享运行器的负载、频率和温度会影响结果；单次比值不是性能保证，需用目标硬件和实际负载复测。\n",
    );

    let out_path = Path::new(&out);
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create output directory: {e}"))?;
        }
    }
    fs::write(out_path, md).map_err(|e| format!("cannot write report: {e}"))?;
    eprintln!("wrote {out}");
    Ok(())
}

fn mark(b: bool) -> &'static str {
    if b {
        "✓"
    } else {
        "✗"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROW: &str = "json-reader-limited,10,200,0.002,100,0.001";

    #[test]
    fn keeps_hyphenated_paths_and_utf8_bom() {
        let text = format!(
            "\u{feff}format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps\n{ROW}\n"
        );
        let (rows, security) = parse_csv(&text, "fixture").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "json-reader-limited");
        assert!(security.is_empty());
    }

    #[test]
    fn rejects_malformed_and_duplicate_rows() {
        for text in [
            format!("{ROW},extra"),
            "json,10,broken,1,2,3".into(),
            format!("{ROW}\n{ROW}"),
            "json,10,1".into(),
        ] {
            let error = parse_csv(&text, "fixture.csv").unwrap_err();
            assert!(error.starts_with("fixture.csv:"));
        }
    }

    #[test]
    fn rejects_non_finite_negative_and_missing_measurements() {
        for number in ["NaN", "inf", "-1", "-inf", "", "0"] {
            let text = format!("json,10,{number},1,2,3");
            assert!(parse_csv(&text, "fixture").is_err(), "{number}");
        }
        for text in [
            "",
            "# security\n",
            "# unknown\n",
            "format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps\n",
        ] {
            assert!(parse_csv(text, "fixture").is_err());
        }
    }

    #[test]
    fn preserves_failed_security_outcomes_and_zero_timer_resolution() {
        let (_, rows) = parse_csv("# security\ncase,10,0,0.01,false,true,2,0", "fixture").unwrap();
        assert!(!rows[0].nj_rejects);
        assert_eq!(rows[0].nj_panics, 2);
        assert_eq!(rows[0].nj_us, 0.0);
        assert!(parse_csv("# security\ncase,10,1,2,unknown,true,0,0", "fixture").is_err());
    }
}

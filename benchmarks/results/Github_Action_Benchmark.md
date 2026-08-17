# GitHub Actions 基准报告 — NextJson vs Serde 生态

> 本报告由 `.github/workflows/benchmark.yml` 自动生成，原始 CSV 随工作流产物一起上传。
> 所有数字均为 **release** 构建（`opt-level=3`, `lto=thin`, `codegen-units=1`），预热后按固定时间窗测量。

| 元数据 | 值 |
|---|---|
| 运行器 OS | `Linux` |
| CPU | `: AMD EPYC 9V74 80-Core Processor` |
| 工具链 | `rustc 1.97.1 (8bab26f4f 2026-07-14)` |
| 提交 | `f0fe60ddf9d5e3a30b758b4378601b7f12911f51` (`main`) |
| 工作流运行 | `32012050134` |
| 生成时间 | `see workflow run #32012050134` |

## 1. Serde 生态对比（encode / decode，MB/s）

| 格式 | nextjson 编码 | nextjson 解码 | serde 编码 | serde 解码 | 编码比(nj/serde) | 解码比(nj/serde) |
|---|---|---|---|---|---|---|
| `json` | 528.3 | 263.5 | 812.6 | 364.2 | 0.65x | 0.72x |
| `json` *(simd-json)* | — | — | 728.5 | 342.0 | — | — |
| `longtext_json` | 6302.6 | 2424.7 | 1587.2 | 2249.8 | 3.97x | 1.08x |
| `longtext_json` *(simd-json)* | — | — | 6784.9 | 1865.6 | — | — |
| `json5` | 493.3 | 74.5 | 149.5 | 14.9 | 3.30x | 5.01x |
| `yaml` | 252.0 | 72.0 | 70.2 | 33.2 | 3.59x | 2.17x |
| `ron` | 332.4 | 110.2 | 243.3 | 119.4 | 1.37x | 0.92x |
| `msgpack` | 650.2 | 275.2 | 675.4 | 287.0 | 0.96x | 0.96x |
| `cbor` | 87.5 | 32.4 | 513.2 | 105.2 | 0.17x | 0.31x |
| `bincode` | — | — | 8527.9 | 1095.0 | — | — |
| `toml` | 92.3 | 59.5 | 35.1 | 29.2 | 2.63x | 2.04x |
| `bson` | 616.5 | 408.9 | 493.0 | 251.3 | 1.25x | 1.63x |
| `postcard` | 427.9 | 347.1 | 466.8 | 415.3 | 0.92x | 0.84x |
| `intonly` | 620.0 | 267.6 | 883.1 | 354.8 | 0.70x | 0.75x |

> 比值为 nextjson ÷ serde（>1 表示 nextjson 更快）。`bincode` 仅 serde 侧有实现（`nextjson` 无 bincode codec），单独列出。
> - `bincode`（serde only）：57352 字节；编码 8527.9 MB/s；解码 1095.0 MB/s

## 2. NextJson 全格式吞吐矩阵（14 格式，encode / decode）

| 格式 | size(字节) | 编码 MB/s | 解码 MB/s |
|---|---|---|---|
| `json` | 23636 | 513.9 | 257.7 |
| `json5` | 23636 | 507.6 | 73.2 |
| `hjson` | 23636 | 509.0 | 121.0 |
| `yaml` | 51283 | 250.5 | 61.8 |
| `ron` | 27347 | 322.4 | 106.4 |
| `sexpr` | 22313 | 299.7 | 149.5 |
| `cbor` | 17090 | 87.1 | 32.7 |
| `msgpack` | 16867 | 648.1 | 255.4 |
| `pickle` | 22691 | 1090.3 | 59.9 |
| `toml` | 27219 | 81.3 | 56.9 |
| `bson` | 32037 | 403.3 | 337.7 |
| `bencode` | 7300 | 210.1 | 155.0 |
| `postcard` | 5231 | 433.9 | 176.7 |
| `csv` | 2771 | 104.3 | 53.9 |

## 3. Serde 对比原始行

| case | size(字节) | 编码 MB/s | 解码 MB/s |
|---|---|---|---|
| `nextjson_json` | 48063 | 528.3 | 263.5 |
| `serde_json` | 48063 | 812.6 | 364.2 |
| `simd_json` | 48063 | 728.5 | 342.0 |
| `nextjson_longtext_json` | 52579 | 6302.6 | 2424.7 |
| `serde_longtext_json` | 52579 | 1587.2 | 2249.8 |
| `simd_longtext_json` | 52579 | 6784.9 | 1865.6 |
| `nextjson_json5` | 48063 | 493.3 | 74.5 |
| `serde_json5` | 47935 | 149.5 | 14.9 |
| `nextjson_yaml` | 103358 | 252.0 | 72.0 |
| `serde_yaml` | 65470 | 70.2 | 33.2 |
| `nextjson_ron` | 55486 | 332.4 | 110.2 |
| `serde_ron` | 44991 | 243.3 | 119.4 |
| `nextjson_msgpack` | 34019 | 650.2 | 275.2 |
| `serde_msgpack` | 25251 | 675.4 | 287.0 |
| `nextjson_cbor` | 34370 | 87.5 | 32.4 |
| `serde_cbor` | 32067 | 513.2 | 105.2 |
| `nextjson_bincode(na)` | 0 | 0.0 | 0.0 |
| `serde_bincode` | 57352 | 8527.9 | 1095.0 |
| `nextjson_toml` | 121 | 92.3 | 59.5 |
| `serde_toml` | 120 | 35.1 | 29.2 |
| `nextjson_bson` | 146 | 616.5 | 408.9 |
| `serde_bson` | 154 | 493.0 | 251.3 |
| `nextjson_postcard` | 93 | 427.9 | 347.1 |
| `serde_postcard` | 58 | 466.8 | 415.3 |
| `nextjson_intonly` | 44446 | 620.0 | 267.6 |
| `serde_intonly` | 44446 | 883.1 | 354.8 |

## 4. 方法学

- **构建**：`--release`，workspace `[profile.release]`（`opt-level=3`, `lto="thin"`, `codegen-units=1`）。
- **测量**：每个 case 先跑 500 次预热（含 decode），再按 `NEXTJSON_BENCH_MS`（默认 2000ms）时间窗计数 encode 与 decode 的 ops，取吞吐量（MB/s = ops × size / 1e6）。
- **同一进程**：nextjson 与 serde 使用相同的 fixture、相同的预热与测量循环，直接可比。
- **nextjson `simd` feature**：本报告中的 nextjson 数字启用了 `simd`（x86-64：SSE2 基线 + AVX2 运行时检测；aarch64：NEON）。`simd` 是 opt-in 特性，默认构建保持 `#![deny(unsafe_code)]` 零 unsafe。
- **诚实声明**：
- `simd-json` 的 serde 解码要求可变输入缓冲（原地解析），因此其每次解码迭代包含一次 `to_vec()` 拷贝，该拷贝计入其耗时。
- 共享 CI 硬件的吞吐会随负载与散热漂移；本报告是单次运行的记录，不是严格的性能基准判决。
- `bincode` 无 nextjson 对应实现；TOML/BSON 需文档根、postcard 拒有符号标量，这三者使用 `Config` fixture 而非 `Vec<Record>`。

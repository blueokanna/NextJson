# GitHub Actions 基准报告 — NextJson vs Serde 生态

> 本报告由 `.github/workflows/benchmark.yml` 自动生成，原始 CSV 随工作流产物一起上传。
> 所有数字均为 **release** 构建（`opt-level=3`, `lto=thin`, `codegen-units=1`），预热后按固定时间窗测量。

| 元数据 | 值 |
|---|---|
| 运行器 OS | `Linux` |
| CPU | `: AMD EPYC 9V45 96-Core Processor` |
| 工具链 | `rustc 1.97.1 (8bab26f4f 2026-07-14)` |
| 提交 | `bb909b1336d7e4e553efe2e9ec0d253656f04dd8` (`main`) |
| 工作流运行 | `32014109742` |
| 生成时间 | `see workflow run #32014109742` |

## 1. Serde 生态对比（encode / decode，MB/s）

| 格式 | nextjson 编码 | nextjson 解码 | serde 编码 | serde 解码 | 编码比(nj/serde) | 解码比(nj/serde) |
|---|---|---|---|---|---|---|
| `json` | 716.8 | 301.4 | 1196.2 | 469.9 | 0.60x | 0.64x |
| `json` *(simd-json)* | — | — | 1021.1 | 478.5 | — | — |
| `longtext_json` | 9473.3 | 3482.4 | 3088.6 | 3027.2 | 3.07x | 1.15x |
| `longtext_json` *(simd-json)* | — | — | 10167.7 | 2635.5 | — | — |
| `json5` | 680.1 | 108.2 | 228.1 | 19.5 | 2.98x | 5.54x |
| `yaml` | 354.7 | 110.1 | 103.7 | 46.0 | 3.42x | 2.39x |
| `ron` | 469.0 | 158.9 | 325.7 | 166.4 | 1.44x | 0.96x |
| `msgpack` | 1040.9 | 340.4 | 979.2 | 399.9 | 1.06x | 0.85x |
| `cbor` | 1618.9 | 298.8 | 841.9 | 150.5 | 1.92x | 1.99x |
| `bincode` | — | — | 13954.1 | 1528.5 | — | — |
| `toml` | 138.5 | 80.0 | 49.9 | 38.7 | 2.78x | 2.07x |
| `bson` | 845.9 | 532.6 | 743.0 | 332.7 | 1.14x | 1.60x |
| `postcard` | 627.6 | 452.9 | 650.2 | 621.1 | 0.97x | 0.73x |
| `intonly` | 898.1 | 312.5 | 1380.2 | 396.4 | 0.65x | 0.79x |

> 比值为 nextjson ÷ serde（>1 表示 nextjson 更快）。`bincode` 仅 serde 侧有实现（`nextjson` 无 bincode codec），单独列出。
> - `bincode`（serde only）：57352 字节；编码 13954.1 MB/s；解码 1528.5 MB/s

## 2. NextJson 全格式吞吐矩阵（14 格式，encode / decode）

| 格式 | size(字节) | 编码 MB/s | 解码 MB/s |
|---|---|---|---|
| `json` | 23636 | 708.5 | 331.4 |
| `json5` | 23636 | 703.2 | 107.0 |
| `hjson` | 23636 | 706.2 | 178.6 |
| `yaml` | 51283 | 354.7 | 89.0 |
| `ron` | 27347 | 471.4 | 156.8 |
| `sexpr` | 22313 | 409.6 | 188.1 |
| `cbor` | 16706 | 1462.8 | 280.2 |
| `msgpack` | 16867 | 1043.8 | 300.6 |
| `pickle` | 22691 | 1695.3 | 89.7 |
| `toml` | 27219 | 121.6 | 83.2 |
| `bson` | 32037 | 567.7 | 436.2 |
| `bencode` | 7300 | 291.6 | 215.6 |
| `postcard` | 5231 | 660.2 | 236.7 |
| `csv` | 2771 | 136.7 | 60.7 |

## 3. Serde 对比原始行

| case | size(字节) | 编码 MB/s | 解码 MB/s |
|---|---|---|---|
| `nextjson_json` | 48063 | 716.8 | 301.4 |
| `serde_json` | 48063 | 1196.2 | 469.9 |
| `simd_json` | 48063 | 1021.1 | 478.5 |
| `nextjson_longtext_json` | 52579 | 9473.3 | 3482.4 |
| `serde_longtext_json` | 52579 | 3088.6 | 3027.2 |
| `simd_longtext_json` | 52579 | 10167.7 | 2635.5 |
| `nextjson_json5` | 48063 | 680.1 | 108.2 |
| `serde_json5` | 47935 | 228.1 | 19.5 |
| `nextjson_yaml` | 103358 | 354.7 | 110.1 |
| `serde_yaml` | 65470 | 103.7 | 46.0 |
| `nextjson_ron` | 55486 | 469.0 | 158.9 |
| `serde_ron` | 44991 | 325.7 | 166.4 |
| `nextjson_msgpack` | 34019 | 1040.9 | 340.4 |
| `serde_msgpack` | 25251 | 979.2 | 399.9 |
| `nextjson_cbor` | 33603 | 1618.9 | 298.8 |
| `serde_cbor` | 32067 | 841.9 | 150.5 |
| `nextjson_bincode(na)` | 0 | 0.0 | 0.0 |
| `serde_bincode` | 57352 | 13954.1 | 1528.5 |
| `nextjson_toml` | 121 | 138.5 | 80.0 |
| `serde_toml` | 120 | 49.9 | 38.7 |
| `nextjson_bson` | 146 | 845.9 | 532.6 |
| `serde_bson` | 154 | 743.0 | 332.7 |
| `nextjson_postcard` | 93 | 627.6 | 452.9 |
| `serde_postcard` | 58 | 650.2 | 621.1 |
| `nextjson_intonly` | 44446 | 898.1 | 312.5 |
| `serde_intonly` | 44446 | 1380.2 | 396.4 |

## 4. 方法学

- **构建**：`--release`，workspace `[profile.release]`（`opt-level=3`, `lto="thin"`, `codegen-units=1`）。
- **测量**：每个 case 先跑 500 次预热（含 decode），再按 `NEXTJSON_BENCH_MS`（默认 2000ms）时间窗计数 encode 与 decode 的 ops，取吞吐量（MB/s = ops × size / 1e6）。
- **同一进程**：nextjson 与 serde 使用相同的 fixture、相同的预热与测量循环，直接可比。
- **nextjson `simd` feature**：本报告中的 nextjson 数字启用了 `simd`（x86-64：SSE2 基线 + AVX2 运行时检测；aarch64：NEON）。`simd` 是 opt-in 特性，默认构建保持 `#![deny(unsafe_code)]` 零 unsafe。
- **诚实声明**：
- `simd-json` 的 serde 解码要求可变输入缓冲（原地解析），因此其每次解码迭代包含一次 `to_vec()` 拷贝，该拷贝计入其耗时。
- 共享 CI 硬件的吞吐会随负载与散热漂移；本报告是单次运行的记录，不是严格的性能基准判决。
- `bincode` 无 nextjson 对应实现；TOML/BSON 需文档根、postcard 拒有符号标量，这三者使用 `Config` fixture 而非 `Vec<Record>`。

# Performance

性能数据全部来自可复现的基准，发布要求记录 CPU / OS / `rustc -Vv` / 提交版本 /
测量时长。**这些数字是该项目数据集的基线，不是普遍排名。**

## 基准协议

```text
cargo bench --locked -p nextjson --bench format_comparison
# 正式记录建议：
$env:NEXTJSON_BENCH_MS = "10000"
cargo bench --locked -p nextjson --bench format_comparison
```

- 计时前先验证原生 JSON 强类型往返并断言相等；
- 预热全部路径；
- CSV 输出：`format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps`。

## 全格式吞吐（本开发机，14 格式，2 秒窗口）

Intel i7-11850H, 32GB RAM, Windows 11, Rust 1.97.0：

| format | size | enc_MBps | dec_MBps |
| --- | --- | --- | --- |
| json | 23636 | 305.47 | 141.43 |
| json5 | 23636 | 307.58 | 39.08 |
| hjson | 23636 | 307.52 | 69.86 |
| yaml | 51219 | 118.80 | 31.36 |
| ron | 27283 | 150.12 | 52.13 |
| sexpr | 22249 | 137.13 | 69.68 |
| cbor | 17090 | 66.67 | 19.71 |
| msgpack | 16867 | 533.18 | 148.07 |
| pickle | 22691 | 987.24 | 38.87 |
| toml | 27155 | 39.16 | 28.39 |
| bson | 32037 | 169.91 | 238.64 |
| bencode | 7300 | 93.78 | 109.35 |
| postcard | 5231 | 237.80 | 124.99 |
| csv | 2771 | 51.75 | 26.77 |

解读看**取舍**而非排名：MessagePack/Pickle 编码领先；紧凑二进制（postcard、
bencode、msgpack）比 JSON 小 3-4 倍；TOML/YAML 以吞吐换可读文档形态；CBOR 走
JSON 兼容 profile（含 bignum/float 机制）所以比 msgpack 慢。

## 与 serde 生态的对比（独立 crate）

对比必须引入第三方 crate，而工作区依赖审计禁止第三方源，因此对比放在
**独立于工作区之外**的 `benchmarks/serde-comparison/`（自持 Cargo.lock）。
同一 fixture（256 条 `Record`）、同一预热、同一测量循环、同一进程：

```text
cd benchmarks/serde-comparison && cargo run --release
```

优化后实测（2 秒窗口，单次运行）：

| case | size | enc_MBps | dec_MBps |
| --- | --- | --- | --- |
| nextjson_encode | 48063 | 368.44 | 131.61 |
| serde_json_encode | 48063 | 800.81 | 186.27 |
| nextjson_encode_intonly | 44446 | 389.00 | 126.86 |
| serde_json_encode_intonly | 44446 | 827.80 | 195.62 |

## 为什么 nextjson 比 serde_json 慢（三原因 + release 证据）

**先证明是 release 模式**：debug 构建下 nextjson 反而快于 serde_json（19.36 vs
8.05 MB/s 编码）——因为 debug 关闭了 serde 的优化。这证明所有发布数字都是
`cargo run --release`（`[profile.release]`：`opt-level=3, lto="thin",
codegen-units=1`）。

release 差距的三个可测量原因：

1. **整数格式化把 `u64`/`i64` 加宽成 `u128`（主因）**：宽除法走 compiler-rt
   `__udivti3` libcall，比原生 64 位除法慢数倍。修复：`ser.rs` 新增栈缓冲的
   `write_u64_into(buf, u64)`（硬件 `div`）+ `write_i64_into`（符号处理 +
   `wrapping_neg`）。隔离数据集编码从约 291 → 368 MB/s（+26%），纯整数约
   339 → 389 MB/s（+15%），线格式字节完全不变。
2. **浮点格式化**：nextjson 用 `fmt::Display`/`core::write!`，serde_json 用
   Ryū。去掉浮点只占差距的约 16%——次要因素。
3. **结构性**：serde_json 是单态化专精 JSON 的十年优化热路径（查表数字/预分配
   缓冲）；nextjson 让每个值穿过通用 `FormatEncoder` 契约 + 每值
   `start_value`/栈帧/深度检查，且必须在 13 种格式下正确。

**诚实声明**：当前纯 JSON 差距约 **2.17x 编码**。这是通用多格式契约的代价——
同一份实现同时驱动 13 种其它格式；serde_json 只服务 JSON 一种。

## CI 与可复现性

- CI 只**编译**两个 benchmark crate（`--no-run --locked`），不在共享机器设置
  吞吐阈值；
- 发布结果必须记录 CPU/OS/rustc/提交/时长/完整表格；
- 详细方法论见 `docs/BENCHMARKS.md`（中英双语）。

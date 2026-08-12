# 可复现基准测试

## 中文

`nextjson/benches/format_comparison.rs` 对同一份 128 条记录数据测量所有能表示该
数据的格式的编码/解码吞吐与编码体积（14 种格式）。

计时前先验证原生 JSON 强类型往返并断言相等，再预热全部路径。构建图不包含
第三方 crate（依赖审计强制）。

```text
cargo bench --locked -p nextjson --bench format_comparison
```

默认每条路径测量 2 秒。正式记录建议至少 10 秒：

```powershell
$env:NEXTJSON_BENCH_MS = "10000"
cargo bench --locked -p nextjson --bench format_comparison
```

```bash
NEXTJSON_BENCH_MS=10000 cargo bench --locked -p nextjson --bench format_comparison
```

CSV 输出：

```text
format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps
```

全模型数据（`Vec<Record>`，含 `u64`/`bool`/`f64`/`String`/`Vec`）覆盖 JSON 家族、
RON、S 表达式、CBOR、MessagePack、Pickle。TOML 与 BSON 是文档形态，数组根被包进
表格根；bencode 与 postcard 的线格式没有 float（postcard 还拒绝有符号标量），
因此使用无 float（postcard 为纯无符号）数据；CSV 使用扁平行。

### 示例测量（本开发机，单次运行，Intel i7-11850H, 32GB RAM, Windows 11, Rust 1.97.0）


| format   | size_bytes | encode_ops | encode_MBps | decode_ops | decode_MBps |
| -------- | ---------- | ---------- | ----------- | ---------- | ----------- |
| json     | 23636      | 12924      | 305.47      | 5984       | 141.43      |
| json5    | 23636      | 13013      | 307.58      | 1653       | 39.08       |
| hjson    | 23636      | 13011      | 307.52      | 2956       | 69.86       |
| yaml     | 51219      | 2319       | 118.80      | 612        | 31.36       |
| ron      | 27283      | 5502       | 150.12      | 1911       | 52.13       |
| sexpr    | 22249      | 6163       | 137.13      | 3132       | 69.68       |
| cbor     | 17090      | 3901       | 66.67       | 1153       | 19.71       |
| msgpack  | 16867      | 31611      | 533.18      | 8779       | 148.07      |
| pickle   | 22691      | 43508      | 987.24      | 1713       | 38.87       |
| toml     | 27155      | 1442       | 39.16       | 1045       | 28.39       |
| bson     | 32037      | 5303       | 169.91      | 7449       | 238.64      |
| bencode  | 7300       | 12847      | 93.78       | 14980      | 109.35      |
| postcard | 5231       | 45461      | 237.80      | 23894      | 124.99      |
| csv      | 2771       | 18676      | 51.75       | 9661       | 26.77       |

解读应看**取舍**而非排名：

- MessagePack 与 Pickle 编码吞吐领先；JSON 在可移植性上无可替代。
- 紧凑二进制（postcard、bencode、msgpack）比 JSON 小 3-4 倍。
- TOML/YAML 以吞吐换人可读的文档形态输出。
- 此处的 CBOR 是 JSON 兼容 profile（含 bignum/float 机制），因此比更简单的
  MessagePack 路径慢。

发布结果必须同时记录 CPU、操作系统、`rustc -Vv`、提交版本、测量时长和完整
表格。该基准是可复现的项目基线，不是普遍性能证明。

## 与 serde 生态的对比

对比成熟的 serde 生态必须引入第三方 crate，而工作区 Cargo.lock 的依赖审计
禁止任何第三方源。因此对比放在 **独立于工作区之外的 crate**：
`benchmarks/serde-comparison/`。它有自己的 Cargo.lock（已提交，支持可复现的
`--locked` 构建），主库保持零依赖，审计照常通过；`serde`/`serde_json` 从不进入
工作区依赖图。

运行方式：

```text
cd benchmarks/serde-comparison
cargo run --release
# 可选：NEXTJSON_BENCH_MS=10000 cargo run --release
```

同一数据（256 条 `Record`）、同一预热、同一测量循环、同一进程、同一机器。
int-only 行使用无 float 的 `IntRecord` 模型，隔离出纯整数格式化路径的成本。

优化后（原生宽度整数写出器，本开发机，2 秒窗口，单次运行）：

| case | size_bytes | encode_ops | encode_MBps | decode_ops | decode_MBps |
| --- | --- | --- | --- | --- | --- |
| nextjson_encode | 48063 | 7666 | 368.44 | 2738 | 131.61 |
| serde_json_encode | 48063 | 16662 | 800.81 | 3876 | 186.27 |
| nextjson_encode_intonly | 44446 | 8752 | 389.00 | 2854 | 126.86 |
| serde_json_encode_intonly | 44446 | 18625 | 827.80 | 4401 | 195.62 |

### 为什么 nextjson 比 serde_json 慢——以及「确实是 release 模式」的证据

以上所有数字都来自 `cargo run --release`，使用工作区 `[profile.release]`
（`opt-level = 3`、`lto = "thin"`、`codegen-units = 1`）。对比跑的是 release
而非 debug 是可证明的：在 **debug** 构建下编译器会关闭 serde 的优化，此时
nextjson 反而**快于** serde_json：

| case（debug 构建） | encode_MBps | decode_MBps |
| --- | --- | --- |
| nextjson_encode | 19.36 | 8.39 |
| serde_json_encode | 8.05 | 8.09 |

release 差距来自三个可测量的原因：

1. **整数格式化把 `u64`/`i64` 加宽成 `u128`**（占主导）。宽路径每次除法都要
   调用 compiler-rt 的 `__udivti3` libcall，比原生 64 位除法慢数倍。nextjson
   现在通过原生宽度的 `write_u64_into`/`write_i64_into`（硬件 `div`）写整数，
   隔离数据集上编码吞吐从约 291 提升到 368 MB/s（纯整数从约 339 到 389 MB/s），
   线格式字节完全不变。
2. **浮点格式化**走 `fmt::Display`/`core::write!`，而 serde_json 用 Ryū。
   去掉浮点只占差距的约 16%，所以这是次要因素。
3. **结构性**：serde_json 是单态化、专精 JSON 的热路径（查表数字、预分配
   缓冲），优化了十年；nextjson 让每个值都穿过通用的 `FormatEncoder` 契约，
   每个值都要维护 `start_value()`/栈帧/深度状态，且必须在 13 种格式下保持正确。

当前纯 JSON 数据下编码差距约为 2.17x。请把这些数字当作**该数据集的基线**而
非普遍排名：nextjson 的编码路径是通用格式中立契约，同时驱动 13 种其它格式；
serde_json 是单一专精的 JSON 热路径。下结论前请在自己的硬件与数据上交叉验证。

发布结果必须同时记录 CPU、操作系统、`rustc -Vv`、提交版本、测量时长和完整
表格。CI 只编译两个 benchmark crate，不在共享机器上设置吞吐阈值。

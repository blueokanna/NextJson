# 可复现基准测试

## 中文

`nextjson/benches/format_comparison.rs` 对同一份 128 条记录数据测量所有能表示该
数据的格式的编码/解码吞吐与编码体积（19 种线格式，注册的 21 种中 `envy` 读环境、
`urlform` 只表示扁平 map，故不计入）。

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
- 此处的 CBOR 是 JSON 兼容 profile（原生 codec，定长容器、128 位整数走 bignum 标签、拒绝字节串/非文本键/非有限浮点/未知标签），行为与历史中继实现一致，但没有中继的 JSON 中间往返。

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

快路径优化之后——原生宽度整数写出、两位一除整数输出、原生宽度整数解析、
字节词法器免 Token 标量读取、顶层入口使用信任型 `FastEncoder`（本开发机，
5 秒窗口，单次运行）：

| case                      | size_bytes | encode_ops | encode_MBps | decode_ops | decode_MBps |
| ------------------------- | ---------- | ---------- | ----------- | ---------- | ----------- |
| nextjson_encode           | 48063      | 8447       | 406.01      | 3104       | 149.18      |
| serde_json_encode         | 48063      | 17427      | 837.59      | 3914       | 188.14      |
| nextjson_encode_intonly   | 44446      | 12412      | 551.67      | 3535       | 157.14      |
| serde_json_encode_intonly | 44446      | 20435      | 908.24      | 3692       | 164.09      |

同一台机器上，优化前的 decode 为 131.61 MB/s（主数据）与 126.86 MB/s（纯
整数）。encode 列同时充当同机对照：FastEncoder 之前编码为 361.54 MB/s
（主数据）与 444.52 MB/s（纯整数），信任型编码器加两位一除整数输出把编码
吞吐提升了约 12%（主数据）与约 24%（纯整数）。紧接着的第二次运行测得
nextjson 354.40/147.84（encode/decode，主数据），serde_json 673.23/189.91
——绝对数字随笔记本电源状态漂移，但纯整数编码对 serde_json 的比值两次都
稳定在约 1.64x（信任型编码器之前约 2.1x）。

笔记本的绝对吞吐随电源状态与散热预算波动：同一个二进制在同一台机器上，一个
下午的多次运行里 nextjson 编码出现在约 158 到 278 MB/s（serde_json 约 340
到 460 MB/s），两个库对频率变化的反应不同，比值也在约 1.2x 到 2.9x 之间移动。
因此上表记录的是某一次特定运行而不是平滑平均值；下一节的原语级 A/B 数字
才是可复现的增量。

### 为什么 nextjson 比 serde_json 慢——以及「确实是 release 模式」的证据

以上所有表格数字都来自 `cargo run --release`，使用工作区 `[profile.release]`
（`opt-level = 3`、`lto = "thin"`、`codegen-units = 1`）。对比跑的是 release
而非 debug 是可证明的：在 **debug** 构建下编译器会关闭 serde 的优化，此时
nextjson 反而**快于** serde_json：

| case（debug 构建） | encode_MBps | decode_MBps |
| ------------------ | ----------- | ----------- |
| nextjson_encode    | 19.36       | 8.39        |
| serde_json_encode  | 8.05        | 8.09        |

release 差距来自四个可测量的原因：

1. **整数转换把 `u64`/`i64` 加宽成 `u128`**（编码侧占主导，解码侧显著）。
   编码侧宽路径每次除法都要调用 compiler-rt 的 `__udivti3` libcall，比原生
   64 位除法慢数倍。nextjson 现在通过原生宽度的 `write_u64_into`/
   `write_i64_into`（硬件 `div`）写整数，隔离数据集上编码吞吐从约 291 提升
   到 368 MB/s（纯整数从约 339 到 389 MB/s）。解析器存在同样的加宽：
   `Number::parse` 曾把每个整数都用 `u128` 运算逐位累积（每位的乘加链都很长）。
   现在改为原生宽度的 `parse_u64_fast`/`parse_i64_fast`，仅在真正溢出时才回退
   到 128 位解析器。同进程 A/B 微基准（混合位数、各 2 秒）实测该原语
   5.1 ns/次（快路径）对比 38.2 ns/次（宽路径），原语级降低 7.4 倍。两个方向
   的线格式字节完全不变。
2. **字节词法器的 Token 往返**（解码侧）。通用 token 面为每个值构造完整
   `Token` 枚举（`Number`/`Cow` 载荷 + 消费侧 match）。类型化标量读取
   （`number`、`string`、`bool`、`unit`、`Option` 分派、`skip_value`）现在按
   源字节分派并直接词法化载荷，仅在类型不匹配时回退 token 路径，诊断保持
   逐字节一致。同进程 A/B（各 2 秒）实测 `Decoder::number` 22.0 → 18.0
   ns/次、`Decoder::string` 25.9 → 23.5 ns/次。
3. **浮点格式化**走 `fmt::Display`/`core::write!`，而 serde_json 用 Ryū。
   去掉浮点只占差距的约 16%，所以这是次要因素。
4. **结构性**：serde_json 是单态化、专精 JSON 的热路径（查表数字、预分配
   缓冲、固定大小的 serializer 状态），优化了十年；nextjson 的编码器保留容器
   帧栈（为分隔符与 pretty 输出），且必须在 21 种格式下保持正确。编码侧校验
   成本大部分已被两项改动消除：整数输出改为每次除法经静态表产出两位数字
   （`write_u64_into`，纯整数 fixture 约 +24%）；顶层入口改用信任型
   `FastEncoder`，把每值的协议检查在编译期裁掉（同进程 A/B：校验型 `Encoder`
   约 440 MB/s vs 信任型发射器约 877 MB/s——检查本身约占编码的 2x）。校验型
   `Encoder` 仍是未经核实 serializer 的默认公开类型。

当前纯 JSON 数据下编码差距约为 1.9-2.1x（纯整数 fixture 约 1.64x）、解码差距
约为 1.3x。请把这些数字当作**该数据集的基线**而非普遍排名：nextjson 的编码
路径是通用格式中立契约，同时驱动 20 种其它格式；serde_json 是单一专精的
JSON 热路径。下结论前请在自己的硬件与数据上交叉验证。

发布结果必须同时记录 CPU、操作系统、`rustc -Vv`、提交版本、测量时长和完整
表格。CI 只编译两个 benchmark crate，不在共享机器上设置吞吐阈值。

## `simd` 特性与 GitHub Actions 基准工作流

`nextjson` 提供可选的 `simd` 特性，加速 JSON 字符串扫描热路径：x86-64 使用
SSE2 + 运行时检测的 AVX2，aarch64 使用 NEON，其它平台回退到可移植的寄存器
宽度（8/16 字节 SWAR）实现，短输入与尾部一律使用标量参考扫描。默认构建保持
`#![deny(unsafe_code)]`；`unsafe` 只存在于 `src/scan.rs` 且仅在启用该特性时
编译。所有加速路径都与标量参考实现对拍（全部字节值、全部两字节组合、长度
`0..=80`、1 MiB 缓冲区、2000 个确定性随机缓冲区）。序列化热路径还以
*干净段整段拷贝 / 转义* 两相方式写转义字符串：扫描定位下一个需要转义的字节、
把干净前缀整段 memcpy、只逐字节写出转义本身。转义位于尾部的长字符串因此几乎
是纯 `memcpy`。

仓库的 GitHub Actions 工作流（`.github/workflows/benchmark.yml`）会以启用
`simd` 特性的方式运行两套基准，把 CSV 合并为
`benchmarks/results/Github_Action_Benchmark.md`（含运行器 OS、CPU、工具链、
提交与方法学），上传原始 CSV 与报告为工作流产物，并在 `main` 分支 / 手动
触发 / 每周定时时把报告提交回仓库。也可在 *Actions → Benchmark → Run
workflow* 手动触发。

在字符串密集的数据集（32 条记录，每条正文约 1.5 KiB）上，分块转义写出器配合
SIMD 扫描在本开发机测得（`simd` 开启，1 秒窗口，单次运行）：

| case | size_bytes | 编码 MB/s | 解码 MB/s |
| --- | ---: | ---: | ---: |
| nextjson 长文本 JSON | 52579 | 5131.9 | 1801.0 |
| serde_json 长文本 | 52579 | 1709.5 | 1841.8 |
| simd-json 长文本 | 52579 | 4991.3 | 1236.6 |

nextjson 在该负载下编码比 serde_json 快约 3.0x、比 simd-json 快约 1.03x
（simd-json 的 serde 解码还因原地解析需要每次迭代一次 `to_vec()` 拷贝），
解码持平。短字符串记录 fixture 上仍有差距（编码约 0.6x、解码约 0.76x）——
serde_json 是单一专精 JSON 热路径，nextjson 的编码路径是驱动 21 种格式的
通用格式中立契约。绝对数值随 CPU 功耗状态漂移，请当作单次运行记录而非裁决。

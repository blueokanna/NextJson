# 小数编码专项：实现与实测

日期：2026-11-08。本轮针对普通小数编码热点，替换通用格式化路径，并补齐 f32
数组批量编码。实现说明见 [浮点文本编码](../../../docs/FLOAT_ENCODING_CN.md)。

普通小数单线程 JSON 从 **135.18 降到 57.48 µs/op**，提速 **2.35 倍**；
NDJSON 从 **151.19 降到 62.10 µs/op**，提速 **2.43 倍**。
每次操作编码 2048 个数值并释放结果。相对同轮 Serde，仍分别需要
**1.82、1.66 倍**耗时，尚未追平。8 线程的对应吞吐提升为
**2.47、2.76 倍**。完整数据包括不利结果，没有筛除样本。

## 测量方法

- Windows x86-64，Rust 1.99.1，CPU 标识 `Intel64 Family 6 Model 141 Stepping 1`，
  16 个逻辑处理器。release、opt-level=3、thin LTO、codegen-units=1；
  NextJson 为 `default-features=false, features=["derive","simd"]`，没有额外 RUSTFLAGS。
- 基线提交 `b95769817e2b7e2d753175a06012f1cd7ff47a56` 导出到隔离目录；仅同步本轮
  基准程序、benchmark 清单和锁文件。优化版为本轮工作区。源码、可执行文件和
  CSV 校验和见 [metadata.json](metadata.json)。库构建图仍只有两个本地 crate。
- [float_encoding.rs](../../serde-comparison/src/bin/float_encoding.rs) 测量 1、4、8
  线程；每个 worker 预热 50 ms，屏障同步后运行 250 ms，每组采集 5 个样本。
  同一组交替运行 NextJson 和 Serde，计入输出分配、写入和析构。
- 每个线程独立生成完整输出；NDJSON 参考实现逐条写入同一个 Vec。
  计时前用 core 数字解析逐位校验两边的输出，包含 f32 源精度和负零。
- 表中数值是五个样本的中位数。多线程的 µs/op 是总墙钟时间除以全部线程完成的
  操作数，即聚合吞吐的倒数，**不是单请求延迟，更不是 P99**。
- 未绑定 CPU 核心、锁定频率或隔离宿主背景负载。基准运行时没有并行执行本任务
  的编译或测试。范围保留最小值和最大值；这些短时数据不能代替部署环境压测。

## 负载

| Fixture | 内容 |
|---|---|
| ordinary | 2048 个 f64，循环使用 0.1、-12.34567、PI、12345.6789；与此前问题中的普通小数样本一致 |
| varied | 固定种子生成的 2048 个有正负号、五位小数的变化值；不只重复四个常量 |
| wide | 固定种子的 2048 个随机有限 f64 位模式，覆盖宽指数范围 |
| f32 | varied 转换为 f32，验证与测量源精度路径 |

wide 的科学计数法输出比旧路径更紧凑；该组收益包含输出长度变化，不能全部归因于
算术速度。表格比较相同数值的完整编码耗时，未用不同字节数计算 MB/s 来夸大优势。

## 配对结果

原始数据：[优化前](before.csv)、[优化后](after.csv)。Before/after 越大越好；
After/Serde 大于 1 表示仍慢于参考实现。单位 µs/op。

| Fixture | Format | Threads | Before µs/op | After µs/op (min–max) | Serde µs/op | Before/after | After/Serde |
|---|---|---:|---:|---:|---:|---:|---:|
| f32 | json | 1 | 257.39 | 64.45 (61.36–69.36) | 44.12 | 3.99 | 1.46 |
| f32 | json | 4 | 95.84 | 22.90 (21.65–24.66) | 14.73 | 4.18 | 1.55 |
| f32 | json | 8 | 54.22 | 12.77 (12.26–13.11) | 8.38 | 4.25 | 1.52 |
| f32 | ndjson | 1 | 240.35 | 61.66 (60.14–64.18) | 47.00 | 3.90 | 1.31 |
| f32 | ndjson | 4 | 94.77 | 22.41 (21.44–22.76) | 14.69 | 4.23 | 1.52 |
| f32 | ndjson | 8 | 53.29 | 12.26 (11.87–12.50) | 8.73 | 4.35 | 1.40 |
| ordinary | json | 1 | 135.18 | 57.48 (53.74–60.95) | 31.50 | 2.35 | 1.82 |
| ordinary | json | 4 | 41.27 | 16.82 (15.52–17.90) | 9.76 | 2.45 | 1.72 |
| ordinary | json | 8 | 28.96 | 11.73 (10.86–13.13) | 6.25 | 2.47 | 1.88 |
| ordinary | ndjson | 1 | 151.19 | 62.10 (58.93–69.83) | 37.35 | 2.43 | 1.66 |
| ordinary | ndjson | 4 | 54.29 | 21.46 (20.00–22.42) | 13.03 | 2.53 | 1.65 |
| ordinary | ndjson | 8 | 32.35 | 11.71 (11.45–12.48) | 7.34 | 2.76 | 1.60 |
| varied | json | 1 | 192.91 | 77.29 (71.53–78.29) | 60.83 | 2.50 | 1.27 |
| varied | json | 4 | 69.40 | 24.93 (24.45–27.02) | 16.60 | 2.78 | 1.50 |
| varied | json | 8 | 38.58 | 14.06 (13.62–14.18) | 9.47 | 2.74 | 1.49 |
| varied | ndjson | 1 | 179.13 | 73.13 (68.40–74.16) | 61.99 | 2.45 | 1.18 |
| varied | ndjson | 4 | 68.22 | 25.21 (23.12–25.73) | 17.19 | 2.71 | 1.47 |
| varied | ndjson | 8 | 37.11 | 13.84 (13.52–14.31) | 9.90 | 2.68 | 1.40 |
| wide | json | 1 | 425.97 | 89.98 (82.70–99.67) | 37.25 | 4.73 | 2.42 |
| wide | json | 4 | 151.02 | 30.76 (29.11–32.10) | 12.75 | 4.91 | 2.41 |
| wide | json | 8 | 86.74 | 17.34 (16.96–17.51) | 7.37 | 5.00 | 2.35 |
| wide | ndjson | 1 | 407.65 | 83.67 (81.94–84.72) | 36.92 | 4.87 | 2.27 |
| wide | ndjson | 4 | 148.68 | 28.70 (27.99–29.90) | 13.99 | 5.18 | 2.05 |
| wide | ndjson | 8 | 86.28 | 15.86 (15.49–16.18) | 7.75 | 5.44 | 2.05 |

## 既有负载回归检查

另运行未改变的 [core_formats.rs](../../serde-comparison/src/bin/core_formats.rs)，
覆盖记录、整数数组和普通小数，以及 JSON、MessagePack、RON、NDJSON。
原始 [core-before.csv](core-before.csv)、[core-after.csv](core-after.csv) 包含编码与
解码样本；下表列出全部 NextJson 编码项目。每项预热 100 ms，再采集 5 × 250 ms。
这组按原程序顺序测量，不与专项配对表混算。MessagePack 没有转换算法修改，
其测量变化不能归因于本轮浮点优化。

| Case | Before µs/op | After µs/op (min–max) | Serde before | Serde after | Before/after |
|---|---:|---:|---:|---:|---:|
| nextjson_decimals_json | 188.99 | 61.83 (58.78–64.52) | 45.59 | 36.19 | 3.06 |
| nextjson_decimals_msgpack | 4.47 | 4.18 (3.95–4.61) | 10.25 | 10.19 | 1.07 |
| nextjson_decimals_ndjson | 159.78 | 62.63 (59.13–63.43) | 39.10 | 35.52 | 2.55 |
| nextjson_decimals_ron | 167.26 | 64.75 (64.43–74.35) | 163.98 | 150.27 | 2.58 |
| nextjson_integers_json | 85.93 | 97.89 (96.05–113.82) | 53.19 | 52.36 | 0.88 |
| nextjson_integers_msgpack | 33.51 | 31.76 (30.87–38.54) | 43.56 | 37.73 | 1.06 |
| nextjson_integers_ndjson | 81.96 | 79.45 (75.91–85.38) | 66.34 | 52.79 | 1.03 |
| nextjson_integers_ron | 110.79 | 92.73 (91.30–100.21) | 189.60 | 173.88 | 1.19 |
| nextjson_records_json | 52.27 | 51.99 (51.69–60.38) | 30.82 | 31.63 | 1.01 |
| nextjson_records_msgpack | 20.54 | 22.22 (21.48–22.42) | 17.90 | 23.55 | 0.92 |
| nextjson_records_ndjson | 52.21 | 61.93 (58.45–64.67) | 31.85 | 37.64 | 0.84 |
| nextjson_records_ron | 54.39 | 59.16 (57.87–61.14) | 138.59 | 163.44 | 0.92 |

精确分数仍保留有界快捷路径，并改用位模式判断；通用转换单独保留调用帧。
这样既能处理普通小数，也避免让简单分数承担完整转换的寄存器和栈开销。

记录 NDJSON 的绝对耗时上升约 18.6%，同时参考实现上升约 18.2%；两者的耗时比
约为 1.64，前后基本一致。保留这些结果是为了呈现运行波动，不能只凭绝对变化
断言算法回退或收益，也不能宣称所有负载都已变快。

## 正确性与兼容

- 全特性测试、Rust 1.78、无默认特性的库测试、严格 Clippy、rustdoc 检查。
- 全特性 release 构建通过 `thumbv6m-none-eabi` 和 `x86_64-unknown-none`。
- f64/f32 各 100 万组位模式，对有限值逐位往返，并与独立 Ryū、Serde/Zmij
  比较十进制数值和最短有效数字；另覆盖全部指数的边界、十进制幂邻接值及舍入中点。
- 负零、非有限值拒绝、分段写出、嵌套 NDJSON、8 线程输出一致性均有测试。
- Python 精确整数核验全部 618 个十进制幂表项，已加入 CI。

部分数值从定点表示改为科学计数法；依赖原始字节签名或快照的调用方需要更新。
本次没有新增 unsafe 或库依赖。转换算术适配自 MIT 许可的 Zmij，来源和许可证
随源码保留；工程改进不等于发明新的浮点转换算法，也不构成零缺陷证明。

## 复现

在基线和优化版分别构建相同基准程序。以下为 PowerShell 命令；输出指定 UTF-8，
避免 Windows PowerShell 默认重定向产生 UTF-16 CSV。

```powershell
cargo run --release --locked --manifest-path benchmarks/serde-comparison/Cargo.toml --bin float_encoding -- --threads=1,4,8 --samples=5 --millis=250 | Set-Content -Encoding utf8 after.csv
python benchmarks/summarize_float.py before.csv after.csv --output comparison.md
python benchmarks/verify_float_tables.py
cargo test --workspace --all-features --locked
cargo test --release --locked --manifest-path benchmarks/serde-comparison/Cargo.toml --test float_oracle
```

# JSON、MessagePack、RON、NDJSON：no_std 对照记录

日期：2026-11-08。计时程序为
[`core_formats.rs`](../../serde-comparison/src/bin/core_formats.rs)。运行库始终关闭默认
特性，仅开启 `derive,simd`，优化前后都没有使用 std 特性。计时器、分配器及
Serde 参考库位于允许使用 std 的宿主 benchmark 中。

原始数据：[优化前](before.csv)、[优化后](after.csv)。
[元数据](metadata.json)记录工具链、源码校验和与基准依赖锁文件校验和。

## 方法与可比性

- Windows x86-64，Intel Core i7-11850H，Rust 1.99.1，release、opt-level=3、
  thin LTO、codegen-units=1。未设置 target-cpu 或额外 target-feature；本轮为 SSE2。
- 基线代码为 `ce09939b7eb34b006489b324e643091340161c9d`，执行前保存在
  `target/nostd-baseline`。仅加入相同计时程序、将 benchmark 的 nextjson 依赖设为
  `default-features=false, features=["derive","simd"]`。优化后为本轮工作区。
- 每种格式先验证完整的类型化往返，再分别预热编解码 100 ms，每项采集
  5 个 250 ms 样本。表中为耗时中位数，包含分配及结果析构。
- records：256 条相同结构记录，包括整数、布尔、UTF-8 名称、三项浮点分数及
  含转义文本的标签。浮点值主要是二进制精确分数，能利用本轮浮点快路径。
- integers：0 到 9999 的 u64 数组。decimals：2048 个值，重复
  `0.1, -12.34567, PI, 12345.6789`，单独检验通用小数格式化路径。
- 参考为锁文件中的 serde_json、rmp-serde、ron。MessagePack 参考使用命名字段。
  NDJSON 参考逐条写入共享 Vec、逐行解码。RON 的 NextJson 结构体采用 map 表示，
  参考库采用原生结构体表示，线格式和字节数不同；记录表比较的是同一逻辑值的
  往返成本，不能解释为相同线格式的解析器对决。
- 新旧程序顺序运行，计时期间未并行运行本任务的编译或测试。未绑定 CPU 核心或
  锁定频率，宿主负载会影响结果；原始 CSV 保留每个样本、字节数和操作数。
  这是短时本机测量，不是生产 P99、并发稳定性或裸机实时性保证。

## 结果

单位为微秒/次。`耗时比` = 优化后 NextJson / 同轮 Serde 参考，**小于 1 更快**。
优化前后列中的变慢结果同样保留；不同轮次的绝对耗时还受到宿主频率变化影响。

| Fixture | Format | Operation | Before (us) | After (us) | Reference (us) | Time ratio | After min-max (us) |
|---|---|---|---:|---:|---:|---:|---:|
| records | json | encode | 87.20 | 52.76 | 37.84 | 1.39 | 49.54-53.13 |
| records | json | decode | 219.55 | 212.20 | 184.89 | 1.15 | 208.82-213.68 |
| records | msgpack | encode | 21.55 | 25.75 | 22.99 | 1.12 | 23.69-29.33 |
| records | msgpack | decode | 111.13 | 130.42 | 131.46 | 0.99 | 118.46-168.86 |
| records | ron | encode | 164.77 | 67.15 | 236.89 | 0.28 | 61.61-72.56 |
| records | ron | decode | 713.65 | 372.40 | 402.10 | 0.93 | 361.85-387.02 |
| records | ndjson | encode | 821.87 | 62.96 | 37.84 | 1.66 | 59.53-69.94 |
| records | ndjson | decode | 257.36 | 332.45 | 262.29 | 1.27 | 283.48-348.71 |
| integers | json | encode | 93.95 | 108.75 | 56.93 | 1.91 | 92.80-111.96 |
| integers | json | decode | 159.68 | 167.49 | 106.88 | 1.57 | 155.79-175.65 |
| integers | msgpack | encode | 61.19 | 33.50 | 38.01 | 0.88 | 31.26-35.60 |
| integers | msgpack | decode | 77.97 | 67.17 | 40.60 | 1.65 | 64.23-69.65 |
| integers | ron | encode | 462.11 | 104.50 | 173.37 | 0.60 | 97.06-115.56 |
| integers | ron | decode | 647.72 | 399.41 | 341.43 | 1.17 | 390.14-428.27 |
| integers | ndjson | encode | 1139.12 | 80.16 | 52.89 | 1.52 | 73.12-90.13 |
| integers | ndjson | decode | 233.40 | 248.20 | 123.22 | 2.01 | 225.74-262.50 |
| decimals | json | encode | 134.98 | 151.49 | 40.09 | 3.78 | 149.27-165.15 |
| decimals | json | decode | 65.55 | 79.40 | 38.22 | 2.08 | 73.90-84.59 |
| decimals | msgpack | encode | 8.79 | 4.56 | 10.84 | 0.42 | 4.08-5.07 |
| decimals | msgpack | decode | 12.48 | 9.61 | 7.16 | 1.34 | 9.15-11.27 |
| decimals | ron | encode | 338.64 | 161.97 | 166.84 | 0.97 | 146.50-172.36 |
| decimals | ron | decode | 187.67 | 115.40 | 130.05 | 0.89 | 110.99-120.10 |
| decimals | ndjson | encode | 337.40 | 157.73 | 38.77 | 4.07 | 151.27-163.59 |
| decimals | ndjson | decode | 87.23 | 100.29 | 41.24 | 2.43 | 87.47-112.08 |

## 结论与剩余工作

NDJSON 去除中间树后，编码成本显著下降；RON 的字符串借用、直接数字输出和删除
最终拷贝也带来明显收益。MessagePack 基础数字数组的批量路径有效，记录解码已与
参考库接近。JSON 记录的编码与解码分别约为参考库耗时的 1.39、1.15 倍。

这不是所有项目都变快的结果。普通小数的 JSON、NDJSON 编码仍分别为参考库的
3.78、4.07 倍，整数数组和 NDJSON 解码也存在差距。精确分数快路径不能替代通用
浮点十进制转换算法；继续改善需要覆盖完整 IEEE-754 范围的转换实现和独立验证，
不能用缩短精度、放松校验或只报告有利样本换取数字。

库迁移、正确性修复及已执行的编译测试见
[no_std 复查](../../../docs/NO_STD_REVIEW_CN.md)。

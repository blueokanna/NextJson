# 可复现基准测试

## 中文

`nextjson/benches/format_comparison.rs` 使用同一份 128 条记录数据测量：

- 原生 JSON `nextencode`；
- 原生 JSON `nextdecode`；
- JSON 到 CBOR 的无中间树事件流；
- CBOR 到 JSON 的无中间树事件流。

benchmark 启动前执行 JSON -> CBOR -> JSON -> 强类型值往返并断言相等，随后预热
全部路径。构建图不包含第三方 crate。

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
case,iterations,operations_per_second
nextjson_native_nextencode,...,...
nextjson_native_nextdecode,...,...
nextjson_json_to_cbor,...,...
nextjson_cbor_to_json,...,...
```

发布结果必须同时记录 CPU、操作系统、`rustc -Vv`、提交版本、测量时长和全部四行。
单一数据集不能证明普遍性能，CI 只编译 benchmark，不在共享机器上设置吞吐阈值。

# Cross-Format Relay

`cross_format::EventSink` 是仓库自有的**格式中立事件协议**，让 JSON 与 CBOR
**流式互转**，不构造中间 `Value` 树，内存不随文档树增长。

## EventSink 契约

```rust
pub trait EventSink {
    fn null(&mut self) -> Result<()>;
    fn boolean(&mut self, value: bool) -> Result<()>;
    fn number(&mut self, value: Number) -> Result<()>;
    fn string(&mut self, value: &str) -> Result<()>;
    fn begin_array(&mut self) -> Result<()>;
    fn end_array(&mut self) -> Result<()>;
    fn begin_object(&mut self) -> Result<()>;
    fn object_key(&mut self, key: &str) -> Result<()>;
    fn end_object(&mut self) -> Result<()>;
}
```

**设计巧思**：`object_key` 与 `string` **分离**——防止目标格式意外接受 JSON 无法
表示的**非字符串键**。实现必须对非法事件顺序返回错误。

## 内置 Sink

- `JsonSink`：事件 → 紧凑/美化 JSON；
- `CborSink`：事件 → RFC 8949 JSON 兼容 profile CBOR。

## 入口 API

| 函数 | 作用 |
| --- | --- |
| `json_into(input, sink)` | JSON 输入流式喂给任意 `EventSink` |
| `json_into_with_config(input, config, sink)` | 带嵌套配置 |
| `cbor_into(input, sink)` / `cbor_into_with_max_depth` | CBOR 输入流式喂给 sink |
| `json_to_cbor` / `json_to_cbor_writer` | JSON → CBOR 字节/写出 |
| `cbor_to_json` / `cbor_to_json_writer` / `cbor_to_json_pretty` / `cbor_to_json_with_config` | CBOR → JSON |

```rust
use nextjson::cross_format;

let json = br#"{"name":"NextJson","values":[1,2,3]}"#;
let cbor = cross_format::json_to_cbor(json)?;
let json_again = cross_format::cbor_to_json(&cbor)?;
assert_eq!(json_again, json);
```

## 零拷贝边界

- 未转义 JSON 字符串在 `json_into` 里以**输入切片的直接借用**传给 sink；
- 转义字符串由 JSON 反转义**必然物化**——这是格式语义要求，文档不伪称零分配。

## 为什么值得单独一个模块

- 常规多格式引擎（[[Multi-Format Engine]]）是"类型驱动"：需要 `NsonSerialize` /
  `NsonDeserialize` 目标类型；
- `EventSink` 是"**数据驱动**"：只要目标格式能表示 JSON 数据模型，就能互转，
  不需要类型、不建树。适合代理、日志、网关等"只转发不解析语义"的场景。

## 诚实边界

- CBOR 侧只接受 **JSON 兼容 profile**：原始 byte string、非字符串 map key、
  非有限浮点、未知语义 tag 明确报错——防止 CBOR→JSON 时静默语义损失；
- 每条路径都有深度上限（默认 128）。

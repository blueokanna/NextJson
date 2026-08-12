# NextJson

> A dependency-free, `no_std + alloc` JSON / CBOR library for Rust, with a
> **schema-driven, visitor-free** design and a built-in 16-format engine.

NextJson 是一个**零第三方依赖**、核心 `no_std + alloc` 的 Rust 序列化库。它不是
serde 的复刻，而是一套独立的契约设计：用 **编译期 schema + 统一 Token 流 + 就地
解码** 取代 serde 的 Visitor 模式，并把 **16 种线格式**（JSON / YAML / TOML /
CBOR / MessagePack / BSON / …）收进同一个 crate。

## 核心亮点（每一条都在仓库里有实现与测试）

- **零第三方依赖**：整个工作区只有 `nextjson` 与 `nextjson-derive` 两个本地
  crate；derive 用标准 `proc_macro` API 手写递归下降解析器，不用 `syn` /
  `quote` / `proc-macro2`。
- **无 Visitor 的就地解码**：`NsonDeserialize::nextdecode_into` 直接写入调用方
  提供的 `DecodeSlot<T>`，支持内存复用，不需要 `T: Default` 或占位值。
- **编译期 schema**：每个类型携带 `const SCHEMA: TypeSchema`，运行时零开销内省，
  可直接生成 JSON Schema。
- **统一 Token 流**：字节流惰性词法（`Bytes`）与内容重放（`Tree`）共用同一套
  解码原语——内部/邻接/untagged 枚举与 `Value` 往返只维护一份引擎。
- **安全边界**：`#![deny(unsafe_code)]`、所有解码器默认 128 层递归上限、检查式
  数字、拒绝非有限浮点（无静默有损回退）。
- **单 crate 多格式**：一套 `NsonSerialize` 实现驱动 16 种格式；JSON ↔ CBOR 可
  通过 `cross_format::EventSink` 流式互转，不构造中间 `Value` 树。

## 快速开始

```toml
[dependencies]
nextjson = "0.1"
```

```rust
use nextjson::{NsonDeserialize, NsonSerialize};

#[derive(Debug, PartialEq, NsonSerialize, NsonDeserialize)]
#[njson(rename_all = "camelCase")]
struct User {
    user_id: u64,
    name: String,
    #[njson(default)]
    tags: Vec<String>,
}

let expected = User {
    user_id: 7,
    name: "Ada".into(),
    tags: vec!["compiler".into()],
};

let bytes = nextjson::nextencode(&expected)?;        // JSON 字节
let actual: User = nextjson::nextdecode(&bytes)?;     // 类型化还原
assert_eq!(actual, expected);
# Ok::<(), nextjson::Error>(())
```

多格式（同一类型、同一份 impl）：

```rust
use nextjson::formats;

let value = ("NextJson", vec![1_u64, 2, 3], true);

let json     = formats::encode_with(&value, formats::Json)?;
let msgpack  = formats::encode_with(&value, formats::MsgPack)?;
let yaml     = formats::encode_with(&value, formats::Yaml)?;
# Ok::<(), nextjson::Error>(())
```

## Wiki 导航

| 主题 | 页面 |
| --- | --- |
| 设计理念 | [[Design Philosophy]] |
| 架构总览 | [[Architecture]] |
| 核心契约 | [[Core Contracts]] |
| 编译期 schema | [[Compile-Time Schema]] |
| 统一 Token 流 | [[Unified Token Stream]] |
| 解码槽与内存复用 | [[Decode Slot]] |
| 零依赖与手写宏 | [[Zero-Dependency Macros]] |
| 安全模型 | [[Safety Model]] |
| 错误模型 | [[Error Model]] |
| 数字模型 | [[Number Model]] |
| Value 与 Map | [[Value and Map]] |
| 多格式引擎 | [[Multi-Format Engine]] |
| 格式支持矩阵 | [[Format Matrix]] |
| 跨格式中继 | [[Cross-Format Relay]] |
| 流式解码 | [[Streaming]] |
| 派生宏能力 | [[Derive Macros]] |
| 性能与基准 | [[Performance]] |
| 与 serde 的对比 | [[Comparison with serde]] |
| 设计决策记录（ADR） | [[Design Decisions]] |
| 术语表 | [[Glossary]] |

> 本 Wiki 的所有表述均以仓库源码、测试与文档为准；涉及"诚实局限"的条目是
> **功能边界声明**而非缺陷掩盖，详见 [[Format Matrix]] 与 [[Comparison with serde]]。

# NextJson

> 一个零第三方依赖、核心 `no_std + alloc` 的 Rust 序列化库。
> 它用 **编译期 schema + 统一 Token 流 + 就地解码** 三套机制取代 serde 的
> Visitor 模式，并把 **16 种线格式**（JSON / YAML / TOML / CBOR / MessagePack /
> BSON / …）收进同一个 crate。

## 这个库在解决什么问题

序列化解决两件事：把内存里的数据变成一串字节（编码），再把字节还原成数据
（解码）。Rust 生态里这件事的事实标准是 serde——它极其强大，但也带来了三个
代价：

1. **依赖链很长**。一个 `serde + serde_json` 的最小项目，构建图里躺着
   `serde_derive`、`syn`、`quote`、`proc-macro2`、`unicode-ident` 一串 crate。
   对嵌入式 / 固件 / 审计严格的场景，这是实打实的风险面。
2. **类型不自述**。serde 只告诉你"怎么编码 / 怎么解码"，不告诉你"类型长什么样"。
   想生成 JSON Schema 得再引入 `schemars`，于是"实现"和"文档"可能漂移。
3. **每加一种格式就要接一个 crate**。JSON 一个、YAML 一个、CBOR 又一个……它们
   之间对同一类型的行为还可能不一致。

NextJson 对这三个问题各给了一个不同的答案，而且每个答案都有对应源码：

- 整个工作区只有两个本地 crate，`cargo tree` 的输出短到能一眼看完；
- 每个可序列化类型都携带 `const SCHEMA`（编译期构造、运行时读取），schema 和
  序列化实现**同一份定义**，不会漂移；
- 一套 `NsonSerialize` 实现直接驱动 16 种格式，加新格式不用改你的类型。

## 三个核心机制（读懂了它们就读懂了整个库）

serde 的解码靠 `Visitor` 状态机：类型向 `Deserializer` 逐个"索取"原语。NextJson
把方向整个反过来，用三套机制替代：

**1. 就地解码（`nextdecode_into` + `DecodeSlot<T>`）**

```rust
fn nextdecode_into<D: FormatDecoder<'de>>(
    decoder: &mut D,
    out: &mut DecodeSlot<Self>,   // 调用方提供的"槽"
) -> Result<(), D::Error>;
```

解码结果不是"返回"出来的，而是写进调用方给的一块存储。这块存储能反复使用
（解码下一帧不用重新分配），而且不需要 `T: Default`。内部就是一个普通
`Option<T>`，所以"没写之前读不到值"由类型系统保证——这也是全库能
`#![deny(unsafe_code)]` 的原因。详见 [[Decode Slot]]。

**2. 编译期 schema（`NsonSchema` 超 trait）**

```rust
pub trait NsonSchema {
    const SCHEMA: TypeSchema;   // 编译期构造、运行时零开销读取
}
pub trait NsonSerialize: NsonSchema { ... }
```

每个类型自述结构，`schema_of::<T>()` 直接读这份数据，`to_json_schema::<T>()`
把它变成标准 JSON Schema。schema 与序列化实现同源，天然一致。详见
[[Compile-Time Schema]]。

**3. 统一 Token 流（`Bytes` / `Tree` 双输入源）**

解码器 `Decoder` 背后有两种输入：对 `&[u8]` 做惰性词法的 `Bytes` 源，和把内存中
`Vec<Token>` 重放出来的 `Tree` 源。两者暴露**完全相同**的解码原语——于是内部
标签 / 邻接标签 / untagged 枚举、`Value` 往返、文档式格式（TOML / YAML）全部
复用同一套引擎，不会出现"第二套实现"。详见 [[Unified Token Stream]]。

一句话：**serde 是"类型去驱动解码器"，NextJson 是"类型直接告诉解码器往哪写、
写什么"**。完整对比见 [[Comparison with serde]]。

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

同样的类型，换一种格式只是换个"终点"（同一份 impl 零改动）：

```rust
use nextjson::formats;

let value = ("NextJson", vec![1_u64, 2, 3], true);

let json     = formats::encode_with(&value, formats::Json)?;
let msgpack  = formats::encode_with(&value, formats::MsgPack)?;
let yaml     = formats::encode_with(&value, formats::Yaml)?;
# Ok::<(), nextjson::Error>(())
```

`formats` 里的 `Json`、`MsgPack`、`Yaml` 都是**零尺寸标记类型**——格式本身是
一个可传递、可存储、可动态选择的值（`by_name` / `by_extension` / `detect`）。
机制见 [[Multi-Format Engine]]。

## 怎么读这套 Wiki

如果你想先看整体怎么运转，按这个顺序读：

1. [[Architecture]] —— 一个值从内存到字节、再从字节回内存，全程经过哪些层；
2. [[Core Contracts]] —— 那几个 trait 到底是什么、每个方法为什么存在；
3. [[Compile-Time Schema]] → [[Unified Token Stream]] → [[Decode Slot]] ——
   三大机制的内部实现；
4. [[Multi-Format Engine]] → [[Format Matrix]] —— 16 种格式怎么收进一个 crate、
   每种格式能做什么、不能做什么；
5. [[Safety Model]] / [[Error Model]] / [[Performance]] —— 边界与代价。

每条都带源码位置或可直接运行的最小示例；涉及"诚实局限"的条目是**功能边界
声明**而不是缺陷掩盖。

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

# Design Philosophy

NextJson 的设计不是"再造一个 serde"，而是围绕一组**互锁的设计原则**展开。每一条
原则都能在源码里找到对应的具体实现；这里解释"为什么这样设计"。

## 1. 零第三方依赖是硬约束，不是巧合

工作区 `[dependencies]` 里唯一项目是本地 `nextjson-derive`，没有 crates.io / Git /
外部路径依赖。`nextjson-derive` 只用标准 `proc_macro` API。

**动机**：序列化库处在供应链的根部，任何依赖都会放大审计面。可审计构建图：

```text
cargo tree --workspace --all-features --edges normal,build,dev
# 只应出现 nextjson 与 nextjson-derive
```

**实现**：
- 核心 `#![no_std]` + `extern crate alloc`，只用 `core` + `alloc`；
- 自定义 `write::Write` trait（`write_all(&[u8])`）取代 `std::io::Write`；
- derive 手写递归下降解析器（见 [[Zero-Dependency Macros]]）。

**代价**：没有生态，`no_std` 下也没有 `std::io`。这是刻意接受的取舍。

## 2. 无 Visitor：解码是"就地写入"，不是"回调喂入"

serde 的 `Deserialize` 依赖 `Visitor` 状态机，由类型向 `Deserializer` 逐个索取
原语。NextJson 把方向反转：

```rust
fn nextdecode_into<D: FormatDecoder<'de>>(
    decoder: &mut D,
    out: &mut DecodeSlot<Self>,   // 调用方提供的槽
) -> Result<(), D::Error>;
```

**收益**：
- **内存复用**：`out` 由调用方分配，可反复使用，无需 `T: Default` 或占位值；
- **无 Visitor 样板**：派生代码直接调用解码器原语，生成代码更直白；
- **类型-格式解耦仍然成立**：`D: FormatDecoder<'de>` 让一份实现服务所有格式。

**代价**：失去 Visitor 那层"格式可主动驱动类型"的抽象（见
[[Comparison with serde]]）。

## 3. 编译期 schema 优先：类型自描述

`NsonSchema` 是 `NsonSerialize` 的**超 trait**，每个类型携带
`const SCHEMA: TypeSchema`：

```rust
pub trait NsonSchema {
    const SCHEMA: TypeSchema;   // 编译期构造、运行时内省
}
```

**动机**：schema 与序列化实现**同源**（一个类型只有一份定义），不会像 serde +
schemars 那样出现"实现与文档漂移"。

**实现细节**：`TypeSchema` 全部是引用型数据（`&'static`），因此能在 `const` 上下文
构造；`schema_of::<T>()` / `to_json_schema::<T>()` 直接从 `const` 读取。
详见 [[Compile-Time Schema]]。

**注意**：超 trait 的关联常量必须经超 trait 路径访问
`<T as NsonSchema>::SCHEMA`；经 `<T as NsonSerialize>::SCHEMA` 会触发 E0576。

## 4. 统一 Token 流：一份原语，两种输入源

解码器 `Decoder<'de>` 持有两种输入源之一：

- **`Bytes`**：对 `&[u8]` 做**惰性单 token 前瞻词法**，未转义字符串零分配借用；
- **`Tree`**：对内存中 `Vec<Token>` 的**内容重放**（内部/邻接标签枚举、`Value`
  解码需要）。

两者暴露**完全相同**的解码原语，派生代码永远不需要第二套机制。详见
[[Unified Token Stream]]。

**动机**：内部标签/邻接标签/untagged 枚举与 `Value` 往返，是序列化库里最容易出现
"第二套实现"的地方；统一 token 流让这些路径共享同一引擎，减少正确性风险。

## 5. 安全优先：拒绝，而不是容忍

- `#![deny(unsafe_code)]`：整个 crate 无 `unsafe`（`DecodeSlot` 用正常 `Option<T>`
  语义，靠类型系统保证"未初始化不可读"）；
- 所有解码器默认 **128 层递归上限**（防栈溢出 DoS）；
- 数字解析用**检查式算术**，溢出报错而非回绕；
- JSON 遇到 `NaN` / `Infinity` **显式报错**，不做 serde_json 那种（无 feature 时）
  静默输出 `null` 的有损回退；
- 每条字符串路径都做 UTF-8 / surrogate 校验。

详见 [[Safety Model]]。

## 6. 诚实局限：能表示才编码，不能表示就报错

每种格式只编码其线格式**能够无损表示**的值；不兼容组合返回明确错误，**绝不静默
有损**：

- `postcard` 非自描述 → 拒绝 `Option` / `Value` / `peek`；
- `bencode` 无 bool/null/float → 拒绝（或按文档映射）；
- `toml` / `bson` 是文档形态 → 裸标量根报错；
- CBOR 走 RFC 8949 的 **JSON 兼容 profile** → 原始 byte string、非字符串键、
  非有限浮点、未知 tag 全部明确报错。

**动机**："在错误的地方声称能力"比"能力不足"更危险。每种格式的模块文档都列出
支持子集。

## 7. 单 crate 多格式：一套实现驱动 16 种格式

`NsonSerialize::nextencode` 泛型于 `FormatEncoder`，`NsonDeserialize` 泛型于
`FormatDecoder`。同一个 `impl` 可服务所有能表示该值的格式；`formats` 注册表把
格式作为**一等值**（名称 / MIME / 扩展名 / 二进制分类）提供，支持按值传递、
`by_name` / `by_extension` / `detect` 动态选择。详见 [[Multi-Format Engine]]。

**代价**：每个值都穿过通用契约（每值栈帧 / 深度检查 / `start_value`），JSON 热
路径因此慢于专精的 serde_json（实测约 2.17x，见 [[Performance]]）。

# Unified Token Stream

NextJson 的解码器只有**一套解码原语**，但背后有两种输入源。这是"内部/邻接标签
枚举、`Value` 往返"等路径没有出现第二套实现的根本原因。

## 双输入源

```rust
enum DecoderInput<'de> {
    Bytes(&'de [u8]),            // 惰性单 token 前瞻词法
    Tree(Vec<Token<'de>>, ...),  // 内容重放
}
```

| 输入源 | 用途 | 特点 |
| --- | --- | --- |
| `Bytes` | 普通 JSON / 各格式字节流 | 逐个 token 惰性词法；未转义字符串 `Cow::Borrowed` **零分配借用**；整数手写解析带溢出检测 |
| `Tree` | 内部标签 / 邻接标签枚举、`Value` 驱动的解码 | 先校验整棵树再重放；字符串是 owned 的 |

两种源暴露**完全相同**的 `FormatDecoder` 方法，所以宏生成的代码不需要知道自己在
哪条路径上。

## Token 种类

```rust
pub enum Token<'de> {
    Null,
    Bool(bool),
    Number(Number),
    Str(Cow<'de, str>),
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
}
```

`Token` 是词法层与重放层共享的最小单位：`Bytes` 源逐 token 产出，`Tree` 源把
预存的 `Vec<Token>` 逐个吐出。

## 为什么这很重要

序列化库最容易出现"第二套实现"的地方：

1. **内部标签枚举**（`#[njson(tag = "type")]`）：解码时先读 tag 决定变体，然后
   继续用同一解码器消费剩余字段；
2. **邻接标签枚举**（`tag + content`）：内容与标签分离，需要把内容"重放"到
   变体解码器；
3. **untagged 枚举**：需要 `save`/`restore` 回溯；
4. **`Value` 解码**：把整棵 JSON 树读进内存再按需解码。

若每种场景都写一套读取逻辑，正确性风险随枚举形态数量线性增长。统一 Token 流让
这些路径共享同一引擎：**一旦 `Bytes` 路径正确，`Tree` 路径在结构上不可能偏离**。

## 惰性单 token 前瞻（`Bytes` 路径）

- 解析器**一次只 lex 一个 token**，不预先扫描全文；
- `peek_token` 只做**一个 token 的前瞻**（存进 lookahead 字段）；
- 未转义字符串直接切输入切片（`Cow::Borrowed`），零分配；
- 转义字符串（`\n`、`\uXXXX`、surrogate 对）才物化新的 UTF-8 字节。

这是"零拷贝边界"的实现基础：**能借就借，不能借才拷贝**，且文档明确不把必要
拷贝伪装成零拷贝。

## 计数式与文档式格式的差异（解码侧）

- `Bytes` 语义对文本格式是"天然"的；
- 计数式二进制格式（msgpack / postcard）用 `array_entry_sep` / `object_entry_sep`
  递减计数；
- 文档式格式（toml / yaml / bson）经 `TreeDecoder` 把收集好的树重放成同一套原语
  ——再次复用统一引擎。

## 相关页面

- 编码侧对应事件流：[[Core Contracts]] / [[Multi-Format Engine]]
- 借用边界：[[Decode Slot]] / [[Streaming]]（流式解码必须 owned，因为无法借用流）

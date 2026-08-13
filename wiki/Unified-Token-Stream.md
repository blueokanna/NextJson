# Unified Token Stream

NextJson 的解码器只有**一套解码原语**，但背后有两种输入源。这是"内部/邻接标签
枚举、`Value` 往返"等路径没有出现第二套实现的根本原因。本页用一个具体例子把
这条机制走一遍。

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

## 一个具体例子：`{"a": 1}` 的两次解码

### 第一次：`Bytes` 源（直接读内存输入）

```rust
let v: MyStruct = nextjson::nextdecode(br#"{"a":1}"#)?;
```

`Decoder` 的输入源是 `Bytes(&b"{\"a\":1}"[..])`。注意它**没有**预先扫描全文，
而是用"一次只 lex 一个 token"的方式工作：

1. 类型问 `begin_object` → 词法器跳过空白，看到 `{`，返回对象开始；
2. 类型问 `object_key` → 词法器切出 `"a"`。因为 `"a"` 里没有转义符，它直接
   **切输入切片**（`Cow::Borrowed("a")`），零分配；
3. 类型问 `u64` → 词法器看到 `1`，用手写整数解析（带溢出检测）得出 `1u64`；
4. 类型问 `object_entry_sep` → 看到 `}`，返回"没有更多条目"；
5. 类型问 `end_object` → 消费 `}`。

整个过程中，只有"被转义的字符串"才需要分配新内存（`Cow::Owned`）。这是
"零拷贝边界"的实现基础：**能借就借，不能借才拷贝**，且文档明确不把必要拷贝
伪装成零拷贝。

### 第二次：`Tree` 源（内部标签枚举 / `Value` 解码）

现在假设同一段数据先被收集成了一棵 token 树：

```rust
let toks: Vec<Token> = vec![
    Token::BeginObject,
    Token::Str(Cow::Borrowed("a")),
    Token::Number(Number::U64(1)),
    Token::EndObject,
];
```

`Decoder` 的输入源是 `Tree(toks)`。类型问同样的问题，`Tree` 源从 `Vec` 里
**逐个吐出**预存的 token：

1. `begin_object` → 弹出 `Token::BeginObject`；
2. `object_key` → 弹出 `Token::Str`；
3. `u64` → 弹出 `Token::Number(Number::U64(1))`；
4. ......

**类型代码一个字都不用改**——它看到的接口和 `Bytes` 源一模一样。这就是"统一
Token 流"的含义：一旦 `Bytes` 路径正确，`Tree` 路径在结构上不可能偏离，因为它
只是把词法结果换成了预存 token。

## 为什么这很重要：哪些场景需要 `Tree` 源

序列化库最容易出现"第二套实现"的地方：

1. **内部标签枚举**（`#[njson(tag = "type")]`）：解码时先读 tag 决定变体，然后
   继续用同一解码器消费剩余字段——中间需要把"已读出的内容"重新放回去或重放；
2. **邻接标签枚举**（`tag + content`）：内容与标签分离，需要把内容"重放"到
   变体解码器；
3. **untagged 枚举**：需要 `save`/`restore` 回溯；
4. **`Value` 解码**：把整棵 JSON 树读进内存再按需解码；
5. **文档式格式**（toml / yaml / bson）：解析器先把整份文档收集成树，再通过
   `TreeDecoder` 重放成同一套原语。

若每种场景都写一套读取逻辑，正确性风险随枚举形态数量线性增长。统一 Token 流
让这些路径共享同一引擎。

## 惰性单 token 前瞻（`Bytes` 路径）

- 解析器**一次只 lex 一个 token**，不预先扫描全文；
- `peek_token` 只做**一个 token 的前瞻**（存进 lookahead 字段）；
- 未转义字符串直接切输入切片（`Cow::Borrowed`），零分配；
- 转义字符串（`\n`、`\uXXXX`、surrogate 对）才物化新的 UTF-8 字节。

> 性能注记：`Bytes` 路径对常见标量（数字 / 未转义字符串 / 布尔 / null）走的是
> 字节直达快路径（Phase 9 起），不再构造 `Token` 再 match——只有当类型不匹配、
> 需要回退保证错误消息一致时才走 token 路径。数字/字符串原语因此各快约 20%。

## 计数式与文档式格式的差异（解码侧）

- `Bytes` 语义对文本格式是"天然"的；
- 计数式二进制格式（msgpack / postcard）用 `array_entry_sep` / `object_entry_sep`
  递减计数；
- 文档式格式（toml / yaml / bson）经 `TreeDecoder` 把收集好的树重放成同一套原语
  ——再次复用统一引擎。

## 相关页面

- 编码侧对应事件流：[[Core Contracts]] / [[Multi-Format Engine]]
- 借用边界：[[Decode Slot]] / [[Streaming]]（流式解码必须 owned，因为无法借用流）


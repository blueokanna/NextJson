# Streaming

`StreamDecoder<R: std::io::Read>` 实现与 `Decoder` **完全相同的**
`FormatDecoder<'de>` 契约，但按需从 reader 增量拉取字节——这是 `from_reader`
（网络 socket、管道、增量源）的底层引擎。

```rust
use nextjson::StreamDecoder;   // std feature
use nextjson::NsonDeserialize;

let mut dec = StreamDecoder::new(reader);
let value: MyType = dec.nextdecode()?;   // 增量拉取
let rest = dec.end()?;                   // 消费完整输入后收尾
```

## 实现要点

- 内部 `buf: Vec<u8>` 保存**所有已读字节**，`pos` 是绝对读位置；
- 每次 `buf.get(i)` 前必须 `fill(i+1)`——chunked / 逐字节 reader 会暴露提前 EOF；
- 词法器逐字节 `has_more(i)?` 防 UTF-8 跨 `fill` 边界截断；数字循环用
  `has_more(i)? && ...` 防提前退出（`buf.get` 返回 `None` 被误判为数字结束）；
- 固有方法全部 `_impl` 后缀（`key_impl`/`obj_sep_impl`/`lex_next`/...），否则与
  trait 方法同名会无限递归。

## 两个诚实的取舍（模块文档原话）

### 1. Owned 字符串

流式输入**无法借用**到解码值的生命周期，所以 `string()`/`bytes()` 总是返回
`Cow::Owned`。要求借用（`&str`、`&[u8]`、`nextjson::Bytes`）的类型**无法从流**
解码。

### 2. 保留全部缓冲

为了满足 untagged 枚举的 `save`/`restore` 回溯契约（`restore` 无错误通道，
必须能任意回溯），解码器保留**每一个已读字节（含已消费前缀）**。内存随总输入
增长；收益是**解码从第一批字节到达就开始**，不用等完整 payload。

> 应用需要"单值常量内存流式"时，应在协议层自行分块，而不是依赖本解码器。

## 与 `from_reader` 的关系

```rust
pub fn from_reader<R: std::io::Read, T: NsonDeserialize<'de>>(reader: R) -> Result<T>;
```

`from_reader` 即 `StreamDecoder::new(reader)` + `nextdecode` + `end` 的封装，且
`end()` 校验整个输入已消费。

## 相关页面

- 契约全貌：[[Core Contracts]]
- 与内存解码的对比：[[Unified Token Stream]] / [[Decode Slot]]

# Core Contracts

NextJson 的全部能力建立在一小组 trait 之上。**这些 trait 就是整个库的地基**：
你写的类型实现它们，16 种格式实现它们的另一侧，宏生成它们的大部分实现。本页
逐个讲清楚每个 trait、每个方法为什么存在、以及背后的机制。

## 0. 先建立一个心智模型

把序列化想成"一场对话"：

- **编码侧**：你的类型是说话的人，`FormatEncoder` 是记录员。类型对记录员说
  "开始一个对象"、"键是 user_id"、"值是 7"、"结束对象"，记录员负责把这句话
  翻译成目标格式的字节（JSON 写 `{`，msgpack 写 `0x81`……）。
- **解码侧**：`FormatDecoder` 是复读机，你的类型是整理员。复读机一句一句念
  "对象开始"、"键是 user_id"、"值是数字 7"，整理员把这些话填进自己的字段。

serde 的对话方向是反过来的（记录员/复读机主动问类型"你要什么"），这就是
Visitor 和"就地解码"的本质区别。方向不同，导致两边整套 API 都不同。

## 1. 序列化：`NsonSerialize`

```rust
pub trait NsonSerialize: NsonSchema {
    fn nextencode<E: FormatEncoder>(&self, encoder: &mut E) -> Result<(), E::Error>;
}
```

- `NsonSchema` 是**超 trait**：想序列化，必须先能描述自己（见
  [[Compile-Time Schema]]）。这是"schema 与实现同源"的机制保证。
- `nextencode` **直接向编码器写事件**，没有中间表示（不经过 `Value` 树）。
- 错误类型是格式自己的 `E::Error`（关联类型）——第三方格式可以携带自己的
  错误类型，`?` 靠 `From<Error>` 自动转换（见 [[Error Model]]）。

## 2. 反序列化：`NsonDeserialize`

```rust
pub trait NsonDeserialize<'de>: Sized {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error>;
}
```

三个要点：

- **就地解码**：结果写进调用方提供的 `DecodeSlot<Self>`，而不是"返回新值"。
  这让调用方可以复用内存，且解码失败时槽保持未初始化、不会拿到半成品
  （机制见 [[Decode Slot]]）。
- **`'de` 是输入生命周期**：借用类型（`&'a str`）要求 `'de: 'a`，意思是"输入
  活得比借用久"。这个约束由派生宏自动生成。
- 对比 serde 的 `Visitor`：serde 是解码器"驱动"类型，这里反过来，类型主动
  拉取。代价与收益见 [[Comparison with serde]]。

## 3. 编码侧事件契约：`FormatEncoder`

每种目标格式实现它。这张表里最关键的是前两行，它们藏着让"一份实现服务
16 种格式"成立的机制：

| 方法 | 语义 | 为什么这样设计 |
| --- | --- | --- |
| `begin_array` / `separator` / `end_array` | 数组容器 | `separator` **每个元素都调用一次（含第一个）**。文本格式把它当"要不要写逗号"的开关；二进制格式把它当**元素计数器**——数够了，在 `end_array` 时回填长度前缀。这是"同一组事件、两种完全不同的写法"的典型例子 |
| `begin_object` / `key` / `end_object` | 对象容器 | `key` 同样每个条目调用一次，同样可当条目计数器（msgpack 的 map 长度就是这么来的） |
| `write_null` / `write_bool` / `write_str` / `write_char` | 标量 | — |
| `write_number` / `write_i64` / `write_u64` / `write_i128` / `write_u128` / `write_f64` / `write_f32` | 数字 | `write_number(&Number)` 保留**精确内部种类**（见 [[Number Model]]）；其余保留语义 |
| `write_i8..i32` / `write_u8..u32` | **宽度方法** | JSON 没有"宽度"概念，`i8` 就是数字；但 postcard 这类定宽二进制格式**必须在线上保留源宽度**。机制：默认加宽到 `i64`/`u64`，二进制格式**覆写**为原生宽度写出 |
| `write_bytes(&[u8])` | 字节串 | 默认按 `u8` 数组发射（与 serde_json 一致：JSON 无原生字节类型）；二进制格式覆写为**长度前缀 + 原始字节**，更紧凑 |
| `write_none` / `write_some` | `Option` | JSON 里 `Option` 就是 null（`write_none`→null、`write_some`→无操作）；二进制格式需要区分 `None` 与 `Some(null)`，于是覆写为区分 tag |
| `map_key<K: NsonSerialize>(&K)` | 对象键 | JSON 只支持字符串键，所以默认把键序列化为字符串；二进制格式覆写为"键即值"，于是 `BTreeMap<u8, V>` 不必经过字符串往返 |
| `is_human_readable(&self) -> bool` | 可读性 | 文本格式 `true`、二进制 `false`；类型可据此分支（时间戳/字节串的表示），镜像 serde |

注意这些方法**都有默认实现**（比如 `write_i8` 默认调 `write_i64`）。一个
"偷懒"的格式只需要实现少数几个方法，其余自动走 JSON 形状——但想要紧凑/定宽/
区分 tag 的格式，就逐个覆写。

## 4. 解码侧事件契约：`FormatDecoder<'de>`

每种输入格式实现它。核心方法：

| 方法 | 语义 |
| --- | --- |
| `begin_object` / `object_key` / `object_entry_sep` / `end_object` | 对象读取。`object_key` 返回 `None` 表示遇到 `}`（不消费）；`object_entry_sep` 返回是否还有条目 |
| `begin_array` / `array_has_more` / `array_entry_sep` / `end_array` | 数组读取 |
| `unit` / `bool` / `number` / `string` / `char` | 标量读取。`string` 返回 `Cow<'de, str>`（可借用） |
| `i8..i32` / `u8..u32` / `i64` / `u64` / `i128` / `u128` / `f32` / `f64` | 宽度读取。默认读 `number` 后 `try_from`（带范围检查）；二进制格式覆写为原生宽度读取 |
| `skip_value` | 跳过任意一个值（必须 `peek_token` 判定类型；"类型字节前置"的格式要先消费 lookahead） |
| `peek_token` | 不消费地窥视下一个 token（flatten 支持） |
| `save` / `restore` | 回溯保存点（untagged 枚举需要；`restore` 无错误通道，因此实现必须能任意回溯） |
| `option_tag` | 探测 `Option::None`（已消费）或 `Some`（下一个 token 是负载） |
| `bytes` | 读原生字节串 |
| `is_human_readable` | 镜像编码端 |

解码侧同样有默认实现：宽度读取默认"读一个通用数字再转换"。`save`/`restore`
是 untagged 枚举的"后悔药"——试一种变体失败后，退回到保存点试下一种。

## 5. 解码槽：`DecodeSlot<T>`

```rust
pub struct DecodeSlot<T> { value: Option<T> }
```

- 内部是**正常 `Option<T>`**——不是公开的 `MaybeUninit<T>`。它把"未初始化"
  变成类型系统可见的状态：`write` 之前无法读到值，错误实现无法在 safe code 里
  造成 UB。这是全库 `deny(unsafe_code)` 能成立的关键。
- `write` / `take` / `is_initialized` 三个方法构成完整生命周期。
  为什么不用 `MaybeUninit`：因为 `Option<T>` 的析构语义是现成的——解码失败时，
  已初始化的字段按正常 drop 顺序清理，不存在"半初始化值泄漏"。详见
  [[Decode Slot]]。

## 6. 无 `std` 的写出：`crate::write::Write`

```rust
pub trait Write {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), crate::Error>;
}
```

`no_std` 下没有 `std::io::Write`，NextJson 自带最小契约。为 `Vec<u8>` 与
`&mut [u8]` 实现。`to_writer` / `to_writer_pretty` / `json_to_cbor_writer` 等都
以它为界。实现 `Write` 就能让任何自定义 sink 参与编码。

## 7. 错误类型：`FormatError` 与 `Result`

```rust
pub trait FormatError: From<crate::Error> {
    fn custom(msg: impl Into<String>) -> Self;
}
pub type Result<T, E = Error> = core::result::Result<T, E>;
```

- 泛型代码里 `?` 自动把 `nextjson::Error` 转成格式自己的错误（靠 `From<Error>`）
  ——这是"格式可携带自定义错误"和"库内代码不需要知道具体错误类型"同时成立的
  机制。
- `Result` 别名带**默认第二参数**（稳定特性，关联类型默认值不是）。所以
  `Result<()>` 与 `Result<(), CodecError>` 都合法。

## 顶层便捷 API（`lib.rs` / `encoding.rs`）

| 函数 | 作用 |
| --- | --- |
| `nextencode(&T) -> Vec<u8>` | 编码为 JSON 字节 |
| `to_writer` / `to_writer_pretty` | 编码到任意 `Write` |
| `nextdecode::<T>(&[u8]) -> T` | 从完整内存输入解码，校验全部输入已消费 |
| `from_reader` | 从 `std::io::Read` 流式解码（内部是 `StreamDecoder`） |
| `schema_of::<T>()` / `to_json_schema::<T>()` | 读取编译期 schema |


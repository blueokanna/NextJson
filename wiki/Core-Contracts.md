# Core Contracts

NextJson 的全部能力建立在一小组 trait 之上。理解它们等于理解整个库。

## 1. 序列化：`NsonSerialize`

```rust
pub trait NsonSerialize: NsonSchema {
    fn nextencode<E: FormatEncoder>(&self, encoder: &mut E) -> Result<(), E::Error>;
}
```

- `NsonSchema` 是**超 trait**：任何可序列化类型必须能描述自己（见
  [[Compile-Time Schema]]）。
- `nextencode` **直接向编码器写事件**，没有中间表示。错误类型是格式自己的
  `E::Error`（关联类型），因此第三方格式可以携带自己的错误。

## 2. 反序列化：`NsonDeserialize`

```rust
pub trait NsonDeserialize<'de>: Sized {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error>;
}
```

- **就地解码**：写入调用方提供的 `DecodeSlot<Self>`，而不是返回新值。这让调用方
  可以复用内存，且解码失败时槽保持未初始化、不会产生半成品。
- `'de` 是输入生命周期；借用类型（`&'a str`）要求 `'de: 'a`（输入活得比借用久）。

## 3. 格式中立输出契约：`FormatEncoder`

每种目标格式实现它。关键点：

| 方法 | 语义 | 设计巧思 |
| --- | --- | --- |
| `begin_array` / `separator` / `end_array` | 数组容器 | `separator` **每个元素都调用一次（含第一个）**，因此二进制格式可把它当**元素计数器**，在容器关闭时回填长度前缀 |
| `begin_object` / `key` / `end_object` | 对象容器 | `key` 同样每个条目调用一次，可当条目计数器 |
| `write_null` / `write_bool` / `write_str` / `write_char` | 标量 | — |
| `write_number` / `write_i64` / `write_u64` / `write_i128` / `write_u128` / `write_f64` / `write_f32` | 数字 | `write_number(&Number)` 保留**精确内部种类**；其余保留语义 |
| `write_i8..i32` / `write_u8..u32` | **宽度方法** | 默认加宽到 `i64`/`u64`（JSON 形状）；二进制格式**覆写**为原生宽度写出，从而在线上保留源宽度（如 `postcard`） |
| `write_bytes(&[u8])` | 字节串 | 默认按 `u8` 数组发射（与 serde_json 一致，JSON 无原生字节类型）；二进制格式覆写为**长度前缀 + 原始字节**，更紧凑 |
| `write_none` / `write_some` | `Option` | 默认映射 null / 无操作（JSON 形状）；二进制格式覆写为区分 tag |
| `map_key<K: NsonSerialize>(&K)` | 对象键 | 默认把键序列化为字符串（JSON 形状）；二进制格式覆写为"键即值"，支持 `BTreeMap<u8, V>` 等**非字符串键** |
| `is_human_readable(&self) -> bool` | 可读性 | 文本格式 `true`、二进制 `false`；类型可据此分支（时间戳/字节串的表示），镜像 serde |

## 4. 格式中立输入契约：`FormatDecoder<'de>`

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

## 5. 解码槽：`DecodeSlot<T>`

```rust
pub struct DecodeSlot<T> { value: Option<T> }
```

- 内部是**正常 `Option<T>`**——不是公开的 `MaybeUninit<T>`。它把"未初始化"
  变成类型系统可见的状态：`write` 之前无法读到值，错误实现无法在 safe code 里
  造成 UB。
- `write` / `take` / `is_initialized` 三个方法构成完整生命周期。详见
  [[Decode Slot]]。

## 6. 无 `std` 的写出：`crate::write::Write`

`no_std` 下没有 `std::io::Write`，NextJson 自带最小契约：

```rust
pub trait Write {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), crate::Error>;
}
```

为 `Vec<u8>` 与 `&mut [u8]` 实现。`to_writer` / `to_writer_pretty` /
`json_to_cbor_writer` 等都以它为界。

## 7. 错误类型：`FormatError` 与 `Result`

```rust
pub trait FormatError: From<crate::Error> {
    fn custom(msg: impl Into<String>) -> Self;
}
pub type Result<T, E = Error> = core::result::Result<T, E>;
```

- 泛型代码用 `?` 自动把 `nextjson::Error` 转成格式自己的错误（靠 `From<Error>`）。
- `Result` 别名带**默认第二参数**（稳定特性），所以 `Result<()>` 与
  `Result<(), CodecError>` 都合法。详见 [[Error Model]]。

## 顶层便捷 API（`lib.rs` / `encoding.rs`）

| 函数 | 作用 |
| --- | --- |
| `nextencode(&T) -> Vec<u8>` | 编码为 JSON 字节 |
| `to_writer` / `to_writer_pretty` | 编码到任意 `Write` |
| `nextdecode::<T>(&[u8]) -> T` | 从完整内存输入解码，校验全部输入已消费 |
| `from_reader` | 从 `std::io::Read` 流式解码（内部是 `StreamDecoder`） |
| `schema_of::<T>()` / `to_json_schema::<T>()` | 读取编译期 schema |

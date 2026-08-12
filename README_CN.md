# NextJson

## 中文文档 - [English Documentation](README.md)

面向生产环境、零第三方 crate、支持 `no_std + alloc` 的 Rust JSON / CBOR 库。

### 当前保证

- 工作区仅包含自有的 `nextjson` 和 `nextjson-derive` 两个 crate。
- `[dependencies]` 中唯一项目是工作区内的 `nextjson-derive`，没有 crates.io、Git 或外部路径依赖。
- `nextjson-derive` 只使用 Rust 标准 `proc_macro` API，同样没有外部依赖。
- 核心 crate 使用 `#![no_std]`、`#![deny(unsafe_code)]` 和 `#![deny(missing_docs)]`。
- 原生 API 使用 `nextencode`、`nextdecode` 和 `nextdecode_into`，不保留旧方法名。
- 未转义 JSON 字符串和定长 CBOR 文本可直接借用输入缓冲区。
- JSON 与 CBOR 通过自有事件流协议转换，不构造中间 `Value` 树。

可以直接审计构建图：

```text
cargo tree --workspace --all-features --edges normal,build,dev
```

预期只出现：

```text
nextjson
└── nextjson-derive (local workspace proc-macro)
nextjson-derive
```

### 安装

```toml
[dependencies]
nextjson = "0.1"
```

纯 `no_std + alloc`：

```toml
[dependencies]
nextjson = { version = "0.1", default-features = false }
```

启用自有派生宏但不启用 `std`：

```toml
[dependencies]
nextjson = { version = "0.1", default-features = false, features = ["derive"] }
```

特性只有两个：

| Feature  | 默认 | 作用                                                |
| -------- | ---: | --------------------------------------------------- |
| `std`    |   是 | 启用标准 IO 适配器和标准库专属类型                  |
| `derive` |   是 | 启用自有 `NsonSerialize` / `NsonDeserialize` 派生宏 |

### 原生 nextencode / nextdecode

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

let bytes = nextjson::nextencode(&expected)?;
let actual: User = nextjson::nextdecode(&bytes)?;
assert_eq!(actual, expected);
# Ok::<(), nextjson::Error>(())
```

`nextdecode` 会验证整个输入已经消费完毕，因此第二个顶层值和尾随垃圾都会报错。

### 零拷贝字符串

```rust
use nextjson::{DecodeSlot, Decoder, NsonDeserialize, Result};

struct Borrowed<'a>(&'a str);

impl<'de> NsonDeserialize<'de> for Borrowed<'de> {
    fn nextdecode_into(
        decoder: &mut Decoder<'de>,
        output: &mut DecodeSlot<Self>,
    ) -> Result<()> {
        output.write(Borrowed(<&str>::nextdecode(decoder)?));
        Ok(())
    }
}

let input = br#""borrowed""#;
let value: Borrowed<'_> = nextjson::nextdecode(input)?;
assert!(value.0.as_ptr() >= input.as_ptr());
# Ok::<(), nextjson::Error>(())
```

无转义字符串返回输入切片；包含 `\n`、`\uXXXX` 等转义时必须生成新的 UTF-8
字节，因此返回拥有所有权的字符串。这是格式语义要求，不伪称零分配。

### 跨格式事件流

`cross_format::EventSink` 是仓库自有的格式中立协议。它覆盖 JSON 数据模型：
null、布尔、有限数字、UTF-8 字符串、数组和字符串键对象。源格式逐事件读取，目标格式
逐事件写入，内存占用不随整棵文档树增长。

```rust
use nextjson::cross_format;

let json = br#"{"name":"NextJson","values":[1,2,3],"ok":true}"#;
let cbor = cross_format::json_to_cbor(json)?;
let json_again = cross_format::cbor_to_json(&cbor)?;

let left: nextjson::Value = nextjson::nextdecode(json)?;
let right: nextjson::Value = nextjson::nextdecode(&json_again)?;
assert_eq!(left, right);
# Ok::<(), nextjson::Error>(())
```

可用入口：

| API                                    | 作用                              |
| -------------------------------------- | --------------------------------- |
| `json_into`                            | JSON 输入流向任意自有 `EventSink` |
| `cbor_into`                            | CBOR 输入流向任意自有 `EventSink` |
| `json_to_cbor` / `json_to_cbor_writer` | JSON 流式写为 CBOR                |
| `cbor_to_json` / `cbor_to_json_writer` | CBOR 流式写为 JSON                |
| `cbor_to_json_pretty`                  | CBOR 流式写为格式化 JSON          |

内置 CBOR 实现遵循 RFC 8949 的 JSON 兼容 profile：

- 支持定长和不定长数组、map、文本；
- 支持 `u64` / `i64` 主要类型；
- 使用标准 tag 2 / tag 3 精确保存 `u128` / `i128`；
- 支持半精度、单精度和双精度有限浮点；
- map key 必须是 UTF-8 文本；
- 原始 byte string、非字符串 map key、非有限浮点和未知语义 tag 会明确报错。

这些限制防止 CBOR 到 JSON 时发生静默语义损失。

### 多格式引擎

`nextjson::formats` 是零依赖、格式中立的多格式编解码引擎。自有的
`NsonSerialize` / `NsonDeserialize` 契约泛型化于 `FormatEncoder` /
`FormatDecoder` 之上；同一份实现可服务所有能够表示该值的线格式。多数编码器直接
发射；TOML 和 YAML 因表顺序要求先收集为 `Value`。不兼容的类型/格式组合按下表
返回错误。

```rust
use nextjson::formats;

let value = ("NextJson", vec![1_u64, 2, 3], true);

let json = formats::encode_with(&value, formats::Json)?;
let msgpack = formats::encode_with(&value, formats::MsgPack)?;
let yaml = formats::encode_with(&value, formats::Yaml)?;

let back: (String, Vec<u64>, bool) = formats::decode_with(&json, formats::Json)?;
assert_eq!(back, formats::decode_with(&msgpack, formats::MsgPack)?);
assert_eq!(back, formats::decode_with(&yaml, formats::Yaml)?);
# Ok::<(), nextjson::Error>(())
```

共注册 16 种格式。格式是一等 `Format` 值，携带规范名、MIME 类型、文件扩展名和
二进制/文本分类，可以按值传递、存储或动态选择：

```rust
use nextjson::formats::{FormatKind, self};

let kind: Option<FormatKind> = formats::by_extension("toml");
let detected: Option<FormatKind> = formats::detect(br#"{"a":1}"#);
let json = formats::encode_with(&42_i64, formats::Json)?; // 按值选择格式
# let _ = (kind, detected, json);
```

| 分组                 | 格式                                                             |
| -------------------- | ---------------------------------------------------------------- |
| 文本、自描述         | `json`、`json5`、`hjson`、`yaml`、`toml`、`ron`、`sexpr`、`csv`、`urlform` |
| 二进制、自描述       | `cbor`、`msgpack`、`bson`、`bencode`、`pickle`                   |
| 二进制、轻模式       | `postcard`                                                       |
| 环境                 | `envy`（仅反序列化，需要 `std`）                                  |

数据模型兼容的格式之间无需类型化值即可互转：

```rust
use nextjson::formats;
let json = br#"{"name":"NextJson","values":[1,2,3]}"#;
let msgpack = formats::transcode(json, formats::Json, formats::MsgPack)?;
let json2 = formats::transcode(&msgpack, formats::MsgPack, formats::Json)?;
assert_eq!(json2, json);
# Ok::<(), nextjson::Error>(())
```

#### 能力矩阵（诚实标注的局限）

每种格式都实现统一契约；线格式模型限制和编解码器明确限定的子集都会以错误报告，
不会静默做有损回退：

| 格式       | 标量                                   | 容器             | 说明 |
| ---------- | -------------------------------------- | ---------------- | ---- |
| `json`     | null/bool/int/float/str                | array/object     | RFC 8259，完整模型 |
| `json5`    | 同 JSON + `Infinity`/`NaN`             | + 注释、未加引号键、单引号、尾随逗号 | 编码器输出严格 JSON |
| `hjson`    | 同 JSON                                | + 未加引号键/字符串、注释 | 编码器输出严格 JSON |
| `yaml`     | null/bool/int/float/str                | 块式 + 流式子集   | 块式 map/序列、`key: value`、`- `、`---`、`{…}`/`[…]` |
| `toml`     | bool/int/float/str（无 null）          | 表、数组、内联表 | 文档形态：裸标量根被拒绝 |
| `ron`      | bool/int/float/str/char                | map/seq/元组/结构体/枚举 | `Some(...)` 包装可往返 |
| `sexpr`    | 原子、带引号字符串、数字、`#t`/`#f`、`nil` | 列表；map 编为 alist | 无模式 `Value` 解码嵌套 map 有歧义，请用类型化目标 |
| `csv`      | int/float/bool/str                     | 行；带表头的对象行 | RFC 4180 |
| `urlform`  | int/float/bool/str                     | 仅扁平 key/value map | RFC 3986 百分号编码 |
| `cbor`     | null/bool/int/float/str                | array/map         | RFC 8949 JSON 兼容 profile，经事件流中继 |
| `msgpack`  | nil/bool/int/float/str                 | array/map         | JSON 兼容标量/容器族；不支持 bin/ext；128 位整数放不进 64 位时拒绝 |
| `bson`     | null/bool/int32/int64/double/str       | document/array    | 文档形态：裸标量根被拒绝 |
| `bencode`  | 整数、UTF-8 字符串                     | list/dict         | key 规范排序；无 null/float；bool 映射为 1/0 |
| `postcard` | null/bool/无符号整数/str               | seq/map           | **非自描述**：拒绝有符号整数、float、`Option`、`Value` 和 peek |
| `pickle`   | `None`/bool/int/float/str              | list/dict/tuple   | CPython 协议 2 子集；128 位经 `LONG1` |
| `envy`     | int/float/bool/str                     | 扁平 map（即环境变量） | 仅反序列化；需要 `std` |

`detect()` 是启发式且刻意保守：只认定强结构签名（pickle 协议头、bencode 开头、
BSON 长度前缀、文本格式 ASCII 开头、MessagePack/CBOR 二进制签名），有歧义输入
返回 `None`。

#### 跨语言兼容

各编解码器不仅通过自往返测试，还使用明确的外部 wire fixture：与 Python
`msgpack`/`cbor2` 匹配的字节、CPython 3 protocol-2 pickle、规范 bencode、
MongoDB 风格 BSON 文档，以及手写 TOML/YAML/RON/S 表达式/JSON5/Hjson 输入。
精确字节见 `formats` 集成测试。

### 派生与 Schema

自有派生宏支持结构体、元组结构体、泛型、常量泛型和多种枚举表示。主要属性：

- 容器：`rename_all`（含 `serialize`/`deserialize` 方向性写法）、`tag`、`content`、`untagged`、`deny_unknown_fields`、`default`、`transparent`、`crate`、`bound`（含方向性 `bound(serialize=…, deserialize=…)`）、`into`、`from`、`try_from`、`remote`、`expecting`；
- 字段：`rename`、`alias`、`default`、`skip`、`skip_serializing`、`skip_deserializing`、`skip_serializing_if`、`flatten`、`borrow`、`with`、`serialize_with`、`deserialize_with`、`getter`；
- 变体：`rename`、`rename_all`、`skip`、方向性 skip。

属性同时接受 `#[njson(...)]`、`#[nextjson(...)]` 与 `#[serde(...)]` 三种写法，迁移既有 serde 类型时无需改写属性。

每个派生类型同时提供 `const SCHEMA: TypeSchema`：

```rust
# use nextjson::{NsonDeserialize, NsonSerialize};
#[derive(NsonSerialize, NsonDeserialize)]
struct Point { x: i32, y: i32 }

let schema = nextjson::schema_of::<Point>();
let json_schema = nextjson::to_json_schema::<Point>();
# let _ = (schema, json_schema);
```

### 安全与资源边界

- `DecodeSlot<T>` 使用 `Option<T>` 状态检查，不公开 `MaybeUninit<T>` 契约；
- 派生字段使用 RAII 槽位，错误和重复字段路径会正常析构；
- JSON 和 CBOR 默认最多嵌套 128 层；
- 整数使用检查运算，支持完整 Rust `i128/u128` 范围；
- 拒绝非有限浮点、非法 UTF-8、非法 surrogate、尾随逗号和尾随数据；
- `from_slice` / `from_str` 针对完整内存输入；`from_reader`（std）从任意 `std::io::Read` 增量拉取（见 `StreamDecoder`）；
- 库不能替代应用层的总长度、集合长度、CPU 时间和输出配额。

详细说明见[安全模型](docs/SAFETY_CN.md)。

### Benchmark

自有 benchmark 比较同一份 128 记录数据的四条路径：原生 JSON nextencode、原生
JSON nextdecode、JSON 到 CBOR、CBOR 到 JSON。它不依赖外部库，也不制造“普遍更快”
结论。

```text
cargo bench --locked -p nextjson --bench format_comparison
```

复现方法和输出格式见[可复现基准测试](docs/BENCHMARKS_CN.md)。

### 验证

```text
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check -p nextjson --no-default-features --locked
cargo doc --workspace --all-features --no-deps --locked
cargo tree --workspace --all-features --edges normal,build,dev
```

## 许可

Apache-2.0

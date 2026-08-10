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

### 派生与 Schema

自有派生宏支持结构体、元组结构体、泛型、常量泛型和多种枚举表示。主要属性：

- 容器：`rename_all`、`tag`、`content`、`untagged`、`deny_unknown_fields`、`default`、`transparent`、`crate`、`bound`；
- 字段：`rename`、`alias`、`default`、`skip`、`skip_serializing`、`skip_deserializing`、`skip_serializing_if`、`flatten`、`borrow`、`with`、`serialize_with`、`deserialize_with`；
- 变体：`rename`、`alias`、`skip`、`other`。

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
- reader API 会缓冲完整输入，服务端必须在传输层限制总字节数；
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

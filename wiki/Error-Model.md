# Error Model

NextJson 的错误设计目标是：**精确定位 + 粗分类 + 可被格式错误包装**。

## 一个错误长什么样

```rust
let err = nextjson::nextdecode::<MyType>(br#"{"a": 1, "b": ]}"#).unwrap_err();

err.line();    // Some(1)
err.column();  // Some(14)
err.offset();  // 14
err.classification();  // 粗分类：解析/类型/字段...
```

每个解析错误都记录**触发位置**；字节流输入还带精确的 1-based 行/列。`Error` 是
`#[derive(Debug, Clone)]`，不是不透明句柄——用户可以看、可以 clone、可以跨线程
传。

## `Error` 结构

```rust
pub struct Error {
    kind: ErrorKind,
    line: Option<u32>,      // 1-based，字节流输入有值
    column: Option<u32>,    // 1-based
    offset: usize,          // 输入偏移
}
```

- `kind` 是私有的 `ErrorKind`，提供语义化分类（见下）；
- 行/列只在**字节流输入**上精确（`from_reader` 的流式输入也能定位）。

## `ErrorKind` 变体（错误语义的真相来源）

```rust
enum ErrorKind {
    Eof,
    Expected { what: &'static str, found: Option<u8> },
    InvalidNumber,
    NumberOutOfRange,
    ControlCharInString,
    InvalidEscape(char),
    InvalidSurrogate,
    InvalidUtf8,
    RecursionLimitExceeded,
    UnknownField(String),
    MissingField(&'static str),
    UnknownVariant(String),
    InvalidType { expected: &'static str, found: &'static str },
    InvalidLength { len: usize, expected: &'static str },
    NonFiniteFloat,
    Custom(String),
}
```

构造辅助（公开 API）：

```rust
Error::custom(msg)
Error::missing_field("name")
Error::unknown_field(field)
Error::unknown_variant(variant)
Error::invalid_length(len, "expected ...")
Error::invalid_type("expected ...", "found ...")
```

这些辅助函数同时也是**给手写 `NsonDeserialize` 实现**用的工具——你的类型在解码
时可以用它们产生与派生代码一致的错误。

## 容器级类型错误的"类型名"注入（Phase 13）

一个细节值得单独说：结构体/枚举解码时遇到类型不匹配（比如 JSON 里是个数组，
但类型期望对象），错误里的 `expected` 会带上**类型名**：

```rust
// 解码 {"a": 1} 到 struct User { b: u64 }
// 错误消息里会包含 "User"，而不是泛泛的 "a struct"
```

机制是 `FormatDecoder::set_expecting(&'static str)`——`Decoder` 把它存在字段里，
`invalid_type` 对结构性 token 期望（`'{'`/`'['` 等）用它替换描述。派生宏在
`nextdecode_into` 入口自动设置，所以嵌套时自然覆盖；标量描述（number/string/
bool 等）不受污染。

## `FormatError`：格式自己的错误

每种格式可携带自己的错误类型，只需满足：

```rust
pub trait FormatError: From<crate::Error> {
    fn custom(msg: impl Into<String>) -> Self;
}
```

- `From<Error>` 让泛型序列化代码里的 `?` 自动把 `nextjson::Error` 转成格式错误；
- 内置格式都用 `nextjson::Error`，所以转换是恒等映射；
- 第三方格式可定义自己的错误枚举。

## `Result` 别名：双参带默认

```rust
pub type Result<T, E = Error> = core::result::Result<T, E>;
```

- 类型别名默认参数是**稳定特性**（关联类型默认值不是），因此
  `Result<()>` 与 `Result<(), CodecError>` 都合法；
- trait 方法签名必须与 impl **逐字匹配**（rustc 不归一化别名），所以格式 impl
  必须写 `Result<(), Self::Error>` 而非 `Result<()>`。

## 与 serde 的差异

| | serde | nextjson |
| --- | --- | --- |
| `serde::Error` | 不透明 trait，无分类 | — |
| `serde_json::Error` | line/column | line/column/offset + `classification()` |
| 自定义错误 | 每 crate 自定 | `FormatError` trait 统一包装 `From<Error>` |

## 用户代码里的典型用法

```rust
let bytes = nextjson::nextencode(&value)?;      // Error: 编码失败（如 NaN 写 JSON）
let parsed: MyType = nextjson::nextdecode(&bytes)
    .map_err(|e| {
        let pos = (e.line(), e.column(), e.offset());   // 精确定位
        e
    })?;
```

## 相关页面

- 契约中的错误类型：[[Core Contracts]]
- 安全语义：[[Safety Model]]
- 各格式的错误行为：[[Format Matrix]]


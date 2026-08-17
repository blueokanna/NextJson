# NextJson

## 中文文档 - [English Documentation](README.md)

## Wiki

仓库的 GitHub Wiki 由 `/wiki` 目录内容发布而来：
[GitHub Wiki](https://github.com/blueokanna/NextJson/wiki)

NextJson 是面向 Rust 的**数据契约引擎**：零依赖、`no_std + alloc`，为受控协议
和资源约束环境而设计。它不是"另一个 serde"，也不声称替代面向设备间高频链路的
Postcard。它把三个性质做成了一等能力：

1. **schema-first**——类型不只负责编解码，还负责*描述契约*。每个派生类型携带
   `const SCHEMA: TypeSchema`：一个在 `const` 上下文构建的编译期元数据树，可
   在运行时内省、渲染为 JSON Schema、用于校验进入系统的数据，并与上一版本
   diff 以检测协议破坏。
2. **multi-format**——同一类型切换线格式是一等操作，而不是适配器。16 种格式
   通过格式中立的 `FormatEncoder` / `FormatDecoder` 契约共享同一份
   `NsonSerialize` / `NsonDeserialize` 实现；经由统一事件流的格式间中继被验证
   与直接编码字节完全一致。
3. **reuse-first**——持续解码优先考虑内存与分配复用。类型化解码通过带检查的
   `DecodeSlot` 状态直接写入字段（无中间树、无占位值），未转义字符串借用输入
   缓冲区，统一 token 流在不拖累热路径的前提下支持内容重放。

对高频设备间通信，真正重要的是字节数、确定性、版本兼容和延迟——
统一 API 不会自动赢得这些指标。NextJson 的价值在线的契约层：描述它、校验什么
能进入、以及当改动破坏对端时把它检测出来。

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
| `simd`   |   否 | 可选的架构加速：JSON 字符串扫描在 x86-64 使用 SSE2 + 运行时检测的 AVX2，在 aarch64 使用 NEON，其它平台回退到可移植寄存器宽度 SWAR。`unsafe` 代码被限定在 `scan` 模块并仅在启用该 feature 时编译；默认构建保持 `#![deny(unsafe_code)]` 零 `unsafe`。 |

### 原生 nextencode / nextdecode

设计是 **data-model-first**（而非 AST-first）：类型化解码直接把字节流解码进
你的字段，零中间树；`Value` 是同一解码器上的可选消费者。公开两种编码策略：
`Encoder` 每次调用都校验事件协议；`FastEncoder`（`nextencode` / `to_vec` /
`to_string` / writer 入口使用）信任派生验证过的调用序列，跳过每值检查以换取
约 2x 编码吞吐。

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

### 契约三支柱

#### 支柱一：schema 作为编译期契约

每个派生类型暴露 `const SCHEMA: TypeSchema`——在 `const` 上下文构建的 `Copy`
元数据树。它不是会漂移的文档：它与驱动编解码的属性解析来自同一来源，因此
描述与线上行为不可能不一致。

```rust
# use nextjson::{NsonDeserialize, NsonSerialize};
#[derive(NsonSerialize, NsonDeserialize)]
struct Point { x: i32, y: i32 }

let schema = nextjson::schema_of::<Point>();
let json_schema = nextjson::to_json_schema::<Point>(); // draft-07 风格
# let _ = (schema, json_schema);
```

schema 树是另外两根支柱的输入：校验与版本兼容性检查。

#### 支柱二：安全策略进入 schema

schema 不只描述"数据长什么样"，还声明"数据允许怎样进入系统"。限制通过派生
属性声明并携带在 `SCHEMA` 内；`nextjson::validate` 将解码出的 `Value` 对照
schema 逐节点校验，报告所有违规，以及敏感字段的路径供日志脱敏。

```rust
use nextjson::{NsonDeserialize, NsonSerialize};

#[derive(NsonSerialize, NsonDeserialize)]
#[njson(max_depth = 4, deny_unknown_fields)]
struct Request {
    #[njson(max_str_len = 64)]
    name: String,
    #[njson(max_items = 100, min = 0, max = 1000)]
    samples: Vec<i32>,
    #[njson(sensitive)]
    token: String,
}

let input = br#"{"name":"NextJson","samples":[1,2,3],"token":"secret"}"#;
let decoded: nextjson::Value = nextjson::nextdecode(input)?;
let report = nextjson::validate_value::<Request>(&decoded);
assert!(report.is_ok());          // 策略通过
for v in &report.violations {     // 或逐条检查违规
    // 例如 path "name" 上的 ViolationKind::StringTooLong { max: 64 }
}
// report.sensitive == ["token"] — 日志前先脱敏
```

已声明的限制（全部可选、全部 const 可构造）：

| 属性                  | 作用域              | 生效对象                           |
| --------------------- | ------------------- | ---------------------------------- |
| `max_str_len = N`     | 字段 / newtype 变体 | 字符串长度（Unicode 标量数）       |
| `max_items = N`       | 字段 / newtype 变体 | 数组元素数 / 对象条目数            |
| `min = N` / `max = N` | 字段 / newtype 变体 | 数字（闭区间，`i128`/`u128` 精确） |
| `sensitive`           | 字段 / newtype 变体 | 仅报告、永不拒绝（脱敏）           |
| `max_depth = N`       | 容器                | 该类型以下的容器嵌套               |
| `deny_unknown_fields` | 容器                | 结构体与带 tag 枚举的未知键        |

运行时调节走 `ValidateConfig`：全局嵌套上限（`max_depth`）与消息大小上限
（`max_message_size` + 实际 `message_len`）——后者是字节层关切，Value 遍历器
自身无法测量。

校验是解码后的闸门：作用于已物化的 `Value`，不触碰热路径。它一趟收集全部违规
（收集式而非快速失败式），生产闸门可以一次性记录所有违规路径。

#### 支柱三：把版本兼容变成 schema diff

因为 schema 是值，协议演进就变成了纯函数：`nextjson::check(旧_schema, 新_schema)`
报告所有可能破坏"旧读者消费新数据"（前向）或"新读者消费旧数据"（后向）的改动，
并给出逐条严重级别。

```rust
use nextjson::{check_between, Severity, NsonDeserialize, NsonSerialize};

#[derive(NsonSerialize, NsonDeserialize)] struct V1 { id: u64, name: String }
#[derive(NsonSerialize, NsonDeserialize)] struct V2 { id: u64, name: String, email: String }

let report = check_between::<V1, V2>();
assert!(!report.backward_compatible); // 旧数据缺少新必填字段
assert!(report.forward_compatible);
assert_eq!(report.worst_severity(), Some(Severity::Critical));
```

可检测的类别：

| 改动                                           | 级别     | 受影响方向          |
| ---------------------------------------------- | -------- | ------------------- |
| 新增必填字段                                   | Critical | 后向                |
| 删除必填字段                                   | Critical | 前向                |
| 字段 / 变体改名                                | Critical | 双向                |
| 类型族改变（string→number、struct→seq、……）    | Critical | 双向                |
| 新增 / 删除枚举变体                            | Critical | 前向 / 后向         |
| tag 表示改变（`tag` / `content` / `untagged`） | Critical | 双向                |
| 可选字段变必填                                 | Critical | 后向                |
| 浮点变整数                                     | Critical | 后向                |
| 整数范围收窄                                   | Warning  | 后向                |
| 整数变浮点                                     | Warning  | 前向                |
| 必填字段变可选                                 | Warning  | 前向                |
| 默认值改变                                     | Note     | —（语义）           |
| 安全策略改变                                   | Note     | —（不影响线上字节） |

这是*静态*报告：它不知道线上的实际数据。`Warning`（例如 `i32` → `u8`）只有在
真实数据永不超出新范围时才安全。建议在每次发布候选的 CI 中运行它。

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

事件顺序校验是集中式的：格式编码器与跨格式 sink 共用同一个协议状态机，唯一
参数是该线格式是否有显式数组分隔符（JSON 有，CBOR 没有）。解码侧字节词法器
按源字节直接服务类型化标量读取，统一 token 流保留给内容重放而不拖累热路径。

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

| 分组           | 格式                                                                       |
| -------------- | -------------------------------------------------------------------------- |
| 文本、自描述   | `json`、`json5`、`hjson`、`yaml`、`toml`、`ron`、`sexpr`、`csv`、`urlform` |
| 二进制、自描述 | `cbor`、`msgpack`、`bson`、`bencode`、`pickle`                             |
| 二进制、轻模式 | `postcard`                                                                 |
| 环境           | `envy`（仅反序列化，需要 `std`）                                           |

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

| 格式 | 标量类型 | 容器类型 | 特性与限制说明 |
| --- | --- | --- | --- |
| **JSON** | `null`, `bool`, `int`, `float`, `str` | `array`, `object` | RFC 8259，完整模型 |
| **JSON5** | 同 JSON + `Infinity` / `NaN` | `array`, `object`（+ 注释、未加引号键、单引号、尾随逗号） | 编码器输出严格 JSON |
| **Hjson** | 同 JSON | `array`, `object`（+ 未加引号键/字符串、注释） | 编码器输出严格 JSON |
| **YAML** | `null`, `bool`, `int`, `float`, `str` | 块式 + 流式子集 | 块式 map/序列（`key: value`、`-`、`---`、`{...}`/`[...]`）；块标量 `|` / `>`（含 `-`/`+` chomping 与缩进指示符）；锚点 `&name` 与别名 `*name`（块上下文，复制解析 + 100 万节点展开预算）；标准 tag（`!!str`/`!!int`/`!!float`/`!!bool`/`!!null`，拒绝自定义 tag）；支持 merge 键 `<<:`、文档结束标记 `...`；拒绝非有限浮点（`.inf`/`.nan`）与多文档流 |
| **TOML** | `bool`, `int`, `float`, `str`（无 `null`） | 表、数组、内联表、多行字符串 | 拒绝裸标量根；支持 `"""`/`'''` 多行字符串与 `\` 续行；支持 10/16/8/2 进制整数（含 `_` 分隔符）；严格校验日期时间形态（TOML 1.0 四种形态：offset/local date-time, date, time）后保留为字符串 |
| **RON** | `bool`, `int`, `float`, `str`, `char` | `map`, `seq`, 元组, 结构体, 枚举 | `Some(...)` 包装可双向往返 |
| **S-expr** | 原子、带引号字符串、数字、`#t`/`#f`, `nil` | 列表（`map` 编为 `alist`） | 无模式 `Value` 解码嵌套 `map` 存在歧义，请使用类型化目标 |
| **CSV** | `int`, `float`, `bool`, `str` | 行、带表头的对象行 | RFC 4180 |
| **Urlform** | `int`, `float`, `bool`, `str` | 仅扁平 key/value `map` | RFC 3986 百分号编码 |
| **CBOR** | `null`, `bool`, `int`, `float`, `str` | `array`, `map` | RFC 8949 JSON 兼容 Profile，经事件流中继 |
| **MessagePack** | `nil`, `bool`, `int`, `float`, `str` | `array`, `map` | JSON 兼容标量/容器族；不支持 `bin`/`ext`；拒绝超出 64 位的 128 位整数；非有限浮点线上无损透传，但中继到无法表示它们的格式（JSON、CBOR）时报错 |
| **BSON** | `null`, `bool`, `int32`, `int64`, `double`, `str` | `document`, `array` | 文档形态（拒绝裸标量根） |
| **Bencode** | 整数, UTF-8 字符串 | `list`, `dict` | Key 规范排序；无 `null`/`float`；`bool` 映射为 `1`/`0` |
| **Postcard** | `null`, `bool`, 无符号整数, `str` | `seq`, `map` | 非自描述：拒绝有符号整数、`float`、`Option`、`Value` 和 `peek` |
| **Pickle** | `None`, `bool`, `int`, `float`, `str` | `list`, `dict`, `tuple` | CPython 协议 2 子集；128 位整数经 `LONG1` 处理 |
| **Envy** | `int`, `float`, `bool`, `str` | 扁平 `map`（环境变量） | 仅反序列化；需要 `std` |

`detect()` 是启发式且刻意保守：只认定强结构签名（pickle 协议头、bencode 开头、
BSON 长度前缀、文本格式 ASCII 开头、MessagePack/CBOR 二进制签名），有歧义输入
返回 `None`。

#### 跨语言兼容

各编解码器不仅通过自往返测试，还使用明确的外部 wire fixture：与 Python
`msgpack`/`cbor2` 匹配的字节、CPython 3 protocol-2 pickle、规范 bencode、
MongoDB 风格 BSON 文档，以及手写 TOML/YAML/RON/S 表达式/JSON5/Hjson 输入。
精确字节见 `formats` 集成测试。

### 格式等价性验证

“一个数据模型、多种线格式、不做有损回退”这个主张由 `tests/equivalence.rs`
中的自动化等价矩阵验证：

- **中继与直接编码字节一致**——对 JSON 兼容家族（JSON、JSON5、Hjson、YAML、
  RON、CBOR、MessagePack）的每一对格式，把值经事件流从一个格式中继到另一个，
  产物必须与目标编码器直接编码的字节完全相同。
- **随机差分**——确定性 LCG 生成 200 个嵌套值；每个值在整个家族内中继并保持
  字节一致。
- **边界值**——精确 `i128`/`u128`、`f64` 极值、`-0.0`、Unicode 标量边界与
  控制字符，穿过每种能表示它们的线格式。
- **歧义语义**——重复键处处以最后一次出现为准；未知字段被无模式 `Value` 消费者
  保留。

这个平台抓到过真实的 codec bug（JSON5/Hjson 的 `\u` 转义与代理对、YAML 单引号
标量折叠换行、文本编解码器丢整值浮点的浮点性、YAML 根空容器不输出字节），并防止
它们回归。

### 派生与 Schema

自有派生宏支持结构体、元组结构体、泛型、常量泛型和多种枚举表示。主要属性：

- 容器：`rename_all`（含 `serialize`/`deserialize` 方向性写法）、`tag`、`content`、`untagged`、`deny_unknown_fields`、`default`、`transparent`、`crate`、`bound`（含方向性 `bound(serialize=…, deserialize=…)`）、`into`、`from`、`try_from`、`remote`、`expecting`（覆写反序列化错误消息中的类型描述；派生实现自动把它安装到解码器，因此容器级类型不匹配如 `begin_object` 遇 `[` 会报告类型名而非裸 `'{'`；默认是类型的完整路径）；
- 字段：`rename`、`alias`、`default`、`skip`、`skip_serializing`、`skip_deserializing`、`skip_serializing_if`、`flatten`、`borrow`、`with`、`serialize_with`、`deserialize_with`、`getter`，以及安全策略属性 `max_str_len`、`max_items`、`min`、`max`、`sensitive`；
- 变体：`rename`、`rename_all`、`skip`、方向性 skip，以及（newtype 变体上）同一组安全策略属性，作用于其内层字段。

属性同时接受 `#[njson(...)]`、`#[nextjson(...)]` 与 `#[serde(...)]` 三种写法，迁移既有 serde 类型时无需改写属性。

这不是 Serde drop-in 语义保证。Visitor/错误语义以及外部 adapter（尤其是大整数、
定长字节、曲线点和 feature-gated 类型）必须单独验证；详见
[Serde 兼容性契约](https://github.com/blueokanna/NextJson/blob/main/docs/SERDE_COMPATIBILITY.md)。

派生宏完全用标准 `proc_macro` API 实现（无 `syn`、`quote`、`proc-macro2`）。
取舍说得很直白：手写解析器无法提供与完整 `syn` 移植同等的 span 级诊断，因此当它
不理解某个 item（包括未来它没见过的 Rust 语法）时，会以指名消息大声失败，而不是
从误解析的子集生成 impl。泛型、`where` 子句、生命周期、路径、`PhantomData` 与
全部四种枚举表示都有支持并被集成测试覆盖。

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

### 示例

`nextjson/examples/` 下有 6 个完整可运行的程序（每个都返回 `Result` 并打印
结果，用 `cargo run -p nextjson --example <名称>` 运行）：

| 示例                 | 演示内容                                                                                                             |
| -------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `contract_engine`    | schema-first：`#[njson]` 策略属性编译进 `SCHEMA`、对敌意载荷的校验闸门、JSON Schema 导出、版本兼容性 `check_between` |
| `multi_format`       | 同一个值走遍全部 14 种线格式：编码体积、精确往返、跨格式转码链                                                       |
| `cross_format_relay` | 流式 JSON ⇄ CBOR 中继（不构造中间 `Value`）、writer 变体、批量体积对比                                               |
| `zero_copy_reuse`    | 借用型 `&str` / `Bytes` 解码（指针级验证零拷贝）、`DecodeSlot` 在持续解码循环中的复用                                |
| `streaming_reader`   | 从任意 `std::io::Read` 增量解码（`from_reader`、`StreamDecoder`），用分片"慢速 socket"读取器演示                     |
| `custom_codec`       | 手写 `NsonSchema` / `NsonSerialize` / `NsonDeserialize` 与 `#[njson(with = "module")]` 字段级定制编解码              |

```text
cargo run -p nextjson --example contract_engine
```

### Benchmark

自有 benchmark 比较同一份 128 记录数据在 14 种可表示该数据的线格式上的编码/解码
吞吐与编码体积（注册的 16 种格式中：`envy` 读取进程环境而非线格式，`urlform`
只能表示扁平 map，故不计入）。工作区不引入任何对比库，也不制造"普遍更快"结论。

```text
cargo bench --locked -p nextjson --bench format_comparison
```

另有**独立于工作区之外的 crate**（`benchmarks/serde-comparison/`）在同一数据上
对比 nextjson 与 serde 生态的 **11 种格式**：JSON（serde_json 与 simd-json）、
JSON5（serde_json5）、YAML（serde_yaml）、RON（ron）、MessagePack（rmp-serde）、
CBOR（ciborium）、TOML（toml）、BSON（bson）、postcard（postcard）与 bincode
（bincode，nextjson 无对应格式故标注 `na`），并额外测量字符串密集的*长文本*
JSON 数据——这正是 `simd` 特性加速字符串扫描最相关的负载。`Vec<Record>`
fixture 覆盖有符号/浮点/嵌套，文档形态或无符号格式（TOML、BSON、postcard）
用 `Config` fixture；每种格式在编码前先做往返 self-check。它持有自己的
Cargo.lock，工作区依赖审计不受影响。

```text
cd benchmarks/serde-comparison && cargo run --release
```

输出为 CSV（`case,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps`），
窗口时长用 `NEXTJSON_BENCH_MS` 调节（默认 2000 ms）。GitHub Actions 工作流
（`.github/workflows/benchmark.yml`）以启用 `simd` 特性的方式运行两套基准，
合并为 `benchmarks/results/Github_Action_Benchmark.md`，作为工作流产物上传，
并在 `main` 分支 / 手动触发 / 每周定时时提交回仓库。复现方法和输出格式见
[可复现基准测试](docs/BENCHMARKS_CN.md)。

### 验证

```text
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check -p nextjson --no-default-features --locked
cargo check -p nextjson --all-features --locked   # 再用固定工具链验证 MSRV
cargo doc --workspace --all-features --no-deps --locked
cargo tree --workspace --all-features --edges normal,build,dev
```

## 许可

Apache-2.0

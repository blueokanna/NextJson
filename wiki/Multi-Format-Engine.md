# Multi-Format Engine

`nextjson::formats` 是零依赖的**多格式编解码引擎**：一套 `NsonSerialize` /
`NsonDeserialize` 实现，服务 16 种线格式。本页讲清楚"一份实现怎么同时服务
16 种格式"这个机制。

## 先建立直觉：格式是"终点"，不是"类型的一部分"

```rust
use nextjson::formats;

let value = ("NextJson", vec![1_u64, 2, 3], true);

let json     = formats::encode_with(&value, formats::Json)?;
let msgpack  = formats::encode_with(&value, formats::MsgPack)?;
let yaml     = formats::encode_with(&value, formats::Yaml)?;
```

你的类型只发射**事件**（`begin_array`、`write_u64(1)`、`end_array`……），
`formats::Json` / `formats::MsgPack` / `formats::Yaml` 是**不同的终点**。类型
不用知道终点是谁——这就是"一套实现驱动 16 种格式"的字面意思。

## 核心抽象：格式是一等值

```rust
pub trait Format {
    const NAME: &'static str;
    const MIME: &'static str;
    const EXTENSIONS: &'static [&'static str];
    const BINARY: bool;
    fn encode<T: NsonSerialize + ?Sized>(&self, value: &T) -> Result<Vec<u8>>;
    fn decode<'de, T: NsonDeserialize<'de>>(&self, input: &'de [u8]) -> Result<T>;
}
```

格式是**零尺寸标记类型**（`Json`、`MsgPack`、`Yaml`……），可按值传递、存储、
比较名字、动态选择：

```rust
use nextjson::formats;

let kind: Option<FormatKind> = formats::by_extension("toml");
let detected: Option<FormatKind> = formats::detect(br#"{"a":1}"#);
let json = formats::encode_with(&42_i64, formats::Json)?;   // 按值选格式
```

注册表 API：`all()`、`by_name(name)`、`by_extension(ext)`、`detect(bytes)`。

## 统一类型化 API

```rust
formats::encode_with(&value, format)   // T → 某格式字节
formats::decode_with::<T>(bytes, format)
formats::transcode(src, From, To)      // 格式间无类型互转
formats::to_value(bytes, format)       // 解码为 Value
```

## 编码侧：两种实现策略

### 直接发射的编码器

多数格式直接消费 `FormatEncoder` 事件流（`json`、`msgpack`、`cbor`、`ron`、
`sexpr`、`csv`、`urlform`、`hjson`、`json5`、`bencode`、`pickle`、`bson`、
`postcard`）。每个格式实现 `FormatEncoder`，把事件翻译成自己的字节：

- JSON 编码器：`begin_array` → `[`，`separator` → `,`；
- msgpack 编码器：`begin_array` → 数元素个数 → `0x9X` 长度前缀（用 `separator`
  当计数器，见 [[Core Contracts]]）；
- 同一个 `begin_array` 事件，两种格式写出的字节完全不同。

### 收集式编码器（`tree.rs` 共享中继层）

`toml` / `yaml` 因**表顺序要求**先收集为 `Value` 再发射。Phase 4b 把重复的
"Value 树 → token 流"转换收敛到单一共享层：

- `value_to_tokens(&Value) -> Vec<Token<'static>>`（收敛 cbor/envy/pickle/toml/yaml
  5 份重复）；
- `TreeDecoder<'de>`（转发 `Decoder<'static>` + `Cow` 重新生命周期）；
- `CollectEncoder<W>`（收集事件流到 `Value` 树，服务 toml/yaml）；
- `number_string(&Number) -> String`（收敛 csv/toml/yaml 3 份）。

真实解码器（bencode/bson/csv/hjson/json5/msgpack/postcard/ron/sexpr/urlform）
逻辑各异，**保持各自实现**——合并才是乱来。共享的只是"树→token"这种真正
重复的部分。

## 泛型化的代价与收益

- `NsonSerialize::nextencode<E: FormatEncoder>` 让一份 impl 服务所有格式；
- 代价：每个值都穿过通用契约（每值栈帧/深度检查/`start_value`），JSON 热路径
  慢于专精实现（见 [[Performance]]）；
- 收益：新增格式只需要实现 `FormatEncoder`/`FormatDecoder`，已有类型**零改动**。

> 性能注记：库内是纯泛型单态化（零 `dyn`），所以"通用契约拖慢 JSON"的代价是
> 每值协议检查，而不是动态派发。编码入口走免校验的 `FastEncoder` 后，这个代价
> 已经大幅压缩（见 [[Performance]]）。

## 格式注册表（16 种）

| 分组 | 格式 |
| --- | --- |
| 文本、自描述 | `json`、`json5`、`hjson`、`yaml`、`toml`、`ron`、`sexpr`、`csv`、`urlform` |
| 二进制、自描述 | `cbor`、`msgpack`、`bson`、`bencode`、`pickle` |
| 二进制、轻模式 | `postcard` |
| 环境 | `envy`（仅反序列化，需 `std`） |

每种格式的能力边界（诚实局限）见 [[Format Matrix]]。

## 相关页面

- 事件契约：[[Core Contracts]]
- 格式能力矩阵：[[Format Matrix]]
- 跨格式流式中继：[[Cross-Format Relay]]


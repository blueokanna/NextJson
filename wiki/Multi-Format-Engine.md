# Multi-Format Engine

`nextjson::formats` 是零依赖的**多格式编解码引擎**：一套 `NsonSerialize` /
`NsonDeserialize` 实现，服务 16 种线格式。

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

## 实现分层（高内聚低耦合）

### 直接发射的编码器

多数格式直接消费 `FormatEncoder` 事件流（`json`、`msgpack`、`cbor`、`ron`、
`sexpr`、`csv`、`urlform`、`hjson`、`json5`、`bencode`、`pickle`、`bson`、
`postcard`）。

### 收集式编码器（`tree.rs` 共享中继层）

`toml` / `yaml` 因**表顺序要求**先收集为 `Value` 再发射。Phase 4b 把重复的
"Value 树 → token 流"转换收敛到单一共享层：

- `value_to_tokens(&Value) -> Vec<Token<'static>>`（收敛 cbor/envy/pickle/toml/yaml
  5 份重复）；
- `TreeDecoder<'de>`（转发 `Decoder<'static>` + `Cow` 重新生命周期）；
- `CollectEncoder<W>`（收集事件流到 `Value` 树，服务 toml/yaml）；
- `number_string(&Number) -> String`（收敛 csv/toml/yaml 3 份）。

真实解码器（bencode/bson/csv/hjson/json5/msgpack/postcard/ron/sexpr/urlform）
逻辑各异，**保持各自实现**——合并才是乱来。

## 泛型化的代价与收益

- `NsonSerialize::nextencode<E: FormatEncoder>` 让一份 impl 服务所有格式；
- 代价：每个值都穿过通用契约（每值栈帧/深度检查/`start_value`），JSON 热路径
  慢于专精实现（见 [[Performance]]）；
- 收益：新增格式只需要实现 `FormatEncoder`/`FormatDecoder`，已有类型**零改动**。

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

# Format Matrix

16 种格式的能力边界。**每条"局限"都是诚实声明**：格式只能编码其线格式能无损
表示的值；不兼容组合返回明确错误，绝不静默有损。本页除了列出边界，还解释每条
边界**在实际中意味着什么**。

## 分组总览

| 分组 | 格式 | 特征 |
| --- | --- | --- |
| 文本、自描述 | json、json5、hjson、yaml、toml、ron、sexpr、csv、urlform | 人可读 |
| 二进制、自描述 | cbor、msgpack、bson、bencode、pickle | 紧凑、带类型信息 |
| 二进制、轻模式 | postcard | 最紧凑、非自描述 |
| 环境 | envy | 从进程环境反序列化，需 `std` |

## 每种格式的诚实局限（源码模块文档原话级）

### 文本格式

| 格式 | 支持 | 明确拒绝 / 局限 |
| --- | --- | --- |
| `json` | 完整 JSON 数据模型 | 非有限浮点（NaN/Infinity）显式报错；CBOR 的字节串/非字符串键不可达 |
| `json5` | JSON5 子集（注释、单引号、尾逗号、十六进制……） | 模块文档诚实列出支持子集；孤立代理项按规范 → U+FFFD |
| `hjson` | HJSON 子集 | 行内 `#` 注释停止、UTF-8 累积等已修复；文档列支持子集 |
| `yaml` | YAML 子集（块标量、锚点/别名、tag、merge 键等） | **非完整 YAML**：无多文档；`---` 多文档流明确报错；`*`/`&` 在 flow 内报错；别名复制有预算防放大 DoS |
| `toml` | TOML 子集（多行字符串、日期时间、hex/oct/bin 整数） | 文档形态：裸标量根报错"requires a top-level table"；重复键/表重复定义**报错**而非静默覆盖 |
| `ron` | RON 子集 | `Some(...)` 词法递归有深度上限 |
| `sexpr` | S 表达式子集 | 嵌套 map 无模式 `Value` 解码有歧义（用 typed 解码） |
| `csv` | 扁平行 | 标量根解码必失败（Undecided 模式检查 cell）；逐字节 UTF-8 累积 |
| `urlform` | `a=b&c=d` | 百分号解码修复（越界 panic + Latin-1 乱码 → 整体 `from_utf8`）；根必为对象 |

### 二进制格式

| 格式 | 支持 | 明确拒绝 / 局限 |
| --- | --- | --- |
| `cbor` | RFC 8949 **JSON 兼容 profile**：定长/不定长数组、map、文本、u64/i64、tag 2/3 的 u128/i128、半/单/双精度浮点 | 原始 byte string、非字符串键、非有限浮点、未知语义 tag 明确报错 |
| `msgpack` | 完整数据模型 | 计数式：分隔符当计数器；解码正数统一 `Number::U64` |
| `bson` | 文档形态 | 根必须是文档（数组/标量根报错）；模块文档不再声称"根标量包装/解包" |
| `bencode` | 整数/字节串/列表/字典 | **无 bool/null/float**（bool↔1/0 映射）；正数解码不能产 I64（相等性） |
| `pickle` | 常用 opcode 子集 | 经 Value 树转 owned 字符串；`mark_depth` 上限 128；BININT 有符号 32 位、long 最高字节补位均已修复 |
| `postcard` | 非自描述、定宽整数 | **拒绝** `Option` / `Value` / `peek`（无法探测）；**拒绝有符号标量**（UintRecord 模式）；无 float |

### 环境格式

| 格式 | 支持 | 局限 |
| --- | --- | --- |
| `envy` | 从进程环境反序列化 | 仅反序列化；**无 `std` 时报错** |

## 每条局限"在实际中意味着什么"

**"文档形态"（toml / bson）**：这两个格式的顶层必须是表/文档。`encode_with(&42_i64, formats::Toml)` 会直接报错——因为 TOML 里没有"裸数字根"这种写法。这不是偷懒，而是线格式本身不允许。要用它们，包一层结构体或 `Map` 即可。

**"非自描述"（postcard）**：postcard 的线上字节不带类型信息——解码器必须预先
知道"这里是个数字、那里是个字符串"。所以 `Option`（需要探测 None）、`Value`
（需要自描述）、`peek`（需要窥视类型）都无法实现，**直接拒绝**而不是猜。
这是线格式的本质约束，换任何库都一样。

**"无 bool/null/float"（bencode）**：bencode 的数据模型只有整数/字节串/列表/
字典。bool 按约定映射为 `1`/`0`，null 和 float 无法表达 → 拒绝。文档明确列出
映射规则，不会静默把 `true` 编成字符串。

**"拒绝有符号标量"（postcard）**：postcard 的变长整数协议只覆盖无符号
（UintRecord 模式）。有符号标量（`i8`/`i16`/...）在非定宽模式下无法表达 →
拒绝。要定宽有符号，用 `postcard` 的定宽路径。

**"拒绝非有限浮点"（cbor / json）**：`NaN`/`Infinity` 在 JSON 里没有合法写法；
CBOR 虽然原生支持，但走 JSON 兼容 profile 的跨格式语义要求拒绝（否则
CBOR→JSON 会静默丢值）。**显式报错而不是输出 `null`**。

## 跨格式转码的正确性准则

`formats::transcode(src, from, to)` 只在**数据模型兼容**的格式间无损互转：
JSON 数据模型（null/布尔/有限数字/UTF-8 字符串/数组/字符串键对象）是交集；
文档式/轻模式格式按各自文档约束。`detect()` 的优先级是：

1. pickle 协议头（`0x80..=0x85`）
2. bencode intro
3. bson LE 长度
4. 文本 ASCII
5. msgpack / cbor 签名

> 注意：positive fixint（`0x00..=0x7F`）与文本不可区分，`detect()` **不声称**
> 能区分它——这是诚实的检测边界。

## 测试覆盖

- `tests/formats.rs`：41 项（wire / roundtrip / foreign / full_matrix / transcode /
  registry / detect）；
- 全矩阵 164 项（bencode/sexpr 移出矩阵，因无模式 Value 有歧义）；
- 单字节矩阵：每个二进制格式对 `0x00..=0xFF` 逐个解码 `Value` 不 panic；
- 双字节种子组合。

## 相关页面

- 引擎机制：[[Multi-Format Engine]]
- 跨格式中继：[[Cross-Format Relay]]
- 性能：[[Performance]]


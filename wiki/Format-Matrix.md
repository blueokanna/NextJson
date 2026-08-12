# Format Matrix

16 种格式的能力边界。**每条"局限"都是诚实声明**：格式只能编码其线格式能无损
表示的值；不兼容组合返回明确错误，绝不静默有损。

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
| `yaml` | YAML 子集 | **非完整 YAML**：无锚点/别名/多文档/标签等；跳空行与注释、引号感知、嵌套块递归下降均已修复 |
| `toml` | TOML 子集 | 文档形态：裸标量根报错"requires a top-level table"；重复键/表重复定义**报错**而非静默覆盖 |
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

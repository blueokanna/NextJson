# Glossary

本 Wiki 使用的一致术语。每个词条先给一句话定义，再给它的"落点"（在哪一页展开）。

## 核心机制

| 术语 | 含义 | 展开 |
| --- | --- | --- |
| **schema / TypeSchema** | 编译期构造、运行时内省的类型结构描述树；每个 `NsonSerialize` 类型携带 `const SCHEMA` | [[Compile-Time Schema]] |
| **visitor-free** | 不采用 serde 的 `Visitor` 回调模式；解码用 `nextdecode_into` 就地写入 `DecodeSlot<T>` | [[Decode Slot]] |
| **DecodeSlot** | 调用方提供的解码槽；内部是 `Option<T>`，`write` 前类型系统禁止读值 | [[Decode Slot]] |
| **FormatEncoder / FormatDecoder** | 格式中立的输出/输入事件契约；`NsonSerialize`/`NsonDeserialize` 泛型于其上 | [[Core Contracts]] |
| **Token** | 词法层与重放层共享的最小单位：`Null/Bool/Number/Str/BeginObject/EndObject/BeginArray/EndArray` | [[Unified Token Stream]] |
| **Bytes 源 / Tree 源** | `Decoder` 的两种输入：惰性单 token 词法（可借用） / 内容重放（owned） | [[Unified Token Stream]] |
| **CheckedEncoder** | 在事件到达具体格式前校验事件协议（数组分隔符/键值顺序）的包装器 | [[Architecture]] |
| **FastEncoder** | `Encoder<W, false>`：顶层入口使用的免校验编码器，信任派生代码的调用序列，编码约 2x | [[Performance]] / [[Design-Decisions]] |
| **EventSink** | `cross_format` 的格式中立事件协议，用于 JSON↔CBOR 流式互转、不建 `Value` 树 | [[Cross-Format Relay]] |
| **CollectEncoder / TreeDecoder** | `formats/tree.rs` 共享中继层：收集事件到 `Value` 树 / 重放树为解码原语（服务 toml/yaml 等文档式格式） | [[Multi-Format Engine]] |
| **宽度方法** | `write_i8..i32`/`write_u8..u32` 等；默认加宽到 64 位，二进制格式覆写为原生宽度 | [[Core Contracts]] |

## 格式与边界

| 术语 | 含义 | 展开 |
| --- | --- | --- |
| **诚实局限** | 格式只编码其线格式能无损表示的值；不兼容组合报错而非静默有损 | [[Format Matrix]] |
| **JSON 兼容 CBOR profile** | RFC 8949 子集：定长/不定长容器、u64/i64、tag2/3 的 u128/i128、半/单/双精度；拒绝字节串/非字符串键/非有限浮点/未知 tag | [[Format Matrix]] |
| **detect()** | 按字节签名探测格式；对无法区分的输入（如 positive fixint vs 文本）返回 `None` | [[Multi-Format Engine]] |
| **transcode** | 在数据模型兼容的格式间无类型互转（`formats::transcode`） | [[Multi-Format Engine]] |
| **is_human_readable** | 格式是否产生人可读输出；类型可据此分支表示，镜像 serde | [[Core Contracts]] |
| **map_key** | 编码端对象键写出；默认字符串化（JSON 形状），二进制格式覆写为键即值（支持非字符串键） | [[Core Contracts]] |
| **option_tag** | 解码端 `Option` 探测：`None`（已消费）或 `Some`（下一 token 是负载） | [[Core Contracts]] |
| **save / restore** | 解码回溯保存点，untagged 枚举需要；`restore` 无错误通道 | [[Core Contracts]] |
| **Opaque** | `TypeSchema` 变体，表示不可内省的字段（`serialize_with`/`with`/`getter`/skip 字段） | [[Compile-Time Schema]] |

## 工程与门禁

| 术语 | 含义 | 展开 |
| --- | --- | --- |
| **MSRV** | Minimum Supported Rust Version。当前 CI 以 **1.78.0** 作为最低测试版本（`Cargo.lock` v4 需要 Rust ≥ 1.78），工作区所有 crate 都用 `edition = "2021"` | — |
| **依赖审计** | CI job：验证工作区 Cargo.lock 无第三方 `source=`，保证零依赖承诺不被破坏 | [[Design Philosophy]] |
| **覆盖率门禁** | CI `cargo llvm-cov --fail-under-lines 80`（行覆盖率 ≥ 80%） | — |
| **诚实子集** | 每种格式只实现其线格式能无损表达的部分，其余明确报错 | [[Format Matrix]] |


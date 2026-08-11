# 安全模型

## 中文

NextJson 在库根启用 `#![deny(unsafe_code)]`。这是可由编译器执行的代码属性，不是
“任何输入都绝不耗尽内存或 CPU”的虚假承诺。

### 初始化与析构

`NsonDeserialize::nextdecode_into` 接收安全的 `DecodeSlot<T>`。实现必须先调用
`DecodeSlot::write` 才能成功返回；`nextdecode` 会检查槽位状态。派生结构体的字段由
基于 `Option<T>` 的 RAII 槽位管理，错误、缺失字段和重复字段路径均使用正常 Rust
析构语义。

### 输入保证

- JSON 字符串验证 UTF-8、转义和 surrogate pair；
- CBOR 文本验证 UTF-8，map key 必须是文本；
- JSON 和 CBOR 都拒绝尾随根值或垃圾字节；
- 整数解析使用检查运算并覆盖 `i128/u128`；
- CBOR tag 2/3 超过 128 位会报错；
- 非有限 JSON/CBOR 浮点会报错；
- 解码器默认拒绝超过 128 层的嵌套。

### 多格式保证

`nextjson::formats` 中每种格式都服务同一个检查式事件契约。各格式专属保证：

- 带长度前缀的二进制格式（MessagePack、BSON、bencode、pickle）拒绝截断的头部，
  并校验已消费字节数是否与声明长度一致；
- BSON 校验元素类型字节和以 NUL 结尾的字段名，声明长度与消费输入不一致即报错；
- Pickle 执行有界栈机子集：`MARK` 帧、栈深度和长整数（`LONG1`/`LONG4`）大小
  全部做边界检查，未知操作码报错；
- Postcard 非自描述；无模式窥视会被拒绝，因为线上无法在无目标类型时分类下一
  token；
- Bencode、TOML、BSON 拒绝其线格式无法表示的值（TOML/BSON 的裸标量根；
  bencode 的 null/float）；Bencode 的 bool 明确映射为规范整数 `1/0`；
- YAML、TOML、JSON5、Hjson 文本解析器验证 UTF-8，并拒绝未闭合的字符串、引号
  和块结构；
- URL 表单解码校验百分号编码（`%XX`），非法转义报错。

任何格式都不做静默有损回退：线格式无法保真的值在 `encode` 或 `decode` 侧一律
显式报错。

### 跨格式保证

JSON 和 CBOR 通过 `EventSink` 逐事件转换，不构造文档树。结构状态机拒绝多个根值、
容器不匹配、缺少 object value 和 object 外的 key。无法无损表示为 JSON 的 CBOR
byte string、非文本 key、非有限浮点和未知 tag 会明确失败。`formats::transcode`
先解码为 `Value` 再重新发射；源与目标数据模型兼容时遵循同样的“无有损回退”规则，
不兼容的组合会返回错误。

### 零拷贝边界

未转义 JSON 字符串和定长 CBOR 文本直接借用输入范围。JSON 转义处理和 CBOR
不定长文本拼接必须分配新字符串。任何输出编码都必须向目标写入新字节。

### 资源限制

深度限制不等于总资源限制。库不统一限制输入总字节数、字符串长度、集合长度、输出
长度或执行时间。处理不可信流量时，应用必须在传输层和业务层设置这些配额。
`from_reader` 会缓冲完整 reader，必须由调用方提供有界 reader。

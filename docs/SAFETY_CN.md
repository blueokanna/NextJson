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
- 默认嵌套深度上限为 128。

### 跨格式保证

JSON 和 CBOR 通过 `EventSink` 逐事件转换，不构造文档树。结构状态机拒绝多个根值、
容器不匹配、缺少 object value 和 object 外的 key。无法无损表示为 JSON 的 CBOR
byte string、非文本 key、非有限浮点和未知 tag 会明确失败。

### 零拷贝边界

未转义 JSON 字符串和定长 CBOR 文本直接借用输入范围。JSON 转义处理和 CBOR
不定长文本拼接必须分配新字符串。任何输出编码都必须向目标写入新字节。

### 资源限制

深度限制不等于总资源限制。库不统一限制输入总字节数、字符串长度、集合长度、输出
长度或执行时间。处理不可信流量时，应用必须在传输层和业务层设置这些配额。
`from_reader` 会缓冲完整 reader，必须由调用方提供有界 reader。

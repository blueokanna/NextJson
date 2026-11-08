# 安全模型

NextJson 默认构建禁止 unsafe Rust。可选的 `simd` 特性仅在 `src/scan.rs` 中
允许 intrinsic；向量读取前检查长度，AVX2 通过运行时检测后才启用。这些是可检查的
实现约束，不是“零漏洞”或“无需生产验证”的承诺。

## 解码

`DecodeSlot<T>` 与派生字段槽使用正常初始化的 Rust 值。实现未写入槽位却返回成功
会被判定为错误；部分解码的值按 Rust 所有权规则析构，不向自定义解码器暴露裸的
未初始化存储。

JSON 检查 UTF-8、转义、代理项配对、数字语法、容器结构和文档结束位置。忽略字段
也必须通过语法检查。整数解析使用检查运算，覆盖 `i128` 与 `u128`。JSON 与 CBOR
的 JSON 兼容配置拒绝非有限浮点；其它格式的数据模型不同：JSON5 接受非有限字面量，
MessagePack 可以携带非有限值，CSV 与 URL-form 的单元格可能保留为文本。跨格式
转换前应核对格式矩阵，不能假定所有类型或线格式特性都能保留。

解码器默认限制为 128 层嵌套。提高可配置的深度上限前，需要评估目标平台的栈空间。
Schema 策略由验证 API 在解码后的值上执行，不会自动限制解析期间的分配。

内置集合解码不会直接相信 codec 提供的长度提示。初始预留同时受 4096 个元素和
64 KiB 元素存储预算约束，包括外部 `FormatDecoder` 实现；实际读取元素后仍可扩容。
这防止伪造长度触发巨额预分配，不是解码结果的总内存上限。

预读属于解析位置的一部分。JSON 与流式 JSON 保存待消费 token 的起点；原生文本
和二进制格式同时保留所需容器信息，BSON 还保存已经读取的字段类型标签。回归测试
覆盖可选 untagged 枚举，以及某个候选失败后的重试。

CSV 的预读保留单元格原文；URL-form 检查点保存字段源区间；NDJSON 检查点同时
记录当前行和行内位置。NDJSON 在切换记录前验证当前行已完整消费，拒绝行内尾随
token。CSV 与 URL-form 的整数字面量支持完整 128 位范围，越界时报错，不借助
浮点数舍入后继续解码。

## 编码

公开 JSON 入口默认使用校验型 `Encoder`；CBOR 与其它格式入口使用事件校验包装。
多个根值、缺少值、容器不匹配和未完成的事件流返回错误。JSON 编码器与校验包装
在进入第 129 层容器之前拒绝继续递归。

`FastEncoder` 保留为显式选项，省略逐值协议检查。使用它需要验证所有嵌套实现和
自定义钩子，只有外层类型使用 derive 并不足够。历史免校验基准不代表当前默认 API。

编码或写入失败后，应丢弃编码器及部分输出。外部写入不是事务，已经发出的字节无法
回滚。容器深度限制也不约束用户 trait 实现内部任意的递归、循环或计算。

## 流式读取与资源上限

`from_reader` 增量读取，但为回溯保留已消费的字节，不是常量内存接口。输入长度
没有外部约束时，使用 `from_reader_with_limit(reader, max_bytes)`，或配置
`StreamDecoder::with_input_limit`。

配额包括空白。达到上限后最多额外读取一个字节，以区分真正的 EOF 和超限输入；
额外字节不保留。超限返回 `input byte limit exceeded`，恢复解析位置也不能重置
这一状态，因此不会把截断前缀当作完整文档。`Interrupted` 读取会重试，其它 I/O
错误正常传播。

输入字节配额不等于结果对象的堆内存、输出大小、CPU 时间、reader 内部缓冲或网络
等待时间上限。应用仍需按实际协议配置这些限制。库中的所有内存分配失败也并非都能
通过 `Result` 恢复。

## 验证

```text
cargo test --workspace --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check -p nextjson --no-default-features --locked
cargo +1.78.0 test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
```

`tests/production_regressions.rs` 覆盖读取分块、非法忽略字段、I/O 中断、输入配额、
预读回溯、自定义序列化协议错误、深层编码，以及筛选函数 panic 后的 Map 一致性。
其它测试还覆盖外部线格式样本、数值边界、部分初始化析构和 SIMD 与标量对照。
`tests/extreme_regressions.rs` 补充跨行与字段回溯、整数溢出、恶意长度提示，以及
大型 UTF-8 字符串跨读取边界的测试。
通过测试只能证明已覆盖用例的行为，不等于独立安全审计；持续模糊测试和实际部署的
负载验证仍有必要。

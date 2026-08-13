# Design Decisions

本页以 ADR（Architecture Decision Record）风格记录每个关键设计选择的**背景 /
选择 / 后果 / 踩坑**。全部基于仓库源码、提交与测试事实。

> 阅读建议：每条 ADR 是独立的。想快速理解全库骨架，先看
> [[Design Philosophy]] 和 [[Core Contracts]]；这里记录的是"为什么是这一步"
> 的具体决策与踩坑。

## ADR-01：无 Visitor 的就地解码

- **背景**：serde 用 `Visitor` 状态机让 Deserializer 驱动类型；需要"解码到调用方
  提供的存储"时很别扭。
- **选择**：`nextdecode_into(decoder, &mut DecodeSlot<Self>)`，直接写入槽。
- **后果**：内存复用一等公民；不需要 `T: Default`；失去 Visitor 抽象。
- **踩坑**：早期用 `MaybeUninit<Self>` 表达"未初始化" → 改为 `Option<T>`，保住
  `deny(unsafe_code)`（见 [[Decode Slot]]）。

## ADR-02：编译期 schema 作为超 trait

- **背景**：需要"类型自描述"且 schema 与实现同源。
- **选择**：`NsonSchema` 是 `NsonSerialize` 的超 trait，`const SCHEMA: TypeSchema`。
- **后果**：零运行时开销内省、可生成 JSON Schema。
- **踩坑**：超 trait 关联常量只能经超 trait 路径访问
  `<T as NsonSchema>::SCHEMA`，经 `<T as NsonSerialize>::SCHEMA` 触发 E0576。

## ADR-03：统一 Token 流（Bytes / Tree 双源）

- **背景**：内部/邻接/untagged 枚举与 `Value` 往返最易出现"第二套实现"。
- **选择**：`Decoder` 持 `Bytes`（惰性词法）或 `Tree`（内容重放），暴露相同原语。
- **后果**：一套引擎服务所有路径；未转义字符串零分配借用。
- **踩坑**：`Tree` 路径字符串 owned；借用要求 `'de: 'a`。

## ADR-04：非负整数统一 `Number::U64`

- **背景**：`1i32.into()` 得 `I64(1)`，解析 `"1"` 得 `U64(1)` → 相等性撕裂。
- **选择**：所有非负整数统一 `U64`；`I64` 只存负数。
- **后果**：解析与构造的 `Number` 恒等。
- **踩坑**：bencode/msgpack 解码正数也不能产 `I64`（否则同样撕裂）。

## ADR-05：`Bytes<'a>` 借用包装，而非 `Vec<u8>` 特化

- **背景**：`impl NsonSerialize for Vec<T>` + `impl for Vec<u8>` 在 `u8:
  NsonSerialize` 时 E0119 冲突（serde 同此，所以 serde 也不特化）。
- **选择**：`Vec<u8>`/`&[u8]` 走通用序列路径（JSON 数组拼写，与 serde_json 一致）；
  需要原生字节串的用 `Bytes<'a>` 包装 → `write_bytes`（二进制格式覆写为长度前缀
  + 原始字节）。
- **后果**：无损、无 E0119；代价是借用版 `Bytes`（无 owned `ByteBuf`）。

## ADR-06：`Option` 的区分 tag

- **背景**：JSON 里 `Option` 即 null（`write_none`→null、`write_some`→无操作）；
  二进制格式需要区分 `None` 与 `Some(null)`。
- **选择**：`FormatEncoder::write_none`/`write_some` 默认 JSON 形状，二进制格式
  覆写；`FormatDecoder::option_tag() -> OptionTag` 对称。
- **后果**：`None` 在自描述二进制里保持 distinct。
- **踩坑**：CheckedEncoder 的 `write_some` 不能消费 value 槽（否则根级 Option
  双值报错）。

## ADR-07：非字符串 map 键

- **背景**：JSON 只支持字符串键；二进制格式支持任意标量键。
- **选择**：`FormatEncoder::map_key<K>(&K)` 默认 key→字符串（JSON 形状）；
  postcard/msgpack 覆写为"键即值"。
- **后果**：`BTreeMap<u8, V>` 等不经过字符串往返。
- **踩坑**：`FormatDecoder::map_key::<K>()` 要对称。

## ADR-08：宽度保留（`write_i8..i32` / `write_u8..u32`）

- **背景**：JSON 无宽度；定宽二进制（postcard）需要在线上保留源宽度。
- **选择**：宽度方法默认加宽到 `i64`/`u64`；二进制格式覆写为原生宽度。
- **后果**：`i8` 在 JSON 里是普通数字，在 postcard 里是 1 字节定宽。
- **踩坑**：`no_std` 下 `f64::fract()` 不可用，用 `% 1.0` 判断整性。

## ADR-09：`is_human_readable`

- **背景**：类型可能需要针对人类可读/二进制给出不同表示（时间戳、字节串）。
- **选择**：两 trait 各加 `is_human_readable(&self) -> bool`，默认 `true`；二进制
  格式覆写 `false`。
- **后果**：镜像 serde 语义。
- **踩坑**：CBOR 因 JSON 兼容中继保持 `true`（文档说明，否则跨格式行为漂移）。

## ADR-10：`CheckedEncoder` 事件协议校验

- **背景**：事件顺序错误（如数组元素前漏 `separator`）在具体格式里报错很误导。
- **选择**：`CheckedEncoder` 在事件到达格式前校验协议（数组先 `separator`、
  对象先 `key`）。
- **后果**：自定义/第三方编码器得到清晰错误；`CheckedEncoder` 仅 `pub(crate)`。

## ADR-11：共享中继层 `formats/tree.rs`

- **背景**：6 个文件重复"Value 树 → token 流"转换（各 70-100 行）+ 3 处
  `number_string` + 2 处 collect 编码器。
- **选择**：`value_to_tokens` / `TreeDecoder` / `CollectEncoder` / `number_string`
  收敛到单一共享层；toml/yaml 改用它。
- **后果**：toml -303、yaml -298、pickle -199、envy -129、cbor -128、json -61 行；
  rustdoc 需 `pub use` 指向私有模块的项，否则 "links to private item" 警告。
- **边界**：真实解码器（bencode/bson/csv/...）逻辑各异，**不合并**。

## ADR-12：`StreamDecoder` 保留全部缓冲

- **背景**：untagged 枚举的 `save`/`restore` 无错误通道，`restore` 必须能任意
  回溯。
- **选择**：`buf` 从不收缩，`pos` 只前进。
- **后果**：内存随总输入增长；收益是解码从第一批字节开始。模块文档明示"需要
  常量内存流式应在协议层分块"。

## ADR-13：格式的"拒绝有损"原则

- **背景**：serde_json 无 feature 时把 NaN/Infinity 输出为 null；YAML 静默吞
  结构是真实发生过的事故。
- **选择**：每种格式拒绝其线格式无法保真的值；JSON 非有限浮点显式报错。
- **后果**：无静默有损；每种格式模块文档列出支持子集（诚实局限，见
  [[Format Matrix]]）。

## ADR-14：错误带位置 + 分类

- **背景**：调试与上层错误处理都需要定位。
- **选择**：`Error` 带 kind/line/column/offset；`FormatError` 统一包装
  `From<Error>`；`Result` 别名双参带默认（`Result<T, E = Error>`）。
- **后果**：`?` 自动转换；trait impl 必须逐字写 `Result<(), Self::Error>`。
- **踩坑**：关联类型默认值不稳定（E0658），类型别名默认参数是稳定特性。

## ADR-15：`detect()` 的诚实边界

- **背景**：格式探测有歧义。
- **选择**：优先级 pickle 协议头 → bencode intro → bson LE 长度 → 文本 ASCII →
  msgpack/cbor 签名；positive fixint 与文本不可区分时**不声称**。
- **后果**：`detect` 可能返回 `None` 而非猜测。

## ADR-16：性能优先优化"原生宽度整数写出"

- **背景**：基准显示编码瓶颈是 u64/i64 加宽 u128 后的 `__udivti3` libcall。
- **选择**：`ser.rs` 新增 `write_u64_into`/`write_i64_into` 栈缓冲 + 硬件除法。
- **后果**：编码 291 → 368 MB/s（+26%），线格式字节不变；与 serde_json 差距
  2.78x → 2.17x。详见 [[Performance]]。

## ADR-17：与 serde 的措辞纪律

- **背景**：报告曾出现"serde 做不到紧凑二进制 / 无法跨格式"的错误断言。
- **选择**：文档改为——差异在**显式可内省 schema + 单 crate 零依赖 + 无中间
  Value 树**，而非能力。安全对比不做"全面更安全"断言。
- **后果**：Wiki 与 README 的所有对比表述均为此纪律（见
  [[Comparison with serde]]）。

## ADR-18：`FastEncoder` 信任策略（校验 vs 免校验）

- **背景**：同进程 A/B 证明 `Encoder` 的每值协议校验 = **2.0x 编码开销**
  （校验型 440 MB/s vs 免校验发射器 877 MB/s）——这是编码差距的真正大头。
- **选择**：`Encoder<W, const VALIDATE: bool = true>` 泛型化；顶层
  `nextencode`/`to_vec`/`to_string`/writer 入口走 `FastEncoder = Encoder<W, false>`
  （信任派生代码，serde 同款信任模型）；校验型保持为公开默认。
- **后果**：编码约 2x；`const` 折叠裁掉校验分支；key/separator 快路径仍取
  first 标志（输出正确性必需）。
- **踩坑**：裸 `Encoder::new(...)` 调用处必须显式标注
  `Encoder::<_, true>::new(...)`（const 泛型无法从用法推断）。

## ADR-19：SWAR 字符串转义快路径

- **背景**：JSON 字符串转义逐字节扫描是编码热路径的一部分。
- **选择**：`write_escaped_str` 快路径用 **8 字节块 SWAR 检测**（hasless<0x20 +
  has_zero('"') + has_zero('\\') + 高位检测），纯 safe 无 unsafe、no_std 兼容；
  `frames: Vec::with_capacity(32)` 预分配。
- **后果**：长字符串负载收益最大；短字符串/键走尾部逐字节等价路径。
- **踩坑**：`u64::from_le_bytes(try_into().unwrap())` 的 `unwrap` 由编译器消掉
  边界检查（切片长度是编译期常量 8）。


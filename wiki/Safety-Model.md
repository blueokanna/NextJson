# Safety Model

NextJson 把"安全"定义为**可审计 + 拒绝有损**，并写进了 `#![deny(unsafe_code)]`。
本页逐条列出机制与边界。

## 1. 全库无 `unsafe`

```rust
#![no_std]
#![deny(unsafe_code)]
#![deny(missing_docs)]
```

整个 crate（含 `nextjson-derive`）零 `unsafe`。曾计划用 `MaybeUninit` 做就地解码，
最终改用 `DecodeSlot<T>`（内部 `Option<T>`）——"未初始化"由类型系统表达，不需要
`assume_init`（见 [[Decode Slot]]）。

对比：serde / serde_json 内部使用 `unsafe`（反射、浮点解析、`RawValue`）。这是
NextJson 可审计安全主张的核心证据，但**不是**"NextJson 一定更安全"的断言——它
只是把 unsafe 的使用面降为零，同时以更简单的机制达成同样目标。

## 2. 递归上限：默认 128

所有解码路径（`Decoder`、各格式 `FormatDecoder`、`StreamDecoder`、CBOR 中继、
pickle 虚拟机）都施加**深度限制**，防止嵌套攻击导致栈溢出 DoS：

- `DecodeConfig { max_depth: 128 }` 可调；
- pickle 是字节级协议（3 字节/层可构造 20 万层嵌套），专门加了 `mark_depth`
  计数器（上限 128）；ron 的 `Some(...)` 词法递归同样有 `some_depth` 上限。

## 3. 数字：检查式算术，溢出报错

- 整数解析**手写 + 溢出检测**：超出 `u128` 的整数报错，不静默丢精度；
- 宽度读取（`u8`/`i8`/...）用 `try_from` 范围检查；
- JSON 解析拒绝非有限浮点（`NaN`/`Infinity` 输入报错）；
- 编码侧拒绝把 `NaN`/`Infinity` 写进 JSON——**显式报错**，而不是像 serde_json
  （无 feature 时）那样输出 `null` 的有损回退。

## 4. 字符串：UTF-8 / surrogate 校验

每条字符串路径都校验：

- 非法 UTF-8 报错；
- `\uXXXX` 孤立代理项按 JSON 规范替换为 U+FFFD（json5），或报错（严格路径）；
- `%XX` 百分号解码（urlform）从"逐字节 Latin-1 乱码"改为"收集字节后整体
  `from_utf8`"（`%C3%A9` → `é`），并修复了 `%X` 结尾的越界 panic。

## 5. 部分初始化析构安全

解码失败或重复字段时，已初始化的字段按正常 `Option<T>`/drop 语义清理，不存在
"半初始化值泄漏"。

## 6. 诚实错误模型

- `Error` 携带 line / column / offset（字节流输入有精确 1-based 行列）；
- `classification()` 提供粗粒度错误分类；
- 详见 [[Error Model]]。

## 7. 应用侧责任（安全边界声明）

库不假装能替代应用的总量控制。文档明确要求应用自行施加：

- 输入总字节数配额；
- 集合大小配额；
- CPU 时间与输出字节配额。

`from_slice`/`from_str` 处理完整内存输入；`from_reader`（std）从任意
`std::io::Read` 增量拉取。

## 8. 与 serde 的安全对照（摘要）

| 属性 | serde / serde_json | nextjson |
| --- | --- | --- |
| `unsafe` 代码 | 内部使用 | 全库 `deny(unsafe_code)` |
| 错误模型 | `serde_json::Error` 带 line/column；serde `Error` 不透明 | line/column/offset + `classification()` |
| 递归限制 | serde_json 128 | 所有解码器默认 128 |
| 数字溢出 | serde_json 返回溢出错误 | 检查式解析，溢出报错 |
| 非有限浮点（JSON） | 无 feature 时输出 `null` | 显式报错（无静默有损） |
| 派生部分析构 | visitor 局部变量 | `Option<T>` 正常析构 |
| no_std | serde 可；serde_json 仅 std | 核心 `no_std + alloc` |

> 声明边界：本页是**能力对比**，不是"全面更安全"的结论。每种格式的行为严格性
> 也各不同——见 [[Format Matrix]]。

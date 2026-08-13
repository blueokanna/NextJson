# Decode Slot

`DecodeSlot<T>` 是"无 Visitor 就地解码"的落点，也是 NextJson 与 serde 在解码侧
最根本的差异之一。本页讲清楚它是什么、为什么这么设计、以及一个完整例子。

## 先看问题：serde 的解码结果是怎么产生的

serde 的 `Deserialize` 是**返回值**的：`Visitor` 收集完所有字段后构造一个新值
返回。这意味着：

- 每次解码都从零构造（字段一个接一个地 `String::new()`、`Vec::new()`）；
- 想复用上一帧的内存？没有原生的槽位概念，得自己想办法；
- 解码中途失败时，"构造到一半的值"靠 Visitor 的局部变量清理——语义是对的，
  但机制绕。

NextJson 把结果产生的方式反过来：**调用方先给一块存储，类型往里面写**。

## 是什么

```rust
pub struct DecodeSlot<T> {
    value: Option<T>,
}

impl<T> DecodeSlot<T> {
    pub const fn new() -> Self { DecodeSlot { value: None } }
    pub fn write(&mut self, value: T) { self.value = Some(value); }
    pub fn take(&mut self) -> Option<T> { self.value.take() }
    pub fn is_initialized(&self) -> bool { self.value.is_some() }
}
```

`NsonDeserialize::nextdecode_into` 的签名是：

```rust
fn nextdecode_into<D: FormatDecoder<'de>>(
    decoder: &mut D,
    out: &mut DecodeSlot<Self>,
) -> Result<(), D::Error>;
```

## 三个设计巧思

### 1. 用 `Option<T>` 表达"未初始化"，而不是公开 `MaybeUninit<T>`

早期设计曾考虑 `MaybeUninit<Self>`——它确实能表达"还没写"。但有两个问题：

- 读一个未初始化的 `MaybeUninit` 是 UB，而 UB 需要 `unsafe` 来处理；
- "部分初始化"的清理要手写 drop guard，容易错。

换成 `Option<T>` 之后：

- `write` 之前**类型系统禁止**读值（`value` 是私有的，只能 `take()` 出来）——
  错误实现无法在 safe code 里产生 UB；
- 析构语义与普通 `Option<T>` 一致：解码失败或字段重复时，已初始化的字段按
  正常 drop 顺序清理（**部分初始化清理安全**）；
- 库保持 `#![deny(unsafe_code)]`：不需要 `assume_init`。

这是"安全靠类型系统、而不是靠纪律"的典型例子。

### 2. 内存复用，不要求 `T: Default`

serde 的 `Deserialize` 从零构造值；`decode_into` 让**调用方**提供存储：

```rust
let mut slot: DecodeSlot<MyStruct> = DecodeSlot::new();
for frame in frames {
    slot.take();               // 取出上一次结果（值为 None 时无事发生）
    MyStruct::nextdecode_into(&mut decoder, &mut slot)?;  // 复用 slot
    handle(slot.take().unwrap());
}
```

没有 `T: Default`、没有占位值、没有"先构造一个空壳再覆盖"。字段级的 `String`、
`Vec` 在上一次解码结束后已经带着容量，下一帧解码可以继续往里面写。

### 3. 与错误传播的配合

解码中途失败时，`slot` 保持未初始化（或部分初始化但已由 drop 清理），调用方
**永远不会拿到半成品**。失败的 `nextdecode_into` 返回 `Err`，`slot` 的状态是
确定的。

## 派生代码如何使用（机制）

宏生成的反序列化实现分两层：

```text
MyStruct::nextdecode_into(decoder, out)
  └─ 先分配字段级槽：let mut __f_user_id = DecodeSlot::<u64>::new();
     let mut __f_name = DecodeSlot::<String>::new();
  └─ 逐个字段：object_key → 匹配名字 → __f_user_id.nextdecode(...)?  （写字段槽）
  └─ 全部成功后：组装 Self { user_id: __f_user_id.take().unwrap(), ... }
                 写入 out（调用方给的槽）
```

关键点：**字段先写进各自的临时槽，全部成功后才组装最终值**。这样即使第 3 个
字段解码失败，前两个字段的值也只是躺在各自的槽里，随函数返回被正常 drop——
不存在"半成品结构体"。

重复字段（map 中同一个键出现两次）：用 `write` 覆盖旧值，旧值按正常 drop
语义释放，行为可预期。

## 与 serde 的对照

| 维度 | serde `Deserialize` | nextjson `nextdecode_into` |
| --- | --- | --- |
| 结果如何产生 | Visitor 回调返回新值，由 Deserializer 组装 | 写入调用方提供的槽 |
| 需要 `T: Default`？ | 不需要（但占位重建常见） | 不需要 |
| 内存复用 | 需手动（且受 Visitor 限制） | 一等公民（槽可反复 `take`/`write`） |
| 未初始化表达 | 无（值由 Visitor 构造） | `Option<T>`，类型系统保证安全 |
| unsafe 需求 | serde 内部有 unsafe | 全库 deny unsafe |

## 相关页面

- 契约全貌：[[Core Contracts]]
- 为什么不用 Visitor：[[Design Philosophy]] / [[Comparison with serde]]
- 流式场景的槽：[[Streaming]]


# Decode Slot

`DecodeSlot<T>` 是"无 Visitor 就地解码"的落点，也是 NextJson 与 serde 在解码侧
最根本的差异之一。

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

早期设计曾考虑 `MaybeUninit<Self>`。最终选择 `Option<T>`：

- `write` 之前**类型系统禁止**读值——错误实现无法在 safe code 里产生 UB；
- 析构语义与普通 `Option<T>` 一致：解码失败或字段重复时，已初始化的字段按正常
  drop 顺序清理（**部分初始化清理安全**）；
- 库保持 `#![deny(unsafe_code)]`：不需要 `assume_init`。

### 2. 内存复用，不要求 `T: Default`

serde 的 `Deserialize` 从零构造值；`decode_into` 让**调用方**提供存储：

```rust
let mut slot: DecodeSlot<MyStruct> = DecodeSlot::new();
for frame in frames {
    slot.take();               // 取出上一次结果
    MyStruct::nextdecode_into(&mut decoder, &mut slot)?;  // 复用 slot
    handle(slot.take().unwrap());
}
```

没有 `T: Default`、没有占位值、没有"先构造一个空壳再覆盖"。

### 3. 与错误传播的配合

解码中途失败时，`slot` 保持未初始化（或部分初始化但已由 drop 清理），调用方
**永远不会拿到半成品**。

## 派生代码如何使用

宏生成的反序列化实现逐字段解码并 `__slot.write(...)` 一次成功后才写最终值；
对于结构体，宏先生成字段级槽、全部成功后再组装 `Self`。重复字段（map 中同一个
键出现两次）用 `write` 覆盖旧值——旧值按正常 drop 语义释放，行为可预期。

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

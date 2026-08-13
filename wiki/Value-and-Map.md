# Value and Map

`Value` 是自描述的 JSON AST，`Map` 是它的插入序对象容器。本页讲清楚它们各解决
什么问题、内部怎么实现。

## 什么时候需要 `Value`

类型化解码（`nextdecode::<MyStruct>`）把字节直接变成你的结构体。但有些场景你
**不知道目标类型**：

- 解析一份任意 JSON 文档，稍后再决定怎么处理（配置、响应体、动态内容）；
- 把一个 `Value` 重新编码成另一种格式（`Value` 实现了
  `NsonSerialize`/`NsonDeserialize`）；
- `to_json_schema::<T>()` 生成标准 JSON Schema——结果就是一个 `Value`。

这些场景的共同点是：需要一个**自描述**的、能装下任意 JSON 数据模型的值。
这就是 `Value`。

## `Value`

```rust
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),        // 无损（见 [[Number Model]]）
    String(String),
    Array(Vec<Value>),
    Object(Map),           // 插入序
}
```

访问 API：

```rust
v.is_null() / is_bool() / is_number() / is_string() / is_array() / is_object()
v.as_str() / as_bool() / as_number()
v.as_i64() / as_u64() / as_i128() / as_u128() / as_f64()   // 经 Number 无损转换
v.as_array() / as_object()
```

`Value` 还实现 `Index`，支持 `value["name"]` 下标访问对象键：

```rust
let v: Value = nextjson::nextdecode(br#"{"a":[1,2,3]}"#)?;
assert_eq!(v["a"][1], Value::from(2_u64));
```

## `Map`：插入序 + 查找索引

```rust
pub struct Map {
    entries: Vec<(String, Value)>,   // 插入序，往返保序
    index: BTreeMap<String, usize>,  // 查找索引，O(log n)
}
```

先看它解决了什么矛盾：标准的 `BTreeMap<String, V>` 保序但**丢插入序**（按 key
排序）；裸 `HashMap` 迭代序随机。JSON 对象的常见需求是"**写进去的顺序、读出来
还是这个顺序**"（往返保序），同时还要能按 key 快速查找。

机制是**双层结构**：

- `entries: Vec<(String, Value)>` 负责**确定性插入序**——`Value` 往返、重新编码
  时顺序不变；
- `index: BTreeMap<String, usize>` 负责**查找**——`O(log n)` 找到下标，再回到
  `Vec` 取值；
- `BTreeMap` 来自 `alloc`，所以 `no_std` 下也能用。

`PartialEq` **只比较有序 entries**，忽略内部 index 布局——两个同内容不同构造
历史的 `Map` 相等。

## 设计巧思：Number 进 Value

`Value::Number(Number)` 而非 `f64`：

- JSON 整数在 `Value` 里**无损**（`u128` 以内）；
- 大整数不会在解析时被转成浮点（见 [[Number Model]]）；
- `as_i64`/`as_u64` 等转换带范围检查，语义清晰。

## 使用场景

- `to_json_schema::<T>()` 生成标准 JSON Schema 时，结果就是 `Value`；
- 文档式格式（toml / yaml）编码前先收集为 `Value`；
- `Value` 实现 `NsonSerialize`/`NsonDeserialize`，可当作"动态类型"解码目标。

## 与 serde_json::Value 的对照（诚实差异）

| 能力 | serde_json::Value | nextjson::Value |
| --- | --- | --- |
| JSON Pointer | `pointer()` 有 | **无** |
| 数字精度 | 默认 f64；可选 arbitrary_precision | 固定 `u128` 以内无损 |
| 深拷贝/合并 | 生态惯用法 | 未提供专门 API |
| `json!` 宏 | 有（社区惯用） | 无（用 `From` 构造） |
| 对象容器 | indexmap 或 BTreeMap（feature） | 自研插入序 `Map` |

这是**能力边界**而非缺陷声称——`Value` 定位是"自描述往返 + schema 生成"的载体，
不是完整的 JSON 工具包。


# Value and Map

`Value` 是自描述的 JSON AST，`Map` 是它的插入序对象容器。

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

`Value` 还实现 `Index`，支持 `value["name"]` 下标访问对象键。

## `Map`：插入序 + 查找索引

```rust
pub struct Map {
    entries: Vec<(String, Value)>,   // 插入序，往返保序
    index: BTreeMap<String, usize>,  // 查找索引，O(log n)
}
```

设计取舍（`map.rs` 文档原话）：

- `BTreeMap`：保序但**丢插入序**；裸 `HashMap`：随机迭代序；
- `Map` 把 `BTreeMap` 查找索引叠加在 `Vec` 上 → **O(log n) 查找 + 确定性插入序**，
  且 `no_std` 兼容（`BTreeMap` 来自 `alloc`）。

`PartialEq` **只比较有序 entries**，忽略内部 index 布局——两个同内容不同构造
历史的 `Map` 相等。

## 设计巧思：Number 进 Value

`Value::Number(Number)` 而非 `f64`：

- JSON 整数在 `Value` 里**无损**（`u128` 以内）；
- 大整数不会在解析时被转成浮点；
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

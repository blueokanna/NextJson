# Compile-Time Schema

这是 NextJson 的核心创新之一：**每个 `NsonSerialize` 类型都携带一个编译期构造、
运行时内省的 `const SCHEMA: TypeSchema`**。

## 为什么这样做

serde 生态里，类型结构与序列化实现是**两套独立代码**：`Deserialize` 负责行为，
`schemars` 之类的 crate 另写一套 derive 生成 JSON Schema。二者可能漂移。

NextJson 让 schema 成为序列化契约的**一部分**：

```rust
pub trait NsonSchema {
    const SCHEMA: TypeSchema;
}
```

任何可序列化类型（`NsonSerialize: NsonSchema`）都能被内省——生成 JSON Schema、
做校验、做工具，都直接读这一份数据。

## `TypeSchema`：完整变体

```rust
pub enum TypeSchema {
    Unit, Bool,
    I8, I16, I32, I64, I128, Isize,
    U8, U16, U32, U64, U128, Usize,
    F32, F64, Char,
    Str,                       // String / &str / Cow<str>
    Bytes,                     // &[u8]
    Opaque,                    // skip_serializing 等不可内省字段
    Seq(&'static TypeSchema),  // Vec<T> / [T; N] / slice
    Map(&'static TypeSchema),  // 描述值类型
    Optional(&'static TypeSchema),
    Tuple(&'static [TypeSchema]),
    Struct(&'static StructSchema),
    Enum(&'static EnumSchema),
}
```

**设计巧思**：
- 全是 `&'static` 引用型数据 → 能在 `const` 上下文构造（`Copy`、`Eq`）；
- `Opaque` 变体让"自定义序列化/跳过的字段"仍然有 schema 占位，而不是缺失；
- `Optional` 的 `name()` 会解包内层类型（`Option<i32>` 显示为 `i32`）。

## 结构体与枚举 schema

```rust
pub struct StructSchema {
    pub name: &'static str,
    pub transparent: bool,
    pub fields: &'static [FieldSchema],
}
pub struct FieldSchema {
    pub name: &'static str,   // 序列化名（经 rename/rename_all）
    pub orig: &'static str,   // 原始 Rust 字段名（错误消息用）
    pub required: bool,
    pub flattened: bool,
    pub ty: TypeSchema,
}
pub struct EnumSchema {
    pub name: &'static str,
    pub tag: Option<&'static str>,      // 内部标签
    pub content: Option<&'static str>,  // 邻接标签
    pub untagged: bool,
    pub default_tag: &'static str,
    pub variants: &'static [VariantSchema],
}
```

`FieldSchema` 同时记录"序列化名"与"原始名"：错误消息可以用原始名，线格式用
序列化名——避免 `rename_all` 之后报错信息与源码对不上。

## 用法

```rust
use nextjson::{NsonSchema, schema_of, to_json_schema};

#[derive(NsonSerialize, NsonDeserialize)]
struct User { id: u64, name: String }

let s = schema_of::<User>();          // TypeSchema::Struct(&StructSchema{...})
let js = to_json_schema::<User>();    // 标准 JSON Schema 的 Value
```

## 踩过的坑（源码注释/提交记录里的事实）

- **超 trait 关联常量必须经超 trait 路径访问**：`<T as NsonSchema>::SCHEMA` 合法，
  `<T as NsonSerialize>::SCHEMA` 触发 E0576。派生代码统一用
  `<T as ::nextjson::NsonSchema>::SCHEMA`。
- **schema 的负担削减**：`serialize_with` / `deserialize_with` / `with` / `getter`
  字段的 schema 一律记 `Opaque`——外部类型可能未实现 `NsonSchema`，强写
  `<外部类型 as NsonSchema>::SCHEMA` 会编译失败。

## 与 serde 的关系

serde 本身没有编译期 schema（`serde-reflection` / `schemars` 是第三方补丁方案）。
NextJson 的 `const SCHEMA` 是**零运行时开销、类型自描述**的内省能力；这也是
"不能造假"地写进 Wiki 的差异点之一。

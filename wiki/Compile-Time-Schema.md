# Compile-Time Schema

这是 NextJson 的核心创新之一：**每个 `NsonSerialize` 类型都携带一个编译期构造、
运行时内省的 `const SCHEMA: TypeSchema`**。

## 先理解问题：为什么 serde 生态里 schema 会漂移

在 serde 的世界里，一个类型有两套独立的代码：

- `#[derive(Serialize, Deserialize)]` 描述**行为**（怎么编码、怎么解码）；
- `schemars` 的 `#[derive(JsonSchema)]` 描述**形状**（JSON Schema 长什么样）。

这两套代码是**分开维护**的。你改了字段名、加了字段，`Serialize` 同步改了，
但 `JsonSchema` 忘了改——文档就漂移了。更糟的是，`schemars` 对复杂类型的支持
和 `serde` 并不完全一致，漂移几乎是必然。

NextJson 的做法是把 schema 变成序列化契约的**一部分**，从结构上消除漂移：

```rust
pub trait NsonSchema {
    const SCHEMA: TypeSchema;
}
```

任何可序列化类型（`NsonSerialize: NsonSchema`）都必须能描述自己。`schema_of::<T>()`
直接读这一份数据，`to_json_schema::<T>()` 把它转成标准 JSON Schema。**schema 和
序列化实现来自同一份 derive 输出**，字段名、重命名、跳过规则全都同源，不可能
不一致。

## 为什么 `const` 能成立：全是 `&'static` 引用型数据

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

关键机制：`TypeSchema` 的所有变体都只装**引用型数据**（`&'static TypeSchema`、
`&'static [TypeSchema]`）。这意味着它可以：

- 在 `const` 上下文构造（不需要运行时堆分配）；
- 整体是 `Copy` + `Eq`（可以自由复制、比较）；
- 读取时零运行时开销——它就是一堆静态数据。

派生宏生成的是 `const SCHEMA: TypeSchema = TypeSchema::Struct(&StructSchema { ... })`
这样的**编译期常量**。内联时编译器甚至能把整个 schema 折叠成已知数据。

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

一个容易忽略的细节：`FieldSchema` 同时记录"序列化名"（`name`）与"原始名"
（`orig`）。原因是重命名之后，**线格式**要用 `name`（比如 `camelCase` 后的
`user_id` → `userId`），而**错误消息**要用 `orig`（"字段 user_id 缺失"才能和
你的源码对上）。两套名字都来自同一份 derive 输入，同样不可能漂移。

## 用法

```rust
use nextjson::{NsonSchema, schema_of, to_json_schema};

#[derive(NsonSerialize, NsonDeserialize)]
struct User { id: u64, name: String }

let s = schema_of::<User>();          // TypeSchema::Struct(&StructSchema{...})
let js = to_json_schema::<User>();    // 标准 JSON Schema 的 Value
```

`schema_of::<T>()` 的实现本质就是一行：`<T as NsonSchema>::SCHEMA`。这也是
"零运行时开销内省"的字面意思——没有解析、没有计算，只有一次常量读取。

## 它实际解锁了什么

- **生成 JSON Schema**：`to_json_schema` 把 `TypeSchema` 翻译成标准 JSON Schema
  的 `Value`（`json_schema.rs`），可喂给校验器 / 编辑器 / 文档工具；
- **校验与工具**：因为 schema 是普通数据，任何工具都能内省它——这正是"类型
  自描述"的落地；
- **编译期能力**：schema 是 `const`，理论上可在编译期驱动代码生成、自动校验
  类型形状等（这是 `serde-reflection` 在 serde 生态里需要外部工具才能做到的事）。

## 踩过的坑（源码注释/提交记录里的事实）

- **超 trait 关联常量必须经超 trait 路径访问**：`<T as NsonSchema>::SCHEMA` 合法，
  `<T as NsonSerialize>::SCHEMA` 触发 E0576。派生代码统一用
  `<T as ::nextjson::NsonSchema>::SCHEMA`。
- **schema 的负担削减**：`serialize_with` / `deserialize_with` / `with` / `getter`
  字段的 schema 一律记 `Opaque`——外部类型可能未实现 `NsonSchema`，强写
  `<外部类型 as NsonSchema>::SCHEMA` 会编译失败。`Opaque` 表示"这块结构不可
  内省"，而不是"这块结构不存在"。

## 与 serde 的关系

serde 本身没有编译期 schema（`serde-reflection` / `schemars` 是第三方补丁方案）。
NextJson 的 `const SCHEMA` 是**零运行时开销、类型自描述**的内省能力；这也是
"不能造假"地写进 Wiki 的差异点之一（措辞纪律见 [[Comparison with serde]]）。


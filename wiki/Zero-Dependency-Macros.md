# Zero-Dependency Macros

`nextjson-derive` 是**零第三方依赖**的 proc-macro crate：不用 `syn`、`quote`、
`proc-macro2`，只用标准 `proc_macro` API + 手写递归下降解析器。这是"零依赖"
承诺的最后一环（否则 `syn` 全家桶会进入构建图）。

## 为什么值得这么做

| | serde_derive | nextjson-derive |
| --- | --- | --- |
| 依赖 | syn / quote / proc-macro2 / unicode-ident ... | 无（仅标准 `proc_macro`） |
| 解析器 | 由 syn 维护（随 Rust 语法演进） | 手写、维护在自己仓库 |
| 生成 | quote! 拼接 token | 生成字符串再 `TokenStream::from_str` |
| 风险 | 无（syn 社区维护） | 必须自己跟上 Rust 语法变化 |

## 先看标准 `proc_macro` API 长什么样

`syn` 帮你把 `TokenStream` 变成一棵可读的 AST。不用 syn 意味着你要自己处理
`TokenStream` 里的原始 token——而它比你想的更"原始"：

```rust
// 输入 `struct Foo<'a> { x: u64 }` 的 token 序列大概是：
// Ident("struct") Ident("Foo")
// Punct('<', Joint)  Punct('\'', Joint) Ident("a") Punct('>', Alone)   ← 泛型参数
// Punct('{', Alone)
// Ident("x") Punct(':', Alone) Ident("u64")
// Punct('}', Alone)
```

注意几个细节（都是踩过的坑）：

- **`<` 和 `>` 是普通 `Punct`**，没有 `Delimiter::Angle` 分组——所以按 `,` 切分
  泛型参数时，必须自己跟踪 `<>` 深度，否则 `BTreeMap<String, i32>` 会被切成
  两个"字段"；
- **`'a` 是 `[Punct('\'', Joint), Ident("a")]`**——`Joint` 表示"紧贴下一个
  token"。把 token 拼回字符串时，`Joint` 的 token 后面**不能加空格**，否则
  `' a` 会被当成字符字面量（编译期直接 panic）；
- 同理 `std::x` 是 `[Ident("std"), Punct(':', Joint), Punct(':', Alone), ...]`，
  丢 Joint 间距会拼出 `: :`（路径分隔符错误）。

这就是"手写解析器"的真实工作环境：一个带 `peek`/`next` 的 token 游标，加上
递归下降。

## 架构

```text
proc_macro::TokenStream 输入
   └─ P<'a>（token 游标：peek / next / is_ident / is_punct ...）
        └─ 递归下降解析 → 小 AST：
             Input { kind: Struct|Enum, name, generics, where, fields/variants }
             + attr.rs：ContainerAttrs / FieldAttrs / VariantAttrs / Meta
        └─ codegen（ser.rs / de.rs / schema.rs）→ 文本 → TokenStream::from_str
```

## 一个具体解析例子：字段类型怎么读

假设要解析字段 `x: BTreeMap<String, i32>`：

1. 游标读到 `Ident("x")`，记录为字段名；
2. 读到 `Punct(':')`，确认是命名字段；
3. 开始读字段类型：`Ident("BTreeMap")` → `Punct('<')`（深度 1）→
   `Ident("String")` → `Punct(',')`（**深度 1，不切分**）→ `Ident("i32")` →
   `Punct('>')`（深度 0，类型结束）；
4. 字段类型被**原样保留**（不透明 token 序列），只在生成代码时拼回字符串。

关键：**类型位置内部的一切都是"不透明携带"**——解析器不需要理解 `BTreeMap` 是
什么。它只关心结构（几个字段、泛型参数表、where 子句），类型本体原样往返。
这就是"类型位置上出现的新 Rust 语法不需要改解析器"的原因。

## 前向兼容契约（关键巧思）

derive 只收到**单个 item 的 token**。解析器解释一个**刻意稳定的语法子集**：
item 头（`struct`/`enum` + 名字）、泛型参数表、`where` 子句、字段/变体结构、
`#[njson]` / `#[nextjson]` / `#[serde]` 属性。**类型位置内部的一切原样携带**
（不透明 token 序列往返），因此类型位置上出现的新 Rust 语法（新字面量、
`impl Trait` 形式、关联类型路径……）不需要改解析器。

防御性收尾：`parse_input` 要求**每个输入 token 都被消费**。若未来 Rust 扩展了
item 级语法而解析器不认识，宏会以 `compile_error!` 报出剩余 token，**而不是静默
从误解析的子集生成 impl**。

## 已踩过的坑（全部在提交/测试里留痕）

1. **proc_macro 没有 `Delimiter::Angle`**：`<`/`>` 是独立 `Punct` token。按 `,`
   切分字段/泛型时必须跟踪 `<>` 深度，否则 `BTreeMap<String, i32>` 被切成两个
   "字段" → E0023。
2. **`join()` 必须保留 `Joint` 间距**：`'a` 在 proc_macro 里是
   `[Punct('), Ident a]`，`std::x` 是 `[Ident, Punct(:Joint), Punct(:Alone), ...]`。
   若一律按空格拼接会产出 `' a`（被当成字符字面量 → 未终止 panic）与 `: :`
   （路径分隔符错误）。修复：`Punct` 的 `spacing()==Joint` 时下一个 token 不加
   空格。
3. **`pub(crate)` 可见性**：`(crate)` 是 `Parenthesis` 分组而非 `Punct('(')`，
   按 `is_punct('(')` 判断是死代码 → 用 `eat_visibility()` 匹配 Group。
4. **`seen_decl` 位图**：用原始字段下标做 `1u64 << i` 会在字段 >64 + skip 时
   shift panic/位冲突 → 改用被跟踪字段的序号。
5. **生成代码引用路径**：`$crate::` 在宏里要用 `#cp::`，且 `cp` 默认带前导
   `::`（否则 `::#cp::` 会变成 `::::`）。
6. **`TokenStream::from_str` 失败会 panic 且不显示 payload**：调试时把生成字符串
   写文件（proc macro 进程 CWD 在 package root）。

## 属性解析

属性解析在 `attr.rs`：

- 接受三种别名：`#[njson(...)]`、`#[nextjson(...)]`、`#[serde(...)]`（后者为
  生态迁移兼容）；
- 支持**方向性** `rename_all` / `bound`：`serialize = "..."` / `deserialize =
  "..."` 分别作用于 ser/de 两侧；
- `bound` 只替换自动生成的 per-type-param 约束，类型自身 `where` 子句无条件
  保留（否则泛型+where 结构体 E0277）。

完整属性清单见 [[Derive Macros]]。


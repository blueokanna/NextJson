# Derive Macros

`#[derive(NsonSerialize, NsonDeserialize)]` 生成序列化、反序列化与 schema 三份
实现。属性接受 `#[njson(...)]`、`#[nextjson(...)]` 与 `#[serde(...)]` 三种写法
（后者为生态迁移兼容）。

## 容器级属性（写在类型上）

| 属性 | 作用 |
| --- | --- |
| `rename = "..."` | 重命名容器（错误消息/内部标识用） |
| `rename_all = "camelCase"` 等 | 字段/变体命名转换；**方向性**：`rename_all(serialize = "...", deserialize = "...")` 可分别指定 |
| `rename_all_fields` | 结构体变体的字段命名（+ 方向性） |
| `tag = "..."` | 内部标签枚举 |
| `content = "..."` | 邻接标签枚举 |
| `untagged` | untagged 枚举（解码走 `save`/`restore` 回溯） |
| `bound = "..."` | 替换自动生成的 per-type-param 约束（**类型自身 `where` 子句无条件保留**） |
| `default` / `default = "path"` | 容器默认实例：缺失字段用默认值补 |
| `transparent` | 透明容器（单字段/单变体） |
| `deny_unknown_fields` | 未知字段报错（与 `flatten` 组合是编译错误，serde 同款） |
| `remote = "path"` | 为外部类型实现（不碰外部类型定义） |
| `into` / `from` / `try_from` | 用转换类型驱动序列化/反序列化 |
| `expecting` | 解析接受（无 visitor 无行为，文档说明） |

## 字段级属性

| 属性 | 作用 |
| --- | --- |
| `rename = "..."` | 字段序列化名 |
| `default` | 缺失时用 `Default::default()` |
| `skip` / `skip_serializing` / `skip_deserializing` | 跳过一侧或双侧 |
| `skip_serializing_if = "path"` | 条件跳过（如 `Option::is_none`） |
| `flatten` | 拍平到父对象（map/struct；**元组位置是编译错误**） |
| `serialize_with` / `deserialize_with` / `with` | 自定义序列化函数（字段或 newtype 变体） |
| `getter = "path"` | 用 getter 方法取值 |
| `borrow` | 借用字段（生成 `'de: 'a` where 谓词） |

## 变体级属性

| 属性 | 作用 |
| --- | --- |
| `rename` | 变体序列化名 |
| `rename_all` | 该结构体变体的字段命名（不改变体名） |
| `alias` | 反序列化别名（匹配臂加 `|`） |
| `other` | 兜底 unit 变体（内部/邻接标签；外部/untagged 是编译错误） |
| `skip_serializing` / `skip_deserializing` | 单侧跳过 |
| `serialize_with` / `deserialize_with` / `with` | newtype 变体内含字段的自定义序列化 |

## 派生生成的 schema 约定

- 普通字段：`<T as NsonSchema>::SCHEMA`；
- `serialize_with` / `deserialize_with` / `with` / `getter` 字段：一律
  `TypeSchema::Opaque`（外部类型可能没实现 `NsonSchema`）；
- `PhantomData` 字段：**自动 skip**（serde 语义），schema 记 `Opaque`、非 required；
- `skip_deserializing`（无 path）与容器 `default`：反序列化 impl 给所有类型参数加
  `T: Default`（serde 同款）。

## 编译期校验（`validate_input()`）

容器与变体配置在编译期校验，非法组合直接 `compile_error!`：

- transparent 枚举/多字段；
- `flatten` 用在元组位置；
- `flatten` + `skip_serializing_if` 组合；
- `deny_unknown_fields` + `flatten` 组合；
- `other` 用在外部/untagged 枚举；
- 解析器 `parse_input` 强制消费全部 token（防新语法静默误解析）。

## 已修的关键 bug（回归测试锁定）

| Bug | 修复 |
| --- | --- |
| `pub(crate)`/`pub(super)` 字段解析失败 | `eat_visibility()` 匹配 `Parenthesis` Group |
| 泛型 + where 结构体 E0277 | `bound` 只替换自动生成约束，`where` 无条件保留 |
| `>64` 字段 + skip 时 `seen_decl` 位图 panic | 改用被跟踪字段序号 |
| `flatten` 声明在中间时拿到全部键 | 分两遍：先显式字段 remove，后 flatten 用剩余键 |
| `into`/`from`/`try_from` 泛型 E0107 | 转换 bound 用 `Dst<T>` 实例化 |
| `remote` + 泛型双泛型参数 | remote 时 target 用路径原样、不再拼 `{ty_generics}` |
| 变体级 `rename_all` 改变体名 | 只作用于结构体变体字段 |
| 嵌套 `bound(serialize="..", deserialize="..")` 组 | 解析为方向性 Named |

## 相关页面

- 零依赖实现细节与坑：[[Zero-Dependency Macros]]
- 生成代码背后的契约：[[Core Contracts]] / [[Compile-Time Schema]]

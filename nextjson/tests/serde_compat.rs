//! serde 语法兼容层的新增能力测试。
//!
//! 覆盖本仓库本次新增的 serde 兼容特性：
//! - 容器级 `default = "path"`（默认实例补缺字段）
//! - 变体 `alias`（反序列化别名）
//! - 变体 `other`（内部/邻接标签枚举的兜底变体）
//! - 容器 `rename_all_fields` 与变体级 `rename_all`（结构体变体字段改名）
//! - `serialize_with` / `deserialize_with` / `with` 字段的 schema 降级为
//!   `Opaque`（字段类型无需实现 `NsonSchema`，可编译）

use nextjson::{from_str, to_string, NsonDeserialize, NsonSerialize, TypeSchema};

// ---------------------------------------------------------------------------
// 容器级 default = "path"
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(default = "default_config")]
struct Config {
    host: String,
    port: u16,
    debug: bool,
}

fn default_config() -> Config {
    Config {
        host: "localhost".into(),
        port: 8080,
        debug: false,
    }
}

#[test]
fn container_default_path() {
    // 缺字段时从默认实例取值（serde 语义）。
    let back: Config = from_str(r#"{"port":9000}"#).unwrap();
    assert_eq!(
        back,
        Config {
            host: "localhost".into(),
            port: 9000,
            debug: false
        }
    );
    // 全量字段照常。
    let full: Config = from_str(r#"{"host":"h","port":1,"debug":true}"#).unwrap();
    assert_eq!(
        full,
        Config {
            host: "h".into(),
            port: 1,
            debug: true
        }
    );
    assert_eq!(
        to_string(&full).unwrap(),
        r#"{"host":"h","port":1,"debug":true}"#
    );
}

// 字段级显式 default 优先于容器级 default。
#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(default = "dflt2")]
struct Precedence {
    a: i32,
    #[njson(default = "special")]
    b: i32,
}

fn dflt2() -> Precedence {
    Precedence { a: 1, b: 2 }
}
fn special() -> i32 {
    99
}

#[test]
fn field_default_wins_over_container_path() {
    let back: Precedence = from_str(r#"{}"#).unwrap();
    assert_eq!(back, Precedence { a: 1, b: 99 });
}

// ---------------------------------------------------------------------------
// 变体 alias
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
enum Status {
    #[njson(alias = "progressing", alias = "in_progress")]
    InProgress,
    Done,
}

#[test]
fn variant_alias_external() {
    let back: Status = from_str(r#"{"progressing":null}"#).unwrap();
    assert_eq!(back, Status::InProgress);
    let back: Status = from_str(r#"{"in_progress":null}"#).unwrap();
    assert_eq!(back, Status::InProgress);
    // 序列化仍用主名。
    assert_eq!(
        to_string(&Status::InProgress).unwrap(),
        r#"{"InProgress":null}"#
    );
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(tag = "kind")]
enum Cmd {
    #[njson(alias = "go")]
    Run,
    Stop,
}

#[test]
fn variant_alias_internal() {
    let back: Cmd = from_str(r#"{"kind":"go"}"#).unwrap();
    assert_eq!(back, Cmd::Run);
    let back: Cmd = from_str(r#"{"kind":"Run"}"#).unwrap();
    assert_eq!(back, Cmd::Run);
}

// ---------------------------------------------------------------------------
// 变体 other（内部标签枚举兜底）
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(tag = "type")]
enum Event {
    Click {
        x: i32,
        y: i32,
    },
    #[njson(other)]
    Unknown,
}

#[test]
fn variant_other_internal() {
    // 未知标签落入兜底变体，而不是报错。
    let back: Event = from_str(r#"{"type":"Scroll","dx":1}"#).unwrap();
    assert_eq!(back, Event::Unknown);
    let back: Event = from_str(r#"{"type":"Click","x":1,"y":2}"#).unwrap();
    assert_eq!(back, Event::Click { x: 1, y: 2 });
}

// ---------------------------------------------------------------------------
// rename_all_fields 与变体级 rename_all
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(rename_all = "snake_case", rename_all_fields = "camelCase")]
enum Api {
    GetUser { user_id: u32 },
}

#[test]
fn rename_all_fields_on_struct_variant() {
    let v = Api::GetUser { user_id: 7 };
    // 变体名按 rename_all=snake_case；变体字段按 rename_all_fields=camelCase。
    assert_eq!(to_string(&v).unwrap(), r#"{"get_user":{"userId":7}}"#);
    let back: Api = from_str(r#"{"get_user":{"userId":7}}"#).unwrap();
    assert_eq!(back, v);
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
enum Local {
    #[njson(rename_all = "snake_case")]
    Ping { host_name: String },
}

#[test]
fn variant_level_rename_all_for_fields() {
    let v = Local::Ping {
        host_name: "h".into(),
    };
    // 变体字段按变体级 rename_all=snake_case。
    assert_eq!(to_string(&v).unwrap(), r#"{"Ping":{"host_name":"h"}}"#);
    let back: Local = from_str(r#"{"Ping":{"host_name":"h"}}"#).unwrap();
    assert_eq!(back, v);
}

// ---------------------------------------------------------------------------
// serialize_with / deserialize_with / with 字段的 schema 必须为 Opaque
// ---------------------------------------------------------------------------

/// 故意不实现 `NsonSchema` / `NsonSerialize` 的外部类型（类似 `SystemTime`）。
#[derive(Debug, PartialEq)]
struct OpaqueMillis(u64);

mod ops {
    use super::OpaqueMillis;
    use nextjson::{FormatDecoder, FormatEncoder};

    pub fn ser_millis<E: FormatEncoder>(v: &OpaqueMillis, e: &mut E) -> Result<(), E::Error> {
        e.write_u64(v.0)
    }

    pub fn de_millis<'de, D: FormatDecoder<'de>>(d: &mut D) -> Result<OpaqueMillis, D::Error> {
        Ok(OpaqueMillis(d.u64()?))
    }
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct Wire {
    name: String,
    #[njson(
        serialize_with = "ops::ser_millis",
        deserialize_with = "ops::de_millis"
    )]
    ts: OpaqueMillis,
}

#[test]
fn custom_serializer_schema_is_opaque() {
    // 编译即验证：OpaqueMillis 未实现 NsonSchema，schema 生成必须走 Opaque。
    let v = Wire {
        name: "x".into(),
        ts: OpaqueMillis(1234),
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"name":"x","ts":1234}"#);
    let back: Wire = from_str(r#"{"ts":99,"name":"x"}"#).unwrap();
    assert_eq!(back.ts, OpaqueMillis(99));

    let schema = nextjson::schema_of::<Wire>();
    match schema {
        TypeSchema::Struct(s) => {
            let ts = s
                .fields
                .iter()
                .find(|f| f.name == "ts")
                .expect("ts field in schema");
            assert_eq!(ts.ty, TypeSchema::Opaque);
        }
        other => panic!("expected struct schema, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// PhantomData 字段自动跳过（serde 语义）
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct WithMarker<T> {
    value: T,
    _marker: std::marker::PhantomData<T>,
}

#[test]
fn phantom_data_field_auto_skipped() {
    let v = WithMarker {
        value: 5_i32,
        _marker: std::marker::PhantomData,
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"value":5}"#);
    let back: WithMarker<i32> = from_str(r#"{"value":5}"#).unwrap();
    assert_eq!(back.value, 5);
}

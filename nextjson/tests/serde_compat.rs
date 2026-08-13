//! Testing new capabilities added to the serde syntax compatibility layer.
//!
//! Overriding the serde compatibility features added in this repository:
//! - Container-level `default = "path"` (default instance filler field)
//! - Variant `alias` (deserialization alias)
//! - Variant `other` (fallback variant for internal/adjacent tag enumeration)
//! - Container `rename_all_fields` and variant-level `rename_all` (struct variant field renaming)
//! - Schema downgrade for `serialize_with` / `deserialize_with` / `with` fields
//! - `Opaque` (field type does not need to implement `NsonSchema`, compileable)

use nextjson::{from_str, to_string, NsonDeserialize, NsonSerialize, TypeSchema};
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
    // When a field is missing, the value is taken from the default instance (serde semantics).
    let back: Config = from_str(r#"{"port":9000}"#).unwrap();
    assert_eq!(
        back,
        Config {
            host: "localhost".into(),
            port: 9000,
            debug: false
        }
    );
    // All fields remain unchanged
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

// Explicit default values ​​at the field level take precedence over container-level default values
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
// Variant alias
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
// Variants other (internal tag enumeration catch-all)
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
    // The unknown label falls into the catch-all variant instead of being reported as an error.
    let back: Event = from_str(r#"{"type":"Scroll","dx":1}"#).unwrap();
    assert_eq!(back, Event::Unknown);
    let back: Event = from_str(r#"{"type":"Click","x":1,"y":2}"#).unwrap();
    assert_eq!(back, Event::Click { x: 1, y: 2 });
}

// ---------------------------------------------------------------------------
// rename_all_fields and variant-level rename_all
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(rename_all = "snake_case", rename_all_fields = "camelCase")]
enum Api {
    GetUser { user_id: u32 },
}

#[test]
fn rename_all_fields_on_struct_variant() {
    let v = Api::GetUser { user_id: 7 };
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
    // The variant field is set to variant level: rename_all = snake_case.
    assert_eq!(to_string(&v).unwrap(), r#"{"Ping":{"host_name":"h"}}"#);
    let back: Local = from_str(r#"{"Ping":{"host_name":"h"}}"#).unwrap();
    assert_eq!(back, v);
}

// ---------------------------------------------------------------------------
// serialize_with / deserialize_with / with 字段的 schema 必须为 Opaque
// ---------------------------------------------------------------------------
#[derive(Debug, PartialEq)]
pub struct OpaqueMillis(pub u64);

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
// PhantomData Fields are automatically skipped (serde semantics)
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

// ---------------------------------------------------------------------------
// Container-level type mismatches name the expected type (`expecting`).
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(expecting = "a config object")]
struct ExpectingObj {
    a: u32,
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct PlainObj {
    a: u32,
}

#[test]
fn container_type_mismatch_carries_expecting_attribute() {
    // `begin_object` hits `[`: the message names the type, not a bare token.
    let err = nextjson::nextdecode::<ExpectingObj>(b"[1]").unwrap_err();
    assert!(
        err.to_string().contains("a config object"),
        "unexpected message: {err}"
    );
    assert!(
        err.to_string().contains("found array"),
        "unexpected message: {err}"
    );
}

#[test]
fn container_type_mismatch_carries_default_type_name() {
    let err = nextjson::nextdecode::<PlainObj>(b"[1]").unwrap_err();
    assert!(
        err.to_string().contains("PlainObj"),
        "unexpected message: {err}"
    );
}

#[test]
fn scalar_errors_are_not_polluted_by_container_expecting() {
    // A field type mismatch keeps its own scalar description.
    let err = nextjson::nextdecode::<PlainObj>(br#"{"a":"x"}"#).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("number"), "unexpected message: {msg}");
    assert!(!msg.contains("PlainObj"), "unexpected message: {msg}");
}

#[test]
fn hand_written_impl_can_set_expecting_for_containers() {
    #[derive(Debug)]
    struct Hand;
    impl<'de> NsonDeserialize<'de> for Hand {
        fn nextdecode_into<D: nextjson::FormatDecoder<'de>>(
            decoder: &mut D,
            out: &mut nextjson::DecodeSlot<Self>,
        ) -> core::result::Result<(), D::Error> {
            decoder.set_expecting("a hand");
            decoder.begin_object()?;
            out.write(Hand);
            Ok(())
        }
    }
    let err = nextjson::nextdecode::<Hand>(b"[1]").unwrap_err();
    assert!(
        err.to_string().contains("a hand"),
        "unexpected message: {err}"
    );
}

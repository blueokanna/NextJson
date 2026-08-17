//! Second round of boundary stress testing for derive: Variant-level serialization customization,
//! transformation attributes, remote,
//! Transparent, untagged, large structures, repeated keys, schema correctness, etc.

use nextjson::{from_str, to_string, NsonDeserialize, NsonSerialize, TypeSchema, Value};

// ---------------------------------------------------------------------------
// 1. The `serialize_with` or `with` method on the `newtype` variant/tuple struct (schema must be `Opaque`)
// ---------------------------------------------------------------------------
#[derive(Debug, PartialEq)]
pub struct Raw(pub u64);

mod raw_ops {
    use super::Raw;
    use nextjson::{FormatDecoder, FormatEncoder};

    pub fn ser_raw<E: FormatEncoder>(v: &Raw, e: &mut E) -> Result<(), E::Error> {
        e.write_u64(v.0)
    }
    pub fn de_raw<'de, D: FormatDecoder<'de>>(d: &mut D) -> Result<Raw, D::Error> {
        Ok(Raw(d.u64()?))
    }

    pub fn serialize<E: FormatEncoder>(v: &Raw, e: &mut E) -> Result<(), E::Error> {
        ser_raw(v, e)
    }
    pub fn deserialize<'de, D: FormatDecoder<'de>>(d: &mut D) -> Result<Raw, D::Error> {
        de_raw(d)
    }
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
enum Wrapped {
    #[njson(
        serialize_with = "raw_ops::ser_raw",
        deserialize_with = "raw_ops::de_raw"
    )]
    Custom(Raw),
    #[njson(with = "raw_ops")]
    WithMod(Raw),
}

#[test]
fn newtype_variant_custom_serializer_schema() {
    let v = Wrapped::Custom(Raw(7));
    assert_eq!(to_string(&v).unwrap(), r#"{"Custom":7}"#);
    let back: Wrapped = from_str(r#"{"Custom":8}"#).unwrap();
    assert_eq!(back, Wrapped::Custom(Raw(8)));

    let w = Wrapped::WithMod(Raw(9));
    assert_eq!(to_string(&w).unwrap(), r#"{"WithMod":9}"#);
    let back: Wrapped = from_str(r#"{"WithMod":10}"#).unwrap();
    assert_eq!(back, Wrapped::WithMod(Raw(10)));

    match nextjson::schema_of::<Wrapped>() {
        TypeSchema::Enum(s) => {
            let custom = s.variants.iter().find(|v| v.orig == "Custom").unwrap();
            assert_eq!(custom.ty, TypeSchema::Opaque);
            let with_mod = s.variants.iter().find(|v| v.orig == "WithMod").unwrap();
            assert_eq!(with_mod.ty, TypeSchema::Opaque);
        }
        other => panic!("expected enum schema, got {other:?}"),
    }
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct TupleWith(
    #[njson(with = "raw_ops")] Raw,
    #[njson(with = "raw_ops")] Raw,
);

#[test]
fn tuple_struct_with_custom_serializer() {
    let v = TupleWith(Raw(1), Raw(2));
    assert_eq!(to_string(&v).unwrap(), r#"[1,2]"#);
    let back: TupleWith = from_str(r#"[3,4]"#).unwrap();
    assert_eq!(back, TupleWith(Raw(3), Raw(4)));

    match nextjson::schema_of::<TupleWith>() {
        TypeSchema::Tuple(items) => {
            assert_eq!(items[0], TypeSchema::Opaque);
            assert_eq!(items[1], TypeSchema::Opaque);
        }
        other => panic!("expected tuple schema, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 2. skip_serializing / skip_deserializing
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[allow(dead_code)]
enum SkippedVariant {
    A,
    #[njson(skip_serializing)]
    Hidden,
    #[njson(skip_deserializing)]
    Ghost(i32),
    Keep,
}

#[test]
fn variant_skip_flags() {
    // Hidden means no serialization; however, skip_serializing does not affect deserialization, and the input is still accepted (serde semantics).
    assert_eq!(to_string(&SkippedVariant::A).unwrap(), r#"{"A":null}"#);
    let back: SkippedVariant = from_str(r#"{"Hidden":null}"#).unwrap();
    assert_eq!(back, SkippedVariant::Hidden);

    // Ghost deserialization is skipped: if Ghost appears in the input, it will be an unknown_variant error.
    assert!(from_str::<SkippedVariant>(r#"{"Ghost":1}"#).is_err());
    assert_eq!(
        to_string(&SkippedVariant::Keep).unwrap(),
        r#"{"Keep":null}"#
    );
    let back: SkippedVariant = from_str(r#"{"Keep":null}"#).unwrap();
    assert_eq!(back, SkippedVariant::Keep);
}

// ---------------------------------------------------------------------------
// 3. transparent + Trait
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(transparent)]
struct Wrap<T>(T);

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(transparent)]
struct WrapNamed<T> {
    inner: T,
}

#[test]
fn transparent_with_generics() {
    let w = Wrap(5_i32);
    assert_eq!(to_string(&w).unwrap(), r#"5"#);
    let back: Wrap<i32> = from_str("6").unwrap();
    assert_eq!(back, Wrap(6));

    let wn = WrapNamed {
        inner: "x".to_string(),
    };
    assert_eq!(to_string(&wn).unwrap(), r#""x""#);
    let back: WrapNamed<String> = from_str(r#""y""#).unwrap();
    assert_eq!(back, WrapNamed { inner: "y".into() });
}

// ---------------------------------------------------------------------------
// 4. into / from / try_from + Trait
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq, Clone)]
#[njson(from = "Src<T>", into = "Src<T>")]
struct Dst<T> {
    x: T,
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct Src<T> {
    x: T,
}

impl<T> From<Src<T>> for Dst<T> {
    fn from(s: Src<T>) -> Self {
        Dst { x: s.x }
    }
}
impl<T> From<Dst<T>> for Src<T> {
    fn from(d: Dst<T>) -> Self {
        Src { x: d.x }
    }
}

#[test]
fn into_from_with_generics() {
    let d = Dst { x: 1_i32 };
    assert_eq!(to_string(&d).unwrap(), r#"{"x":1}"#);
    let back: Dst<i32> = from_str(r#"{"x":2}"#).unwrap();
    assert_eq!(back, Dst { x: 2 });
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(try_from = "TrySrc")]
struct TryDst {
    n: i32,
}

#[derive(NsonSerialize, NsonDeserialize, Debug)]
struct TrySrc {
    n: i32,
}

impl TryFrom<TrySrc> for TryDst {
    type Error = &'static str;
    fn try_from(s: TrySrc) -> Result<Self, Self::Error> {
        if s.n < 0 {
            Err("negative")
        } else {
            Ok(TryDst { n: s.n })
        }
    }
}

#[test]
fn try_from_conversion() {
    let back: TryDst = from_str(r#"{"n":5}"#).unwrap();
    assert_eq!(back, TryDst { n: 5 });
    assert!(from_str::<TryDst>(r#"{"n":-1}"#).is_err());
}

// ---------------------------------------------------------------------------
// 5. remote + getter + Trait
// ---------------------------------------------------------------------------

mod external {
    #[derive(Debug, PartialEq)]
    pub struct Foreign<T> {
        pub pub_id: u64,
        pub name: T,
    }

    impl<T> Foreign<T> {
        pub fn id(&self) -> &u64 {
            &self.pub_id
        }
        pub fn name(&self) -> &T {
            &self.name
        }
    }
}

#[derive(NsonSerialize, NsonDeserialize)]
#[allow(dead_code)]
#[njson(remote = "external::Foreign<T>")]
struct ForeignMirror<T> {
    #[njson(getter = "external::Foreign::id")]
    pub_id: u64,
    #[njson(getter = "external::Foreign::name")]
    name: T,
}

#[test]
fn remote_with_getter_generic() {
    let f = external::Foreign {
        pub_id: 42,
        name: "hi".to_string(),
    };
    let json = to_string(&f).unwrap();
    assert_eq!(json, r#"{"pub_id":42,"name":"hi"}"#);
    let back: external::Foreign<String> = from_str(&json).unwrap();
    assert_eq!(back, f);
}

// ---------------------------------------------------------------------------
// 6. untagged enumeration + struct variant
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(untagged)]
enum Shape {
    Circle { radius: f64 },
    Point(i32, i32),
    Nothing,
}

#[test]
fn untagged_with_struct_variant() {
    let c = Shape::Circle { radius: 1.5 };
    assert_eq!(to_string(&c).unwrap(), r#"{"radius":1.5}"#);
    let back: Shape = from_str(r#"{"radius":2.0}"#).unwrap();
    assert_eq!(back, Shape::Circle { radius: 2.0 });

    let p = Shape::Point(1, 2);
    assert_eq!(to_string(&p).unwrap(), r#"[1,2]"#);
    let back: Shape = from_str(r#"[3,4]"#).unwrap();
    assert_eq!(back, Shape::Point(3, 4));

    // null 匹配 unit 变体。
    let back: Shape = from_str("null").unwrap();
    assert_eq!(back, Shape::Nothing);
    // 无法匹配任何变体。
    assert!(from_str::<Shape>(r#""str""#).is_err());
}

// ---------------------------------------------------------------------------
// 7. Adjacency tag + other
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(tag = "t", content = "c")]
enum AdjOther {
    Known {
        v: i32,
    },
    #[njson(other)]
    Fallback,
}

#[test]
fn adjacent_tagged_with_other() {
    let k = AdjOther::Known { v: 1 };
    assert_eq!(to_string(&k).unwrap(), r#"{"t":"Known","c":{"v":1}}"#);
    let back: AdjOther = from_str(r#"{"t":"Nope","c":1}"#).unwrap();
    assert_eq!(back, AdjOther::Fallback);
    let back: AdjOther = from_str(r#"{"t":"Known","c":{"v":9}}"#).unwrap();
    assert_eq!(back, AdjOther::Known { v: 9 });
}

// ---------------------------------------------------------------------------
// 8. Large structs (>64 fields, seen follows the Vec<bool> path)
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct Big {
    f00: i32,
    f01: i32,
    f02: i32,
    f03: i32,
    f04: i32,
    f05: i32,
    f06: i32,
    f07: i32,
    f08: i32,
    f09: i32,
    f10: i32,
    f11: i32,
    f12: i32,
    f13: i32,
    f14: i32,
    f15: i32,
    f16: i32,
    f17: i32,
    f18: i32,
    f19: i32,
    f20: i32,
    f21: i32,
    f22: i32,
    f23: i32,
    f24: i32,
    f25: i32,
    f26: i32,
    f27: i32,
    f28: i32,
    f29: i32,
    f30: i32,
    f31: i32,
    f32: i32,
    f33: i32,
    f34: i32,
    f35: i32,
    f36: i32,
    f37: i32,
    f38: i32,
    f39: i32,
    f40: i32,
    f41: i32,
    f42: i32,
    f43: i32,
    f44: i32,
    f45: i32,
    f46: i32,
    f47: i32,
    f48: i32,
    f49: i32,
    f50: i32,
    f51: i32,
    f52: i32,
    f53: i32,
    f54: i32,
    f55: i32,
    f56: i32,
    f57: i32,
    f58: i32,
    f59: i32,
    f60: i32,
    f61: i32,
    f62: i32,
    f63: i32,
    f64: i32,
    f65: i32,
    f66: i32,
    f67: i32,
    f68: i32,
    f69: i32,
}

impl Big {
    fn all(v: i32) -> Big {
        Big {
            f00: v,
            f01: v,
            f02: v,
            f03: v,
            f04: v,
            f05: v,
            f06: v,
            f07: v,
            f08: v,
            f09: v,
            f10: v,
            f11: v,
            f12: v,
            f13: v,
            f14: v,
            f15: v,
            f16: v,
            f17: v,
            f18: v,
            f19: v,
            f20: v,
            f21: v,
            f22: v,
            f23: v,
            f24: v,
            f25: v,
            f26: v,
            f27: v,
            f28: v,
            f29: v,
            f30: v,
            f31: v,
            f32: v,
            f33: v,
            f34: v,
            f35: v,
            f36: v,
            f37: v,
            f38: v,
            f39: v,
            f40: v,
            f41: v,
            f42: v,
            f43: v,
            f44: v,
            f45: v,
            f46: v,
            f47: v,
            f48: v,
            f49: v,
            f50: v,
            f51: v,
            f52: v,
            f53: v,
            f54: v,
            f55: v,
            f56: v,
            f57: v,
            f58: v,
            f59: v,
            f60: v,
            f61: v,
            f62: v,
            f63: v,
            f64: v,
            f65: v,
            f66: v,
            f67: v,
            f68: v,
            f69: v,
        }
    }
}

#[test]
fn large_struct_over_64_fields() {
    let v = Big::all(1);
    let json = to_string(&v).unwrap();
    let back: Big = from_str(&json).unwrap();
    assert_eq!(back, v);
    // Random input: All 70 fields are provided in reverse order, yet they can still be correctly placed.
    let mut flipped = String::from("{");
    for i in (0..70).rev() {
        if i != 69 {
            flipped.push(',');
        }
        flipped.push_str(&format!(r#""f{i:02}":{i}"#));
    }
    flipped.push('}');
    let back: Big = from_str(&flipped).unwrap();
    assert_eq!(back.f00, 0);
    assert_eq!(back.f01, 1);
    assert_eq!(back.f02, 2);
    assert_eq!(back.f67, 67);
    assert_eq!(back.f68, 68);
    assert_eq!(back.f69, 69);
}

// ---------------------------------------------------------------------------
// 9. Duplicate fields are rejected (serde derive semantics)
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct Dup {
    a: i32,
    b: String,
}

#[test]
fn duplicate_keys_are_rejected() {
    let err = from_str::<Dup>(r#"{"a":1,"a":2,"b":"x"}"#).unwrap_err();
    assert_eq!(err.classification(), "duplicate field");
    assert!(err.to_string().contains("duplicate field `a`"));
}

// ---------------------------------------------------------------------------
// 10. deny_unknown_fields + default
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(deny_unknown_fields, default)]
struct StrictDefault {
    a: i32,
    b: Option<String>,
}

#[test]
fn deny_unknown_with_default() {
    let back: StrictDefault = from_str(r#"{"a":1}"#).unwrap();
    assert_eq!(back, StrictDefault { a: 1, b: None });
    assert!(from_str::<StrictDefault>(r#"{"a":1,"nope":2}"#).is_err());
}

// ---------------------------------------------------------------------------
// 11. Flatten runtime behavior (map and nested structs)
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct FlatOuter {
    id: u32,
    #[njson(flatten)]
    rest: std::collections::BTreeMap<String, Value>,
}

#[test]
fn flatten_map_roundtrip() {
    let v = FlatOuter {
        id: 1,
        rest: std::collections::BTreeMap::from([
            ("k".to_string(), Value::from(2)),
            ("s".to_string(), Value::from("x")),
        ]),
    };
    let json = to_string(&v).unwrap();
    assert_eq!(json, r#"{"id":1,"k":2,"s":"x"}"#);
    let back: FlatOuter = from_str(r#"{"k":2,"id":9,"s":"x"}"#).unwrap();
    assert_eq!(back.id, 9);
    assert_eq!(back.rest.get("k"), Some(&Value::from(2)));
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct FlatInner {
    x: i32,
    y: i32,
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct FlatNested {
    name: String,
    #[njson(flatten)]
    inner: FlatInner,
}

#[test]
fn flatten_struct_roundtrip() {
    let v = FlatNested {
        name: "n".into(),
        inner: FlatInner { x: 1, y: 2 },
    };
    let json = to_string(&v).unwrap();
    assert_eq!(json, r#"{"name":"n","x":1,"y":2}"#);
    let back: FlatNested = from_str(r#"{"y":2,"name":"n","x":1}"#).unwrap();
    assert_eq!(back, v);
}

#[test]
fn flatten_struct_roundtrips_without_a_json_intermediate() {
    use nextjson::formats::{Cbor, Format, Json, MsgPack};

    let value = FlatNested {
        name: "nested".into(),
        inner: FlatInner { x: 11, y: 22 },
    };
    let json = Json.encode(&value).unwrap();
    assert_eq!(Json.decode::<FlatNested>(&json).unwrap(), value);
    let cbor = Cbor.encode(&value).unwrap();
    assert_eq!(Cbor.decode::<FlatNested>(&cbor).unwrap(), value);
    let msgpack = MsgPack.encode(&value).unwrap();
    assert_eq!(MsgPack.decode::<FlatNested>(&msgpack).unwrap(), value);
}

#[test]
fn flatten_optional_struct_preserves_root_option_semantics() {
    use nextjson::formats::{Cbor, Format, Json, MsgPack};

    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct OptionalFlatten {
        id: u32,
        #[njson(flatten)]
        inner: Option<FlatInner>,
    }

    let value = OptionalFlatten {
        id: 7,
        inner: Some(FlatInner { x: 11, y: 22 }),
    };
    let json = Json.encode(&value).unwrap();
    assert_eq!(json, br#"{"id":7,"x":11,"y":22}"#);
    assert_eq!(Json.decode::<OptionalFlatten>(&json).unwrap(), value);
    let cbor = Cbor.encode(&value).unwrap();
    assert_eq!(Cbor.decode::<OptionalFlatten>(&cbor).unwrap(), value);
    let msgpack = MsgPack.encode(&value).unwrap();
    assert_eq!(MsgPack.decode::<OptionalFlatten>(&msgpack).unwrap(), value);

    let absent = OptionalFlatten { id: 7, inner: None };
    assert!(Json.encode(&absent).is_err());
    assert!(Cbor.encode(&absent).is_err());
    assert!(MsgPack.encode(&absent).is_err());
}

#[test]
fn flatten_rejects_a_non_object_root() {
    #[derive(NsonSerialize)]
    struct InvalidFlatten {
        id: u32,
        #[njson(flatten)]
        scalar: u32,
    }

    let error = to_string(&InvalidFlatten { id: 1, scalar: 2 }).unwrap_err();
    assert!(error.to_string().contains("expected an object root"));
}

// ---------------------------------------------------------------------------
// 12. The `skip_serializing_if` function interacts with the `default` function.
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct Cond {
    name: String,
    #[njson(skip_serializing_if = "Option::is_none", default)]
    note: Option<i32>,
}

#[test]
fn skip_if_with_default() {
    let v = Cond {
        name: "a".into(),
        note: None,
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"name":"a"}"#);
    let back: Cond = from_str(r#"{"name":"b"}"#).unwrap();
    assert_eq!(back.note, None);
}

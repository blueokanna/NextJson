//! 派生宏：结构体属性的集成测试。

use nextjson::{from_str, to_string, NsonDeserialize, NsonSerialize};

#[test]
fn basic_struct_roundtrip() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Point {
        x: f64,
        y: f64,
    }
    let p = Point { x: 1.5, y: -2.0 };
    assert_eq!(to_string(&p).unwrap(), r#"{"x":1.5,"y":-2.0}"#);
    let back: Point = from_str(r#"{"y":-2.0,"x":1.5}"#).unwrap();
    assert_eq!(back, p);
}

#[test]
fn tuple_struct() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Pair(i32, String);
    let p = Pair(42, "hi".into());
    assert_eq!(to_string(&p).unwrap(), r#"[42,"hi"]"#);
    let back: Pair = from_str(r#"[42,"hi"]"#).unwrap();
    assert_eq!(back, p);
}

#[test]
fn unit_struct() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Unit;
    assert_eq!(to_string(&Unit).unwrap(), "null");
    let back: Unit = from_str("null").unwrap();
    assert_eq!(back, Unit);
}

#[test]
fn empty_tuple_struct() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct EmptyTuple();
    assert_eq!(to_string(&EmptyTuple()).unwrap(), "[]");
    let back: EmptyTuple = from_str("[]").unwrap();
    assert_eq!(back, EmptyTuple());
}

#[test]
fn rename_and_rename_all() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(rename_all = "camelCase")]
    struct User {
        first_name: String,
        last_name: String,
        #[njson(rename = "emailAddr")]
        email_address: String,
    }
    let u = User {
        first_name: "Ada".into(),
        last_name: "Lovelace".into(),
        email_address: "ada@x.com".into(),
    };
    assert_eq!(
        to_string(&u).unwrap(),
        r#"{"firstName":"Ada","lastName":"Lovelace","emailAddr":"ada@x.com"}"#
    );
    let back: User = from_str(
        r#"{"firstName":"Ada","lastName":"Lovelace","emailAddr":"ada@x.com"}"#,
    )
    .unwrap();
    assert_eq!(back, u);
}

#[test]
fn skip_serializing() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Secret {
        name: String,
        #[njson(skip_serializing, default)]
        password: String,
    }
    let s = Secret {
        name: "a".into(),
        password: "hunter2".into(),
    };
    assert_eq!(to_string(&s).unwrap(), r#"{"name":"a"}"#);
    // 反序列化时跳过序列化的字段使用默认值。
    let back: Secret = from_str(r#"{"name":"a"}"#).unwrap();
    assert_eq!(back.password, "");
}

#[test]
fn skip_both() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        a: i32,
        #[njson(skip)]
        b: i32,
    }
    assert_eq!(to_string(&S { a: 1, b: 2 }).unwrap(), r#"{"a":1}"#);
    let back: S = from_str(r#"{"a":1,"b":99}"#).unwrap();
    assert_eq!(back, S { a: 1, b: 0 });
}

#[test]
fn skip_serializing_if() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        name: String,
        #[njson(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    }
    let s = S {
        name: "x".into(),
        note: None,
    };
    assert_eq!(to_string(&s).unwrap(), r#"{"name":"x"}"#);
    let s = S {
        name: "x".into(),
        note: Some("y".into()),
    };
    assert_eq!(to_string(&s).unwrap(), r#"{"name":"x","note":"y"}"#);
}

#[test]
fn default_field() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        a: i32,
        #[njson(default)]
        b: i32,
        #[njson(default = "default_c")]
        c: i32,
    }
    fn default_c() -> i32 {
        42
    }
    let back: S = from_str(r#"{"a":1}"#).unwrap();
    assert_eq!(back, S { a: 1, b: 0, c: 42 });
}

#[test]
fn container_default() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(default)]
    struct S {
        a: i32,
        b: String,
    }
    let back: S = from_str(r#"{"a":5}"#).unwrap();
    assert_eq!(back, S { a: 5, b: String::new() });
}

#[test]
fn option_field_is_optional() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        a: i32,
        b: Option<String>,
    }
    let back: S = from_str(r#"{"a":1}"#).unwrap();
    assert_eq!(back, S { a: 1, b: None });
    let back: S = from_str(r#"{"a":1,"b":null}"#).unwrap();
    assert_eq!(back, S { a: 1, b: None });
    let back: S = from_str(r#"{"a":1,"b":"x"}"#).unwrap();
    assert_eq!(back, S { a: 1, b: Some("x".into()) });
}

#[test]
fn missing_required_field_errors() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        a: i32,
        b: i32,
    }
    let err = from_str::<S>(r#"{"a":1}"#).unwrap_err();
    assert!(err.to_string().contains("missing field `b`"));
}

#[test]
fn deny_unknown_fields() {
    #[derive(NsonSerialize, NsonDeserialize)]
    #[njson(deny_unknown_fields)]
    struct S {
        a: i32,
    }
    assert!(from_str::<S>(r#"{"a":1,"extra":2}"#).is_err());
    assert!(from_str::<S>(r#"{"a":1}"#).is_ok());
}

#[test]
fn unknown_fields_are_skipped_by_default() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        a: i32,
    }
    let back: S = from_str(r#"{"a":1,"extra":2,"more":[1,2]}"#).unwrap();
    assert_eq!(back, S { a: 1 });
}

#[test]
fn aliases() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        #[njson(alias = "A", alias = "a")]
        value: i32,
    }
    let back: S = from_str(r#"{"A":1}"#).unwrap();
    assert_eq!(back.value, 1);
    let back: S = from_str(r#"{"a":2}"#).unwrap();
    assert_eq!(back.value, 2);
    let back: S = from_str(r#"{"value":3}"#).unwrap();
    assert_eq!(back.value, 3);
}

#[test]
fn transparent_newtype() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(transparent)]
    struct Wrapper(String);

    assert_eq!(to_string(&Wrapper("hi".into())).unwrap(), r#""hi""#);
    let back: Wrapper = from_str(r#""hi""#).unwrap();
    assert_eq!(back.0, "hi");
}

#[test]
fn with_module() {
    mod custom {
        use nextjson::{Decoder, Encoder, Result, Write};

        pub fn serialize(s: &str, e: &mut Encoder<impl Write>) -> Result<()> {
            e.write_str(&s.to_uppercase())
        }
        pub fn deserialize<'de>(d: &mut Decoder<'de>) -> Result<String> {
            Ok(d.string()?.to_lowercase())
        }
    }

    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        #[njson(with = "custom")]
        name: String,
    }
    let s = S { name: "abc".into() };
    assert_eq!(to_string(&s).unwrap(), r#"{"name":"ABC"}"#);
    let back: S = from_str(r#"{"name":"DEF"}"#).unwrap();
    assert_eq!(back.name, "def");
}

#[test]
fn serialize_with_and_deserialize_with() {
    fn ser_double(v: &i32, e: &mut nextjson::Encoder<impl nextjson::Write>) -> nextjson::Result<()> {
        e.write_i64(*v as i64 * 2)
    }
    fn de_half(d: &mut nextjson::Decoder) -> nextjson::Result<i32> {
        Ok(d.number()?.as_i64().unwrap() as i32 / 2)
    }

    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        #[njson(serialize_with = "ser_double", deserialize_with = "de_half")]
        v: i32,
    }
    assert_eq!(to_string(&S { v: 10 }).unwrap(), r#"{"v":20}"#);
    let back: S = from_str(r#"{"v":20}"#).unwrap();
    assert_eq!(back.v, 10);
}

#[test]
fn flatten_struct() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Inner {
        x: i32,
        y: i32,
    }
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Outer {
        name: String,
        #[njson(flatten)]
        inner: Inner,
    }
    let o = Outer {
        name: "n".into(),
        inner: Inner { x: 1, y: 2 },
    };
    assert_eq!(
        to_string(&o).unwrap(),
        r#"{"name":"n","x":1,"y":2}"#
    );
    let back: Outer = from_str(r#"{"y":2,"name":"n","x":1}"#).unwrap();
    assert_eq!(back, o);
}

#[test]
fn flatten_map() {
    use std::collections::HashMap;
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Outer {
        a: i32,
        #[njson(flatten)]
        extra: HashMap<String, i32>,
    }
    let o = Outer {
        a: 1,
        extra: HashMap::from([("b".to_string(), 2), ("c".to_string(), 3)]),
    };
    let text = to_string(&o).unwrap();
    let back: Outer = from_str(&text).unwrap();
    assert_eq!(back.a, 1);
    assert_eq!(back.extra.get("b"), Some(&2));
    assert_eq!(back.extra.get("c"), Some(&3));
    // 反序列化时未命中的键进入 flatten map。
    let back: Outer = from_str(r#"{"a":9,"z":42}"#).unwrap();
    assert_eq!(back.extra.get("z"), Some(&42));
}

#[test]
fn generic_struct() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Box2<T> {
        value: T,
        count: usize,
    }
    let b = Box2 {
        value: vec![1, 2, 3],
        count: 3,
    };
    let text = to_string(&b).unwrap();
    assert_eq!(text, r#"{"value":[1,2,3],"count":3}"#);
    let back: Box2<Vec<i32>> = from_str(&text).unwrap();
    assert_eq!(back, b);
}

#[test]
fn const_generic_struct() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Grid<T, const N: usize> {
        cells: [T; N],
    }
    let g = Grid { cells: [1, 2, 3] };
    let text = to_string(&g).unwrap();
    assert_eq!(text, r#"{"cells":[1,2,3]}"#);
    let back: Grid<i32, 3> = from_str(&text).unwrap();
    assert_eq!(back, g);
}

#[test]
fn custom_bound() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq, Clone)]
    #[njson(bound = "T: Clone + NsonSerialize + for<'a> NsonDeserialize<'a>")]
    struct S<T> {
        v: T,
        #[njson(skip)]
        marker: std::marker::PhantomData<T>,
    }
    let s: S<i32> = S {
        v: 7,
        marker: std::marker::PhantomData,
    };
    let text = to_string(&s).unwrap();
    assert_eq!(text, r#"{"v":7}"#);
    let back: S<i32> = from_str(&text).unwrap();
    assert_eq!(back.v, 7);
}

#[test]
fn borrowed_str() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Borrowed<'a> {
        name: &'a str,
        #[njson(borrow)]
        note: &'a str,
    }
    let b = Borrowed {
        name: "n",
        note: "note",
    };
    assert_eq!(to_string(&b).unwrap(), r#"{"name":"n","note":"note"}"#);
    let input = r#"{"name":"hello","note":"world"}"#;
    let back: Borrowed<'_> = from_str(input).unwrap();
    assert_eq!(back.name, "hello");
    assert_eq!(back.note, "world");
}

#[test]
fn nested_containers() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Root {
        items: Vec<Option<Vec<i32>>>,
        table: std::collections::BTreeMap<String, Vec<(i32, bool)>>,
    }
    let r = Root {
        items: vec![Some(vec![1, 2]), None, Some(vec![])],
        table: std::collections::BTreeMap::from([
            ("k".to_string(), vec![(1, true), (2, false)]),
        ]),
    };
    let text = to_string(&r).unwrap();
    let back: Root = from_str(&text).unwrap();
    assert_eq!(back, r);
}

#[test]
fn many_fields_uses_bitmask() {
    // 8 个字段验证位掩码路径。
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Many {
        a: i32,
        b: i32,
        c: i32,
        d: i32,
        e: i32,
        f: i32,
        g: i32,
        h: i32,
    }
    let m = Many {
        a: 1,
        b: 2,
        c: 3,
        d: 4,
        e: 5,
        f: 6,
        g: 7,
        h: 8,
    };
    let text = to_string(&m).unwrap();
    let back: Many = from_str(&text).unwrap();
    assert_eq!(back, m);
    // 缺失必填字段报错。
    assert!(from_str::<Many>(r#"{"a":1}"#).is_err());
}

#[test]
fn unit_variant_field() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        flag: (),
    }
    let s = S { flag: () };
    assert_eq!(to_string(&s).unwrap(), r#"{"flag":null}"#);
    let back: S = from_str(r#"{"flag":null}"#).unwrap();
    assert_eq!(back, s);
}

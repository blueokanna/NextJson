//! Core API integration testing: application entry point, Value,
//! Map, Number, json! macros, error handling.

use nextjson::{json, to_value, Error, Map, NsonDeserialize, Number, Value};

// ---------------------------------------------------------------------------
// Entrance
// ---------------------------------------------------------------------------

#[test]
fn to_string_and_from_str() {
    let v = Value::Object(Map::from_iter(vec![
        ("a".to_string(), Value::Number(1.into())),
        ("b".to_string(), Value::Bool(true)),
        ("c".to_string(), Value::Null),
        (
            "d".to_string(),
            Value::Array(vec![Value::String("x".into())]),
        ),
    ]));
    let text = nextjson::to_string(&v).unwrap();
    assert_eq!(text, r#"{"a":1,"b":true,"c":null,"d":["x"]}"#);
    let back: Value = nextjson::from_str(&text).unwrap();
    assert_eq!(back, v);
}

#[test]
fn to_string_pretty() {
    let v = json!({ "a": 1, "b": [2, 3] });
    let text = nextjson::to_string_pretty(&v).unwrap();
    assert_eq!(text, "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}");
}

#[test]
fn from_slice_and_reader() {
    let v: Value = nextjson::from_slice(br#"{"x":1}"#).unwrap();
    assert_eq!(v["x"], 1.into());
    let cursor = std::io::Cursor::new(br#"{"y":[1,2]}"#);
    let v: Value = nextjson::from_reader(cursor).unwrap();
    assert_eq!(v["y"][0], 1.into());
}

#[test]
fn to_writer() {
    let v = json!({"k":"v"});
    let mut buf = Vec::new();
    nextjson::to_writer(&mut buf, &v).unwrap();
    assert_eq!(buf, br#"{"k":"v"}"#);
}

// ---------------------------------------------------------------------------
// json! Macro
// ---------------------------------------------------------------------------

#[test]
fn json_macro_basics() {
    let v = json!(null);
    assert_eq!(v, Value::Null);
    let v = json!(true);
    assert_eq!(v, Value::Bool(true));
    let v = json!([1, 2, 3]);
    assert_eq!(v, Value::Array(vec![1.into(), 2.into(), 3.into()]));
    let v = json!({"a": 1, "b": [true, null]});
    assert_eq!(v["a"], 1.into());
    assert_eq!(v["b"][0], Value::Bool(true));
    assert_eq!(v["b"][1], Value::Null);
}

#[test]
fn json_macro_interpolation() {
    let x = 42;
    let v = json!({"x": x, "y": (x * 2), "nested": json!({"z": x})});
    assert_eq!(v["x"], 42.into());
    assert_eq!(v["y"], 84.into());
    assert_eq!(v["nested"]["z"], 42.into());
}

#[test]
fn json_macro_bare_idents() {
    let v = json!({ hello: "world" });
    assert_eq!(v["hello"], "world".into());
}

// ---------------------------------------------------------------------------
// Value API
// ---------------------------------------------------------------------------

#[test]
fn value_accessors() {
    let v = json!({
        "str": "text",
        "int": 42,
        "float": 4.5,
        "bool": false,
        "arr": [1],
        "obj": {"k": "v"},
    });
    assert_eq!(v.as_str(), None);
    assert_eq!(v["str"].as_str(), Some("text"));
    assert_eq!(v["int"].as_i64(), Some(42));
    assert_eq!(v["float"].as_f64(), Some(4.5));
    assert_eq!(v["bool"].as_bool(), Some(false));
    assert_eq!(v["arr"].as_array().unwrap().len(), 1);
    assert!(v["obj"].is_object());
}

#[test]
fn value_pointer() {
    let v = json!({"a": {"b": [10, 20, 30]}});
    assert_eq!(v.pointer("/a/b/1"), Some(&Value::Number(20.into())));
    assert_eq!(v.pointer("/a/b/0"), Some(&Value::Number(10.into())));
    assert_eq!(v.pointer("/a/b/9"), None);
    assert_eq!(v.pointer("/missing"), None);
    assert_eq!(v.pointer(""), Some(&v));
}

#[test]
fn value_display() {
    let v = json!({"a": [1, "x"], "b": true});
    assert_eq!(v.to_string(), r#"{"a":[1,"x"],"b":true}"#);
}

#[test]
fn value_from_traits() {
    let v: Value = 42.into();
    assert_eq!(v, Value::Number(42.into()));
    let v: Value = "hi".into();
    assert_eq!(v, Value::String("hi".into()));
    let v: Value = true.into();
    assert_eq!(v, Value::Bool(true));
    let v: Value = Some(1).into();
    assert_eq!(v, Value::Number(1.into()));
    let v: Value = None::<i32>.into();
    assert_eq!(v, Value::Null);
}

// ---------------------------------------------------------------------------
// Map
// ---------------------------------------------------------------------------

#[test]
fn map_preserves_insertion_order() {
    let mut m = Map::new();
    m.insert("b".into(), 1.into());
    m.insert("a".into(), 2.into());
    m.insert("c".into(), 3.into());
    let keys: Vec<&str> = m.keys().collect();
    assert_eq!(keys, vec!["b", "a", "c"]);
    assert_eq!(m.get("a"), Some(&Value::Number(2.into())));
    assert_eq!(m.remove("a"), Some(Value::Number(2.into())));
    assert_eq!(m.get("a"), None);
    assert_eq!(m.len(), 2);
}

#[test]
fn map_roundtrip() {
    let text = r#"{"z":1,"y":2,"x":3}"#;
    let m: Map = nextjson::from_str(text).unwrap();
    assert_eq!(nextjson::to_string(&m).unwrap(), text);
}

// ---------------------------------------------------------------------------
// Number
// ---------------------------------------------------------------------------

#[test]
fn number_variants() {
    // 非负整数统一存为 U64（与解析结果一致，保证相等性）。
    let n: Number = 42i64.into();
    assert!(n.is_u64());
    assert_eq!(n.as_i64(), Some(42));
    let n: Number = (-42i64).into();
    assert!(n.is_i64());
    let n: Number = 42u64.into();
    assert!(n.is_u64());
    let n: Number = 1.5f64.into();
    assert!(n.is_f64());
    assert_eq!(n.as_f64(), 1.5);
}

#[test]
fn number_big_values() {
    let v: Value = nextjson::from_str("18446744073709551615").unwrap();
    assert_eq!(v.as_u64(), Some(u64::MAX));
    // Integers above u64 remain exact through the full u128 domain.
    let v: Value = nextjson::from_str("18446744073709551616").unwrap();
    assert_eq!(v.as_u128(), Some(u64::MAX as u128 + 1));
}

// ---------------------------------------------------------------------------
// Standard library type round trip
// ---------------------------------------------------------------------------

#[test]
fn std_collections_roundtrip() {
    let m = std::collections::BTreeMap::from([
        ("a".to_string(), vec![1, 2]),
        ("b".to_string(), vec![]),
    ]);
    let text = nextjson::to_string(&m).unwrap();
    assert_eq!(text, r#"{"a":[1,2],"b":[]}"#);
    let back: std::collections::BTreeMap<String, Vec<i32>> = nextjson::from_str(&text).unwrap();
    assert_eq!(back, m);

    let s: std::collections::BTreeSet<i32> = [3, 1, 2].into_iter().collect();
    let text = nextjson::to_string(&s).unwrap();
    assert_eq!(text, "[1,2,3]");
    let back: std::collections::BTreeSet<i32> = nextjson::from_str(&text).unwrap();
    assert_eq!(back, s);
}

#[test]
fn tuple_and_array_roundtrip() {
    let t = (1, "x", 2.5, true);
    let text = nextjson::to_string(&t).unwrap();
    assert_eq!(text, r#"[1,"x",2.5,true]"#);
    let back: (i32, String, f64, bool) = nextjson::from_str(&text).unwrap();
    assert_eq!(back, (1, "x".into(), 2.5, true));

    let a = [1, 2, 3];
    let text = nextjson::to_string(&a).unwrap();
    assert_eq!(text, "[1,2,3]");
    let back: [i32; 3] = nextjson::from_str(&text).unwrap();
    assert_eq!(back, a);
}

#[test]
fn misc_types_roundtrip() {
    let dur = std::time::Duration::from_nanos(123);
    assert_eq!(nextjson::to_string(&dur).unwrap(), "123");
    assert_eq!(
        nextjson::from_str::<std::time::Duration>("123").unwrap(),
        dur
    );

    let long_duration = std::time::Duration::new(u64::MAX, 999_999_999);
    let encoded = nextjson::to_string(&long_duration).unwrap();
    assert_eq!(encoded, long_duration.as_nanos().to_string());
    assert_eq!(
        nextjson::from_str::<std::time::Duration>(&encoded).unwrap(),
        long_duration
    );
    let overflow = (long_duration.as_nanos() + 1).to_string();
    assert!(nextjson::from_str::<std::time::Duration>(&overflow).is_err());

    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    assert_eq!(nextjson::to_string(&ip).unwrap(), r#""127.0.0.1""#);
    assert_eq!(
        nextjson::from_str::<std::net::IpAddr>(r#""127.0.0.1""#).unwrap(),
        ip
    );

    let p = std::path::PathBuf::from("/tmp/a");
    assert_eq!(nextjson::to_string(&p).unwrap(), r#""/tmp/a""#);

    let r: std::ops::Range<i32> = 1..5;
    assert_eq!(nextjson::to_string(&r).unwrap(), "[1,5]");
    assert_eq!(
        nextjson::from_str::<std::ops::Range<i32>>("[1,5]").unwrap(),
        r
    );

    let res: Result<i32, String> = Ok(7);
    assert_eq!(nextjson::to_string(&res).unwrap(), r#"{"Ok":7}"#);
    let back: Result<i32, String> = nextjson::from_str(r#"{"Err":"boom"}"#).unwrap();
    assert_eq!(back, Err("boom".into()));

    let opt: Option<i32> = None;
    assert_eq!(nextjson::to_string(&opt).unwrap(), "null");
    let back: Option<i32> = nextjson::from_str("null").unwrap();
    assert_eq!(back, None);
}

#[test]
fn boxed_and_atomic() {
    let b = Box::new(vec![1, 2]);
    assert_eq!(nextjson::to_string(&b).unwrap(), "[1,2]");

    let a = std::sync::atomic::AtomicI32::new(5);
    assert_eq!(nextjson::to_string(&a).unwrap(), "5");
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

#[test]
fn parse_errors() {
    let cases = [
        r#""unterminated"#,
        r#"{"a":}"#,
        r#"[1,2"#,
        r#"tru"#,
        r#"01"#,
        r#"1."#,
        r#"{"a" 1}"#,
        r#"}"#,
    ];
    for c in cases {
        let r: Result<Value, Error> = nextjson::from_str(c);
        assert!(r.is_err(), "expected error for {c:?}");
    }
}

#[test]
fn error_has_position() {
    let err: Error = nextjson::from_str::<Value>("{\n  \"a\": tru\n}").unwrap_err();
    assert_eq!(err.line(), Some(2));
    assert!(err.column().is_some());
    assert!(err.offset() > 0);
    let msg = err.to_string();
    assert!(msg.contains("line 2"), "got: {msg}");
}

#[test]
fn depth_limit_protection() {
    let deep = format!("{}0{}", "[".repeat(500), "]".repeat(500));
    let r: Result<Value, Error> = nextjson::from_str(&deep);
    assert!(r.is_err());
    // Larger depths can be configured.
    let mut d = nextjson::Decoder::with_config(
        deep.as_bytes(),
        nextjson::DecodeConfig::default().max_depth(1000),
    );
    assert!(Value::nextdecode(&mut d).is_ok());
}

#[test]
fn unicode_handling() {
    let v = json!({"emoji": "💩", "chinese": "中文", "escaped": "\u{1f4a9}"});
    let text = nextjson::to_string(&v).unwrap();
    let back: Value = nextjson::from_str(&text).unwrap();
    assert_eq!(back, v);
}

#[test]
fn escaped_input() {
    let v: Value = nextjson::from_str(r#""\u0041\u00e9\u4e2d""#).unwrap();
    assert_eq!(v, Value::String("Aé中".into()));
    // Agent pair
    let v: Value = nextjson::from_str(r#""\ud83d\udca9""#).unwrap();
    assert_eq!(v, Value::String("💩".into()));
    // Lone surrogate is invalid
    assert!(nextjson::from_str::<Value>(r#""\ud83d""#).is_err());
}

#[test]
fn whitespace_tolerance() {
    let v: Value = nextjson::from_str(" \t\n {\"a\" : 1 } \r\n").unwrap();
    assert_eq!(v["a"], 1.into());
}

#[test]
fn big_roundtrip() {
    let mut v = json!({"users": []});
    let users = v
        .as_object_mut()
        .unwrap()
        .get_mut("users")
        .unwrap()
        .as_array_mut()
        .unwrap();
    for i in 0..1000 {
        users.push(json!({"id": i, "name": format!("user{i}"), "score": i as f64 * 1.5}));
    }
    let text = nextjson::to_string(&v).unwrap();
    let back: Value = nextjson::from_str(&text).unwrap();
    assert_eq!(back, v);
}

#[test]
fn to_value_roundtrip() {
    let v = json!({"a": [1, 2], "b": {"c": null}});
    let v2: Value = to_value(&v).unwrap();
    assert_eq!(v, v2);
}

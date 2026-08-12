//! Miscellaneous entry coverage: TypeSchema::name full variant, top-level pretty/slice/reader entry,
//! Value Display, Map operations.

use nextjson::{schema_of, Bytes, NsonSerialize, Value};

#[test]
fn type_schema_name_all_variants() {
    assert_eq!(schema_of::<()>().name(), "unit");
    assert_eq!(schema_of::<bool>().name(), "bool");
    assert_eq!(schema_of::<i8>().name(), "i8");
    assert_eq!(schema_of::<i16>().name(), "i16");
    assert_eq!(schema_of::<i32>().name(), "i32");
    assert_eq!(schema_of::<i64>().name(), "i64");
    assert_eq!(schema_of::<i128>().name(), "i128");
    assert_eq!(schema_of::<isize>().name(), "isize");
    assert_eq!(schema_of::<u8>().name(), "u8");
    assert_eq!(schema_of::<u16>().name(), "u16");
    assert_eq!(schema_of::<u32>().name(), "u32");
    assert_eq!(schema_of::<u64>().name(), "u64");
    assert_eq!(schema_of::<u128>().name(), "u128");
    assert_eq!(schema_of::<usize>().name(), "usize");
    assert_eq!(schema_of::<f32>().name(), "f32");
    assert_eq!(schema_of::<f64>().name(), "f64");
    assert_eq!(schema_of::<char>().name(), "char");
    assert_eq!(schema_of::<String>().name(), "string");
    assert_eq!(schema_of::<Bytes<'static>>().name(), "bytes");
    assert_eq!(schema_of::<Vec<i32>>().name(), "sequence");
    assert_eq!(
        schema_of::<std::collections::BTreeMap<String, i32>>().name(),
        "map"
    );
    assert_eq!(schema_of::<Option<i32>>().name(), "i32");
    assert_eq!(schema_of::<(i32, i32)>().name(), "tuple");

    #[derive(NsonSerialize)]
    struct S {
        x: i32,
    }
    assert_eq!(schema_of::<S>().name(), "S");
    assert!(schema_of::<S>().is_object());

    #[derive(NsonSerialize)]
    #[allow(dead_code)]
    enum E {
        A,
    }
    assert_eq!(schema_of::<E>().name(), "E");
    assert!(!schema_of::<i32>().is_object());
}

#[test]
fn pretty_and_slice_and_reader_entry_points() {
    let pretty = nextjson::to_vec_pretty(&vec![1_i32, 2]).unwrap();
    assert!(String::from_utf8_lossy(&pretty).contains('\n'));
    let mut sink = Vec::new();
    nextjson::to_writer_pretty(&mut sink, &true).unwrap();
    assert_eq!(sink.as_slice(), b"true");
    let v: u32 = nextjson::from_slice(b"7").unwrap();
    assert_eq!(v, 7);
    let r: u32 = nextjson::from_reader(&b"9"[..]).unwrap();
    assert_eq!(r, 9);
    assert_eq!(
        nextjson::nextencode(&1_i32).unwrap(),
        nextjson::to_vec(&1_i32).unwrap()
    );
}

#[test]
fn value_display_and_map_ops() {
    use nextjson::Map;
    let cases: &[(Value, &str)] = &[
        (Value::Null, "null"),
        (Value::from(true), "true"),
        (Value::from(1_i64), "1"),
        (Value::from(1.5_f64), "1.5"),
        (Value::from("s"), "\"s\""),
        (Value::Array(vec![Value::from(1)]), "[1]"),
        (Value::Object(Map::new()), "{}"),
    ];
    for (v, expected) in cases {
        assert_eq!(v.to_string(), *expected, "display failed for {expected}");
    }

    let mut m = Map::new();
    m.insert("b".into(), Value::from(1));
    m.insert("a".into(), Value::from(2));
    assert_eq!(m.len(), 2);
    assert_eq!(m.get("a"), Some(&Value::from(2)));
    assert!(m.contains_key("b"));
    m.insert("a".into(), Value::from(3));
    assert_eq!(m.get("a"), Some(&Value::from(3)));
    assert_eq!(m.len(), 2);
    assert!(m.remove("b").is_some());
    assert!(!m.contains_key("b"));
}

#[test]
fn value_typed_accessors() {
    let v = Value::from(42_i64);
    assert_eq!(v.as_i64(), Some(42));
    assert_eq!(v.as_u64(), Some(42)); // 非负 i64 可转 u64
    assert_eq!(Value::from(-5_i64).as_u64(), None);
    assert_eq!(v.as_f64(), Some(42.0));
    assert_eq!(Value::from(3_u64).as_u64(), Some(3));
    assert_eq!(Value::from("x").as_str(), Some("x"));
    assert!(Value::Null.is_null());
    assert!(Value::from(true).as_bool() == Some(true));
    assert_eq!(Value::from('c').as_str(), Some("c"));
}

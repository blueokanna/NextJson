//! The public API coverage of Number/Map provides a safety margin for coverage access control

use nextjson::{Map, Number, Value};

#[test]
fn number_typed_accessors_and_flags() {
    let u = Number::from(7_u64);
    assert!(u.is_u64());
    assert!(u.is_integer());
    assert!(u.is_finite());
    assert!(!u.is_f64());
    assert_eq!(u.as_u64(), Some(7));
    assert_eq!(u.as_i64(), Some(7));
    assert_eq!(u.as_i128(), Some(7));
    assert_eq!(u.as_u128(), Some(7));
    assert_eq!(u.as_f64(), 7.0);

    let i = Number::from(-7_i64);
    assert!(i.is_i64());
    assert_eq!(i.as_i64(), Some(-7));
    assert_eq!(i.as_u64(), None);
    assert_eq!(i.as_u128(), None);

    // Non-negative values ​​are uniformly stored as unsigned (i128::MAX → U128);
    // Negative i128 values ​​follow the I128 path
    let big = Number::from(-(1_i128 << 70));
    assert!(big.is_i128());
    assert_eq!(big.as_i128(), Some(-(1_i128 << 70)));
    assert_eq!(big.as_i64(), None);

    let maxi = Number::from(i128::MAX);
    assert!(maxi.is_u128());
    assert_eq!(maxi.as_u128(), Some(i128::MAX as u128));

    let bigu = Number::from(u128::MAX);
    assert!(bigu.is_u128());
    assert_eq!(bigu.as_u128(), Some(u128::MAX));
    assert_eq!(bigu.as_u64(), None);

    let f = Number::from(1.5_f64);
    assert!(f.is_f64());
    assert!(!f.is_integer());
    assert_eq!(f.as_f64(), 1.5);
    assert_eq!(f.as_i64(), Some(1));

    // 所有 From 路径。
    let _ = Number::from(1_i8);
    let _ = Number::from(1_i16);
    let _ = Number::from(1_i32);
    let _ = Number::from(1_isize);
    let _ = Number::from(1_u8);
    let _ = Number::from(1_u16);
    let _ = Number::from(1_u32);
    let _ = Number::from(1_usize);
    let _ = Number::from(1.0_f32);
    // Non-finite float → None
    assert!(Number::from_f64(f64::NAN).is_none());
    assert!(Number::from_f64(f64::INFINITY).is_none());
    assert!(Number::from_f64(2.5).is_some());
}

#[test]
fn number_in_json_value_roundtrip_and_display() {
    let u: Number = 5_u64.into();
    assert_eq!(u.to_string(), "5");
    let f: Number = 5.0_f64.into();
    assert_eq!(f.to_string(), "5");
    let big: Number = u64::MAX.into();
    assert_eq!(big.to_string(), u64::MAX.to_string());

    let v = Value::from(3_u64);
    match v {
        Value::Number(n) => assert_eq!(n.as_u64(), Some(3)),
        _ => panic!("expected number"),
    }
}

#[test]
fn map_iteration_and_construction() {
    let mut m = Map::new();
    m.insert("a".into(), Value::from(1));
    m.insert("b".into(), Value::from(2));

    // Iteration maintains the insertion order
    let keys: Vec<&str> = m.keys().collect();
    assert_eq!(keys, vec!["a", "b"]);
    let values: Vec<&Value> = m.values().collect();
    assert_eq!(values.len(), 2);
    let pairs: Vec<(&str, i64)> = m.iter().map(|(k, v)| (k, v.as_i64().unwrap())).collect();
    assert_eq!(pairs, vec![("a", 1), ("b", 2)]);

    // from_iter / into_iter。
    let m2 = Map::from_iter(vec![("x".to_string(), Value::from(9))]);
    assert_eq!(m2.get("x"), Some(&Value::from(9)));
    let collected: Vec<(String, Value)> = m2.into_iter().collect();
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].0, "x");

    // Overwrite inserts maintain length; iterate and synchronize after removal
    m.insert("a".into(), Value::from(10));
    assert_eq!(m.len(), 2);
    m.remove("a");
    assert_eq!(m.len(), 1);
    assert!(m.contains_key("b"));

    // clear / is_empty。
    let mut empty = Map::new();
    assert!(empty.is_empty());
    empty.insert("k".into(), Value::Null);
    assert!(!empty.is_empty());
    empty.clear();
    assert!(empty.is_empty());
}

#[test]
fn map_equality_and_pointer() {
    let mut a = Map::new();
    a.insert("k".into(), Value::from(1));
    let mut b = Map::new();
    b.insert("k".into(), Value::from(1));
    assert_eq!(a, b);

    let v = Value::Object(a);
    assert_eq!(v.pointer("/k"), Some(&Value::from(1)));
    assert_eq!(v.pointer("/missing"), None);
    let arr = Value::from(vec![Value::from(1), Value::from(2)]);
    assert_eq!(arr.pointer("/1"), Some(&Value::from(2)));
    assert_eq!(arr.pointer("/x"), None);
}

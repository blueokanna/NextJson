//! Depth / skip / option / map-key coverage for the native decoder and the
//! Value / Map / Number public APIs.

use nextjson::{DecodeConfig, Decoder, FormatDecoder, Map, NsonDeserialize, Number, Value};

#[test]
fn decoder_skip_value_covers_all_shapes() {
    let mut d = Decoder::new(br#"{"a":[1,{"b":2},[3,4]],"c":null,"d":[true,false,"s"]}"#);
    d.begin_object().unwrap();
    let mut seen = Vec::new();
    while let Some(key) = d.object_key().unwrap() {
        seen.push(key.to_string());
        d.skip_value().unwrap();
        if !d.object_entry_sep().unwrap() {
            break;
        }
    }
    d.end_object().unwrap();
    d.end().unwrap();
    assert_eq!(seen, ["a", "c", "d"]);
}

#[test]
fn decoder_skip_value_scalars() {
    for input in ["1", "-2.5", "true", "false", "null", r#""str""#] {
        let mut d = Decoder::new(input.as_bytes());
        d.skip_value().unwrap();
        d.end().unwrap();
    }
}

#[test]
fn decoder_config_max_depth() {
    let config = DecodeConfig::new().max_depth(2);
    let deep = r#"{"a":{"b":{"c":1}}}"#;
    let mut d = Decoder::with_config(deep.as_bytes(), config);
    assert!(Value::nextdecode(&mut d).is_err());
    assert_eq!(d.max_depth(), 2);

    // Default depth allows the same input.
    let mut d = Decoder::new(deep.as_bytes());
    let v = Value::nextdecode(&mut d).unwrap();
    assert_eq!(v["a"]["b"]["c"], Value::from(1_i64));
}

#[test]
fn decoder_option_tag_and_bytes() {
    let mut d = Decoder::new(b"null");
    assert_eq!(d.option_tag().unwrap(), nextjson::OptionTag::None);
    let mut d = Decoder::new(b"5");
    assert_eq!(d.option_tag().unwrap(), nextjson::OptionTag::Some);

    // bytes: string form (borrowed) and array-of-u8 form.
    let mut d = Decoder::new(br#""abc""#);
    assert_eq!(d.bytes().unwrap().as_ref(), b"abc");
    let mut d = Decoder::new(b"[1,2,3]");
    assert_eq!(d.bytes().unwrap().as_ref(), b"\x01\x02\x03");
}

#[test]
fn map_and_number_api_surface() {
    // Map public API.
    let mut m = Map::new();
    assert!(m.is_empty());
    m.insert("a".to_string(), Value::from(1));
    m.insert("b".to_string(), Value::from("x"));
    assert_eq!(m.len(), 2);
    assert!(m.contains_key("a"));
    assert!(m.get("a").is_some());
    assert!(m.get_mut("a").is_some());
    assert_eq!(m.keys().count(), 2);
    assert_eq!(m.values().count(), 2);
    assert_eq!(m.iter().count(), 2);
    assert_eq!(m.iter_mut().count(), 2);
    let removed = m.remove("a");
    assert!(removed.is_some());
    m.retain(|k, _| k == "b");
    assert_eq!(m.len(), 1);
    let mut m2 = Map::with_capacity(4);
    m2.insert("k".to_string(), Value::from(1));
    m2.clear();
    assert!(m2.is_empty());

    // Number API surface.
    // Non-negative integers normalize to the U64 representation (so parsed
    // and constructed values compare equal); `as_i64` still returns a value.
    let n = Number::from(1_i64);
    assert!(n.is_u64());
    assert!(!n.is_f64());
    assert!(n.is_integer());
    assert!(n.is_finite());
    assert_eq!(n.as_i64(), Some(1));
    assert_eq!(n.as_f64(), 1.0);
    let u = Number::from(2_u64);
    assert!(u.is_u64());
    assert_eq!(u.as_u64(), Some(2));
    assert_eq!(u.as_i128(), Some(2));
    let i = Number::from(-3_i128);
    assert!(i.is_i64()); // values in i64 range normalize to I64
    assert_eq!(i.as_i128(), Some(-3));
    let big_i = Number::from(i128::MIN);
    assert!(big_i.is_i128());
    assert_eq!(big_i.as_i128(), Some(i128::MIN));
    let big = Number::from(u128::MAX);
    assert!(big.is_u128());
    assert_eq!(big.as_u128(), Some(u128::MAX));
    let f = Number::from_f64(2.5).unwrap();
    assert!(f.is_f64());
    assert_eq!(f.as_f64(), 2.5);
    assert!(Number::from_f64(f64::NAN).is_none());
    assert!(Number::from_f64(f64::INFINITY).is_none());
}

#[test]
fn value_api_surface() {
    let v = nextjson::json!({"a": [1, 2.5, "s", true, null], "b": "t"});
    assert!(v.is_object());
    assert!(v["a"].is_array());
    assert!(v["a"][0].is_number());
    assert!(v["a"][1].is_number());
    assert!(v["a"][2].is_string());
    assert!(v["a"][3].is_bool());
    assert!(v["a"][4].is_null());
    assert!(v["b"].is_string());
    assert!(v.as_object().is_some());
    assert!(v.as_array().is_none());
    assert!(v.get("b").is_some());
    assert!(v.pointer("/b").is_some());
    assert!(v.pointer("/a/0").is_some());
    assert!(v.pointer("/missing").is_none());
    assert_eq!(v["a"][0].as_u64(), Some(1));
    assert_eq!(v["a"][0].as_i64(), Some(1));
    assert_eq!(v["a"][0].as_i128(), Some(1));
    assert_eq!(v["a"][0].as_u128(), Some(1));
    assert_eq!(v["a"][1].as_f64(), Some(2.5));
    assert_eq!(v["a"][2].as_str(), Some("s"));
    assert_eq!(v["a"][3].as_bool(), Some(true));
    assert!(v["a"][4].is_null());
    assert!(v["a"][1].as_number().is_some());

    let mut arr = nextjson::json!([1, 2]);
    assert!(arr.as_array_mut().is_some());
    assert!(arr.as_object_mut().is_none());
    let mut obj = nextjson::json!({"x": 1});
    assert!(obj.as_object_mut().is_some());
    assert!(obj.get_mut("x").is_some());

    // into_* consumers.
    assert!(nextjson::json!({"x": 1}).into_object().is_some());
    assert!(nextjson::json!([1]).into_array().is_some());
    assert!(nextjson::json!(1).into_object().is_none());
    assert!(nextjson::json!(1).into_array().is_none());

    // From conversions.
    let _: Value = 5i8.into();
    let _: Value = 5i16.into();
    let _: Value = 5i32.into();
    let _: Value = 5i64.into();
    let _: Value = 5i128.into();
    let _: Value = 5u8.into();
    let _: Value = 5u16.into();
    let _: Value = 5u32.into();
    let _: Value = 5u64.into();
    let _: Value = 5u128.into();
    let _: Value = 5isize.into();
    let _: Value = 5usize.into();
    let _: Value = 1.5f32.into();
    let _: Value = 1.5f64.into();
    let _: Value = 'c'.into();
    let _: Value = "str".into();
    let _: Value = String::from("s").into();
    let _: Value = true.into();
    let _: Value = nextjson::json!(null);
    let _: Value = None::<Value>.into();
    let _: Value = Some(1_i64).into();
    let _: Value = ().into();

    // Display.
    assert_eq!(nextjson::json!({"a": 1}).to_string(), r#"{"a":1}"#);
}

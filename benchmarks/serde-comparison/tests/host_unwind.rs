//! Host unwind checks; deliberately isolated from the no_std crate.
use nextjson::Value;

#[test]
fn map_remains_consistent_when_retain_predicate_panics() {
    let mut map: nextjson::Map = (0..32).map(|i| (i.to_string(), Value::from(i))).collect();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        map.retain(|key, _| {
            assert_ne!(key, "20", "predicate failure");
            key.parse::<u32>().unwrap() % 2 == 0
        });
    }));
    assert!(result.is_err());
    for (key, value) in map.iter() {
        assert_eq!(map.get(key), Some(value), "stale index for {key}");
    }
    assert!(!map.contains_key("1"));
    map.insert("new".into(), Value::Null);
    assert_eq!(map.remove("new"), Some(Value::Null));
}

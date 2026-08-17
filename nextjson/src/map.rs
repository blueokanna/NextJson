//! Insertion-ordered JSON object map.
//!
//! Unlike `BTreeMap` (loses insertion order) and a raw `HashMap` (random
//! iteration order), `Map` keeps entries in a `Vec` and builds a `BTreeMap`
//! lookup index only for larger objects. Small JSON objects stay cache-local
//! and avoid cloning every key; larger objects retain O(log n) lookup while
//! preserving deterministic insertion order and `no_std` compatibility.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Index;

use crate::value::Value;

/// Lookup index type: `BTreeMap` keeps the core `no_std`.
type IndexMap = BTreeMap<String, usize>;

// Linear search wins for small objects: it touches one contiguous allocation
// and avoids a second allocation and key copy per entry. The exact crossover
// varies by key length and CPU, so keep this deliberately conservative.
const INDEX_THRESHOLD: usize = 16;

/// Insertion-ordered JSON object.
#[derive(Clone, Debug, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
    index: Option<IndexMap>,
}

impl PartialEq for Map {
    /// Compare only the ordered entries, ignoring index layout.
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}
impl Eq for Map {}

impl Map {
    /// Create an empty map.
    pub fn new() -> Self {
        Map::default()
    }

    /// Create an empty map with the given capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Map {
            entries: Vec::with_capacity(cap),
            index: None,
        }
    }

    #[inline]
    fn position(&self, key: &str) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(key).copied(),
            None => self.entries.iter().position(|(entry, _)| entry == key),
        }
    }

    fn build_index(&mut self) {
        if self.entries.len() < INDEX_THRESHOLD {
            return;
        }
        let mut index = IndexMap::new();
        for (position, (key, _)) in self.entries.iter().enumerate() {
            index.insert(key.clone(), position);
        }
        self.index = Some(index);
    }

    /// Insert a key-value pair; returns the previous value if the key existed.
    pub fn insert(&mut self, key: String, value: Value) -> Option<Value> {
        if let Some(idx) = self.position(&key) {
            let old = core::mem::replace(&mut self.entries[idx].1, value);
            return Some(old);
        }
        if let Some(index) = &mut self.index {
            index.insert(key.clone(), self.entries.len());
        }
        self.entries.push((key, value));
        if self.index.is_none() && self.entries.len() == INDEX_THRESHOLD {
            self.build_index();
        }
        None
    }

    /// Get a value by key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.position(key).map(|i| &self.entries[i].1)
    }

    /// Get a value by key (mutable).
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.position(key).map(|i| &mut self.entries[i].1)
    }

    /// Remove a key; returns the removed value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let idx = self.position(key)?;
        if let Some(index) = &mut self.index {
            index.remove(key);
        }
        let (_, value) = self.entries.remove(idx);
        if self.entries.len() < INDEX_THRESHOLD {
            self.index = None;
        } else if idx < self.entries.len() {
            if let Some(index) = &mut self.index {
                for position in index.values_mut() {
                    if *position > idx {
                        *position -= 1;
                    }
                }
            }
        }
        Some(value)
    }

    /// Whether the key exists.
    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index = None;
    }

    /// Iterate `(&str, &Value)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Iterate `(&str, &mut Value)`.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Value)> {
        self.entries.iter_mut().map(|(k, v)| (k.as_str(), v))
    }

    /// Iterate keys.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// Iterate values.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, v)| v)
    }

    /// Retain entries satisfying the predicate.
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&str, &mut Value) -> bool,
    {
        self.entries.retain_mut(|(k, v)| f(k, v));
        self.index = None;
        self.build_index();
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = alloc::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl Index<&str> for Map {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        self.get(key).expect("key not found in Map")
    }
}

impl Index<&alloc::string::String> for Map {
    type Output = Value;
    fn index(&self, key: &alloc::string::String) -> &Value {
        self.get(key).expect("key not found in Map")
    }
}

impl<K, V> FromIterator<(K, V)> for Map
where
    K: Into<String>,
    V: Into<Value>,
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let mut map = Map::with_capacity(iter.size_hint().0);
        for (k, v) in iter {
            map.insert(k.into(), v.into());
        }
        map
    }
}

impl Extend<(String, Value)> for Map {
    fn extend<T: IntoIterator<Item = (String, Value)>>(&mut self, iter: T) {
        let iter = iter.into_iter();
        self.entries.reserve(iter.size_hint().0);
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn order_and_lookup() {
        let mut m = Map::new();
        m.insert("b".into(), 1.into());
        m.insert("a".into(), 2.into());
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(m.get("a"), Some(&Value::Number(2.into())));
        assert_eq!(m.remove("a"), Some(Value::Number(2.into())));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn eq_ignores_index_layout() {
        let mut a = Map::new();
        a.insert("x".into(), 1.into());
        a.insert("y".into(), 2.into());
        let b = Map::from_iter(vec![
            ("x".to_string(), Value::Number(1.into())),
            ("y".to_string(), Value::Number(2.into())),
        ]);
        assert_eq!(a, b);
    }

    #[test]
    fn indexed_map_stays_consistent_after_mutation() {
        let mut map = Map::with_capacity(32);
        for i in 0..32 {
            map.insert(i.to_string(), Value::from(i as u64));
        }
        assert_eq!(map.get("17"), Some(&Value::from(17_u64)));
        assert_eq!(map.remove("5"), Some(Value::from(5_u64)));
        assert_eq!(map.get("17"), Some(&Value::from(17_u64)));

        map.retain(|key, _| key.parse::<usize>().unwrap() % 2 == 0);
        assert!(!map.contains_key("17"));
        assert_eq!(map.get("18"), Some(&Value::from(18_u64)));

        for i in (2..32).step_by(2) {
            if i != 18 {
                map.remove(&i.to_string());
            }
        }
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("0"), Some(&Value::from(0_u64)));
        assert_eq!(map.get("18"), Some(&Value::from(18_u64)));
    }
}

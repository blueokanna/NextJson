//! Self-describing JSON [`Value`] type (AST).

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ops::Index;

use crate::map::Map;
use crate::number::Number;

/// A self-describing JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// Number (lossless through `i128` / `u128`, see [`Number`]).
    Number(Number),
    /// string
    String(String),
    /// array
    Array(Vec<Value>),
    /// object (insertion-ordered [`Map`])
    Object(Map),
}

impl Value {
    /// Whether this is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    /// Whether this is a boolean.
    pub fn is_bool(&self) -> bool {
        matches!(self, Value::Bool(_))
    }
    /// Whether this is a number.
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_))
    }
    /// Whether this is a string.
    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }
    /// Whether this is an array.
    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }
    /// Whether this is an object.
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    /// The string, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    /// The boolean, if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// The number reference, if this is a number.
    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Value::Number(n) => Some(n),
            _ => None,
        }
    }
    /// Best-effort `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(Number::as_i64)
    }
    /// Best-effort `u64`.
    pub fn as_u64(&self) -> Option<u64> {
        self.as_number().and_then(Number::as_u64)
    }
    /// Exact `i128` conversion when integral and in range.
    pub fn as_i128(&self) -> Option<i128> {
        self.as_number().and_then(Number::as_i128)
    }
    /// Exact `u128` conversion when integral and in range.
    pub fn as_u128(&self) -> Option<u128> {
        self.as_number().and_then(Number::as_u128)
    }
    /// Convert to `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().map(Number::as_f64)
    }
    /// The array reference, if this is an array.
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    /// The mutable array reference, if this is an array.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    /// The object reference, if this is an object.
    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    /// The mutable object reference, if this is an object.
    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// Get a value by key (objects only).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|m| m.get(key))
    }
    /// Get a value by key, mutable (objects only).
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.as_object_mut().and_then(|m| m.get_mut(key))
    }

    /// Locate a value by RFC 6901 JSON Pointer.
    ///
    /// `/a/b/0` descends into `a` -> `b` -> first array element; `~0` / `~1`
    /// escape `~` and `/`. The empty pointer returns `self`.
    pub fn pointer(&self, pointer: &str) -> Option<&Value> {
        if pointer.is_empty() {
            return Some(self);
        }
        if !pointer.starts_with('/') {
            return None;
        }
        let mut current = self;
        for raw in pointer.split('/').skip(1) {
            let token = decode_pointer_token(raw)?;
            current = match current {
                Value::Object(m) => m.get(token.as_ref())?,
                Value::Array(a) => a.get(parse_array_index(token.as_ref())?)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// Consume self into a [`Map`] (if object).
    pub fn into_object(self) -> Option<Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    /// Consume self into a `Vec` (if array).
    pub fn into_array(self) -> Option<Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

/// Decode one RFC 6901 reference token, borrowing tokens that need no escapes.
fn decode_pointer_token(token: &str) -> Option<Cow<'_, str>> {
    let Some(first_escape) = token.as_bytes().iter().position(|&byte| byte == b'~') else {
        return Some(Cow::Borrowed(token));
    };

    let mut decoded = String::with_capacity(token.len());
    decoded.push_str(&token[..first_escape]);
    let mut remainder = &token[first_escape..];
    while let Some(escape) = remainder.strip_prefix('~') {
        match escape.as_bytes().first() {
            Some(b'0') => decoded.push('~'),
            Some(b'1') => decoded.push('/'),
            _ => return None,
        }
        remainder = &escape[1..];
        match remainder.find('~') {
            Some(next_escape) => {
                decoded.push_str(&remainder[..next_escape]);
                remainder = &remainder[next_escape..];
            }
            None => {
                decoded.push_str(remainder);
                break;
            }
        }
    }
    Some(Cow::Owned(decoded))
}

/// RFC 6901 array indexes are either `0` or a non-zero decimal without a sign.
fn parse_array_index(token: &str) -> Option<usize> {
    let bytes = token.as_bytes();
    if bytes.is_empty() || (bytes.len() > 1 && bytes[0] == b'0') {
        return None;
    }
    if !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    token.parse().ok()
}

impl Index<&str> for Value {
    type Output = Value;
    /// Index access; panics on a missing key.
    fn index(&self, key: &str) -> &Value {
        self.get(key).expect("key not found in Value")
    }
}

impl Index<usize> for Value {
    type Output = Value;
    /// Index access; panics on out-of-range.
    fn index(&self, idx: usize) -> &Value {
        match self {
            Value::Array(a) => &a[idx],
            _ => panic!("cannot index non-array value with usize"),
        }
    }
}

impl core::fmt::Display for Value {
    /// Compact JSON text.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Number(n) => write!(f, "{n}"),
            Value::String(s) => write!(f, "{}", escape_for_display(s)),
            Value::Array(a) => {
                write!(f, "[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Object(m) => {
                write!(f, "{{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{}:{v}", escape_for_display(k))?;
                }
                write!(f, "}}")
            }
        }
    }
}

/// Minimal display escaping, always quoted.
fn escape_for_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&alloc::format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

macro_rules! from_number {
    ($($t:ty),* $(,)?) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Self {
                Value::Number(Number::from(v))
            }
        }
    )*};
}

from_number! {
    i8, i16, i32, i64, i128, isize,
    u8, u16, u32, u64, u128, usize,
    f32, f64,
}

impl From<Number> for Value {
    fn from(n: Number) -> Self {
        Value::Number(n)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::String(s.clone())
    }
}

impl From<char> for Value {
    fn from(c: char) -> Self {
        Value::String(c.to_string())
    }
}

impl From<Map> for Value {
    fn from(m: Map) -> Self {
        Value::Object(m)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Array(v)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

impl From<()> for Value {
    fn from(_: ()) -> Self {
        Value::Null
    }
}

impl<'a> From<&'a Value> for Value {
    fn from(v: &'a Value) -> Self {
        v.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn pointer_access() {
        let mut v = Value::Object(Map::from_iter(vec![(
            "a".to_string(),
            Value::Array(vec![Value::Number(10.into()), Value::Number(20.into())]),
        )]));
        assert_eq!(v.pointer("/a/1"), Some(&Value::Number(20.into())));
        assert!(v.get_mut("a").is_some());
        assert!(v.pointer("/x").is_none());
    }

    #[test]
    fn pointer_decodes_escapes_and_rejects_invalid_tokens() {
        let v = Value::Object(Map::from_iter(vec![
            ("a/b".to_string(), Value::Bool(true)),
            ("m~n".to_string(), Value::Bool(false)),
            ("~2".to_string(), Value::Null),
        ]));

        assert_eq!(v.pointer("/a~1b"), Some(&Value::Bool(true)));
        assert_eq!(v.pointer("/m~0n"), Some(&Value::Bool(false)));
        assert!(v.pointer("/~2").is_none());
        assert!(v.pointer("/trailing~").is_none());
        assert!(matches!(
            decode_pointer_token("plain"),
            Some(Cow::Borrowed("plain"))
        ));
    }

    #[test]
    fn pointer_enforces_array_index_grammar() {
        let v = Value::Array(vec![Value::Null, Value::Bool(true)]);

        assert_eq!(v.pointer("/0"), Some(&Value::Null));
        assert_eq!(v.pointer("/1"), Some(&Value::Bool(true)));
        assert!(v.pointer("/01").is_none());
        assert!(v.pointer("/+1").is_none());
        assert!(v.pointer("/-").is_none());
        assert!(v.pointer("/184467440737095516160").is_none());
    }
}

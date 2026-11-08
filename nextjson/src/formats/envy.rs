//! Envy codec: deserialize explicitly supplied configuration key/value pairs.
//!
//! Every supplied variable becomes a key/value entry (values are
//! type-coerced: `"42"` -> integer, `"true"`/`"false"` -> boolean, otherwise
//! string). This mirrors the `envy` crate's ergonomics and is
//! deserialization-only; serialization is not meaningful for an environment.

use alloc::vec::Vec;

use crate::de::NsonDeserialize;
use crate::error::{Error, Result};
use crate::formats::tree;
use crate::formats::Format;
use crate::map::Map;
use crate::ser::NsonSerialize;
use crate::value::Value;

/// Configuration-pair format marker. No process environment is accessed.
#[derive(Clone, Copy, Debug)]
pub struct Envy;

impl Format for Envy {
    const NAME: &'static str = "envy";
    const MIME: &'static str = "text/plain";
    const EXTENSIONS: &'static [&'static str] = &[];
    const BINARY: bool = false;

    fn encode<T: NsonSerialize + ?Sized>(self, _value: &T) -> Result<Vec<u8>> {
        Err(Error::custom(
            "envy: serialization to environment is not supported",
        ))
    }

    /// Decode UTF-8 `KEY=value` lines; blank lines are ignored.
    /// Values are literal text: quotes, substitutions and escapes are not expanded.
    fn decode<'de, T: NsonDeserialize<'de>>(self, input: &'de [u8]) -> Result<T> {
        let text = core::str::from_utf8(input).map_err(|_| Error::custom("envy: invalid UTF-8"))?;
        let mut pairs = Vec::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let pair = line
                .split_once('=')
                .ok_or_else(|| Error::custom("envy: expected KEY=value"))?;
            pairs.push(pair);
        }
        self.from_pairs(pairs)
    }
}

impl Envy {
    /// Decode key/value pairs supplied by the caller, rejecting empty or duplicate keys.
    /// The result owns its strings; it cannot borrow the temporary value tree.
    pub fn from_pairs<'de, T, I, K, V>(self, pairs: I) -> Result<T>
    where
        T: NsonDeserialize<'de>,
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut map = Map::new();
        for (key, value) in pairs {
            let key = key.as_ref();
            if key.is_empty() || map.contains_key(key) {
                return Err(Error::custom("envy: empty or duplicate key"));
            }
            map.insert(
                alloc::string::String::from(key),
                coerce_env_value(value.as_ref()),
            );
        }
        let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&Value::Object(map))?);
        let out = T::nextdecode(&mut decoder)?;
        decoder.end()?;
        Ok(out)
    }
}

/// Coerce an environment string into a typed [`Value`].
fn coerce_env_value(v: &str) -> Value {
    if let Ok(n) = v.parse::<i64>() {
        return Value::from(n);
    }
    if let Ok(n) = v.parse::<u64>() {
        return Value::from(n);
    }
    // Non-finite floats stay strings; the JSON data model has no NaN / inf.
    if let Some(f) = crate::formats::tree::parse_float(v) {
        return Value::from(f);
    }
    match v {
        "true" | "1" => Value::from(true),
        "false" | "0" => Value::from(false),
        _ => Value::from(v),
    }
}

/// Envy decoder serving the unified interface from a parsed [`Value`].
///
/// The environment map is replayed through the shared
/// [`crate::formats::TreeDecoder`].
pub type EnvyDecoder<'de> = tree::TreeDecoder<'de>;

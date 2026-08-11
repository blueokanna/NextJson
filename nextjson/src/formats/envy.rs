//! Envy codec: deserialize structures from process environment variables.
//!
//! Every environment variable becomes a key/value entry (values are
//! type-coerced: `"42"` -> integer, `"true"`/`"false"` -> boolean, otherwise
//! string). This mirrors the `envy` crate's ergonomics and is
//! deserialization-only; serialization is not meaningful for an environment.

use alloc::vec::Vec;

use crate::de::NsonDeserialize;
use crate::error::{Error, Result};
use crate::formats::tree;
use crate::formats::Format;
#[cfg(feature = "std")]
use crate::map::Map;
use crate::ser::NsonSerialize;
use crate::value::Value;

/// Envy format marker (deserialization from `std::env`).
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

    fn decode<'de, T: NsonDeserialize<'de>>(self, _input: &'de [u8]) -> Result<T> {
        #[cfg(feature = "std")]
        {
            let mut map = Map::new();
            for (k, v) in std::env::vars() {
                map.insert(k, coerce_env_value(&v));
            }
            let value = Value::Object(map);
            let mut decoder = tree::TreeDecoder::new(tree::value_to_tokens(&value));
            let out = T::nextdecode(&mut decoder)?;
            decoder.end()?;
            Ok(out)
        }
        #[cfg(not(feature = "std"))]
        {
            let _ = coerce_env_value;
            let _ = tree::value_to_tokens;
            Err(Error::custom("envy: requires the `std` feature"))
        }
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
    if let Ok(f) = v.parse::<f64>() {
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

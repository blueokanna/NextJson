//! Encoder configuration and construction.
//!
//! [`Encoder`] and [`EncodeConfig`] are defined in the serialization module;
//! they are re-exported here to keep `nextjson::encode` a stable entry point.

pub use crate::ser::{EncodeConfig, Encoder};

//! Schema-declared safety-policy validation.
//!
//! This module turns a [`TypeSchema`] (with its declared [`Policy`] nodes)
//! from "what data looks like" into "what data is allowed to enter the
//! system". A decoded [`Value`] is walked against the schema; every node is
//! checked for shape and for the declared limits:
//!
//! - `max_str_len` on string values;
//! - `max_items` on arrays and objects;
//! - `min` / `max` on numbers;
//! - `deny_unknown_fields` on structs and tagged enums;
//! - `max_depth` on containers and as a global [`ValidateConfig`] cap;
//! - `sensitive` fields are reported (never rejected) so logging code can
//!   redact them.
//!
//! Validation is a post-decode gate: it operates on an already-materialized
//! [`Value`] and never touches the hot decode path. Byte-level limits (total
//! message size) stay at the decode / stream boundary, where the payload
//! length is actually known; [`ValidateConfig::message_len`] lets the caller
//! pass that length in so the declared `max_message_size` can still be
//! enforced here.
//!
//! The walk reports every violation it finds (fail-collect, not fail-fast),
//! which is the shape a production validation gate wants: the caller can log
//! all offending paths at once instead of fixing one error at a time.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::map::Map;
use crate::number::Number;
use crate::schema::{NsonSchema, Policy, TypeSchema};
use crate::value::Value;

/// The kind of a single policy or shape violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViolationKind {
    /// String longer than the declared `max_str_len`.
    StringTooLong {
        /// The declared maximum.
        max: u64,
    },
    /// Array or object with more elements / entries than the declared
    /// `max_items`.
    TooManyItems {
        /// The declared maximum.
        max: u64,
    },
    /// Number below the declared inclusive `min`.
    NumberBelowMin {
        /// The declared inclusive lower bound.
        min: i128,
    },
    /// Number above the declared inclusive `max`.
    NumberAboveMax {
        /// The declared inclusive upper bound.
        max: i128,
    },
    /// Unknown field while `deny_unknown_fields` is declared.
    UnknownField(String),
    /// Unknown variant in a tagged or plain enum.
    UnknownVariant(String),
    /// A required tag / field is missing from an enum object.
    MissingField(&'static str),
    /// The value's shape does not match the schema at this node.
    ShapeMismatch {
        /// The expected shape name.
        expected: &'static str,
    },
    /// Nesting exceeded a depth limit (`max_depth`).
    DepthExceeded {
        /// The effective maximum nesting allowed.
        max: u64,
    },
    /// The declared `max_message_size` was exceeded.
    MessageTooLarge {
        /// The declared maximum message size in bytes.
        max: u64,
    },
}

/// A single validation finding at a value path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// RFC 6901 JSON-pointer style path into the value (`a.b`, `a[0]`).
    pub path: String,
    /// What was violated.
    pub kind: ViolationKind,
}

/// The result of a validation walk.
///
/// Validation collects findings instead of stopping at the first error, so a
/// report can list every offending path in one pass. Sensitive-field paths
/// are reported separately from violations: declaring `sensitive` never fails
/// validation, it labels data for redaction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Policy / shape violations found during the walk.
    pub violations: Vec<Violation>,
    /// Paths of values declared `sensitive` that were actually present.
    pub sensitive: Vec<String>,
}

impl Report {
    /// Whether the walk found no violations.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
    /// The paths of sensitive values that were present (for redaction).
    pub fn sensitive_paths(&self) -> &[String] {
        &self.sensitive
    }
    /// Convert into a `Result`, carrying the report as the error value.
    pub fn into_result(self) -> Result<(), Report> {
        if self.is_ok() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

/// Runtime tuning for a validation walk.
///
/// The schema already carries its declared limits; this config supplies the
/// deployment-specific ones: a global nesting cap and the original payload
/// length used to enforce `max_message_size`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidateConfig {
    /// Absolute cap on container nesting from the value root. `None` disables
    /// the caller-supplied cap; the walk still enforces an internal hard
    /// ceiling ([`HARD_DEPTH_CAP`]) so the recursion always terminates.
    pub max_depth: Option<u64>,
    /// Maximum size of the original payload in bytes. Only enforced when
    /// [`message_len`](Self::message_len) is also supplied.
    pub max_message_size: Option<u64>,
    /// Length in bytes of the original payload that produced the value being
    /// validated.
    pub message_len: Option<u64>,
}

/// Hard ceiling on the validation recursion, applied regardless of config.
///
/// Decoded values are bounded by the decoder's own limit (128 by default,
/// configurable), so anything deeper must be a hand-constructed tree. The
/// walk must never recurse without bound on such a tree: this is the
/// termination guarantee. 1024 frames are well within the stack.
pub const HARD_DEPTH_CAP: u64 = 1024;

/// Validate a [`Value`] against a [`TypeSchema`] with default tuning.
pub fn validate(schema: TypeSchema, value: &Value) -> Report {
    validate_with(schema, value, ValidateConfig::default())
}

/// Validate a [`Value`] against a [`TypeSchema`] with explicit tuning.
pub fn validate_with(schema: TypeSchema, value: &Value, config: ValidateConfig) -> Report {
    let mut report = Report::default();
    if let (Some(limit), Some(len)) = (config.max_message_size, config.message_len) {
        if len > limit {
            report.violations.push(Violation {
                path: String::new(),
                kind: ViolationKind::MessageTooLarge { max: limit },
            });
        }
    }
    // The recursion must always terminate: clamp any caller-supplied cap to
    // the hard ceiling, and default to it when none is given.
    let root_limit = match config.max_depth {
        Some(d) => Some(d.min(HARD_DEPTH_CAP)),
        None => Some(HARD_DEPTH_CAP),
    };
    walk(
        &mut report,
        &config,
        schema,
        value,
        0,
        root_limit,
        "",
        &Policy::default(),
    );
    report
}

/// Validate a [`Value`] against the compile-time schema of `T`.
pub fn validate_value<T: NsonSchema>(value: &Value) -> Report {
    validate(T::SCHEMA, value)
}

/// Validate a [`Value`] against the compile-time schema of `T` with explicit
/// tuning.
pub fn validate_value_with<T: NsonSchema>(value: &Value, config: ValidateConfig) -> Report {
    validate_with(T::SCHEMA, value, config)
}

/// The core recursive walk.
///
/// `depth` is the container nesting level of `value` (root is 0). `limit` is
/// the maximum `depth` allowed for this node and its descendants: it starts
/// from [`ValidateConfig::max_depth`] and is narrowed by every container's
/// declared `max_depth` on the path. `policy` is the [`Policy`] of the field
/// that holds this value (or an empty policy at the root).
#[allow(clippy::too_many_arguments)]
fn walk(
    report: &mut Report,
    config: &ValidateConfig,
    schema: TypeSchema,
    value: &Value,
    depth: u64,
    limit: Option<u64>,
    path: &str,
    policy: &Policy,
) {
    if let Some(max) = limit {
        if depth > max {
            report.violations.push(Violation {
                path: path.to_string(),
                kind: ViolationKind::DepthExceeded { max },
            });
            return;
        }
    }
    apply_policy(report, path, value, policy);

    // The remaining nesting allowed below this node: the same absolute limit
    // the parent enforced, narrowed by this container's declared `max_depth`
    // (`m` additional levels below this container at `depth`).
    let child_limit = match container_max_depth(schema) {
        Some(m) => match limit {
            Some(l) => Some(l.min(depth.saturating_add(m))),
            None => Some(depth.saturating_add(m)),
        },
        None => limit,
    };

    match schema {
        TypeSchema::Optional(inner) => {
            if !matches!(value, Value::Null) {
                walk(report, config, *inner, value, depth, limit, path, policy);
            }
        }
        TypeSchema::Opaque => {}
        TypeSchema::Unit => {
            if !matches!(value, Value::Null) {
                shape(report, path, "null");
            }
        }
        TypeSchema::Bool => {
            if !matches!(value, Value::Bool(_)) {
                shape(report, path, "boolean");
            }
        }
        TypeSchema::Char => match value {
            Value::String(s) if s.chars().count() == 1 => {}
            _ => shape(report, path, "char"),
        },
        TypeSchema::Str => {
            if !matches!(value, Value::String(_)) {
                shape(report, path, "string");
            }
        }
        TypeSchema::Bytes => match value {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    match item {
                        Value::Number(n) => {
                            let fits = match (n.as_u64(), n.as_i64()) {
                                (Some(u), _) => u <= 255,
                                (None, Some(i)) => (0..=255).contains(&i),
                                (None, None) => false,
                            };
                            if !fits {
                                shape(report, &format!("{path}[{i}]"), "byte (0..=255)");
                            }
                        }
                        _ => shape(report, &format!("{path}[{i}]"), "byte (0..=255)"),
                    }
                }
            }
            _ => shape(report, path, "byte array"),
        },
        TypeSchema::I8
        | TypeSchema::I16
        | TypeSchema::I32
        | TypeSchema::I64
        | TypeSchema::I128
        | TypeSchema::Isize
        | TypeSchema::U8
        | TypeSchema::U16
        | TypeSchema::U32
        | TypeSchema::U64
        | TypeSchema::U128
        | TypeSchema::Usize
        | TypeSchema::F32
        | TypeSchema::F64 => match value {
            Value::Number(n) => {
                if !number_fits(schema, n) {
                    shape(report, path, schema.name());
                }
            }
            _ => shape(report, path, schema.name()),
        },
        TypeSchema::Seq(inner) => match value {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    let p = format!("{path}[{i}]");
                    walk(
                        report,
                        config,
                        *inner,
                        item,
                        depth + 1,
                        child_limit,
                        &p,
                        &Policy::default(),
                    );
                }
            }
            _ => shape(report, path, "array"),
        },
        TypeSchema::Tuple(items) => match value {
            Value::Array(elems) => {
                if elems.len() != items.len() {
                    shape(report, path, "fixed-size tuple");
                }
                for (i, (&item_schema, item)) in items.iter().zip(elems.iter()).enumerate() {
                    let p = format!("{path}[{i}]");
                    walk(
                        report,
                        config,
                        item_schema,
                        item,
                        depth + 1,
                        child_limit,
                        &p,
                        &Policy::default(),
                    );
                }
            }
            _ => shape(report, path, "tuple"),
        },
        TypeSchema::Map(inner) => match value {
            Value::Object(obj) => {
                for (k, v) in obj.iter() {
                    let p = format!("{path}[{k}]");
                    walk(
                        report,
                        config,
                        *inner,
                        v,
                        depth + 1,
                        child_limit,
                        &p,
                        &Policy::default(),
                    );
                }
            }
            _ => shape(report, path, "object"),
        },
        TypeSchema::Struct(s) => {
            validate_struct(report, config, s, value, depth, child_limit, path)
        }
        TypeSchema::Enum(e) => validate_enum(report, config, e, value, depth, child_limit, path),
    }
}

/// Apply a field's declared policy to the value at this node.
fn apply_policy(report: &mut Report, path: &str, value: &Value, policy: &Policy) {
    if policy.sensitive && !report.sensitive.iter().any(|p| p == path) {
        report.sensitive.push(path.to_string());
    }
    if let Some(max) = policy.max_str_len {
        if let Value::String(s) = value {
            // Count with an early exit; `max + 1` could overflow for
            // `max == u64::MAX`.
            let mut len = 0u64;
            for _ in s.chars() {
                len += 1;
                if len > max {
                    break;
                }
            }
            if len > max {
                report.violations.push(Violation {
                    path: path.to_string(),
                    kind: ViolationKind::StringTooLong { max },
                });
            }
        }
    }
    if let Some(max) = policy.max_items {
        let len = match value {
            Value::Array(a) => Some(a.len() as u64),
            Value::Object(o) => Some(o.len() as u64),
            _ => None,
        };
        if let Some(len) = len {
            if len > max {
                report.violations.push(Violation {
                    path: path.to_string(),
                    kind: ViolationKind::TooManyItems { max },
                });
            }
        }
    }
    if let (Some(min), Some(n)) = (policy.min, value.as_number()) {
        if number_below(n, min) {
            report.violations.push(Violation {
                path: path.to_string(),
                kind: ViolationKind::NumberBelowMin { min },
            });
        }
    }
    if let (Some(max), Some(n)) = (policy.max, value.as_number()) {
        if number_above(n, max) {
            report.violations.push(Violation {
                path: path.to_string(),
                kind: ViolationKind::NumberAboveMax { max },
            });
        }
    }
}

/// Whether `n` is strictly below the inclusive `i128` bound.
fn number_below(n: &Number, min: i128) -> bool {
    if let Some(i) = n.as_i128() {
        return i < min;
    }
    if let Some(u) = n.as_u128() {
        // Only reachable when `u > i128::MAX`; any non-negative bound is met.
        return min > 0 && u < min as u128;
    }
    n.as_f64() < min as f64
}

/// Whether `n` is strictly above the inclusive `i128` bound.
fn number_above(n: &Number, max: i128) -> bool {
    if let Some(i) = n.as_i128() {
        return i > max;
    }
    if let Some(u) = n.as_u128() {
        return max < 0 || u > max as u128;
    }
    n.as_f64() > max as f64
}

/// The declared `max_depth` of a container schema, if any.
fn container_max_depth(schema: TypeSchema) -> Option<u64> {
    match schema {
        TypeSchema::Struct(s) => s.max_depth,
        TypeSchema::Enum(e) => e.max_depth,
        _ => None,
    }
}

/// Whether a `Number` fits the shape of a numeric schema.
///
/// Integral-valued floats are accepted for integer schemas (matching common
/// JSON-decoder behavior); fractional floats are not.
fn number_fits(schema: TypeSchema, n: &Number) -> bool {
    if let Some((min, max)) = crate::schema::integer_range(schema) {
        if let Some(i) = n.as_i128() {
            return i >= min && i <= max;
        }
        if let Some(u) = n.as_u128() {
            // Only reachable when `u > i128::MAX`.
            if schema == TypeSchema::U128 {
                return true;
            }
            return max as u128 >= u;
        }
        let f = n.as_f64();
        return f.is_finite() && f % 1.0 == 0.0 && f >= min as f64 && f <= max as f64;
    }
    match schema {
        TypeSchema::F32 => {
            let f = n.as_f64();
            f.is_finite() && f >= f32::MIN as f64 && f <= f32::MAX as f64
        }
        TypeSchema::F64 => n.is_finite(),
        _ => false,
    }
}

/// Validate an object against a struct schema.
fn validate_struct(
    report: &mut Report,
    config: &ValidateConfig,
    s: &crate::schema::StructSchema,
    value: &Value,
    depth: u64,
    child_limit: Option<u64>,
    path: &str,
) {
    let Value::Object(obj) = value else {
        shape(report, path, "object");
        return;
    };
    let fields = s.fields;
    let mut unknown: Vec<(&str, &Value)> = Vec::new();
    for (k, v) in obj.iter() {
        let Some(f) = fields.iter().find(|f| f.name == k && !f.flattened) else {
            unknown.push((k, v));
            continue;
        };
        let p = join_path(path, k);
        walk(
            report,
            config,
            f.ty,
            v,
            depth + 1,
            child_limit,
            &p,
            &f.policy,
        );
    }
    if unknown.is_empty() {
        return;
    }
    // Unknown keys: route them into a flattened field, or reject them.
    if let Some(flat) = fields.iter().find(|f| f.flattened) {
        let p = join_path(path, flat.name);
        // The flattened field receives `unknown.len()` entries.
        if let Some(max) = flat.policy.max_items {
            if unknown.len() as u64 > max {
                report.violations.push(Violation {
                    path: p.clone(),
                    kind: ViolationKind::TooManyItems { max },
                });
            }
        }
        if flat.policy.sensitive && !report.sensitive.iter().any(|x| x == &p) {
            report.sensitive.push(p.clone());
        }
        match flat.ty {
            TypeSchema::Map(inner) => {
                for (k, v) in &unknown {
                    let pk = format!("{p}[{k}]");
                    walk(
                        report,
                        config,
                        *inner,
                        v,
                        depth + 1,
                        child_limit,
                        &pk,
                        &Policy::default(),
                    );
                }
            }
            TypeSchema::Struct(fs) => {
                let mut rest = Map::new();
                for (k, v) in &unknown {
                    rest.insert((*k).to_string(), (*v).clone());
                }
                let rest = Value::Object(rest);
                validate_struct(report, config, fs, &rest, depth + 1, child_limit, &p);
            }
            _ => {}
        }
    } else if s.deny_unknown_fields {
        for (k, _) in &unknown {
            report.violations.push(Violation {
                path: path.to_string(),
                kind: ViolationKind::UnknownField((*k).to_string()),
            });
        }
    }
}

/// Validate a value against an enum schema.
///
/// Matching follows the same representations the decoder uses: externally
/// tagged (`{"Variant": content}`), internally tagged (`{tag, ...rest}`),
/// adjacently tagged (`{tag, content}`), plain string enums, and untagged
/// (first variant whose validation is clean wins — the same "try in order"
/// rule the decoder applies).
#[allow(clippy::too_many_arguments)]
fn validate_enum(
    report: &mut Report,
    config: &ValidateConfig,
    e: &crate::schema::EnumSchema,
    value: &Value,
    depth: u64,
    child_limit: Option<u64>,
    path: &str,
) {
    let all_unit = e.variants.iter().all(|v| v.ty == TypeSchema::Unit);
    if e.untagged {
        let clean = e.variants.iter().any(|v| {
            let mut sub = Report::default();
            walk(
                &mut sub,
                config,
                v.ty,
                value,
                depth,
                child_limit,
                path,
                &Policy::default(),
            );
            sub.violations.is_empty()
        });
        if !clean {
            shape(report, path, e.name);
        }
        return;
    }
    if let Some(tag) = e.tag {
        let Value::Object(obj) = value else {
            shape(report, path, "object");
            return;
        };
        // Unknown-key rejection applies to the enum object itself.
        if e.deny_unknown_fields {
            let known = |k: &str| k == tag || (e.content.is_some() && k == e.content.unwrap());
            for (k, _) in obj.iter() {
                if !known(k) {
                    report.violations.push(Violation {
                        path: path.to_string(),
                        kind: ViolationKind::UnknownField(k.to_string()),
                    });
                }
            }
        }
        let Some(Value::String(name)) = obj.get(tag) else {
            report.violations.push(Violation {
                path: path.to_string(),
                kind: ViolationKind::MissingField(tag),
            });
            return;
        };
        let Some(variant) = e.variants.iter().find(|v| v.name == name.as_str()) else {
            report.violations.push(Violation {
                path: path.to_string(),
                kind: ViolationKind::UnknownVariant(name.clone()),
            });
            return;
        };
        if let Some(content) = e.content {
            // Adjacently tagged: the content key carries the variant value.
            match obj.get(content) {
                Some(content_value) => {
                    if variant.ty == TypeSchema::Unit {
                        // A unit variant must not carry content (decode errors).
                        if !matches!(content_value, Value::Null) {
                            shape(report, path, "unit variant (no content)");
                        }
                    } else {
                        let p = join_path(path, variant.name);
                        walk(
                            report,
                            config,
                            variant.ty,
                            content_value,
                            depth + 1,
                            child_limit,
                            &p,
                            &variant.policy,
                        );
                    }
                }
                None => {
                    report.violations.push(Violation {
                        path: path.to_string(),
                        kind: ViolationKind::MissingField(content),
                    });
                }
            }
        } else {
            // Internally tagged: validate the remaining keys as the variant.
            if variant.ty == TypeSchema::Unit {
                return; // the decoder ignores extra keys for unit variants.
            }
            let mut rest = Map::new();
            for (k, v) in obj.iter() {
                if k != tag {
                    rest.insert(k.to_string(), v.clone());
                }
            }
            let rest = Value::Object(rest);
            let p = join_path(path, variant.name);
            walk(
                report,
                config,
                variant.ty,
                &rest,
                depth + 1,
                child_limit,
                &p,
                &variant.policy,
            );
        }
        return;
    }
    if all_unit {
        // Plain string enum.
        match value {
            Value::String(s) if e.variants.iter().any(|v| v.name == s.as_str()) => {}
            _ => shape(report, path, e.name),
        }
        return;
    }
    // Externally tagged: exactly one key naming the variant.
    match value {
        Value::Object(obj) if obj.len() == 1 => {
            let (k, v) = obj.iter().next().expect("len == 1");
            match e.variants.iter().find(|var| var.name == k) {
                Some(variant) => {
                    let p = join_path(path, variant.name);
                    walk(
                        report,
                        config,
                        variant.ty,
                        v,
                        depth + 1,
                        child_limit,
                        &p,
                        &variant.policy,
                    );
                }
                None => {
                    report.violations.push(Violation {
                        path: path.to_string(),
                        kind: ViolationKind::UnknownVariant(k.to_string()),
                    });
                }
            }
        }
        _ => shape(report, path, e.name),
    }
}

/// Join a child segment onto a value path, avoiding a leading dot for the
/// root (`""` + `"name"` == `"name"`).
fn join_path(path: &str, segment: &str) -> String {
    if path.is_empty() {
        segment.to_string()
    } else {
        format!("{path}.{segment}")
    }
}

/// Push a shape-mismatch violation at `path`.
fn shape(report: &mut Report, path: &str, expected: &'static str) {
    report.violations.push(Violation {
        path: path.to_string(),
        kind: ViolationKind::ShapeMismatch { expected },
    });
}

/// Helper used by tests and examples: build an object value from pairs.
#[cfg(test)]
fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(Map::from_iter(
        pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{EnumSchema, FieldSchema, StructSchema, VariantSchema};
    use alloc::string::ToString;
    use alloc::vec;

    const EMPTY: Policy = Policy {
        max_str_len: None,
        max_items: None,
        min: None,
        max: None,
        sensitive: false,
    };

    const fn field(name: &'static str, ty: TypeSchema) -> FieldSchema {
        FieldSchema {
            name,
            orig: name,
            required: true,
            flattened: false,
            policy: EMPTY,
            ty,
        }
    }

    const S: TypeSchema = TypeSchema::Struct(&StructSchema {
        name: "S",
        transparent: false,
        max_depth: None,
        deny_unknown_fields: false,
        fields: &[field("name", TypeSchema::Str), field("age", TypeSchema::U8)],
    });

    #[test]
    fn valid_value_passes() {
        let v = obj(&[("name", Value::from("Ada")), ("age", Value::from(36u8))]);
        let r = validate(S, &v);
        assert!(r.is_ok(), "{:?}", r);
    }

    #[test]
    fn shape_mismatch_and_unknown_field() {
        let v = obj(&[("name", Value::from(7u8)), ("age", Value::from(36u8))]);
        let r = validate(S, &v);
        assert_eq!(r.violations.len(), 1);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::ShapeMismatch { expected: "string" }
        ));
        assert_eq!(r.violations[0].path, "name");

        // Unknown field is allowed by default.
        let v = obj(&[
            ("name", Value::from("a")),
            ("age", Value::from(1u8)),
            ("x", Value::Null),
        ]);
        assert!(validate(S, &v).is_ok());
    }

    #[test]
    fn deny_unknown_fields_rejects() {
        const D: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "D",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: true,
            fields: &[field("a", TypeSchema::I32)],
        });
        let v = obj(&[("a", Value::from(1i32)), ("b", Value::Null)]);
        let r = validate(D, &v);
        assert_eq!(r.violations.len(), 1);
        assert_eq!(
            r.violations[0].kind,
            ViolationKind::UnknownField("b".to_string())
        );
    }

    #[test]
    fn numeric_bounds_and_string_length() {
        const P: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "P",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[FieldSchema {
                name: "code",
                orig: "code",
                required: true,
                flattened: false,
                policy: Policy {
                    min: Some(0),
                    max: Some(100),
                    ..EMPTY
                },
                ty: TypeSchema::I32,
            }],
        });
        let ok = obj(&[("code", Value::from(50i32))]);
        assert!(validate(P, &ok).is_ok());
        let low = obj(&[("code", Value::from(-5i32))]);
        let r = validate(P, &low);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::NumberBelowMin { min: 0 }
        ));
        let high = obj(&[("code", Value::from(200i32))]);
        let r = validate(P, &high);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::NumberAboveMax { max: 100 }
        ));

        const T: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "T",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[FieldSchema {
                name: "tag",
                orig: "tag",
                required: true,
                flattened: false,
                policy: Policy {
                    max_str_len: Some(4),
                    ..EMPTY
                },
                ty: TypeSchema::Str,
            }],
        });
        let ok = obj(&[("tag", Value::from("abcd"))]);
        assert!(validate(T, &ok).is_ok());
        let long = obj(&[("tag", Value::from("abcde"))]);
        let r = validate(T, &long);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::StringTooLong { max: 4 }
        ));
    }

    #[test]
    fn max_items_and_depth() {
        const L: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "L",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[FieldSchema {
                name: "list",
                orig: "list",
                required: true,
                flattened: false,
                policy: Policy {
                    max_items: Some(2),
                    ..EMPTY
                },
                ty: TypeSchema::Seq(&TypeSchema::I32),
            }],
        });
        let ok = obj(&[(
            "list",
            Value::Array(vec![Value::from(1i32), Value::from(2i32)]),
        )]);
        assert!(validate(L, &ok).is_ok());
        let long = obj(&[(
            "list",
            Value::Array(vec![
                Value::from(1i32),
                Value::from(2i32),
                Value::from(3i32),
            ]),
        )]);
        let r = validate(L, &long);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::TooManyItems { max: 2 }
        ));

        // Global depth cap.
        let config = ValidateConfig {
            max_depth: Some(1),
            ..ValidateConfig::default()
        };
        let deep = obj(&[(
            "list",
            Value::Array(vec![Value::Array(vec![Value::from(1i32)])]),
        )]);
        let r = validate_with(L, &deep, config);
        assert!(r
            .violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKind::DepthExceeded { max: 1 })));
    }

    #[test]
    fn sensitive_is_reported_not_rejected() {
        const SEC: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "Sec",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[FieldSchema {
                name: "token",
                orig: "token",
                required: true,
                flattened: false,
                policy: Policy {
                    sensitive: true,
                    ..EMPTY
                },
                ty: TypeSchema::Str,
            }],
        });
        let v = obj(&[("token", Value::from("secret"))]);
        let r = validate(SEC, &v);
        assert!(r.is_ok());
        assert_eq!(r.sensitive, vec!["token".to_string()]);
    }

    #[test]
    fn message_size_limit() {
        let config = ValidateConfig {
            max_message_size: Some(10),
            message_len: Some(100),
            ..ValidateConfig::default()
        };
        let r = validate_with(
            S,
            &obj(&[("name", Value::from("a")), ("age", Value::from(1u8))]),
            config,
        );
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::MessageTooLarge { max: 10 }
        ));
    }

    #[test]
    fn enum_string_and_external_tag() {
        const E: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "E",
            tag: None,
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[
                VariantSchema {
                    name: "A",
                    orig: "A",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
                VariantSchema {
                    name: "B",
                    orig: "B",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
            ],
        });
        assert!(validate(E, &Value::from("A")).is_ok());
        let r = validate(E, &Value::from("C"));
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::ShapeMismatch { .. }
        ));
        let r = validate(E, &Value::from(3i32));
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::ShapeMismatch { .. }
        ));

        const N: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "N",
            tag: None,
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[VariantSchema {
                name: "Num",
                orig: "Num",
                policy: EMPTY,
                ty: TypeSchema::Struct(&StructSchema {
                    name: "Num",
                    transparent: false,
                    max_depth: None,
                    deny_unknown_fields: false,
                    fields: &[field("x", TypeSchema::I32)],
                }),
            }],
        });
        let ok = obj(&[("Num", obj(&[("x", Value::from(5i32))]))]);
        assert!(validate(N, &ok).is_ok());
        let bad = obj(&[("Num", obj(&[("x", Value::from("no"))]))]);
        let r = validate(N, &bad);
        assert_eq!(r.violations.len(), 1);
        assert_eq!(r.violations[0].path, "Num.x");
    }

    #[test]
    fn internal_tag_matching() {
        const T: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "T",
            tag: Some("type"),
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[
                VariantSchema {
                    name: "Point",
                    orig: "Point",
                    policy: EMPTY,
                    ty: TypeSchema::Struct(&StructSchema {
                        name: "Point",
                        transparent: false,
                        max_depth: None,
                        deny_unknown_fields: false,
                        fields: &[field("x", TypeSchema::I32)],
                    }),
                },
                VariantSchema {
                    name: "Nil",
                    orig: "Nil",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
            ],
        });
        let ok = obj(&[("type", Value::from("Point")), ("x", Value::from(1i32))]);
        assert!(validate(T, &ok).is_ok());
        let ok_unit = obj(&[("type", Value::from("Nil")), ("extra", Value::Null)]);
        assert!(validate(T, &ok_unit).is_ok());
        let missing = obj(&[("x", Value::from(1i32))]);
        let r = validate(T, &missing);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::MissingField("type")
        ));
        let bad = obj(&[("type", Value::from("Nope"))]);
        let r = validate(T, &bad);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::UnknownVariant(_)
        ));
    }

    #[test]
    fn tuple_and_optional() {
        const TUP: TypeSchema = TypeSchema::Tuple(&[TypeSchema::I32, TypeSchema::Str]);
        const OPT: TypeSchema = TypeSchema::Optional(&TypeSchema::I32);

        let ok = Value::Array(vec![Value::from(1i32), Value::from("x")]);
        assert!(validate(TUP, &ok).is_ok());
        let bad = Value::Array(vec![Value::from(1i32), Value::from(2i32)]);
        let r = validate(TUP, &bad);
        assert!(matches!(
            r.violations[0].kind,
            ViolationKind::ShapeMismatch { expected: "string" }
        ));

        assert!(validate(OPT, &Value::Null).is_ok());
        assert!(validate(OPT, &Value::from(3i32)).is_ok());
        let r = validate(OPT, &Value::from("x"));
        assert!(!r.is_ok());
    }

    #[test]
    fn big_unsigned_bounds() {
        // u128 values beyond i128::MAX must not false-positive on bounds.
        assert!(!number_below(&Number::U128(u128::MAX), i128::MAX));
        assert!(number_above(&Number::U128(u128::MAX), i128::MAX));
        assert!(number_fits(TypeSchema::U128, &Number::U128(u128::MAX)));
        assert!(!number_fits(TypeSchema::U64, &Number::U128(u128::MAX)));
        assert!(!number_fits(TypeSchema::I32, &Number::U128(u128::MAX)));
        // Integral floats fit integer schemas.
        assert!(number_fits(TypeSchema::I32, &Number::F64(3.0)));
        assert!(!number_fits(TypeSchema::I32, &Number::F64(3.5)));
    }
}

//! Version-compatibility checking between two schemas.
//!
//! The same-derived-schema contract lets protocol evolution be checked as a
//! pure function of two [`TypeSchema`] values: given the schema a previous
//! release shipped and the schema the next release ships, [`check`] reports
//! every change that can break an old reader consuming new data (forward
//! compatibility) or a new reader consuming old data (backward
//! compatibility), plus lower-severity risks and semantic notes.
//!
//! Directional meaning (matching how decoders behave):
//!
//! - **forward** — an old reader decodes data produced by the new schema;
//! - **backward** — a new reader decodes data produced by the old schema.
//!
//! Reported classes:
//!
//! - `Critical` — a guaranteed (or near-guaranteed) decode break in one or
//!   both directions: added required field, removed required field, renamed
//!   field or variant, type-family change, added/removed enum variant, tag
//!   representation change, requiredness narrowing.
//! - `Warning` — no guaranteed break, but data-loss or edge-case risk:
//!   narrowed integer range, integer-to-float, added optionality.
//! - `Note` — behavior-compatible but semantically different: default value
//!   changed, safety policy changed, optionality widened.
//!
//! This is a *static* report: it cannot know the actual values in the wild.
//! A `Warning` (e.g. `i32` → `u8`) is safe only if the real data never
//! exceeds the new range.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::{NsonSchema, TypeSchema};

/// Severity of a compatibility issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Behavior-compatible but semantically different.
    Note,
    /// Possible data loss or edge-case failure.
    Warning,
    /// Decode break in at least one direction.
    Critical,
}

/// The class of a compatibility issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompatKind {
    /// A field was added. `required` says whether it is required in the new
    /// schema (a required addition breaks backward compatibility).
    FieldAdded {
        /// Whether the added field is required in the new schema.
        required: bool,
    },
    /// A field was removed. `was_required` says whether it was required in
    /// the old schema (a required removal breaks forward compatibility).
    FieldRemoved {
        /// Whether the removed field was required in the old schema.
        was_required: bool,
    },
    /// A field changed its serialized name (same original Rust name).
    FieldRenamed {
        /// Old serialized name.
        old: &'static str,
        /// New serialized name.
        new: &'static str,
    },
    /// The type family changed (string → number, struct → seq, ...).
    TypeChanged {
        /// Old type name.
        old: &'static str,
        /// New type name.
        new: &'static str,
    },
    /// The inclusive integer range was narrowed (old values may not fit).
    RangeNarrowed {
        /// Old inclusive range `(min, max)`.
        old: (i128, i128),
        /// New inclusive range `(min, max)`.
        new: (i128, i128),
    },
    /// An integer became a float (fractional new values may break old
    /// readers).
    IntToFloat,
    /// A float became an integer (fractional old values may break new
    /// readers).
    FloatToInt,
    /// An enum variant was added (old readers may see an unknown variant).
    VariantAdded {
        /// The added variant's serialized name.
        name: &'static str,
    },
    /// An enum variant was removed (new readers may see an unknown variant).
    VariantRemoved {
        /// The removed variant's serialized name.
        name: &'static str,
    },
    /// An enum variant changed its serialized name.
    VariantRenamed {
        /// Old serialized variant name.
        old: &'static str,
        /// New serialized variant name.
        new: &'static str,
    },
    /// The enum tag / content / untagged representation changed.
    TagChanged {
        /// Old tag name.
        old: Option<&'static str>,
        /// New tag name.
        new: Option<&'static str>,
    },
    /// The overall value shape changed (struct → enum, tuple arity, ...).
    ShapeChanged {
        /// Old shape name.
        old: &'static str,
        /// New shape name.
        new: &'static str,
    },
    /// A required field became optional (new data may contain `null`).
    OptionalAdded,
    /// An optional field became required (old data may lack it).
    OptionalRemoved,
    /// A field's default value changed.
    DefaultChanged {
        /// The field whose default changed.
        field: &'static str,
    },
    /// A field's safety policy changed (never wire-breaking).
    PolicyChanged {
        /// The field whose policy changed.
        field: &'static str,
    },
}

/// A single compatibility finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatIssue {
    /// How severe the change is.
    pub severity: Severity,
    /// Dotted path to the changed node (e.g. `user.address.city`).
    pub path: String,
    /// The class of change.
    pub kind: CompatKind,
    /// Human-readable explanation.
    pub message: String,
}

/// The full result of a schema diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatReport {
    /// Whether an old reader can consume data produced by the new schema.
    pub forward_compatible: bool,
    /// Whether a new reader can consume data produced by the old schema.
    pub backward_compatible: bool,
    /// Every detected change.
    pub issues: Vec<CompatIssue>,
}

impl CompatReport {
    /// No `Critical` issues (warnings and notes are tolerated).
    pub fn is_compatible(&self) -> bool {
        !self.issues.iter().any(|i| i.severity == Severity::Critical)
    }
    /// The most severe issue, if any.
    pub fn worst_severity(&self) -> Option<Severity> {
        self.issues.iter().map(|i| i.severity).max()
    }
}

/// Diff two schemas and report every change.
///
/// `old` is the schema a previous release shipped; `new` is the schema the
/// next release ships. The report is a pure function of the two schemas; it
/// cannot know the actual data in the field.
pub fn check(old: TypeSchema, new: TypeSchema) -> CompatReport {
    let mut report = CompatReport {
        forward_compatible: true,
        backward_compatible: true,
        issues: Vec::new(),
    };
    diff(&mut report, "", old, new);
    for issue in &report.issues {
        let (breaks_forward, breaks_backward) = direction(&issue.kind);
        if breaks_forward {
            report.forward_compatible = false;
        }
        if breaks_backward {
            report.backward_compatible = false;
        }
    }
    report
}

/// Diff the compile-time schemas of two types.
pub fn check_between<O: NsonSchema, N: NsonSchema>() -> CompatReport {
    check(O::SCHEMA, N::SCHEMA)
}

/// Which directions a kind can break (`(forward, backward)`).
fn direction(kind: &CompatKind) -> (bool, bool) {
    match kind {
        CompatKind::FieldAdded { required: true } => (false, true),
        CompatKind::FieldAdded { required: false } => (false, false),
        CompatKind::FieldRemoved { was_required: true } => (true, false),
        CompatKind::FieldRemoved {
            was_required: false,
        } => (false, false),
        CompatKind::FieldRenamed { .. } => (true, true),
        CompatKind::TypeChanged { .. } => (true, true),
        CompatKind::RangeNarrowed { .. } => (false, true),
        CompatKind::IntToFloat => (true, false),
        CompatKind::FloatToInt => (false, true),
        CompatKind::VariantAdded { .. } => (true, false),
        CompatKind::VariantRemoved { .. } => (false, true),
        CompatKind::VariantRenamed { .. } => (true, true),
        CompatKind::TagChanged { .. } => (true, true),
        CompatKind::ShapeChanged { .. } => (true, true),
        CompatKind::OptionalAdded => (true, false),
        CompatKind::OptionalRemoved => (false, true),
        CompatKind::DefaultChanged { .. } => (false, false),
        CompatKind::PolicyChanged { .. } => (false, false),
    }
}

fn push(report: &mut CompatReport, severity: Severity, path: &str, kind: CompatKind) {
    let message = message(&kind, path);
    report.issues.push(CompatIssue {
        severity,
        path: path.to_string(),
        kind,
        message,
    });
}

fn message(kind: &CompatKind, path: &str) -> String {
    match kind {
        CompatKind::FieldAdded { required: true } => {
            format!("field `{path}` was added as required: new readers cannot read old data")
        }
        CompatKind::FieldAdded { required: false } => {
            format!("field `{path}` was added (optional): compatible")
        }
        CompatKind::FieldRemoved { was_required: true } => {
            format!("required field `{path}` was removed: old readers cannot read new data")
        }
        CompatKind::FieldRemoved {
            was_required: false,
        } => {
            format!("optional field `{path}` was removed: compatible")
        }
        CompatKind::FieldRenamed { old, new } => {
            format!("field `{path}` was renamed from `{old}` to `{new}` on the wire")
        }
        CompatKind::TypeChanged { old, new } => {
            format!("field `{path}` changed type from `{old}` to `{new}`")
        }
        CompatKind::RangeNarrowed { old, new } => {
            format!(
                "field `{path}` integer range narrowed from {old:?} to {new:?}: \
                 values outside the new range cannot be represented"
            )
        }
        CompatKind::IntToFloat => {
            format!("field `{path}` changed from an integer to a float: fractional new values may not decode with old readers")
        }
        CompatKind::FloatToInt => {
            format!("field `{path}` changed from a float to an integer: fractional old values may not decode with new readers")
        }
        CompatKind::VariantAdded { name } => {
            format!("enum variant `{name}` was added: old readers may see an unknown variant")
        }
        CompatKind::VariantRemoved { name } => {
            format!("enum variant `{name}` was removed: new readers may see an unknown variant")
        }
        CompatKind::VariantRenamed { old, new } => {
            format!("enum variant `{old}` was renamed to `{new}` on the wire")
        }
        CompatKind::TagChanged { old, new } => {
            format!("enum tag representation changed from `{old:?}` to `{new:?}`")
        }
        CompatKind::ShapeChanged { old, new } => {
            format!("`{path}` shape changed from `{old}` to `{new}`")
        }
        CompatKind::OptionalAdded => {
            format!(
                "field `{path}` became optional: new data may contain null and fail old readers"
            )
        }
        CompatKind::OptionalRemoved => {
            format!("field `{path}` became required: old data may lack it and fail new readers")
        }
        CompatKind::DefaultChanged { field } => {
            format!("default value of field `{field}` changed (behavior-compatible, semantically different)")
        }
        CompatKind::PolicyChanged { field } => {
            format!("safety policy of `{field}` changed (not wire-breaking)")
        }
    }
}

fn diff(report: &mut CompatReport, path: &str, old: TypeSchema, new: TypeSchema) {
    use TypeSchema::*;
    if old == new {
        return;
    }
    match (old, new) {
        (Optional(oi), Optional(ni)) => diff(report, path, *oi, *ni),
        (Optional(oi), ni) => {
            push(
                report,
                Severity::Critical,
                path,
                CompatKind::OptionalRemoved,
            );
            diff(report, path, *oi, ni);
        }
        (oi, Optional(ni)) => {
            push(report, Severity::Warning, path, CompatKind::OptionalAdded);
            diff(report, path, oi, *ni);
        }
        (Seq(oi), Seq(ni)) => diff(report, path, *oi, *ni),
        (Map(oi), Map(ni)) => diff(report, path, *oi, *ni),
        (Tuple(oi), Tuple(ni)) => {
            if oi.len() != ni.len() {
                push(
                    report,
                    Severity::Critical,
                    path,
                    CompatKind::ShapeChanged {
                        old: "tuple",
                        new: "tuple",
                    },
                );
            }
            for (idx, (o, n)) in oi.iter().zip(ni.iter()).enumerate() {
                diff(report, &format!("{path}[{idx}]"), *o, *n);
            }
        }
        (Struct(os), Struct(ns)) => diff_struct(report, path, os, ns),
        (Enum(oe), Enum(ne)) => diff_enum(report, path, oe, ne),
        (a, b) => {
            if let (Some(ra), Some(rb)) = (
                crate::schema::integer_range(a),
                crate::schema::integer_range(b),
            ) {
                // Same range (e.g. `isize` vs `i64` on 64-bit): wire-compatible.
                if ra == rb {
                    return;
                }
                if rb.0 <= ra.0 && rb.1 >= ra.1 {
                    // New range contains the old range: every old value fits.
                    return;
                }
                push(
                    report,
                    Severity::Warning,
                    path,
                    CompatKind::RangeNarrowed { old: ra, new: rb },
                );
            } else if crate::schema::integer_range(a).is_some() && crate::schema::is_float_type(b) {
                push(report, Severity::Warning, path, CompatKind::IntToFloat);
            } else if crate::schema::is_float_type(a) && crate::schema::integer_range(b).is_some() {
                push(report, Severity::Critical, path, CompatKind::FloatToInt);
            } else {
                push(
                    report,
                    Severity::Critical,
                    path,
                    CompatKind::TypeChanged {
                        old: a.name(),
                        new: b.name(),
                    },
                );
            }
        }
    }
}

fn diff_struct(
    report: &mut CompatReport,
    path: &str,
    os: &crate::schema::StructSchema,
    ns: &crate::schema::StructSchema,
) {
    if os.transparent != ns.transparent {
        push(
            report,
            Severity::Critical,
            path,
            CompatKind::ShapeChanged {
                old: "struct",
                new: "struct",
            },
        );
    }
    if os.max_depth != ns.max_depth || os.deny_unknown_fields != ns.deny_unknown_fields {
        push(
            report,
            Severity::Note,
            path,
            CompatKind::PolicyChanged { field: os.name },
        );
    }
    for of in os.fields {
        let p = format!("{path}.{}", of.name);
        match ns.fields.iter().find(|nf| nf.name == of.name) {
            Some(nf) => {
                diff(report, &p, of.ty, nf.ty);
                if of.policy != nf.policy {
                    push(
                        report,
                        Severity::Note,
                        &p,
                        CompatKind::PolicyChanged { field: of.name },
                    );
                }
                match (of.required, nf.required) {
                    (false, true) => {
                        push(report, Severity::Critical, &p, CompatKind::OptionalRemoved);
                    }
                    (true, false) => {
                        push(report, Severity::Note, &p, CompatKind::OptionalAdded);
                    }
                    _ => {}
                }
            }
            None => {
                if let Some(nf) = ns.fields.iter().find(|nf| nf.orig == of.orig) {
                    push(
                        report,
                        Severity::Critical,
                        &p,
                        CompatKind::FieldRenamed {
                            old: of.name,
                            new: nf.name,
                        },
                    );
                } else {
                    let severity = if of.required {
                        Severity::Critical
                    } else {
                        Severity::Note
                    };
                    push(
                        report,
                        severity,
                        &p,
                        CompatKind::FieldRemoved {
                            was_required: of.required,
                        },
                    );
                }
            }
        }
    }
    for nf in ns.fields {
        if !os
            .fields
            .iter()
            .any(|of| of.name == nf.name || of.orig == nf.orig)
        {
            push(
                report,
                if nf.required {
                    Severity::Critical
                } else {
                    Severity::Note
                },
                &format!("{path}.{}", nf.name),
                CompatKind::FieldAdded {
                    required: nf.required,
                },
            );
        }
    }
}

fn diff_enum(
    report: &mut CompatReport,
    path: &str,
    oe: &crate::schema::EnumSchema,
    ne: &crate::schema::EnumSchema,
) {
    if oe.tag != ne.tag || oe.content != ne.content || oe.untagged != ne.untagged {
        push(
            report,
            Severity::Critical,
            path,
            CompatKind::TagChanged {
                old: oe.tag,
                new: ne.tag,
            },
        );
    }
    if oe.max_depth != ne.max_depth || oe.deny_unknown_fields != ne.deny_unknown_fields {
        push(
            report,
            Severity::Note,
            path,
            CompatKind::PolicyChanged { field: oe.name },
        );
    }
    for ov in oe.variants {
        match ne.variants.iter().find(|nv| nv.name == ov.name) {
            Some(nv) => {
                diff(report, &format!("{path}.{}", ov.name), ov.ty, nv.ty);
                if ov.policy != nv.policy {
                    push(
                        report,
                        Severity::Note,
                        &format!("{path}.{}", ov.name),
                        CompatKind::PolicyChanged { field: ov.name },
                    );
                }
            }
            None => {
                if let Some(nv) = ne.variants.iter().find(|nv| nv.orig == ov.orig) {
                    push(
                        report,
                        Severity::Critical,
                        path,
                        CompatKind::VariantRenamed {
                            old: ov.name,
                            new: nv.name,
                        },
                    );
                } else {
                    push(
                        report,
                        Severity::Critical,
                        path,
                        CompatKind::VariantRemoved { name: ov.name },
                    );
                }
            }
        }
    }
    for nv in ne.variants {
        if !oe
            .variants
            .iter()
            .any(|ov| ov.name == nv.name || ov.orig == nv.orig)
        {
            push(
                report,
                Severity::Critical,
                path,
                CompatKind::VariantAdded { name: nv.name },
            );
        }
    }
}

/// Helper for tests: a required field with no policy.
#[cfg(test)]
const fn field(name: &'static str, ty: TypeSchema) -> crate::schema::FieldSchema {
    crate::schema::FieldSchema {
        name,
        orig: name,
        required: true,
        flattened: false,
        policy: crate::schema::Policy {
            max_str_len: None,
            max_items: None,
            min: None,
            max: None,
            sensitive: false,
        },
        ty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{EnumSchema, FieldSchema, StructSchema, VariantSchema};

    const EMPTY: crate::schema::Policy = crate::schema::Policy {
        max_str_len: None,
        max_items: None,
        min: None,
        max: None,
        sensitive: false,
    };

    #[test]
    fn identical_schemas_are_compatible() {
        const A: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "S",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[field("x", TypeSchema::I32)],
        });
        let r = check(A, A);
        assert!(r.is_compatible());
        assert!(r.forward_compatible && r.backward_compatible);
        assert!(r.issues.is_empty());
    }

    #[test]
    fn added_required_field_breaks_backward_only() {
        const OLD: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "S",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[field("x", TypeSchema::I32)],
        });
        const NEW: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "S",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[field("x", TypeSchema::I32), field("y", TypeSchema::I32)],
        });
        let r = check(OLD, NEW);
        assert!(!r.backward_compatible);
        assert!(r.forward_compatible);
        assert_eq!(r.worst_severity(), Some(Severity::Critical));
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::FieldAdded { required: true })));
    }

    #[test]
    fn removed_required_field_breaks_forward_only() {
        const OLD: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "S",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[field("x", TypeSchema::I32)],
        });
        const NEW: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "S",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[],
        });
        let r = check(OLD, NEW);
        assert!(!r.forward_compatible);
        assert!(r.backward_compatible);
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::FieldRemoved { was_required: true })));
    }

    #[test]
    fn widened_range_is_compatible_narrowed_is_warning() {
        // i8 -> i16: every old value fits.
        let r = check(TypeSchema::I8, TypeSchema::I16);
        assert!(r.is_compatible());
        assert!(r.issues.is_empty());

        // i16 -> i8: values outside [-128,127] are lost.
        let r = check(TypeSchema::I16, TypeSchema::I8);
        assert_eq!(r.worst_severity(), Some(Severity::Warning));
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::RangeNarrowed { .. })));

        // isize -> i64 on a 64-bit target: same range, wire-compatible.
        let r = check(TypeSchema::Isize, TypeSchema::I64);
        assert!(r.is_compatible());
    }

    #[test]
    fn int_to_float_and_back() {
        assert_eq!(
            check(TypeSchema::I32, TypeSchema::F64).worst_severity(),
            Some(Severity::Warning)
        );
        assert_eq!(
            check(TypeSchema::F64, TypeSchema::I32).worst_severity(),
            Some(Severity::Critical)
        );
    }

    #[test]
    fn renamed_field_breaks_both() {
        const OLD: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "S",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[field("user_id", TypeSchema::U64)],
        });
        const NEW: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "S",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[FieldSchema {
                name: "userId",
                orig: "user_id",
                required: true,
                flattened: false,
                policy: crate::schema::Policy {
                    max_str_len: None,
                    max_items: None,
                    min: None,
                    max: None,
                    sensitive: false,
                },
                ty: TypeSchema::U64,
            }],
        });
        let r = check(OLD, NEW);
        assert!(!r.forward_compatible);
        assert!(!r.backward_compatible);
        assert!(r.issues.iter().any(|i| matches!(
            i.kind,
            CompatKind::FieldRenamed {
                old: "user_id",
                new: "userId"
            }
        )));
    }

    #[test]
    fn enum_variant_added_and_removed() {
        const E_ONE: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "E",
            tag: None,
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[
                VariantSchema {
                    name: "One",
                    orig: "One",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
                VariantSchema {
                    name: "Two",
                    orig: "Two",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
            ],
        });
        const E_TWO: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "E",
            tag: None,
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[
                VariantSchema {
                    name: "One",
                    orig: "One",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
                VariantSchema {
                    name: "Two",
                    orig: "Two",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
                VariantSchema {
                    name: "Three",
                    orig: "Three",
                    policy: EMPTY,
                    ty: TypeSchema::Unit,
                },
            ],
        });
        const E_THREE: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "E",
            tag: None,
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[VariantSchema {
                name: "One",
                orig: "One",
                policy: EMPTY,
                ty: TypeSchema::Unit,
            }],
        });

        let added = check(E_ONE, E_TWO);
        assert!(!added.forward_compatible);
        assert!(added.backward_compatible);
        assert!(added
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::VariantAdded { name: "Three" })));

        let removed = check(E_ONE, E_THREE);
        assert!(removed.forward_compatible);
        assert!(!removed.backward_compatible);
        assert!(removed
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::VariantRemoved { name: "Two" })));
    }

    #[test]
    fn optionality_changes() {
        // Option<i32> -> i32: old data may be null.
        let r = check(TypeSchema::Optional(&TypeSchema::I32), TypeSchema::I32);
        assert!(!r.backward_compatible);
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::OptionalRemoved)));

        // i32 -> Option<i32>: new data may be null.
        let r = check(TypeSchema::I32, TypeSchema::Optional(&TypeSchema::I32));
        assert!(!r.forward_compatible);
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::OptionalAdded)));
    }

    #[test]
    fn nested_widening_is_compatible() {
        const OLD: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "Outer",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[field(
                "inner",
                TypeSchema::Struct(&StructSchema {
                    name: "Inner",
                    transparent: false,
                    max_depth: None,
                    deny_unknown_fields: false,
                    fields: &[field("a", TypeSchema::I8)],
                }),
            )],
        });
        const NEW: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "Outer",
            transparent: false,
            max_depth: None,
            deny_unknown_fields: false,
            fields: &[field(
                "inner",
                TypeSchema::Struct(&StructSchema {
                    name: "Inner",
                    transparent: false,
                    max_depth: None,
                    deny_unknown_fields: false,
                    fields: &[field("a", TypeSchema::I32)],
                }),
            )],
        });
        let r = check(OLD, NEW);
        // i8 -> i32 is a widening: fully compatible, no issues.
        assert!(r.is_compatible());
        assert!(r.issues.is_empty());
    }

    #[test]
    fn tag_change_is_critical() {
        const PLAIN: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "E",
            tag: None,
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[VariantSchema {
                name: "A",
                orig: "A",
                policy: EMPTY,
                ty: TypeSchema::Unit,
            }],
        });
        const TAGGED: TypeSchema = TypeSchema::Enum(&EnumSchema {
            name: "E",
            tag: Some("type"),
            content: None,
            untagged: false,
            max_depth: None,
            deny_unknown_fields: false,
            default_tag: "type",
            variants: &[VariantSchema {
                name: "A",
                orig: "A",
                policy: EMPTY,
                ty: TypeSchema::Unit,
            }],
        });
        let r = check(PLAIN, TAGGED);
        assert_eq!(r.worst_severity(), Some(Severity::Critical));
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::TagChanged { .. })));
    }

    #[test]
    fn shape_change_is_critical() {
        let r = check(TypeSchema::Str, TypeSchema::Seq(&TypeSchema::Str));
        assert_eq!(r.worst_severity(), Some(Severity::Critical));
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i.kind, CompatKind::TypeChanged { .. })));
    }
}

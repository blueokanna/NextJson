//! Integration tests: the data-contract pillars end to end.
//!
//! - Version compatibility: protocol evolution is reported as a pure function
//!   of two derived schemas.
//! - Safety policy: schema-declared limits are enforced on decoded values.
//! - Contract inspection: derived types expose their full policy tree.

use nextjson::{
    check, check_between, CompatKind, NsonDeserialize, NsonSchema, NsonSerialize, Severity,
};

#[derive(NsonSerialize, NsonDeserialize)]
struct V1 {
    id: u64,
    name: String,
}

#[derive(NsonSerialize, NsonDeserialize)]
struct V2AddedRequired {
    id: u64,
    name: String,
    email: String, // newly required -> breaks backward compatibility
}

#[derive(NsonSerialize, NsonDeserialize)]
struct V2AddedOptional {
    id: u64,
    name: String,
    #[njson(default)]
    email: Option<String>,
}

#[derive(NsonSerialize, NsonDeserialize)]
struct V2Renamed {
    id: u64,
    #[njson(rename = "fullName")]
    name: String,
}

#[test]
fn check_between_reports_added_required_field() {
    let r = check_between::<V1, V2AddedRequired>();
    assert!(!r.backward_compatible);
    assert!(r.forward_compatible);
    assert_eq!(r.worst_severity(), Some(Severity::Critical));
    assert!(r
        .issues
        .iter()
        .any(|i| matches!(i.kind, CompatKind::FieldAdded { required: true })));
}

#[test]
fn added_optional_field_is_compatible() {
    let r = check_between::<V1, V2AddedOptional>();
    assert!(r.is_compatible(), "{:?}", r);
    assert!(r.forward_compatible && r.backward_compatible);
}

#[test]
fn rename_breaks_both_directions() {
    let r = check_between::<V1, V2Renamed>();
    assert!(!r.forward_compatible);
    assert!(!r.backward_compatible);
    assert!(r.issues.iter().any(|i| matches!(
        i.kind,
        CompatKind::FieldRenamed {
            old: "name",
            new: "fullName"
        }
    )));
}

#[test]
fn identical_derived_types_are_compatible() {
    let r = check_between::<V1, V1>();
    assert!(r.is_compatible());
    assert!(r.issues.is_empty());
}

#[test]
fn widening_is_compatible() {
    #[derive(NsonSerialize, NsonDeserialize)]
    struct Small {
        x: i8,
    }
    #[derive(NsonSerialize, NsonDeserialize)]
    struct Wide {
        x: i32,
    }
    let r = check_between::<Small, Wide>();
    assert!(r.is_compatible(), "{:?}", r);
}

#[test]
fn check_takes_schemas_directly() {
    let r = check(
        <V1 as NsonSchema>::SCHEMA,
        <V2AddedRequired as NsonSchema>::SCHEMA,
    );
    assert!(!r.backward_compatible);
}

/// The safety-policy pillar: limits declared on a struct are part of its
/// contract and enforced by the validator.
#[test]
fn policy_is_part_of_the_contract() {
    #[derive(NsonSerialize, NsonDeserialize)]
    #[njson(deny_unknown_fields, max_depth = 3)]
    struct Envelope {
        #[njson(max_str_len = 16)]
        label: String,
        #[njson(min = 0, max = 999)]
        code: i32,
    }

    let schema = Envelope::SCHEMA;
    let nextjson::TypeSchema::Struct(s) = schema else {
        panic!("expected struct schema");
    };
    assert!(s.deny_unknown_fields);
    assert_eq!(s.max_depth, Some(3));
    assert_eq!(s.fields[0].policy.max_str_len, Some(16));
    assert_eq!(s.fields[1].policy.min, Some(0));
    assert_eq!(s.fields[1].policy.max, Some(999));

    let ok = nextjson::json!({ "label": "short", "code": 42 });
    assert!(nextjson::validate_value::<Envelope>(&ok).is_ok());

    let too_long = nextjson::json!({ "label": "0123456789abcdefg", "code": 42 });
    let r = nextjson::validate_value::<Envelope>(&too_long);
    assert!(r
        .violations
        .iter()
        .any(|v| matches!(v.kind, nextjson::ViolationKind::StringTooLong { .. })));

    let out_of_range = nextjson::json!({ "label": "x", "code": 1000 });
    let r = nextjson::validate_value::<Envelope>(&out_of_range);
    assert!(r
        .violations
        .iter()
        .any(|v| matches!(v.kind, nextjson::ViolationKind::NumberAboveMax { max: 999 })));

    let unknown = nextjson::json!({ "label": "x", "code": 1, "extra": true });
    let r = nextjson::validate_value::<Envelope>(&unknown);
    assert!(r
        .violations
        .iter()
        .any(|v| matches!(v.kind, nextjson::ViolationKind::UnknownField(_))));
}

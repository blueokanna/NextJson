//! Integration tests: schema-declared safety policies flow from derive
//! attributes into `SCHEMA` and are enforced by `nextjson::validate`.

use nextjson::{NsonDeserialize, NsonSchema, NsonSerialize, Policy, TypeSchema};

#[derive(NsonSerialize, NsonDeserialize)]
struct Bounded {
    #[njson(max_str_len = 8)]
    tag: String,
    #[njson(max_items = 3)]
    samples: Vec<i32>,
    #[njson(min = 0, max = 100)]
    level: i32,
    #[njson(sensitive)]
    secret: String,
}

#[derive(NsonSerialize, NsonDeserialize)]
#[njson(max_depth = 2, deny_unknown_fields)]
struct Strict {
    inner: Bounded,
}

#[test]
fn policy_attributes_reach_schema() {
    let TypeSchema::Struct(s) = Bounded::SCHEMA else {
        panic!("expected struct schema");
    };
    assert_eq!(s.fields.len(), 4);

    let tag = &s.fields[0];
    assert_eq!(tag.name, "tag");
    assert_eq!(
        tag.policy,
        Policy {
            max_str_len: Some(8),
            ..Policy::default()
        }
    );

    let samples = &s.fields[1];
    assert_eq!(
        samples.policy,
        Policy {
            max_items: Some(3),
            ..Policy::default()
        }
    );

    let level = &s.fields[2];
    assert_eq!(
        level.policy,
        Policy {
            min: Some(0),
            max: Some(100),
            ..Policy::default()
        }
    );

    let secret = &s.fields[3];
    assert!(secret.policy.sensitive);
}

#[test]
fn container_policy_reaches_schema() {
    let TypeSchema::Struct(s) = Strict::SCHEMA else {
        panic!("expected struct schema");
    };
    assert_eq!(s.max_depth, Some(2));
    assert!(s.deny_unknown_fields);

    // The nested struct keeps its own declared policy.
    let TypeSchema::Struct(inner) = s.fields[0].ty else {
        panic!("expected nested struct");
    };
    assert_eq!(inner.fields[0].policy.max_str_len, Some(8));
}

#[test]
fn validator_enforces_declared_policy() {
    let good = nextjson::json!({
        "tag": "abcdefgh",
        "samples": [1, 2, 3],
        "level": 50,
        "secret": "hunter2",
    });
    let report = nextjson::validate_value::<Bounded>(&good);
    assert!(report.is_ok(), "{:?}", report);
    // Sensitive paths are reported without failing validation.
    assert_eq!(report.sensitive, vec!["secret".to_string()]);

    let bad = nextjson::json!({
        "tag": "abcdefghi",        // 9 chars > 8
        "samples": [1, 2, 3, 4],   // 4 items > 3
        "level": 200,              // > 100
        "secret": "x",
    });
    let report = nextjson::validate_value::<Bounded>(&bad);
    assert!(!report.is_ok());
    let kinds: Vec<&str> = report
        .violations
        .iter()
        .map(|v| match &v.kind {
            nextjson::ViolationKind::StringTooLong { .. } => "StringTooLong",
            nextjson::ViolationKind::TooManyItems { .. } => "TooManyItems",
            nextjson::ViolationKind::NumberAboveMax { .. } => "NumberAboveMax",
            other => panic!("unexpected violation kind: {other:?}"),
        })
        .collect();
    assert!(kinds.contains(&"StringTooLong"), "{kinds:?}");
    assert!(kinds.contains(&"TooManyItems"), "{kinds:?}");
    assert!(kinds.contains(&"NumberAboveMax"), "{kinds:?}");
    // Paths are precise.
    assert!(report.violations.iter().any(
        |v| v.path == "tag" && matches!(&v.kind, nextjson::ViolationKind::StringTooLong { .. })
    ));
}

#[test]
fn deny_unknown_fields_enforced_on_values() {
    let with_unknown = nextjson::json!({
        "inner": { "tag": "a", "samples": [], "level": 1, "secret": "s" },
        "unexpected": 1,
    });
    let report = nextjson::validate_value::<Strict>(&with_unknown);
    assert!(!report.is_ok());
    assert!(report
        .violations
        .iter()
        .any(|v| matches!(&v.kind, nextjson::ViolationKind::UnknownField(_))));
}

#[test]
fn enum_variants_carry_policy() {
    #[derive(NsonSerialize, NsonDeserialize)]
    enum Message {
        #[njson(max_items = 2)]
        Batch(Vec<i32>),
        #[njson(max_str_len = 4)]
        Text(String),
    }

    let ok = nextjson::json!({ "Batch": [1, 2] });
    let report = nextjson::validate_value::<Message>(&ok);
    assert!(report.is_ok(), "{:?}", report);

    let bad = nextjson::json!({ "Batch": [1, 2, 3] });
    let report = nextjson::validate_value::<Message>(&bad);
    assert!(!report.is_ok());
    assert!(report
        .violations
        .iter()
        .any(|v| matches!(&v.kind, nextjson::ViolationKind::TooManyItems { .. })));

    let long = nextjson::json!({ "Text": "abcde" });
    let report = nextjson::validate_value::<Message>(&long);
    assert!(!report.is_ok());
    assert!(report
        .violations
        .iter()
        .any(|v| matches!(&v.kind, nextjson::ViolationKind::StringTooLong { .. })));
}

#[test]
fn global_depth_config_composes_with_schema() {
    // Strict declares max_depth = 2; the global cap must also be honored.
    let deep = nextjson::json!({
        "inner": { "tag": "a", "samples": [[1]], "level": 1, "secret": "s" }
    });
    let report = nextjson::validate_value_with::<Strict>(
        &deep,
        nextjson::ValidateConfig {
            max_depth: Some(1),
            ..nextjson::ValidateConfig::default()
        },
    );
    assert!(!report.is_ok());
    assert!(report
        .violations
        .iter()
        .any(|v| matches!(&v.kind, nextjson::ViolationKind::DepthExceeded { .. })));
}

#[test]
fn message_size_bound() {
    let value = nextjson::json!({ "tag": "a", "samples": [], "level": 1, "secret": "s" });
    let report = nextjson::validate_value_with::<Bounded>(
        &value,
        nextjson::ValidateConfig {
            max_message_size: Some(4),
            message_len: Some(100),
            ..nextjson::ValidateConfig::default()
        },
    );
    assert!(!report.is_ok());
    assert!(report
        .violations
        .iter()
        .any(|v| matches!(&v.kind, nextjson::ViolationKind::MessageTooLarge { max: 4 })));
}

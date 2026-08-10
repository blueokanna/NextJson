#![cfg(feature = "serde")]

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Borrowed<'a> {
    #[serde(borrow)]
    name: &'a str,
    #[serde(borrow)]
    note: Cow<'a, str>,
}

#[test]
fn serde_deserialization_preserves_unescaped_borrows() {
    let input = br#"{"name":"zero-copy","note":"borrowed"}"#;
    let value: Borrowed<'_> = nextjson::serde_compat::from_slice(input).unwrap();

    assert_eq!(value.name, "zero-copy");
    assert!(matches!(value.note, Cow::Borrowed("borrowed")));
    let input_range = input.as_ptr_range();
    assert!(value.name.as_ptr() >= input_range.start && value.name.as_ptr() < input_range.end);
    assert!(value.note.as_ptr() >= input_range.start && value.note.as_ptr() < input_range.end);
}

#[test]
fn escaped_cow_is_owned_and_borrowed_str_is_rejected() {
    #[derive(Debug, Deserialize)]
    struct CowValue<'a> {
        #[serde(borrow)]
        value: Cow<'a, str>,
    }

    #[derive(Debug, Deserialize)]
    struct RefValue<'a> {
        #[serde(borrow)]
        value: &'a str,
    }

    let cow: CowValue<'_> = nextjson::serde_compat::from_str(r#"{"value":"line\nfeed"}"#).unwrap();
    assert!(matches!(cow.value, Cow::Owned(ref value) if value == "line\nfeed"));

    if let Ok(value) = nextjson::serde_compat::from_str::<RefValue<'_>>(r#"{"value":"line\nfeed"}"#)
    {
        panic!("escaped string was incorrectly borrowed: {}", value.value);
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum Event {
    Ready,
    Count(u64),
    Point(i32, i32),
    Message { id: u128, text: String },
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Document {
    title: String,
    enabled: bool,
    events: Vec<Event>,
    by_id: BTreeMap<i32, String>,
}

fn document() -> Document {
    Document {
        title: "interop".into(),
        enabled: true,
        events: vec![
            Event::Ready,
            Event::Count(42),
            Event::Point(-3, 7),
            Event::Message {
                id: u128::MAX,
                text: "hello".into(),
            },
        ],
        by_id: [(-7, "negative".into()), (12, "positive".into())]
            .into_iter()
            .collect(),
    }
}

#[test]
fn serde_data_model_matches_serde_json_for_compound_values() {
    let value = document();
    let ours = nextjson::serde_compat::to_vec(&value).unwrap();
    let reference = serde_json::to_vec(&value).unwrap();

    let ours_semantics: serde_json::Value = serde_json::from_slice(&ours).unwrap();
    let reference_semantics: serde_json::Value = serde_json::from_slice(&reference).unwrap();
    assert_eq!(ours_semantics, reference_semantics);
    assert_eq!(
        nextjson::serde_compat::from_slice::<Document>(&ours).unwrap(),
        value
    );
    assert_eq!(
        nextjson::serde_compat::from_slice::<Document>(&reference).unwrap(),
        value
    );
}

#[test]
fn serde_json_value_round_trips_through_the_adapter() {
    let value = serde_json::json!({
        "null": null,
        "bool": true,
        "integer": 18446744073709551615_u64,
        "float": -12.5,
        "text": "unicode \u{4e2d}",
        "array": [1, 2, 3]
    });
    let encoded = nextjson::serde_compat::to_vec(&value).unwrap();
    let decoded: serde_json::Value = nextjson::serde_compat::from_slice(&encoded).unwrap();
    assert_eq!(decoded, value);
}

#[test]
fn serde_adapter_rejects_invalid_json_boundaries() {
    for input in ["[1,]", r#"{"x":1,}"#, "true false", "1e400"] {
        assert!(nextjson::serde_compat::from_str::<serde_json::Value>(input).is_err());
    }
}

#[test]
fn serde_reader_requires_and_returns_owned_data() {
    let input =
        std::io::Cursor::new(br#"{"title":"reader","enabled":false,"events":[],"by_id":{}}"#);
    let value: Document = nextjson::serde_compat::from_reader(input).unwrap();
    assert_eq!(value.title, "reader");
}

#[test]
fn non_finite_float_serialization_is_an_error() {
    assert!(nextjson::serde_compat::to_string(&f64::NAN).is_err());
    assert!(nextjson::serde_compat::to_string(&f64::INFINITY).is_err());
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
enum TaggedEvent {
    Started { id: u64 },
    Stopped,
}

#[test]
fn serde_tagging_and_pretty_writer_are_supported() {
    let values = vec![TaggedEvent::Started { id: 9 }, TaggedEvent::Stopped];
    let mut output = Vec::new();
    nextjson::serde_compat::to_writer_pretty(&mut output, &values).unwrap();
    let decoded: Vec<TaggedEvent> = nextjson::serde_compat::from_slice(&output).unwrap();
    assert_eq!(decoded, values);
    assert!(output.contains(&b'\n'));
}

struct DeterministicRng(u64);

impl DeterministicRng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
}

fn generated_value(rng: &mut DeterministicRng, depth: u8) -> serde_json::Value {
    let kind = if depth == 0 {
        rng.next() % 5
    } else {
        rng.next() % 7
    };
    match kind {
        0 => serde_json::Value::Null,
        1 => serde_json::Value::Bool(rng.next() & 1 == 0),
        2 => serde_json::Value::from(rng.next() as i64),
        3 => serde_json::Value::from((rng.next() % 1_000_000) as f64 / 17.0 - 20_000.0),
        4 => {
            const TEXT: &[&str] = &[
                "plain",
                "quote\"slash\\",
                "line\nfeed\ttab",
                "unicode-\u{4e2d}-\u{1f4a9}",
                "",
            ];
            serde_json::Value::from(TEXT[(rng.next() as usize) % TEXT.len()])
        }
        5 => {
            let len = (rng.next() % 5) as usize;
            serde_json::Value::Array((0..len).map(|_| generated_value(rng, depth - 1)).collect())
        }
        _ => {
            let len = (rng.next() % 5) as usize;
            let mut object = serde_json::Map::new();
            for index in 0..len {
                object.insert(
                    format!("key-{index}-{}", rng.next() % 11),
                    generated_value(rng, depth - 1),
                );
            }
            serde_json::Value::Object(object)
        }
    }
}

#[test]
fn deterministic_differential_corpus_matches_serde_json() {
    let mut rng = DeterministicRng(0x4e65_7874_4a73_6f6e);
    for case in 0..1_000 {
        let value = generated_value(&mut rng, 4);
        let ours = nextjson::serde_compat::to_vec(&value).unwrap();
        let reference = serde_json::to_vec(&value).unwrap();
        let ours_semantics: serde_json::Value = serde_json::from_slice(&ours).unwrap();
        let reference_semantics: serde_json::Value = serde_json::from_slice(&reference).unwrap();
        assert_eq!(
            ours_semantics,
            reference_semantics,
            "nextencode mismatch in case {case}; ours={}, reference={}",
            String::from_utf8_lossy(&ours),
            String::from_utf8_lossy(&reference)
        );
        let decoded: serde_json::Value = nextjson::serde_compat::from_slice(&reference).unwrap();
        assert_eq!(decoded, value, "nextdecode mismatch in case {case}");
    }
}

#[cfg(feature = "transcode")]
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CrossFormatDocument {
    name: String,
    active: bool,
    counters: Vec<i64>,
    ratio: f64,
    labels: BTreeMap<String, String>,
}

#[cfg(feature = "transcode")]
fn cross_format_document() -> CrossFormatDocument {
    CrossFormatDocument {
        name: "streamed".into(),
        active: true,
        counters: vec![-9, 0, 17, i32::MAX as i64],
        ratio: 0.125,
        labels: [
            ("alpha".into(), "one".into()),
            ("beta".into(), "two".into()),
        ]
        .into_iter()
        .collect(),
    }
}

#[cfg(feature = "transcode")]
#[test]
fn json_streams_to_messagepack_without_an_intermediate_value() {
    let expected = cross_format_document();
    let json = serde_json::to_vec(&expected).unwrap();
    let mut messagepack = Vec::new();
    let mut serializer = rmp_serde::Serializer::new(&mut messagepack).with_struct_map();

    nextjson::serde_compat::transcode::json_to(&json, &mut serializer).unwrap();

    let actual: CrossFormatDocument = rmp_serde::from_slice(&messagepack).unwrap();
    assert_eq!(actual, expected);
}

#[cfg(feature = "transcode")]
#[test]
fn messagepack_streams_to_nextjson_and_uses_nextdecode() {
    let expected = cross_format_document();
    let messagepack = rmp_serde::to_vec_named(&expected).unwrap();
    let mut source = rmp_serde::Deserializer::new(std::io::Cursor::new(messagepack.as_slice()));

    let json = nextjson::serde_compat::transcode::json_from(&mut source).unwrap();
    let actual: CrossFormatDocument = nextjson::serde_compat::nextdecode(&json).unwrap();

    assert_eq!(actual, expected);
}

#[cfg(feature = "transcode")]
#[test]
fn transcode_propagates_source_and_destination_errors() {
    struct RejectWriter;

    impl std::io::Write for RejectWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "intentional target failure",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut target = serde_json::Serializer::new(RejectWriter);
    assert!(nextjson::serde_compat::transcode::json_to(b"[1,2,3]", &mut target).is_err());

    let invalid_messagepack = [0xc1];
    let mut source =
        rmp_serde::Deserializer::new(std::io::Cursor::new(invalid_messagepack.as_slice()));
    assert!(nextjson::serde_compat::transcode::json_from(&mut source).is_err());

    let mut valid_target = serde_json::Serializer::new(Vec::new());
    assert!(nextjson::serde_compat::transcode::json_to(
        b"{\"ok\":true} trailing",
        &mut valid_target
    )
    .is_err());
}

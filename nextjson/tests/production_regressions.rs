//! Regressions for public entry points and adversarial reader boundaries.

use nextjson::{FormatEncoder, NsonDeserialize, NsonSchema, NsonSerialize, TypeSchema, Value};
use std::io::{self, Read};

struct Chunked<'a> {
    input: &'a [u8],
    size: usize,
}

impl Read for Chunked<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let len = self.size.min(out.len());
        self.input.read(&mut out[..len])
    }
}

#[test]
fn stream_whitespace_is_independent_of_read_boundaries() {
    let inputs = [
        " \t\r\n { \"a\" \t: [ 1 , true , null , \"x\" ] , \"b\" : { } } \r\n ",
        " [ ] ",
        " 123 ",
        " \"hello\" ",
    ];
    for input in inputs {
        let expected: Value = nextjson::from_str(input).unwrap();
        for size in 1..=input.len() {
            let actual: Value = nextjson::from_reader(Chunked {
                input: input.as_bytes(),
                size,
            })
            .unwrap_or_else(|err| panic!("chunk {size}, input {input:?}: {err}"));
            assert_eq!(actual, expected);
        }
    }
}

#[derive(Debug, NsonDeserialize)]
struct Known {
    #[allow(dead_code)]
    known: Option<bool>,
}

#[test]
fn ignored_fields_still_require_a_json_value() {
    for input in [r#"{"ignored":}}"#, r#"{"ignored":]}"#, r#"{"ignored":[}]}"#] {
        assert!(
            nextjson::from_str::<Known>(input).is_err(),
            "accepted {input}"
        );
        for size in 1..=input.len() {
            assert!(
                nextjson::from_reader::<_, Known>(Chunked {
                    input: input.as_bytes(),
                    size
                })
                .is_err(),
                "accepted {input} at chunk {size}"
            );
        }
    }
}

#[test]
fn text_formats_validate_ignored_values() {
    use nextjson::formats::*;
    for input in [r#"{"ignored":}}"#, r#"{"ignored":]}"#] {
        assert!(decode_with::<Known, _>(input.as_bytes(), Json5).is_err());
        assert!(decode_with::<Known, _>(input.as_bytes(), Hjson).is_err());
        assert!(decode_with::<Known, _>(input.as_bytes(), Ron).is_err());
    }
}

#[test]
fn stream_rejects_trailing_commas_at_every_boundary() {
    for input in [
        r#"{"known":true,}"#,
        r#"{"known":true,   }"#,
        "[1,]",
        "[1,  ]",
    ] {
        for size in 1..=input.len() {
            assert!(
                nextjson::from_reader::<_, Value>(Chunked {
                    input: input.as_bytes(),
                    size
                })
                .is_err(),
                "accepted {input} at chunk {size}"
            );
        }
    }
}

struct Interrupted<'a> {
    input: &'a [u8],
    interrupt: bool,
}

impl Read for Interrupted<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            Err(io::ErrorKind::Interrupted.into())
        } else {
            let len = out.len().min(1);
            self.input.read(&mut out[..len])
        }
    }
}

#[test]
fn stream_retries_interrupted_reads() {
    let input = br#"{"a": [true, "text"]}"#;
    let expected: Value = nextjson::from_slice(input).unwrap();
    let actual: Value = nextjson::from_reader(Interrupted {
        input,
        interrupt: false,
    })
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn stream_input_limit_covers_whitespace_and_exact_boundaries() {
    for input in ["null", " 42 ", r#"{"x":"hello"}"#] {
        let expected: Value = nextjson::from_str(input).unwrap();
        for size in 1..=input.len() {
            for limit in 0..=input.len() + 1 {
                let actual = nextjson::from_reader_with_limit::<_, Value>(
                    Chunked {
                        input: input.as_bytes(),
                        size,
                    },
                    limit,
                );
                if limit < input.len() {
                    let err = actual.unwrap_err();
                    assert_eq!(err.classification(), "input byte limit exceeded");
                    assert_eq!(err.offset(), limit);
                } else {
                    assert_eq!(actual.unwrap(), expected);
                }
            }
        }
    }
    let mut reader = io::repeat(b' ');
    let err = nextjson::from_reader_with_limit::<_, Value>(&mut reader, 8192).unwrap_err();
    assert_eq!(err.classification(), "input byte limit exceeded");
}

#[test]
fn stream_limit_cannot_be_bypassed_by_restore() {
    use nextjson::FormatDecoder;
    let mut reader = &b"1234"[..];
    let mut decoder = nextjson::StreamDecoder::new(&mut reader).with_input_limit(3);
    let mark = decoder.save();
    assert!(decoder.number().is_err());
    decoder.restore(mark);
    assert!(decoder.number().is_err());
    assert!(decoder.end().is_err());
}

#[test]
fn stream_limit_diagnosis_survives_untagged_variant_retries() {
    let err = nextjson::from_reader_with_limit::<_, Option<Choice>>(&b"1234"[..], 3).unwrap_err();
    assert_eq!(err.classification(), "input byte limit exceeded");
    assert_eq!(err.offset(), 3);
}

#[test]
fn stream_propagates_io_failure_while_reading_trailing_whitespace() {
    struct Failing;
    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "connection lost",
            ))
        }
    }
    let reader = (&b"true   "[..]).chain(Failing);
    let err = nextjson::from_reader::<_, bool>(reader).unwrap_err();
    assert!(err.to_string().contains("connection lost"));
}

#[derive(Debug, PartialEq, NsonSerialize, NsonDeserialize)]
#[njson(untagged)]
enum Choice {
    Number(u64),
    Numbers(Vec<u64>),
    Texts(Vec<String>),
    Text(String),
}

#[test]
fn format_backtracking_preserves_peeked_values() {
    use nextjson::formats::*;
    fn check<F: Format>(format: F) {
        for value in [
            Choice::Number(42),
            Choice::Text("text".into()),
            Choice::Numbers(vec![1, 2]),
            Choice::Texts(vec!["text".into()]),
        ] {
            let input = encode_with(&value, format).unwrap();
            let actual: Option<Choice> =
                decode_with(&input, format).unwrap_or_else(|err| panic!("{}: {err}", F::NAME));
            assert_eq!(actual, Some(value));
        }
    }
    check(Json);
    check(Json5);
    check(Hjson);
    check(Ron);
    check(Sexpr);
    check(Cbor);
    check(MsgPack);
    check(Bencode);
    check(Smile);
    check(Ubjson);
}

#[derive(Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Envelope {
    choice: Option<Choice>,
    tail: u64,
}

#[test]
fn bson_backtracking_preserves_field_type() {
    for choice in [
        Choice::Number(42),
        Choice::Text("text".into()),
        Choice::Texts(vec!["text".into()]),
    ] {
        let value = Envelope {
            choice: Some(choice),
            tail: 99,
        };
        let bytes = nextjson::formats::encode_with(&value, nextjson::formats::Bson).unwrap();
        let actual: Envelope =
            nextjson::formats::decode_with(&bytes, nextjson::formats::Bson).unwrap();
        assert_eq!(actual, value);
    }
}

#[test]
fn public_encoders_bound_recursive_values() {
    let mut value = Value::Null;
    for _ in 0..128 {
        value = Value::Array(vec![value]);
    }
    assert!(nextjson::nextencode(&value).is_ok());
    assert!(nextjson::formats::encode_with(&value, nextjson::formats::Cbor).is_ok());
    value = Value::Array(vec![value]);
    assert_eq!(
        nextjson::nextencode(&value).unwrap_err().classification(),
        "recursion limit exceeded"
    );
    assert_eq!(
        nextjson::formats::encode_with(&value, nextjson::formats::Cbor)
            .unwrap_err()
            .classification(),
        "recursion limit exceeded"
    );
    for _ in 129..5000 {
        value = Value::Array(vec![value]);
    }
    assert!(nextjson::nextencode(&value).is_err());
    assert!(nextjson::formats::encode_with(&value, nextjson::formats::MsgPack).is_err());
}

#[test]
fn public_cbor_encoder_rejects_invalid_events() {
    for case in 0..=5 {
        assert!(
            nextjson::formats::encode_with(&InvalidEvents(case), nextjson::formats::Cbor).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn backtracking_preserves_peeked_tokens() {
    for input in ["42", r#""text""#] {
        let expected: Choice = nextjson::from_str(input).unwrap();
        assert_eq!(
            nextjson::from_str::<Option<Choice>>(input).unwrap(),
            Some(expected)
        );
        let expected: Choice = nextjson::from_str(input).unwrap();
        assert_eq!(
            nextjson::from_reader::<_, Option<Choice>>(input.as_bytes()).unwrap(),
            Some(expected)
        );
        let value: Value = nextjson::from_str(input).unwrap();
        let expected: Choice = nextjson::from_str(input).unwrap();
        assert_eq!(
            nextjson::from_value::<Option<Choice>>(value).unwrap(),
            Some(expected)
        );
    }
}

struct InvalidEvents(u8);

impl NsonSchema for InvalidEvents {
    const SCHEMA: TypeSchema = TypeSchema::Opaque;
}

impl NsonSerialize for InvalidEvents {
    fn nextencode<E: FormatEncoder>(&self, e: &mut E) -> Result<(), E::Error> {
        match self.0 {
            0 => Ok(()),
            1 => {
                e.write_null()?;
                e.write_null()
            }
            2 => e.begin_array(),
            3 => {
                e.begin_object()?;
                e.key("missing")?;
                e.end_object()
            }
            4 => {
                e.begin_array()?;
                e.write_null()?;
                e.end_array()
            }
            _ => e.end_array(),
        }
    }
}

#[test]
fn public_json_encoders_reject_invalid_event_sequences() {
    for case in 0..=5 {
        let value = InvalidEvents(case);
        assert!(nextjson::nextencode(&value).is_err(), "case {case}");
        assert!(nextjson::to_string(&value).is_err());
        assert!(nextjson::to_vec_pretty(&value).is_err());
        assert!(nextjson::to_writer(Vec::new(), &value).is_err());
        assert!(nextjson::to_writer_pretty(Vec::new(), &value).is_err());
        assert!(nextjson::to_io_writer(Vec::new(), &value).is_err());
    }
}

#[test]
fn map_remains_consistent_when_retain_predicate_panics() {
    let mut map: nextjson::Map = (0..32).map(|i| (i.to_string(), Value::from(i))).collect();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        map.retain(|key, _| {
            assert_ne!(key, "20", "predicate failure");
            key.parse::<u32>().unwrap() % 2 == 0
        });
    }));
    assert!(result.is_err());
    for (key, value) in map.iter() {
        assert_eq!(map.get(key), Some(value), "stale index for {key}");
    }
    assert!(!map.contains_key("1"));
    map.insert("new".into(), Value::Null);
    assert_eq!(map.remove("new"), Some(Value::Null));
}

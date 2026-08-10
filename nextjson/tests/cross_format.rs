use nextjson::cross_format::{self, CborSink, EventSink};
use nextjson::{Error, Number, Result, Value, Write};

#[test]
fn nested_json_cbor_json_round_trip() {
    let expected = nextjson::json!({
        "name": "cross-format",
        "enabled": true,
        "ratio": -12.5,
        "values": [null, 0, 1, -7, "text", {"nested": [1, 2, 3]}]
    });
    let json = nextjson::nextencode(&expected).unwrap();
    let cbor = cross_format::json_to_cbor(&json).unwrap();
    let output = cross_format::cbor_to_json(&cbor).unwrap();
    let actual: Value = nextjson::nextdecode(&output).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn rust_128_bit_integer_domain_round_trips_through_cbor_bignums() {
    let json = nextjson::nextencode(&(u128::MAX, i128::MIN)).unwrap();
    let cbor = cross_format::json_to_cbor(&json).unwrap();
    assert!(cbor.contains(&0xc2));
    assert!(cbor.contains(&0xc3));
    let output = cross_format::cbor_to_json(&cbor).unwrap();
    assert_eq!(
        nextjson::nextdecode::<(u128, i128)>(&output).unwrap(),
        (u128::MAX, i128::MIN)
    );
}

#[test]
fn accepts_rfc_8949_definite_containers_and_half_float() {
    let document = [
        0xa3, 0x61, b'a', 0x01, 0x61, b'b', 0x82, 0xf5, 0xf6, 0x61, b'c', 0xf9, 0x3e, 0x00,
    ];
    let json = cross_format::cbor_to_json(&document).unwrap();
    let value: Value = nextjson::nextdecode(&json).unwrap();
    assert_eq!(
        value,
        nextjson::json!({"a": 1, "b": [true, null], "c": 1.5})
    );
}

#[test]
fn accepts_indefinite_text_chunks() {
    let document = [0x7f, 0x62, b'h', b'e', 0x63, b'l', b'l', b'o', 0xff];
    assert_eq!(
        cross_format::cbor_to_json(&document).unwrap(),
        br#""hello""#
    );
}

#[test]
fn rejects_cbor_values_that_json_cannot_represent() {
    for document in [
        &[0x41, 0x00][..],
        &[0xa1, 0x01, 0x02][..],
        &[0xf9, 0x7c, 0x00][..],
        &[0xc4, 0x01][..],
    ] {
        assert!(cross_format::cbor_to_json(document).is_err());
    }
}

#[test]
fn rejects_malformed_trailing_and_overdeep_cbor() {
    for document in [
        &[0x9f][..],
        &[0xff][..],
        &[0x01, 0x02][..],
        &[0x7f, 0xff, 0xff][..],
    ] {
        assert!(cross_format::cbor_to_json(document).is_err());
    }

    let nested = [0x9f, 0x9f, 0x01, 0xff, 0xff];
    let mut sink = nextjson::cross_format::JsonSink::new(Vec::new());
    assert!(cross_format::cbor_into_with_max_depth(&nested, 1, &mut sink).is_err());
}

struct RejectWriter;

impl Write for RejectWriter {
    fn write_all(&mut self, _buf: &[u8]) -> Result<()> {
        Err(Error::custom("intentional writer failure"))
    }
}

#[test]
fn cross_format_writer_errors_are_propagated() {
    let input = vec![b'x'; 9000];
    let json = nextjson::nextencode(&String::from_utf8(input).unwrap()).unwrap();
    assert!(cross_format::json_to_cbor_writer(&json, RejectWriter).is_err());
}

#[test]
fn event_sinks_reject_invalid_structure_order() {
    let mut sink = CborSink::new(Vec::new());
    assert!(sink.object_key("outside").is_err());

    let mut sink = CborSink::new(Vec::new());
    sink.begin_object().unwrap();
    sink.object_key("missing").unwrap();
    assert!(sink.end_object().is_err());

    let mut sink = CborSink::new(Vec::new());
    sink.null().unwrap();
    assert!(sink.boolean(true).is_err());
}

struct BorrowProbe {
    start: usize,
    end: usize,
    observed_borrow: bool,
}

impl BorrowProbe {
    fn reject(&self) -> Result<()> {
        Err(Error::custom("unexpected event for borrow probe"))
    }
}

impl EventSink for BorrowProbe {
    fn null(&mut self) -> Result<()> {
        self.reject()
    }

    fn boolean(&mut self, _value: bool) -> Result<()> {
        self.reject()
    }

    fn number(&mut self, _value: Number) -> Result<()> {
        self.reject()
    }

    fn string(&mut self, value: &str) -> Result<()> {
        let pointer = value.as_ptr() as usize;
        self.observed_borrow = pointer >= self.start && pointer < self.end;
        Ok(())
    }

    fn begin_array(&mut self) -> Result<()> {
        self.reject()
    }

    fn end_array(&mut self) -> Result<()> {
        self.reject()
    }

    fn begin_object(&mut self) -> Result<()> {
        self.reject()
    }

    fn object_key(&mut self, _key: &str) -> Result<()> {
        self.reject()
    }

    fn end_object(&mut self) -> Result<()> {
        self.reject()
    }
}

#[test]
fn unescaped_json_and_definite_cbor_strings_are_forwarded_by_borrow() {
    let json = br#""borrowed-json""#;
    let mut json_probe = BorrowProbe {
        start: json.as_ptr() as usize,
        end: json.as_ptr() as usize + json.len(),
        observed_borrow: false,
    };
    cross_format::json_into(json, &mut json_probe).unwrap();
    assert!(json_probe.observed_borrow);

    let cbor = [
        0x6d, b'b', b'o', b'r', b'r', b'o', b'w', b'e', b'd', b'-', b'c', b'b', b'o', b'r',
    ];
    let mut cbor_probe = BorrowProbe {
        start: cbor.as_ptr() as usize,
        end: cbor.as_ptr() as usize + cbor.len(),
        observed_borrow: false,
    };
    cross_format::cbor_into(&cbor, &mut cbor_probe).unwrap();
    assert!(cbor_probe.observed_borrow);
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

fn generated_value(rng: &mut DeterministicRng, depth: u8) -> Value {
    let kind = if depth == 0 {
        rng.next() % 5
    } else {
        rng.next() % 7
    };
    match kind {
        0 => Value::Null,
        1 => Value::Bool(rng.next() & 1 == 0),
        2 => Value::from(rng.next() as i64),
        3 => Value::from((rng.next() % 1_000_000) as f64 / 17.0 - 20_000.0),
        4 => {
            const TEXT: &[&str] = &["plain", "quote\"slash\\", "line\nfeed", "unicode-中", ""];
            Value::from(TEXT[(rng.next() as usize) % TEXT.len()])
        }
        5 => {
            let length = (rng.next() % 5) as usize;
            Value::Array(
                (0..length)
                    .map(|_| generated_value(rng, depth - 1))
                    .collect(),
            )
        }
        _ => {
            let length = (rng.next() % 5) as usize;
            let mut object = nextjson::Map::new();
            for index in 0..length {
                object.insert(
                    format!("key-{index}-{}", rng.next() % 11),
                    generated_value(rng, depth - 1),
                );
            }
            Value::Object(object)
        }
    }
}

#[test]
fn deterministic_cross_format_corpus_preserves_semantics() {
    let mut rng = DeterministicRng(0x4e65_7874_4a73_6f6e);
    for case in 0..1_000 {
        let expected = generated_value(&mut rng, 4);
        let json = nextjson::nextencode(&expected).unwrap();
        let cbor = cross_format::json_to_cbor(&json).unwrap();
        let output = cross_format::cbor_to_json(&cbor).unwrap();
        let actual: Value = nextjson::nextdecode(&output).unwrap();
        assert_eq!(actual, expected, "cross-format mismatch in case {case}");
    }
}

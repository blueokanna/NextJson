//! Input integrity regressions for text formats and streaming adapters.

#![no_std]
extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use alloc::borrow::Cow;
use nextjson::formats::{self, Csv, Ndjson, UrlForm};
use nextjson::{FormatDecoder, NsonDeserialize};

#[test]
fn csv_lookahead_preserves_string_spelling() {
    for text in ["001", "1e3", "-0", "18446744073709551617"] {
        let value: Option<String> = formats::decode_with(text.as_bytes(), Csv).unwrap();
        assert_eq!(value.as_deref(), Some(text));
    }
}

#[derive(Debug, PartialEq, NsonDeserialize)]
#[njson(untagged)]
enum Cell {
    Integer(u128),
    Text(String),
}

#[derive(Debug, PartialEq, NsonDeserialize)]
struct Field {
    value: Cell,
}

#[test]
fn csv_failed_variant_does_not_consume_the_cell() {
    let value: Cell = formats::decode_with(b"hello", Csv).unwrap();
    assert_eq!(value, Cell::Text("hello".into()));
    let value: Option<Cell> = formats::decode_with(b"hello", Csv).unwrap();
    assert_eq!(value, Some(Cell::Text("hello".into())));
}

#[test]
fn urlform_failed_variant_does_not_consume_the_field() {
    let value: Field = formats::decode_with(b"value=hello", UrlForm).unwrap();
    assert_eq!(value.value, Cell::Text("hello".into()));
}

#[test]
fn flat_text_numbers_preserve_the_full_integer_domain() {
    for value in [u64::MAX as u128 + 2, (1u128 << 100) + 123, u128::MAX] {
        let text = value.to_string();
        let csv: u128 = formats::decode_with(text.as_bytes(), Csv).unwrap();
        assert_eq!(csv, value);
        let csv: Option<u128> = formats::decode_with(text.as_bytes(), Csv).unwrap();
        assert_eq!(csv, Some(value));
        let form = format!("value={value}");
        let url: Field = formats::decode_with(form.as_bytes(), UrlForm).unwrap();
        assert_eq!(url.value, Cell::Integer(value));
    }
}

#[test]
fn csv_cannot_close_a_row_with_unread_cells() {
    let mut decoder = formats::CsvDecoder::new(vec![vec!["1".into(), "2".into()]]);
    decoder.begin_array().unwrap();
    decoder.begin_array().unwrap();
    assert_eq!(decoder.u64().unwrap(), 1);
    assert!(decoder.end_array().is_err());
}

#[test]
fn ndjson_rejects_trailing_tokens_on_each_line() {
    for input in [b"1 2\n".as_slice(), b"1 trailing\n", b"1\n2 3\n"] {
        assert!(formats::decode_with::<Vec<u64>, _>(input, Ndjson).is_err());
    }
    assert!(formats::decode_with::<u64, _>(b"1 2\n", Ndjson).is_err());
}

#[test]
fn ndjson_backtracks_within_the_current_line() {
    let values: Vec<Cell> = formats::decode_with(b"42\n\"text\"\n7\n", Ndjson).unwrap();
    assert_eq!(
        values,
        vec![
            Cell::Integer(42),
            Cell::Text("text".into()),
            Cell::Integer(7)
        ]
    );
}

#[test]
fn integer_overflow_is_not_reinterpreted_as_a_float() {
    for text in [
        "340282366920938463463374607431768211456",
        "-170141183460469231731687303715884105729",
        "+-1",
    ] {
        assert!(formats::decode_with::<nextjson::Number, _>(text.as_bytes(), Csv).is_err());
    }
    for value in [i128::MIN, -((1i128 << 100) + 123)] {
        let actual: i128 = formats::decode_with(value.to_string().as_bytes(), Csv).unwrap();
        assert_eq!(actual, value);
    }
}

#[test]
fn csv_backtracking_restores_table_mode_and_depth() {
    #[derive(Debug, PartialEq, NsonDeserialize)]
    #[njson(untagged)]
    enum Table {
        Numbers(Vec<Vec<u64>>),
        Texts(Vec<Vec<String>>),
    }
    let table: Table = formats::decode_with(b"1,hello\n2,world\n", Csv).unwrap();
    assert_eq!(
        table,
        Table::Texts(vec![
            vec!["1".into(), "hello".into()],
            vec!["2".into(), "world".into()]
        ])
    );
}

#[test]
fn urlform_backtracking_restores_root_and_percent_encoded_fields() {
    #[derive(Debug, PartialEq, NsonDeserialize)]
    struct Fields {
        first: Cell,
        second: Cell,
    }
    #[derive(Debug, PartialEq, NsonDeserialize)]
    #[njson(untagged)]
    enum Document {
        Wrong { first: u64 },
        Fields(Fields),
    }
    let value: Document =
        formats::decode_with(b"first=hello%26world&second=%E4%B8%AD+text", UrlForm).unwrap();
    assert_eq!(
        value,
        Document::Fields(Fields {
            first: Cell::Text("hello&world".into()),
            second: Cell::Text("中 text".into())
        })
    );
    let value: Fields = formats::decode_with(b"first=&second=tail&", UrlForm).unwrap();
    assert_eq!(value.first, Cell::Text(String::new()));
    assert_eq!(value.second, Cell::Text("tail".into()));
}

#[test]
fn ndjson_retries_nested_variants_without_skipping_records() {
    #[derive(Debug, PartialEq, NsonDeserialize)]
    struct Record {
        value: Cell,
    }
    let values: Vec<Record> =
        formats::decode_with(b"{\"value\":\"text\"}\r\n\n{\"value\":42}", Ndjson).unwrap();
    assert_eq!(
        values,
        vec![
            Record {
                value: Cell::Text("text".into())
            },
            Record {
                value: Cell::Integer(42)
            }
        ]
    );
    assert!(formats::decode_with::<Vec<Record>, _>(b"{\"value\":42} {}\n", Ndjson).is_err());
}

struct OversizedHint<'a>(nextjson::Decoder<'a>);

macro_rules! delegate_reads {
    ($($method:ident => $ty:ty),* $(,)?) => {$(
        fn $method(&mut self) -> nextjson::Result<$ty> { self.0.$method() }
    )*};
}

impl<'de> FormatDecoder<'de> for OversizedHint<'de> {
    type Error = nextjson::Error;
    delegate_reads! {
        begin_array => (), end_array => (), array_has_more => bool, array_entry_sep => bool,
        begin_object => (), end_object => (), object_key => Option<Cow<'de, str>>,
        object_entry_sep => bool, unit => (), bool => bool, number => nextjson::Number,
        string => Cow<'de, str>, char => char, skip_value => ()
    }
    fn peek_token(&mut self) -> nextjson::Result<nextjson::Token<'de>> {
        FormatDecoder::peek_token(&mut self.0)
    }
    fn next_token(&mut self) -> nextjson::Result<nextjson::Token<'de>> {
        FormatDecoder::next_token(&mut self.0)
    }
    fn save(&self) -> nextjson::de::Mark {
        self.0.save()
    }
    fn restore(&mut self, mark: nextjson::de::Mark) {
        self.0.restore(mark);
    }
    fn array_len_hint(&self) -> Option<usize> {
        Some(usize::MAX)
    }
    fn object_len_hint(&self) -> Option<usize> {
        Some(usize::MAX)
    }
}

#[test]
fn speculative_allocations_are_bounded_even_for_external_codecs() {
    let mut decoder = OversizedHint(nextjson::Decoder::new(b"[]"));
    let bytes: Vec<u8> = NsonDeserialize::nextdecode(&mut decoder).unwrap();
    assert!(bytes.capacity() <= 4096);
    let mut decoder = OversizedHint(nextjson::Decoder::new(b"[]"));
    let large: Vec<[u8; 4096]> = NsonDeserialize::nextdecode(&mut decoder).unwrap();
    assert!(large.capacity() * 4096 <= 64 * 1024);
    let mut decoder = OversizedHint(nextjson::Decoder::new(b"{}"));
    let map = nextjson::Map::nextdecode(&mut decoder).unwrap();
    assert!(map.is_empty());
    let mut decoder = OversizedHint(nextjson::Decoder::new(b"[]"));
    assert!(decoder.bytes().unwrap().is_empty());
    for input in [b"[]", b"{}"] {
        let mut decoder = OversizedHint(nextjson::Decoder::new(input));
        nextjson::Value::nextdecode(&mut decoder).unwrap();
    }
}

#[test]
fn streamed_large_strings_preserve_utf8_across_buffer_boundaries() {
    use nextjson::Read;
    struct Chunked<'a>(&'a [u8], usize);
    impl Read for Chunked<'_> {
        fn read(&mut self, out: &mut [u8]) -> nextjson::Result<usize> {
            let len = out.len().min(self.1);
            self.0.read(&mut out[..len])
        }
    }
    let expected = "中🙂abc\n\"\\".repeat(8192);
    let input = nextjson::to_vec(&expected).unwrap();
    for chunk in [1, 3, 7, 4095, 4096, 4097, 8192] {
        let actual: String =
            nextjson::from_reader_with_limit(Chunked(&input, chunk), input.len()).unwrap();
        assert_eq!(actual, expected);
    }
    let mut invalid = vec![b' '; 4095];
    invalid.extend_from_slice(b"\"\xf0\x9f\" ");
    assert!(nextjson::from_reader::<_, String>(invalid.as_slice()).is_err());
}

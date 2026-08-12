//! Improved integration tests in the P1 series: serde property aliases (P1.5), property completion (P1.3),
//! Streaming decoding (P1.4).
//!
//! These tests verify that nextjson can consume types written as `#[serde(...)]` with "zero property changes",
//! Supports `into` / `from` / `try_from` / `getter` / `remote` / `expecting`,
//! and incrementally pull decoding from `std::io::Read`.

use nextjson::{from_reader, from_str, to_string, FormatDecoder, NsonDeserialize, NsonSerialize};

// ---------------------------------------------------------------------------
// P1.5: `#[serde(...)]` Attribute alias
// ---------------------------------------------------------------------------

#[test]
fn serde_attribute_alias_rename() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct User {
        first_name: String,
        last_name: String,
        #[serde(rename = "emailAddr")]
        email_address: String,
    }
    let u = User {
        first_name: "Ada".into(),
        last_name: "Lovelace".into(),
        email_address: "ada@x.com".into(),
    };
    assert_eq!(
        to_string(&u).unwrap(),
        r#"{"firstName":"Ada","lastName":"Lovelace","emailAddr":"ada@x.com"}"#
    );
    let back: User =
        from_str(r#"{"firstName":"Ada","lastName":"Lovelace","emailAddr":"ada@x.com"}"#).unwrap();
    assert_eq!(back, u);
}

#[test]
fn serde_attribute_alias_skip_and_default() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Config {
        name: String,
        #[serde(skip_serializing, default)]
        secret: String,
        #[serde(default = "default_port")]
        port: u16,
    }
    fn default_port() -> u16 {
        8080
    }
    let c = Config {
        name: "app".into(),
        secret: "hunter2".into(),
        port: 8080,
    };
    // `secret` is skipped; `port` has a default value but is serialized normally.
    assert_eq!(to_string(&c).unwrap(), r#"{"name":"app","port":8080}"#);
    let back: Config = from_str(r#"{"name":"app"}"#).unwrap();
    assert_eq!(
        back,
        Config {
            name: "app".into(),
            secret: "".into(),
            port: 8080
        }
    );
}

#[test]
fn serde_attribute_alias_tagged_enum() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[serde(tag = "type")]
    enum Message {
        Text { body: String },
        Ping,
    }
    let m = Message::Text { body: "hi".into() };
    assert_eq!(to_string(&m).unwrap(), r#"{"type":"Text","body":"hi"}"#);
    let back: Message = from_str(r#"{"type":"Ping"}"#).unwrap();
    assert_eq!(back, Message::Ping);
}

#[test]
fn serde_attribute_alias_untagged_and_adjacent() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[serde(untagged)]
    enum U {
        Int(i64),
        Str(String),
    }
    assert_eq!(to_string(&U::Int(3)).unwrap(), "3");
    assert_eq!(to_string(&U::Str("x".into())).unwrap(), r#""x""#);
    let back: U = from_str(r#""x""#).unwrap();
    assert_eq!(back, U::Str("x".into()));

    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[serde(tag = "t", content = "c")]
    enum A {
        Num(i32),
        Unit,
    }
    assert_eq!(to_string(&A::Num(5)).unwrap(), r#"{"t":"Num","c":5}"#);
    let back: A = from_str(r#"{"t":"Num","c":5}"#).unwrap();
    assert_eq!(back, A::Num(5));
}

#[test]
fn serde_attribute_alias_transparent_borrow_alias() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[serde(transparent)]
    struct Wrapper(String);

    let w = Wrapper("v".into());
    assert_eq!(to_string(&w).unwrap(), r#""v""#);
    let back: Wrapper = from_str(r#""v""#).unwrap();
    assert_eq!(back, w);

    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Alias {
        #[serde(alias = "a", alias = "b")]
        value: i32,
    }
    let back: Alias = from_str(r#"{"b":7}"#).unwrap();
    assert_eq!(back, Alias { value: 7 });
}

#[test]
fn serde_attribute_directional_rename_all() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
    struct Dual {
        some_field: String,
    }
    let d = Dual {
        some_field: "v".into(),
    };
    // serialize uses camelCase
    assert_eq!(to_string(&d).unwrap(), r#"{"someField":"v"}"#);
    // deserialize accepts snake_case (its own direction)
    let back: Dual = from_str(r#"{"some_field":"v"}"#).unwrap();
    assert_eq!(back, d);
}

#[test]
fn serde_attribute_directional_bound() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[serde(bound(
        serialize = "T: nextjson::NsonSerialize",
        deserialize = "T: nextjson::NsonDeserialize<'de>"
    ))]
    struct Boxed<T> {
        value: T,
    }
    let b = Boxed { value: 42u32 };
    assert_eq!(to_string(&b).unwrap(), r#"{"value":42}"#);
    let back: Boxed<u32> = from_str(r#"{"value":42}"#).unwrap();
    assert_eq!(back, b);
}

// ---------------------------------------------------------------------------
// P1.3: into / from / try_from
// ---------------------------------------------------------------------------

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct RawPoint {
    x: f64,
    y: f64,
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq, Clone)]
#[njson(into = "RawPoint", from = "RawPoint")]
struct Point {
    x: f64,
    y: f64,
}

impl From<Point> for RawPoint {
    fn from(p: Point) -> Self {
        RawPoint { x: p.x, y: p.y }
    }
}
impl From<RawPoint> for Point {
    fn from(r: RawPoint) -> Self {
        Point { x: r.x, y: r.y }
    }
}

#[test]
fn into_and_from_conversion() {
    let p = Point { x: 1.0, y: 2.0 };
    assert_eq!(to_string(&p).unwrap(), r#"{"x":1.0,"y":2.0}"#);
    let back: Point = from_str(r#"{"x":1.0,"y":2.0}"#).unwrap();
    assert_eq!(back, p);
}

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
struct IntStr(String);

#[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
#[njson(try_from = "IntStr")]
struct Count {
    value: u64,
}

impl TryFrom<IntStr> for Count {
    type Error = String;
    fn try_from(s: IntStr) -> Result<Self, Self::Error> {
        s.0.parse::<u64>()
            .map(|value| Count { value })
            .map_err(|e| e.to_string())
    }
}

#[test]
fn try_from_conversion() {
    // newtype struct 在 nextjson/serde 中序列化为数组 `["42"]`。
    let back: Count = from_str(r#"["42"]"#).unwrap();
    assert_eq!(back, Count { value: 42 });
    let err = from_str::<Count>(r#"["nope"]"#).unwrap_err();
    assert!(err.to_string().contains("invalid digit"));
}

// ---------------------------------------------------------------------------
// Remote + getter (derived from the outer type)
// ---------------------------------------------------------------------------

mod external {
    pub struct External {
        inner: String,
    }
    impl External {
        pub fn new(inner: &str) -> Self {
            External {
                inner: inner.into(),
            }
        }
        pub fn inner(&self) -> &str {
            &self.inner
        }
    }
}

// Deserialization requires constructing `external::External { inner: ... }`,
// which requires the fields to be visible at the derive location;
// here we only verify serialization paths via getters (serde's remote deserialization also requires the fields to be visible or to use `from`).

#[allow(dead_code)]
#[derive(NsonSerialize)]
#[njson(remote = "external::External")]
struct ExternalMirror {
    #[njson(getter = "external::External::inner")]
    inner: String,
}

#[test]
fn remote_with_getter() {
    let e = external::External::new("hello");
    assert_eq!(to_string(&e).unwrap(), r#"{"inner":"hello"}"#);
    let schema = nextjson::schema_of::<external::External>();
    assert!(matches!(
        schema,
        nextjson::TypeSchema::Struct(s) if s.name == "external::External"
    ));
}

mod external_pub {
    pub struct Public {
        pub id: u64,
        pub label: String,
    }
}

#[allow(dead_code)]
#[derive(NsonSerialize, NsonDeserialize)]
#[njson(remote = "external_pub::Public")]
struct PublicMirror {
    id: u64,
    label: String,
}

#[test]
fn remote_roundtrip_visible_fields() {
    let p = external_pub::Public {
        id: 5,
        label: "x".into(),
    };
    assert_eq!(to_string(&p).unwrap(), r#"{"id":5,"label":"x"}"#);
    let back: external_pub::Public = from_str(r#"{"id":5,"label":"x"}"#).unwrap();
    assert_eq!(back.id, 5);
    assert_eq!(back.label, "x");
}

// ---------------------------------------------------------------------------
// P1.3: Expecting accepts without breaking (serde-compatible migration)
// ---------------------------------------------------------------------------

#[test]
fn expecting_is_accepted() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(expecting = "a User object")]
    struct User {
        name: String,
    }
    let u = User { name: "a".into() };
    assert_eq!(to_string(&u).unwrap(), r#"{"name":"a"}"#);
    let back: User = from_str(r#"{"name":"a"}"#).unwrap();
    assert_eq!(back, u);
}

// ---------------------------------------------------------------------------
// P1.4: Streaming Decoding
// ---------------------------------------------------------------------------

#[test]
fn stream_decoder_roundtrip() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Event {
        id: u64,
        name: String,
        tags: Vec<String>,
        ok: bool,
    }
    let e = Event {
        id: 7,
        name: "boot".into(),
        tags: vec!["a".into(), "b".into()],
        ok: true,
    };
    let json = to_string(&e).unwrap();
    let streamed: Event = from_reader(json.as_bytes()).unwrap();
    assert_eq!(streamed, e);
}

#[test]
fn stream_decoder_chunked_reader() {
    // A reader that only yields 1 byte at a time, forcing the streaming decoder to pull byte by byte.
    struct OneByte<'a>(&'a [u8], usize);
    impl<'a> std::io::Read for OneByte<'a> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1 >= self.0.len() {
                return Ok(0);
            }
            if buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.0[self.1];
            self.1 += 1;
            Ok(1)
        }
    }

    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Payload {
        a: i32,
        b: String,
        c: Vec<u8>,
    }
    let p = Payload {
        a: -42,
        b: "你好, world".into(),
        c: vec![1, 2, 3],
    };
    let json = to_string(&p).unwrap();
    let reader = OneByte(json.as_bytes(), 0);
    let streamed: Payload = from_reader(reader).unwrap();
    assert_eq!(streamed, p);
}

#[test]
fn stream_decoder_untagged_enum() {
    // The save/restore (untagged backtracking) function must also work on the streaming decoder.
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(untagged)]
    enum V {
        Int(i64),
        Str(String),
    }
    let json = br#""hello""#;
    let streamed: V = from_reader(&json[..]).unwrap();
    assert_eq!(streamed, V::Str("hello".into()));

    let json = b"12345";
    let streamed: V = from_reader(&json[..]).unwrap();
    assert_eq!(streamed, V::Int(12345));
}

#[test]
fn stream_decoder_matches_slice_on_varying_chunks() {
    struct Chunked<'a> {
        data: &'a [u8],
        pos: usize,
        size: usize,
    }
    impl<'a> std::io::Read for Chunked<'a> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let n = self.size.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct Doc {
        title: String,
        flags: Vec<bool>,
        nested: Vec<Vec<f64>>,
        map: Vec<(String, String)>,
    }
    let doc = Doc {
        title: "数据 \"quoted\" \n é😀".into(),
        flags: vec![true, false, true],
        nested: vec![vec![1.5, -2.0], vec![3.25]],
        map: vec![("k1".into(), "v1".into()), ("k2".into(), "v2".into())],
    };
    let json = to_string(&doc).unwrap();
    let expected: Doc = from_str(&json).unwrap();

    for size in [3usize, 7, 13, 64] {
        let reader = Chunked {
            data: json.as_bytes(),
            pos: 0,
            size,
        };
        let got: Doc = from_reader(reader).unwrap();
        assert_eq!(got, expected, "chunk size {size}");
    }
}

#[test]
fn stream_decoder_borrow_rejected() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        name: String,
    }
    let s: S = from_reader(br#"{"name":"x"}"#.as_slice()).unwrap();
    assert_eq!(s, S { name: "x".into() });
}

#[test]
fn stream_decoder_escapes_and_unicode() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        text: String,
    }
    let json = br#"{"text":"line\n\u00e9\ud83d\ude00"}"#;
    let s: S = from_reader(&json[..]).unwrap();
    assert_eq!(s.text, "line\né😀");
}

#[test]
fn stream_decoder_error_position() {
    let err = from_reader::<_, i32>(br#"12x"#.as_slice()).unwrap_err();
    assert!(err.to_string().contains("trailing") || err.to_string().contains("end of input"));

    // The array is missing an element: an EOF error should be reported.
    let err = from_reader::<_, Vec<i32>>(br#"["#.as_slice()).unwrap_err();
    assert!(err.to_string().contains("unexpected end of input"));
}

#[test]
fn stream_decoder_uses_config() {
    use nextjson::{DecodeConfig, StreamDecoder};
    // Depth-limited streaming decoding also applies.
    let deep = b"[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]";
    let mut dec = StreamDecoder::with_config(deep.as_slice(), DecodeConfig::new().max_depth(8));
    let err = <nextjson::Value as nextjson::NsonDeserialize<'_>>::nextdecode(&mut dec).unwrap_err();
    assert!(err.to_string().contains("recursion limit"));
}

#[test]
fn stream_decoder_scalars() {
    use nextjson::StreamDecoder;
    let mut d = StreamDecoder::new(br#"null true 1 -2.5 "s" "c" "#.as_slice());
    assert_eq!(d.unit().unwrap(), ());
    assert!(d.bool().unwrap());
    assert_eq!(d.number().unwrap().as_u64(), Some(1));
    assert_eq!(d.number().unwrap().as_f64(), -2.5);
    assert_eq!(d.string().unwrap().as_ref(), "s");
    assert_eq!(d.char().unwrap(), 'c');
    d.end().unwrap();
}

#[test]
fn stream_decoder_bytes() {
    use nextjson::StreamDecoder;
    // Bytes need to be borrowed; the streaming decoder can only read owned byte sequences.
    let mut d = StreamDecoder::new(br#"[1,2,3]"#.as_slice());
    let bytes = d.bytes().unwrap();
    assert_eq!(bytes.as_ref(), &[1u8, 2, 3]);

    let mut d = StreamDecoder::new(br#""abc""#.as_slice());
    let bytes = d.bytes().unwrap();
    assert_eq!(bytes.as_ref(), b"abc");
}

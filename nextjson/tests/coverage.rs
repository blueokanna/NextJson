//! Coverage-oriented supplementary testing: JSON Schema generation, Write implementation, Bytes wrapping, Error surface,
//! formats error paths and detection/registry, Envy environment variable decoding.
//!
//! The goal is to improve full repository line coverage from ~74.5% to over 80% (CI coverage gate).
//!
//! Most types in this file exist only to derive a schema/serialization and are
//! never constructed, so dead-code is allowed at the crate level.
//!

#![allow(dead_code)]
use nextjson::formats::{Format, FormatKind};
use nextjson::{
    to_json_schema, Bytes, Decoder, Error, FormatDecoder, NsonDeserialize, NsonSerialize, Value,
    Write,
};
// ---------------------------------------------------------------------------
// JSON Schema Generation
// ---------------------------------------------------------------------------

#[test]
fn json_schema_all_primitive_kinds() {
    #[derive(NsonSerialize)]
    struct Prims {
        b: bool,
        i8_: i8,
        i16_: i16,
        i32_: i32,
        i64_: i64,
        i128_: i128,
        isz: isize,
        u8_: u8,
        u16_: u16,
        u32_: u32,
        u64_: u64,
        u128_: u128,
        usz: usize,
        f32_: f32,
        f64_: f64,
        c: char,
        s: String,
    }
    let schema = to_json_schema::<Prims>();
    let obj = schema.as_object().unwrap();
    let props = obj.get("properties").unwrap().as_object().unwrap();
    assert_eq!(
        props.get("b").unwrap().get("type").unwrap(),
        &Value::from("boolean")
    );
    assert_eq!(
        props.get("i8_").unwrap().get("type").unwrap(),
        &Value::from("integer")
    );
    assert_eq!(
        props.get("i128_").unwrap().get("type").unwrap(),
        &Value::from("integer")
    );
    assert_eq!(
        props.get("u8_").unwrap().get("type").unwrap(),
        &Value::from("integer")
    );
    assert_eq!(
        props.get("u8_").unwrap().get("minimum").unwrap(),
        &Value::from(0_u64)
    );
    assert_eq!(
        props.get("u128_").unwrap().get("minimum").unwrap(),
        &Value::from(0_u64)
    );
    assert_eq!(
        props.get("f64_").unwrap().get("type").unwrap(),
        &Value::from("number")
    );
    assert_eq!(
        props.get("c").unwrap().get("type").unwrap(),
        &Value::from("string")
    );
    assert_eq!(
        props.get("s").unwrap().get("type").unwrap(),
        &Value::from("string")
    );
    // All fields are required
    let required = obj.get("required").unwrap().as_array().unwrap();
    assert!(required.iter().any(|v| v == &Value::from("b")));
    assert!(required.iter().any(|v| v == &Value::from("s")));
}

#[test]
fn json_schema_unit_bytes_optional_seq_map_tuple() {
    #[derive(NsonSerialize)]
    struct Unit;
    assert_eq!(
        to_json_schema::<Unit>().get("type").unwrap(),
        &Value::from("null")
    );

    #[derive(NsonSerialize)]
    struct Containers {
        bytes: Bytes<'static>,
        opt: Option<i32>,
        seq: Vec<String>,
        map: std::collections::BTreeMap<String, u8>,
        pair: (i32, String),
        #[njson(skip_serializing)]
        hidden: u64,
    }
    let schema = to_json_schema::<Containers>();
    let obj = schema.as_object().unwrap();
    let props = obj.get("properties").unwrap().as_object().unwrap();

    // Bytes → 0..255 integer array.
    let bytes = props.get("bytes").unwrap().as_object().unwrap();
    assert_eq!(bytes.get("type").unwrap(), &Value::from("array"));
    assert_eq!(
        bytes.get("items").unwrap().get("maximum").unwrap(),
        &Value::from(255_u64)
    );

    // Option<T> → nullable。
    let opt = props.get("opt").unwrap().as_object().unwrap();
    assert_eq!(opt.get("nullable").unwrap(), &Value::from(true));
    assert_eq!(opt.get("type").unwrap(), &Value::from("integer"));

    // Vec<T> → array + items。
    let seq = props.get("seq").unwrap().as_object().unwrap();
    assert_eq!(seq.get("type").unwrap(), &Value::from("array"));
    assert_eq!(
        seq.get("items").unwrap().get("type").unwrap(),
        &Value::from("string")
    );

    // Map<K, V> → object + additionalProperties = value schema。
    let map = props.get("map").unwrap().as_object().unwrap();
    assert_eq!(map.get("type").unwrap(), &Value::from("object"));
    assert_eq!(
        map.get("additionalProperties")
            .unwrap()
            .get("minimum")
            .unwrap(),
        &Value::from(0_u64)
    );

    // Tuple → array + minItems/maxItems
    let pair = props.get("pair").unwrap().as_object().unwrap();
    assert_eq!(pair.get("type").unwrap(), &Value::from("array"));
    assert_eq!(pair.get("minItems").unwrap(), &Value::from(2_u64));
    assert_eq!(pair.get("maxItems").unwrap(), &Value::from(2_u64));
    assert_eq!(pair.get("items").unwrap().as_array().unwrap().len(), 2);

    // Skip field → Opaque (does not generate type)
    let hidden = props.get("hidden").unwrap().as_object().unwrap();
    assert!(hidden.get("type").is_none());
}

#[test]
fn json_schema_struct_required_transparent_and_enum_shapes() {
    #[derive(NsonSerialize)]
    struct Named {
        a: i32,
        #[njson(default)]
        b: String,
    }
    let schema = to_json_schema::<Named>();
    let required = schema.get("required").unwrap().as_array().unwrap();
    assert_eq!(required.len(), 1);
    assert_eq!(required[0], Value::from("a"));
    assert_eq!(
        schema.get("additionalProperties").unwrap(),
        &Value::from(true)
    );
    assert_eq!(schema.get("title").unwrap(), &Value::from("Named"));

    #[derive(NsonSerialize)]
    enum Plain {
        Red,
        Green,
    }
    let schema = to_json_schema::<Plain>();
    assert_eq!(schema.get("type").unwrap(), &Value::from("string"));
    let enums = schema.get("enum").unwrap().as_array().unwrap();
    assert_eq!(enums.len(), 2);
    assert!(enums.contains(&Value::from("Red")));

    #[derive(NsonSerialize)]
    #[njson(tag = "kind")]
    enum Tagged {
        A { x: i32 },
        B,
    }
    let schema = to_json_schema::<Tagged>();
    let one_of = schema.get("oneOf").unwrap().as_array().unwrap();
    assert_eq!(one_of.len(), 2);
    assert!(one_of[0].get("kind").is_some());

    #[derive(NsonSerialize)]
    #[njson(tag = "t", content = "c")]
    enum Adj {
        V { n: i32 },
    }
    let schema = to_json_schema::<Adj>();
    assert!(schema.get("oneOf").is_some());

    #[derive(NsonSerialize)]
    #[njson(untagged)]
    enum Un {
        N(i32),
        S(String),
    }
    let schema = to_json_schema::<Un>();
    assert!(schema.get("oneOf").unwrap().as_array().unwrap().len() >= 2);
}

// ---------------------------------------------------------------------------
// Write implementation
// ---------------------------------------------------------------------------

#[test]
fn write_impls_cover_all_sinks() {
    // Vec<u8>。
    let mut v: Vec<u8> = Vec::new();
    v.write_all(b"abc").unwrap();
    assert_eq!(v, b"abc");

    // String (valid UTF-8 and illegal byte reporting)
    let mut s = String::new();
    s.write_all("你好".as_bytes()).unwrap();
    assert_eq!(s, "你好");
    assert!(s.write_all(&[0xff, 0xfe]).is_err());

    // &mut [u8]: Precise, overflow error, partial write
    let mut buf = [0u8; 4];
    let mut writer: &mut [u8] = &mut buf;
    writer.write_all(b"abcd").unwrap();
    assert_eq!(&buf, b"abcd");
    let mut small = [0u8; 2];
    let mut writer: &mut [u8] = &mut small;
    assert!(writer.write_all(b"toolarge").is_err());
    writer.write_all(b"xy").unwrap();
    assert_eq!(&small, b"xy");

    // &mut W forwards (written to Vec<u8> via Encoder). Encoder buffers until finish before writing to disk
    let mut sink = Vec::new();
    let mut out = nextjson::Encoder::<&mut Vec<u8>>::new(&mut sink);
    nextjson::NsonSerialize::nextencode(&42_i32, &mut out).unwrap();
    out.finish().unwrap();
    assert_eq!(sink.as_slice(), b"42");

    let mut v2: Vec<u8> = Vec::new();
    v2.flush().unwrap();
}

#[cfg(feature = "std")]
#[test]
fn io_writer_adapter_maps_io_errors() {
    let mut underlying = Vec::new();
    nextjson::to_io_writer(&mut underlying, &7_u32).unwrap();
    assert_eq!(underlying.as_slice(), b"7");

    struct Failing;
    impl std::io::Write for Failing {
        fn write(&mut self, _b: &[u8]) -> std::io::Result<usize> {
            // MSRV 1.71：`std::io::Error::other` 需 1.74+，用兼容写法。
            #[allow(clippy::io_other_error)]
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "boom"));
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let err = nextjson::to_io_writer(Failing, &7_u32).unwrap_err();
    assert!(err.to_string().contains("io error"));
}

// ---------------------------------------------------------------------------
// Bytes wrapper (bytes.rs fully covers)
// ---------------------------------------------------------------------------

#[test]
fn bytes_api_and_json_roundtrip() {
    let b = Bytes(b"\x00\x01hi");
    assert_eq!(b.as_bytes(), b"\x00\x01hi");
    assert_eq!(&*b, b"\x00\x01hi");
    assert_eq!(b.as_ref(), b"\x00\x01hi");
    let from_slice = Bytes::from(&b"\x00\x01"[..]);
    assert_eq!(from_slice.as_bytes(), b"\x00\x01");
    let from_str = Bytes::from("text");
    assert_eq!(from_str.as_bytes(), b"text");

    let json = nextjson::nextencode(&b).unwrap();
    assert_eq!(json, b"[0,1,104,105]");
    assert!(nextjson::nextdecode::<Bytes<'_>>(&json).is_err());

    // The binary format uses the native byte string (msgpack), which can be borrowed and is consistent in both directions
    let wire = nextjson::formats::MsgPack.encode(&b).unwrap();
    let back: Bytes<'_> = nextjson::formats::MsgPack.decode(&wire).unwrap();
    assert_eq!(back, b);

    // JSON also accepts string spelling and decoding to bytes
    let back: Bytes<'_> = nextjson::nextdecode(br#""hello""#).unwrap();
    assert_eq!(back.as_bytes(), b"hello");
}

// ---------------------------------------------------------------------------
// Error Surface
// ---------------------------------------------------------------------------

#[test]
fn error_constructors_accessors_classification() {
    let e = Error::custom("boom");
    assert!(e.is_custom());
    assert_eq!(e.classification(), "custom error");
    assert_eq!(e.line(), None);
    assert_eq!(e.column(), None);
    assert_eq!(e.offset(), 0);
    assert!(e.to_string().contains("boom"));

    assert_eq!(Error::missing_field("f").classification(), "missing field");
    assert_eq!(
        Error::unknown_field("k".into()).classification(),
        "unknown field"
    );
    assert_eq!(
        Error::unknown_variant("v".into()).classification(),
        "unknown variant"
    );
    assert_eq!(
        Error::invalid_length(3, "a tuple").classification(),
        "invalid length"
    );
    assert_eq!(
        Error::invalid_type("integer", "string").classification(),
        "invalid type"
    );
    assert!(Error::missing_field("f").to_string().contains("f"));
    assert!(Error::unknown_field("k".into()).to_string().contains("k"));
}

#[test]
fn error_kinds_via_real_decode_failures() {
    use nextjson::{from_str, Decoder};

    let cases: &[(&str, &str)] = &[
        ("", "unexpected end of input"),
        ("{", "unexpected end of input"),
        ("01", "invalid number"),
        ("1e999", "invalid number"),
        (r#""\q""#, "invalid escape sequence"),
        ("[1,2,]", "expected a specific token"),
    ];
    for (input, class) in cases {
        let err = from_str::<Value>(input).unwrap_err();
        assert_eq!(err.classification(), *class, "input: {input:?}");
    }

    // Invalid UTF-8 sequence.
    let mut d = Decoder::new(b"\"\xff\xfe\"");
    assert_eq!(
        from_into::<Value>(&mut d).unwrap_err().classification(),
        "invalid utf-8"
    );

    // Unescaped control character.
    let mut d = Decoder::new(b"\"a\x01b\"");
    assert_eq!(
        from_into::<Value>(&mut d).unwrap_err().classification(),
        "unexpected control character in string"
    );

    // Isolated proxy
    let mut d = Decoder::new(br#""\ud800""#);
    assert_eq!(
        from_into::<Value>(&mut d).unwrap_err().classification(),
        "invalid surrogate pair"
    );

    // Recursion depth exceeded limit
    let deep = "[".repeat(200);
    let err = from_str::<Value>(&deep).unwrap_err();
    assert_eq!(err.classification(), "recursion limit exceeded");

    // Error with non-finite floating-point encoding
    let err = nextjson::to_vec(&f64::NAN).unwrap_err();
    assert_eq!(err.classification(), "non-finite float");
}

fn from_into<'de, T: NsonDeserialize<'de>>(d: &mut Decoder<'de>) -> Result<T, Error> {
    T::nextdecode(d)
}

// ---------------------------------------------------------------------------
// Registry + detection
// ---------------------------------------------------------------------------

#[test]
fn format_registry_fields_are_consistent() {
    let all = nextjson::formats::all();
    assert!(!all.is_empty());
    for info in all {
        assert!(!info.name.is_empty());
        assert!(!info.mime.is_empty());
        assert_eq!(
            nextjson::formats::by_name(info.name),
            Some(info.kind),
            "by_name failed for {}",
            info.name
        );
        if let Some(first_ext) = info.extensions.first() {
            assert_eq!(
                nextjson::formats::by_extension(first_ext),
                Some(info.kind),
                "by_extension failed for {}",
                first_ext
            );
        }
    }
    assert_eq!(nextjson::formats::by_name("nope"), None);
    assert_eq!(nextjson::formats::by_extension("nope"), None);
}

#[test]
fn format_detection_signatures() {
    let cases: &[(&[u8], FormatKind)] = &[
        (&[0x80, 0x04], FormatKind::Pickle),
        (b"d3:keyi1ee", FormatKind::Bencode),
        (b"5:hello", FormatKind::Bencode),
        (b"i42e", FormatKind::Bencode),
        (
            &[
                0x0c, 0x00, 0x00, 0x00, 0x10, b'a', 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
            ],
            FormatKind::Bson,
        ),
        (b"---\nkey: 1", FormatKind::Yaml),
        (b"{\"a\":1}", FormatKind::Json),
        (b"[1,2]", FormatKind::Json),
        (b"\"str\"", FormatKind::Json),
        (b"-1", FormatKind::Json),
        (b"+1", FormatKind::Json),
        (b".5", FormatKind::Json),
        (b"true", FormatKind::Json),
        (b"(a b)", FormatKind::Sexpr),
        (b"# comment", FormatKind::Toml),
        (b"%20", FormatKind::UrlForm),
        (&[0x80], FormatKind::MsgPack),
        (&[0x90], FormatKind::MsgPack),
        (&[0xc2], FormatKind::MsgPack),
        (&[0xe0], FormatKind::MsgPack),
        (&[0x1b], FormatKind::Cbor),
        (&[0x38], FormatKind::Cbor),
        (&[0x78], FormatKind::Cbor),
    ];
    for (input, expected) in cases {
        assert_eq!(
            nextjson::formats::detect(input),
            Some(*expected),
            "detect failed for {input:?}"
        );
    }
    // No signature input → None; Empty input → None
    assert_eq!(nextjson::formats::detect(b""), None);
    assert_eq!(nextjson::formats::detect(b"123"), None);
    assert_eq!(nextjson::formats::detect(b"plain text words"), None);
}

// ---------------------------------------------------------------------------
// Bad path
// ---------------------------------------------------------------------------

#[test]
fn format_decode_rejects_type_mismatch() {
    let wrong_type = |f: &dyn FormatTyped, bytes: &[u8]| f.decode_u32(bytes).is_err();
    let pairs: &[(&dyn FormatTyped, Vec<u8>)] = &[
        (&Jt, nextjson::formats::Json.encode(&"hello").unwrap()),
        (&J5t, nextjson::formats::Json5.encode(&"hello").unwrap()),
        (&Ht, nextjson::formats::Hjson.encode(&"hello").unwrap()),
        (&Yt, nextjson::formats::Yaml.encode(&"hello").unwrap()),
        (&Rt, nextjson::formats::Ron.encode(&"hello").unwrap()),
        (&St, nextjson::formats::Sexpr.encode(&"hello").unwrap()),
        (&Ct, nextjson::formats::Cbor.encode(&"hello").unwrap()),
        (&Mt, nextjson::formats::MsgPack.encode(&"hello").unwrap()),
        (&Pt, nextjson::formats::Postcard.encode(&"hello").unwrap()),
        (&Kt, nextjson::formats::Pickle.encode(&"hello").unwrap()),
    ];
    for (f, bytes) in pairs {
        assert!(wrong_type(*f, bytes), "{} accepted string as u32", f.name());
    }
}

trait FormatTyped {
    fn name(&self) -> &str;
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error>;
}
struct Jt;
impl FormatTyped for Jt {
    fn name(&self) -> &str {
        "json"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Json.decode(input)
    }
}
struct J5t;
impl FormatTyped for J5t {
    fn name(&self) -> &str {
        "json5"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Json5.decode(input)
    }
}
struct Ht;
impl FormatTyped for Ht {
    fn name(&self) -> &str {
        "hjson"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Hjson.decode(input)
    }
}
struct Yt;
impl FormatTyped for Yt {
    fn name(&self) -> &str {
        "yaml"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Yaml.decode(input)
    }
}
struct Rt;
impl FormatTyped for Rt {
    fn name(&self) -> &str {
        "ron"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Ron.decode(input)
    }
}
struct St;
impl FormatTyped for St {
    fn name(&self) -> &str {
        "sexpr"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Sexpr.decode(input)
    }
}
struct Ct;
impl FormatTyped for Ct {
    fn name(&self) -> &str {
        "cbor"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Cbor.decode(input)
    }
}
struct Mt;
impl FormatTyped for Mt {
    fn name(&self) -> &str {
        "msgpack"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::MsgPack.decode(input)
    }
}
struct Pt;
impl FormatTyped for Pt {
    fn name(&self) -> &str {
        "postcard"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Postcard.decode(input)
    }
}
struct Kt;
impl FormatTyped for Kt {
    fn name(&self) -> &str {
        "pickle"
    }
    fn decode_u32(&self, input: &[u8]) -> Result<u32, Error> {
        nextjson::formats::Pickle.decode(input)
    }
}

#[test]
fn format_encode_rejects_non_finite_floats() {
    use nextjson::formats::{Bson, Json, Toml};
    assert!(Json.encode(&f64::NAN).is_err());
    assert!(Json.encode(&f64::INFINITY).is_err());
    assert!(Toml.encode(&f64::NAN).is_err());
    assert!(Bson.encode(&f64::NAN).is_err());
}

#[test]
fn format_decode_rejects_malformed_input() {
    use nextjson::formats::*;

    // Text format: truncated / syntax error.
    assert!(Json.decode::<Value>(b"{").is_err());
    assert!(Json5.decode::<Value>(b"{\"a\":}").is_err());
    assert!(Hjson.decode::<Value>(b"[[[").is_err());
    assert!(Yaml.decode::<Value>(b"key: [unclosed").is_err());
    assert!(Toml.decode::<Value>(b"a = ").is_err());
    assert!(Ron.decode::<Value>(b"Struct(").is_err());
    assert!(Sexpr.decode::<Value>(b"(unclosed").is_err());
    assert!(Csv.decode::<Value>(b"\"unterminated").is_err());
    assert!(UrlForm.decode::<Value>(b"%zz").is_err());

    assert!(MsgPack.decode::<Value>(&[0xc1]).is_err());
    assert!(Cbor.decode::<Value>(&[0xff]).is_err());
    assert!(Bson.decode::<Value>(&[0x05, 0x00, 0x00, 0x00]).is_err());
    assert!(Postcard.decode::<Value>(&[0xff]).is_err());
    assert!(Pickle.decode::<Value>(&[0x80, 0x04, 0x4b]).is_err());
    assert!(Bencode.decode::<Value>(b"3:a").is_err());
    assert!(Json.decode::<Value>(b"1 2").is_err());
}

// ---------------------------------------------------------------------------
// Environment variable deserialization
// ---------------------------------------------------------------------------

#[cfg(feature = "std")]
#[test]
fn envy_decodes_from_environment() {
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap();

    std::env::set_var("NX_ENV_COUNT", "42");
    std::env::set_var("NX_ENV_FLAG", "true");
    std::env::set_var("NX_ENV_NAME", "nextjson");

    #[derive(NsonDeserialize)]
    struct Loose {
        #[njson(rename = "NX_ENV_COUNT")]
        count: u32,
        #[njson(rename = "NX_ENV_FLAG")]
        flag: bool,
        #[njson(rename = "NX_ENV_NAME")]
        name: String,
    }
    let cfg: Loose = nextjson::formats::Envy.decode(&[]).unwrap();
    assert_eq!(cfg.count, 42);
    assert!(cfg.flag);
    assert_eq!(cfg.name, "nextjson");

    // Serialization to an environment that does not support it → Error
    let err = nextjson::formats::Envy.encode(&42).unwrap_err();
    assert!(err.to_string().contains("not supported"));

    // There are usually other variables in the environment, and errors will occur if the types do not match or the fields are missing
    #[derive(NsonDeserialize)]
    struct EnvConfig {
        n: u32,
    }
    let r = nextjson::formats::Envy.decode::<EnvConfig>(&[]);
    assert!(r.is_err());
}

// ---------------------------------------------------------------------------
// Low-level decoding primitive walkthrough
// ---------------------------------------------------------------------------

#[test]
fn decoder_width_methods_and_bool_char() {
    let mut d = Decoder::new(b"true 123 -7 3.5 'x' 65535 4294967295 18446744073709551615");
    assert!(d.bool().unwrap());
    assert_eq!(d.i32().unwrap(), 123);
    assert_eq!(d.i64().unwrap(), -7);
    assert_eq!(d.number().unwrap().as_f64(), 3.5);

    let mut d = Decoder::new(br#""x" 99 "#);
    assert_eq!(d.char().unwrap(), 'x');
    assert_eq!(d.u8().unwrap(), 99);

    // Overflow detection
    let mut d = Decoder::new(b"18446744073709551616");
    assert!(d.u64().is_err());
    let mut d = Decoder::new(b"300");
    assert!(d.u8().is_err());
}

#[test]
fn option_and_map_key_primitives() {
    let json = br#"{"key":1,"other":null}"#;
    let mut d = Decoder::new(json);
    // Option：null → None。
    let m: std::collections::BTreeMap<String, Option<i32>> =
        <std::collections::BTreeMap<String, Option<i32>> as NsonDeserialize>::nextdecode(&mut d)
            .unwrap();
    assert_eq!(m.get("key"), Some(&Some(1)));
    assert_eq!(m.get("other"), Some(&None));
}

// ---------------------------------------------------------------------------
// Streaming decoder
// ---------------------------------------------------------------------------

#[cfg(feature = "std")]
#[test]
fn stream_decoder_chunked_and_errors() {
    use nextjson::StreamDecoder;
    struct OneByte<'a>(&'a [u8], usize);
    impl std::io::Read for OneByte<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1 >= self.0.len() {
                return Ok(0);
            }
            buf[0] = self.0[self.1];
            self.1 += 1;
            Ok(1)
        }
    }
    let data = br#"{"name":"nextjson","n":7}"#;
    let mut d = StreamDecoder::new(OneByte(data, 0));
    let v: Value = Value::nextdecode(&mut d).unwrap();
    assert_eq!(v["name"], Value::from("nextjson"));
    d.end().unwrap();

    // Input truncated → EOF error
    let mut d = StreamDecoder::new(&br#"{"a":"#[..]);
    assert!(Value::nextdecode(&mut d).is_err());
}

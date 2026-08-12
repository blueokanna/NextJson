//! Intensive syntax and error branch coverage for toml/msgpack/json5/yaml

use nextjson::formats::Format;
use nextjson::Value;

#[test]
fn toml_grammar_and_error_branches() {
    use nextjson::formats::Toml;
    let v: Value = Toml.decode(b"a.b.c = 1\nk = [1, 2, 3]").unwrap();
    assert_eq!(v["a"]["b"]["c"], Value::from(1_i64));
    let v: Value = Toml
        .decode(b"[[items]]\nname = 'a'\n[[items]]\nname = 'b'")
        .unwrap();
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let v: Value = Toml.decode(b"t = { x = 1, y = 's' }").unwrap();
    assert_eq!(v["t"]["x"], Value::from(1_i64));
    let v: Value = Toml.decode(b"a = true\nb = 1.5\nc = \"str\"").unwrap();
    assert_eq!(v["a"], Value::from(true));

    // 错误分支。
    assert!(Toml.decode::<Value>(b"= 1").is_err()); // Missing keys
    assert!(Toml.decode::<Value>(b"a =").is_err()); // Missing values
    assert!(Toml.decode::<Value>(b"[a]\n[a]\nx = 1").is_err()); // Duplicate Tables
    assert!(Toml.decode::<Value>(b"a.b = 1\na = 2").is_err()); // Key conflict
    assert!(Toml.decode::<Value>(b"a = \"unterminated").is_err()); // Unclosed string
    assert!(Toml.decode::<Value>(b"[unclosed\nx = 1").is_err()); // Unclosed table
    assert!(Toml.decode::<Value>(b"a = \"\\u\"").is_err()); // Illegal escape sequence (basic string)
    assert!(Toml.decode::<Value>(b"[a.b]\nx=1\n[a]\ny=2").is_err()); // Table conflict
}

#[test]
fn msgpack_deep_error_branches() {
    use nextjson::formats::MsgPack;
    assert!(MsgPack.decode::<Value>(&[0x81, 0xa1, b'k']).is_err()); // map1 missing key value
    assert!(MsgPack.decode::<Value>(&[0x92, 0x01]).is_err()); // array2 is missing elements
    assert!(MsgPack.decode::<Value>(&[0xdc, 0x00, 0x02, 0x01]).is_err()); // array16 is missing elements
    assert!(MsgPack.decode::<Value>(&[0xdd, 0x00]).is_err()); // array32 is out of length
    assert!(MsgPack
        .decode::<Value>(&[0xde, 0x00, 0x02, 0xa1, b'k'])
        .is_err()); // map16 Missing Values
    assert!(MsgPack.decode::<Value>(&[0xdb, 0x00]).is_err()); // str32 is out of length
    assert!(MsgPack.decode::<Value>(&[0xc5, 0x00]).is_err()); // bin16 is out of length
    assert!(MsgPack.decode::<Value>(&[0xcb, 0x00]).is_err()); // f64 is missing 8 bytes
    assert!(MsgPack.decode::<Value>(&[0xd3, 0x00]).is_err()); // i64 is missing 8 bytes
    assert!(MsgPack.decode::<Value>(&[0xa1, 0xff]).is_err()); // str is an illegal UTF-8
    assert_eq!(
        MsgPack.decode::<Value>(&[0xff]).unwrap(),
        Value::from(-1_i64)
    );
    assert_eq!(
        MsgPack.decode::<Value>(&[0xe0]).unwrap(),
        Value::from(-32_i64)
    );
    let deep = vec![0x91u8; 200];
    assert!(MsgPack.decode::<Value>(&deep).is_err()); // Depth Exceeded
    let bin = nextjson::formats::MsgPack
        .encode(&nextjson::Bytes(b"\x01\x02\x03"))
        .unwrap();
    let back: nextjson::Bytes<'_> = MsgPack.decode(&bin).unwrap();
    assert_eq!(back.as_bytes(), b"\x01\x02\x03");
}

#[test]
fn json5_syntax_and_error_branches() {
    use nextjson::formats::Json5;
    let v: Value = Json5.decode(b"{ // comment\n key: 'v', }").unwrap();
    assert_eq!(v["key"], Value::from("v"));
    let v: Value = Json5.decode(b"0x1F").unwrap();
    assert_eq!(v, Value::from(31_i64));
    let v: Value = Json5.decode(b"1e3").unwrap();
    assert_eq!(v.as_f64(), Some(1000.0));
    let v: Value = Json5.decode(b"Infinity").unwrap();
    assert!(v.is_number());
    // 错误分支。
    assert!(Json5.decode::<Value>(b"{\"a\": }").is_err()); // Missing Values
    assert!(Json5.decode::<Value>(b"\"unterminated").is_err()); // Unclosed string
    assert!(Json5.decode::<Value>(b"'\\u12'").is_err()); // truncation of Unicode escape sequences
    assert!(Json5.decode::<Value>(b"1e").is_err()); // Index missing numbers
    assert!(Json5.decode::<Value>(b"[1, 2").is_err()); // Unclosed array
    assert!(Json5.decode::<Value>(b"0x").is_err()); // Hexadecimal truncation
    assert!(Json5.decode::<Value>(b"-").is_err()); // Isolated minus sign
}

#[test]
fn yaml_flow_and_error_branches() {
    use nextjson::formats::Yaml;
    // 合法分支：流式、锚点式、多行。
    let v: Value = Yaml.decode(b"a: {b: 1, c: [1, 2]}").unwrap();
    assert_eq!(v["a"]["b"], Value::from(1_i64));
    let v: Value = Yaml.decode(b"- one\n- two\n").unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
    // 错误分支。
    assert!(Yaml.decode::<Value>(b"a: [1, 2").is_err()); // Unclosed flow cytometry
    assert!(Yaml.decode::<Value>(b"{a: 1").is_err()); // Unclosed flow objects
    assert!(Yaml.decode::<Value>(b"a:").is_err()); // Missing values
    assert!(Yaml.decode::<Value>(b"a: 1\n- b").is_err()); // Mixed indentation error
}

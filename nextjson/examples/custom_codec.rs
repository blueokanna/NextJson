//! 手写编解码器：不依赖 derive 的 `NsonSchema` / `NsonSerialize` /
//! `NsonDeserialize` 实现，以及 `#[njson(with = "...")]` 字段级定制（示例 6/6）。
//!
//! 运行：`cargo run -p nextjson --example custom_codec`
//!
//! 某些类型无法用 derive 表达（例如"以十六进制字符串上线的字节串"）。
//! 此时可以手写实现：
//! 1. `NsonSchema::SCHEMA`（编译期描述，驱动校验/JSON Schema/兼容性检查）；
//! 2. `NsonSerialize::nextencode`（事件流编码，对任意 `FormatEncoder` 通用）；
//! 3. `NsonDeserialize::nextdecode_into`（通过 `DecodeSlot` 就地写入）。
//!
//! 同一份实现即对所有 14 种格式通用——这正是统一 token 流的价值。
//! 另外演示 `with` 模块：在 derive 的结构体上按字段挂接自定义编解码。

use nextjson::formats::Format;
use nextjson::{
    DecodeSlot, FormatDecoder, FormatEncoder, FormatError, NsonDeserialize, NsonSchema,
    NsonSerialize, TypeSchema,
};

/// 以十六进制字符串上线的字节串包装。
#[derive(Clone, Debug, PartialEq, Eq)]
struct HexBytes(Vec<u8>);

// ---- 十六进制工具（示例自带，零依赖） ----

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    // 位运算判断偶数长度，避免 `% 2`（clippy 建议的 is_multiple_of 需要
    // Rust 1.87+，本项目 MSRV 1.78 不可用）。
    if s.len() & 1 != 0 {
        return Err("hex 长度必须为偶数".into());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = nibble(bytes[i]).ok_or_else(|| format!("非法 hex 字符: {}", bytes[i] as char))?;
        let lo = nibble(bytes[i + 1])
            .ok_or_else(|| format!("非法 hex 字符: {}", bytes[i + 1] as char))?;
        out.push(hi << 4 | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---- 手写实现 ----

impl NsonSchema for HexBytes {
    const SCHEMA: TypeSchema = TypeSchema::Str;
}

impl NsonSerialize for HexBytes {
    fn nextencode<E: FormatEncoder>(&self, encoder: &mut E) -> Result<(), E::Error> {
        encoder.write_str(&encode_hex(&self.0))
    }
}

impl<'de> NsonDeserialize<'de> for HexBytes {
    fn nextdecode_into<D: FormatDecoder<'de>>(
        decoder: &mut D,
        out: &mut DecodeSlot<Self>,
    ) -> Result<(), D::Error> {
        let s = decoder.string()?;
        let bytes = decode_hex(&s).map_err(D::Error::custom)?;
        out.write(HexBytes(bytes));
        Ok(())
    }
}

// ---- with 模块：derive 结构体按字段挂接自定义编解码 ----

mod hex_module {
    /// 序列化 `&Vec<u8>` 为十六进制字符串。
    pub fn serialize<E: nextjson::FormatEncoder>(
        value: &[u8],
        encoder: &mut E,
    ) -> Result<(), E::Error> {
        encoder.write_str(&super::encode_hex(value))
    }

    /// 反序列化十六进制字符串为 `Vec<u8>`。
    pub fn deserialize<'de, D: nextjson::FormatDecoder<'de>>(
        decoder: &mut D,
    ) -> Result<Vec<u8>, D::Error> {
        let s = decoder.string()?;
        super::decode_hex(&s).map_err(<D::Error as nextjson::FormatError>::custom)
    }
}

/// derive 结构体：`digest` 字段用 `with` 挂接自定义编解码。
#[derive(Clone, Debug, PartialEq, Eq, NsonSerialize, NsonDeserialize)]
struct Packet {
    #[njson(with = "hex_module")]
    digest: Vec<u8>,
    seq: u32,
}

fn main() -> nextjson::Result<()> {
    // ---- 手写类型：JSON 上线 = 十六进制字符串 ----
    let blob = HexBytes(vec![0xde, 0xad, 0xbe, 0xef, 0x01, 0x02]);
    let json = nextjson::to_string(&blob)?;
    println!("HexBytes as JSON: {json}");

    let back: HexBytes = nextjson::from_str(&json)?;
    assert_eq!(back, blob);
    println!("JSON 往返一致: {:?}", back.0);

    // 同一类型直接切到二进制格式（统一 token 流的通用性）。
    let cbor = nextjson::formats::Cbor.encode(&blob)?;
    let back2: HexBytes = nextjson::formats::Cbor.decode(&cbor)?;
    assert_eq!(back2, blob);
    println!("CBOR 往返一致: {:?}", back2.0);

    // 非法输入必须报错（不是 panic）。
    let bad = r#""xyz""#;
    let err = nextjson::from_str::<HexBytes>(bad).unwrap_err();
    println!("非法 hex 输入被拒绝: {err}");

    // ---- with 模块 ----
    let packet = Packet {
        digest: vec![0x00, 0xff, 0x10],
        seq: 3,
    };
    let pj = nextjson::to_string_pretty(&packet)?;
    println!("\nPacket as JSON:\n{pj}");
    let pback: Packet = nextjson::from_str(r#"{"digest":"00ff10","seq":3}"#)?;
    assert_eq!(pback, packet);
    println!("Packet(with) 往返一致");

    // 导出 JSON Schema：手写类型与 derive 类型都能生成。
    let schema = nextjson::to_json_schema::<Packet>();
    println!("\nPacket JSON Schema: {}", nextjson::to_string(&schema)?);

    Ok(())
}

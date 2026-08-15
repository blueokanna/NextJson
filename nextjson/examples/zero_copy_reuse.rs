//! 零拷贝与解码存储复用（示例 4/6）。
//!
//! 运行：`cargo run -p nextjson --example zero_copy_reuse`
//!
//! 演示 NextJson 的第三根支柱——reuse-first：
//!
//! 1. **借用输入**：`&str` / `Bytes` 字段直接从输入切片借用（未转义字符串
//!    零分配），通过指针范围断言证明没有发生拷贝；
//! 2. **`DecodeSlot` 就地解码**：`nextdecode_into` 把值写进调用方提供的槽，
//!    在持续解码循环里复用同一个槽与缓冲区，避免每次消息重建存储。

use nextjson::formats::Format;
use nextjson::{Bytes, DecodeSlot, Decoder, NsonDeserialize, NsonSerialize};

/// 借用型结构：`event` 与 `payload` 的生命周期与输入切片绑定。
/// `#[njson(borrow)]` 让 derive 生成 `'de: 'a` 边界（serde 的 `#[serde(borrow)]` 语义）。
#[derive(Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Header<'a> {
    #[njson(borrow)]
    event: &'a str,
    #[njson(borrow)]
    payload: Bytes<'a>,
    seq: u64,
}

/// 拥有型等价物：可在任意格式（包括经 Value 中继的二进制格式）往返。
/// 借用只对直接词法解析的 JSON 解码器成立——经 CBOR/MsgPack 中继的路径
/// 必然经过一颗拥有的 Value 树，因此借用字段无法在其中存活。
#[derive(Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct HeaderOwned {
    event: String,
    payload: Vec<u8>,
    seq: u64,
}

fn main() -> nextjson::Result<()> {
    // ---- 1. 零拷贝借用 ----
    // payload 用未转义字符串书写：JSON 解码器对未转义字符串返回 Cow::Borrowed，
    // 从而 `Bytes` 能直接借用输入切片（若写成 `[10,20,30,40]` 数组则必然拷贝，
    // 与 serde 的 `&[u8]` 借用语义一致）。
    let input = br#"{"event":"market.tick","payload":"raw-bytes","seq":7}"#;
    let header: Header = nextjson::from_slice(input)?;
    println!(
        "借用解码: event = {:?}, payload = {:?}, seq = {}",
        header.event,
        header.payload.as_bytes(),
        header.seq
    );

    // 指针范围断言：event / payload 必须落在输入切片内（零拷贝的直接证据）。
    let input_start = input.as_ptr() as usize;
    let input_end = input_start + input.len();
    let event_ptr = header.event.as_ptr() as usize;
    let payload_ptr = header.payload.as_bytes().as_ptr() as usize;
    assert!(
        (input_start..input_end).contains(&event_ptr),
        "event 必须借用输入，不能拷贝"
    );
    assert!(
        (input_start..input_end).contains(&payload_ptr),
        "payload 必须借用输入，不能拷贝"
    );
    println!("指针断言通过: event 与 payload 均直接指向输入切片");

    // 二进制格式通过 Value 中继，无法借用——改用拥有型等价物往返，
    // 证明同一数据模型在 CBOR 中原生字节串无损。
    let owned = HeaderOwned {
        event: header.event.to_owned(),
        payload: header.payload.as_bytes().to_vec(),
        seq: header.seq,
    };
    let cbor = nextjson::formats::Cbor.encode(&owned)?;
    let back: HeaderOwned = nextjson::formats::Cbor.decode(&cbor)?;
    assert_eq!(back, owned);
    println!("拥有型等价物经 CBOR 原生字节串往返一致: {} B", cbor.len());

    // ---- 2. 就地解码 + 槽复用 ----
    // 持续解码场景：同一个槽反复用于下一条消息，不重新分配存储。
    let messages = [
        br#"{"event":"a","payload":"x","seq":1}"#.as_slice(),
        br#"{"event":"b","payload":"yy","seq":2}"#.as_slice(),
        br#"{"event":"c","payload":"zzz","seq":3}"#.as_slice(),
    ];

    // 复用同一个 DecodeSlot<Header>，逐条 nextdecode_into。
    let mut slot = DecodeSlot::<Header>::new();
    let mut decoded: Vec<Header> = Vec::new();
    for raw in &messages {
        let mut decoder = Decoder::new(raw);
        Header::nextdecode_into(&mut decoder, &mut slot)?;
        decoder.end()?;
        // take 取出本条，槽回归空，下一条继续复用。
        decoded.push(slot.take().expect("解码成功必须写入槽"));
    }
    assert_eq!(decoded[0].seq, 1);
    assert_eq!(decoded[1].event, "b");
    assert_eq!(decoded[2].payload.as_bytes(), b"zzz");
    println!(
        "槽复用: 3 条消息共用 1 个 DecodeSlot，逐条解码成功: {:?}",
        decoded.iter().map(|h| h.seq).collect::<Vec<_>>()
    );

    // 显式证明复用：一个槽连续解两条，且第二条完成后 is_initialized 恢复。
    let mut slot2 = DecodeSlot::<Header>::new();
    let mut d1 = Decoder::new(messages[0]);
    Header::nextdecode_into(&mut d1, &mut slot2)?;
    d1.end()?;
    let first = slot2.take().unwrap();
    let mut d2 = Decoder::new(messages[1]);
    Header::nextdecode_into(&mut d2, &mut slot2)?;
    d2.end()?;
    let second = slot2.take().unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    println!(
        "同一个槽连续承载两条消息: seq {} -> seq {}",
        first.seq, second.seq
    );

    Ok(())
}

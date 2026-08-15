//! 流式读取：从 `std::io::Read` 增量解码，边到边处理（示例 5/6）。
//!
//! 运行：`cargo run -p nextjson --example streaming_reader`
//!
//! 网络/管道场景里整段载荷往往不会一次性到齐。`StreamDecoder` 只按需向
//! `Read` 拉取字节，因此解码可以在数据尚未全部到达时就开始：
//! - `nextjson::from_reader`：顶层便捷入口（要求拥有型目标）；
//! - `nextjson::StreamDecoder`：手动控制解码进度。
//!
//! 示例用一个"每次最多吐 3 字节"的 `ChunkedReader` 模拟慢速 socket，
//! 证明解码不依赖整包到达。

use std::io::{Cursor, Read};

use nextjson::{from_reader, NsonDeserialize, NsonSerialize, StreamDecoder};

/// 拥有型消息（流式解码要求 `for<'de> NsonDeserialize<'de>`，不能借用输入）。
#[derive(Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Message {
    id: u64,
    body: String,
}

/// 慢速读取器：每次 `read` 最多返回 `chunk` 字节，模拟分片到达的网络流。
struct ChunkedReader<R> {
    inner: R,
    chunk: usize,
}

impl<R: Read> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = buf.len().min(self.chunk);
        let mut slice = buf;
        slice = &mut slice[..want];
        self.inner.read(slice)
    }
}

fn main() -> nextjson::Result<()> {
    let messages = vec![
        Message {
            id: 1,
            body: "first message".into(),
        },
        Message {
            id: 2,
            body: "second message".into(),
        },
        Message {
            id: 3,
            body: "third message".into(),
        },
    ];

    // 1. 顶层入口：把一条 JSON 载荷放进 Cursor，再包一层慢速读取器。
    let payload = nextjson::to_vec(&messages)?;
    println!("序列化载荷: {} 字节", payload.len());

    let slow = ChunkedReader {
        inner: Cursor::new(payload.clone()),
        chunk: 3,
    };
    let decoded: Vec<Message> = from_reader(slow)?;
    assert_eq!(decoded, messages);
    println!(
        "from_reader + 3 字节/次: 解码 {} 条消息，与源数据一致",
        decoded.len()
    );

    // 2. 手动控制：StreamDecoder 边到边读取单条消息。
    let single = Message {
        id: 42,
        body: "streamed".into(),
    };
    let one_payload = nextjson::to_vec(&single)?;
    let slow_one = ChunkedReader {
        inner: Cursor::new(one_payload),
        chunk: 2,
    };

    let mut decoder = StreamDecoder::new(slow_one);
    let value: Message = Message::nextdecode(&mut decoder)?;
    decoder.end()?;
    assert_eq!(value, single);
    println!("StreamDecoder + 2 字节/次: 解码 msg id = {}", value.id);

    // 3. 流式容器：数组到达时逐条解码（消息没有一次性到齐也没关系）。
    let mut full = Cursor::new(nextjson::to_vec(&messages)?);
    let mut d = StreamDecoder::new(&mut full);
    let streamed: Vec<Message> = Vec::<Message>::nextdecode(&mut d)?;
    d.end()?;
    assert_eq!(streamed, messages);
    println!(
        "流式解码整个数组: {} 条消息，首条 id = {}",
        streamed.len(),
        streamed[0].id
    );

    Ok(())
}

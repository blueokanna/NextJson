//! 流式跨格式中继：JSON ⇄ CBOR，不构造中间 `Value`（示例 3/6）。
//!
//! 运行：`cargo run -p nextjson --example cross_format_relay`
//!
//! `nextjson::cross_format` 以事件流（`EventSink`）的形式在两种格式之间
//! 逐 token 转写：源格式解码一个 token 就立刻把它交给目标格式的编码器，
//! 中间不分配整棵值树。适合"网关/代理/协议转换"这类低延迟、低内存场景。
//!
//! 关键点：
//! - 中转结果与"先解码成 `Value` 再编码"在语义上完全一致；
//! - `*_writer` 变体把输出直接写进调用方提供的缓冲区，可完全避免中转分配。

use nextjson::cross_format;

fn main() -> nextjson::Result<()> {
    // 源数据：一段 JSON。
    let source = br#"{
        "order_id": "ord_1024",
        "customer": "Ada Lovelace",
        "items": [
            {"sku": "A-1", "qty": 2, "price": 9.99},
            {"sku": "B-7", "qty": 1, "price": 149.5}
        ],
        "paid": true,
        "notes": null
    }"#;

    // 1. JSON -> CBOR（输出到新分配的 Vec）。
    let cbor = cross_format::json_to_cbor(source)?;
    println!("json_to_cbor:   {} B -> {} B", source.len(), cbor.len());

    // 2. CBOR -> JSON（输出到新分配的 Vec）。
    let json_round = cross_format::cbor_to_json(&cbor)?;
    println!("cbor_to_json:   {} B -> {} B", cbor.len(), json_round.len());

    // 3. 语义一致性：中继往返后的 JSON 与直接解析等价。
    //    用 Value 比较（键顺序、数字归一化后语义相等即可）。
    let a: nextjson::Value = nextjson::from_slice(source)?;
    let b: nextjson::Value = nextjson::from_slice(&json_round)?;
    assert_eq!(a, b, "中继必须保持数据语义");
    println!("中继往返数据语义一致: Value 相等");

    // 4. writer 变体：输出直接写入调用方缓冲区，零中转分配。
    let mut out_cbor = Vec::new();
    cross_format::json_to_cbor_writer(source, &mut out_cbor)?;
    assert_eq!(out_cbor, cbor, "writer 变体与分配版本逐字节一致");

    let mut out_json = Vec::new();
    cross_format::cbor_to_json_writer(&out_cbor, &mut out_json)?;
    assert_eq!(out_json, json_round, "writer 变体与分配版本逐字节一致");
    println!("writer 变体输出与分配版本逐字节一致");

    // 5. 漂亮的 JSON 输出（如用于日志/调试）。
    let pretty = cross_format::cbor_to_json_pretty(&cbor)?;
    let text = String::from_utf8(pretty).expect("cbor_to_json_pretty 输出 UTF-8");
    println!("\n--- cbor_to_json_pretty 输出 ---\n{text}");

    // 6. 批量中继：构造一个合法的 JSON 数组（100 条订单对象），
    //    再整体转成紧凑的 CBOR，展示网关场景的字节节省。
    println!("\n== 批量 JSON -> CBOR 体积对比 ==");
    let mut batch = Vec::new();
    batch.extend_from_slice(b"[");
    for i in 0..100 {
        if i > 0 {
            batch.push(b',');
        }
        batch.extend_from_slice(source);
    }
    batch.push(b']');
    let batch_cbor = cross_format::json_to_cbor(&batch)?;
    println!(
        "100 条订单 JSON: {} B -> CBOR: {} B ({:.1}%)",
        batch.len(),
        batch_cbor.len(),
        batch_cbor.len() as f64 / batch.len() as f64 * 100.0
    );

    Ok(())
}

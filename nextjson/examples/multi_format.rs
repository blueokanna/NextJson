//! 多格式引擎：同一个值走遍 14 种线格式（示例 2/6）。
//!
//! 运行：`cargo run -p nextjson --example multi_format`
//!
//! 演示 NextJson 的第二根支柱——多格式是一等能力：一份类型定义，通过
//! `nextjson::formats` 里统一的 `Format` trait 即可在任意线格式之间切换，
//! 无需为每种格式编写单独的序列化逻辑。输出：每种格式的编码体积、
//! 往返（round-trip）是否逐字节相等，以及跨格式转码链。
//!
//! 说明：某些格式对数据模型有硬约束（TOML/BSON 需要表根、bencode 无浮点、
//! postcard 拒绝有符号标量、CSV 只支持扁平行），示例用与 bench 相同的
//! 包装类型如实呈现这些约束，而不是伪造"所有格式都装得下"。

use nextjson::formats::Format;
use nextjson::{from_str, nextdecode, nextencode, NsonDeserialize, NsonSerialize};

/// 全模型记录：float + bool + 嵌套容器，9 种格式可用。
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Record {
    id: u64,
    active: bool,
    score: f64,
    name: String,
    tags: Vec<String>,
    samples: Vec<i64>,
}

/// 无浮点记录：bencode 可用。
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct IntRecord {
    id: u64,
    count: i64,
    name: String,
    tags: Vec<String>,
}

/// 仅无符号记录：postcard 拒绝有符号标量。
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct UintRecord {
    id: u64,
    count: u64,
    name: String,
    tags: Vec<String>,
}

/// 文档根：TOML / BSON 要求顶层是表而不是数组。
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct Doc<T> {
    records: Vec<T>,
}

/// 扁平行：CSV 只支持行式文本。
#[derive(Clone, Debug, PartialEq, NsonSerialize, NsonDeserialize)]
struct FlatRow {
    id: u64,
    active: bool,
    score: f64,
    name: String,
}

fn records() -> Vec<Record> {
    (0..128)
        .map(|index| Record {
            id: index,
            active: index % 3 != 0,
            score: index as f64 * 1.25 - 17.5,
            name: format!("record-{index:04}"),
            tags: vec![
                "json".into(),
                "zero-copy".into(),
                format!("group-{}", index % 8),
            ],
            samples: (0..16).map(|sample| index as i64 * 31 - sample).collect(),
        })
        .collect()
}

fn main() -> nextjson::Result<()> {
    let data = records();
    let doc = Doc {
        records: data.clone(),
    };
    let ints: Vec<IntRecord> = data
        .iter()
        .map(|r| IntRecord {
            id: r.id,
            count: r.id as i64 * 7 - 3,
            name: r.name.clone(),
            tags: r.tags.clone(),
        })
        .collect();
    let uints: Vec<UintRecord> = ints
        .iter()
        .map(|r| UintRecord {
            id: r.id,
            count: r.count.max(0) as u64,
            name: r.name.clone(),
            tags: r.tags.clone(),
        })
        .collect();
    let rows: Vec<FlatRow> = (0..128)
        .map(|i| FlatRow {
            id: i,
            active: i % 2 == 0,
            score: i as f64 * 0.5,
            name: format!("row-{i:04}"),
        })
        .collect();

    println!("{:<10} {:>10}  round-trip", "format", "size(B)");
    println!("{}", "-".repeat(44));

    // 全模型格式。
    macro_rules! bench_full {
        ($($mod:ident),* $(,)?) => {
            $(
                let bytes = nextjson::formats::$mod.encode(&data)?;
                let back: Vec<Record> = nextjson::formats::$mod.decode(&bytes)?;
                println!(
                    "{:<10} {:>10}  {}",
                    <nextjson::formats::$mod as Format>::NAME,
                    bytes.len(),
                    if back == data { "ok" } else { "MISMATCH" }
                );
            )*
        };
    }
    bench_full!(Json, Json5, Hjson, Yaml, Ron, Sexpr, Cbor, MsgPack, Pickle);

    // 文档根格式。
    macro_rules! bench_doc {
        ($($mod:ident),* $(,)?) => {
            $(
                let bytes = nextjson::formats::$mod.encode(&doc)?;
                let back: Doc<Record> = nextjson::formats::$mod.decode(&bytes)?;
                println!(
                    "{:<10} {:>10}  {}",
                    <nextjson::formats::$mod as Format>::NAME,
                    bytes.len(),
                    if back == doc { "ok" } else { "MISMATCH" }
                );
            )*
        };
    }
    bench_doc!(Toml, Bson);

    // 约束型格式：如实使用各自的 fixture。
    let b = nextjson::formats::Bencode.encode(&ints)?;
    let back: Vec<IntRecord> = nextjson::formats::Bencode.decode(&b)?;
    println!(
        "{:<10} {:>10}  {}",
        <nextjson::formats::Bencode as Format>::NAME,
        b.len(),
        if back == ints { "ok" } else { "MISMATCH" }
    );

    let b = nextjson::formats::Postcard.encode(&uints)?;
    let back: Vec<UintRecord> = nextjson::formats::Postcard.decode(&b)?;
    println!(
        "{:<10} {:>10}  {}",
        <nextjson::formats::Postcard as Format>::NAME,
        b.len(),
        if back == uints { "ok" } else { "MISMATCH" }
    );

    let b = nextjson::formats::Csv.encode(&rows)?;
    let back: Vec<FlatRow> = nextjson::formats::Csv.decode(&b)?;
    println!(
        "{:<10} {:>10}  {}",
        <nextjson::formats::Csv as Format>::NAME,
        b.len(),
        if back == rows { "ok" } else { "MISMATCH" }
    );

    // 跨格式转码：JSON -> MsgPack -> CBOR -> JSON，全程不出现中间类型。
    println!("\n== 跨格式转码链（不构造中间类型） ==");
    let json_bytes = nextjson::formats::Json.encode(&data)?;
    let msgpack = nextjson::formats::transcode(
        &json_bytes,
        nextjson::formats::Json,
        nextjson::formats::MsgPack,
    )?;
    let cbor = nextjson::formats::transcode(
        &msgpack,
        nextjson::formats::MsgPack,
        nextjson::formats::Cbor,
    )?;
    let json_again =
        nextjson::formats::transcode(&cbor, nextjson::formats::Cbor, nextjson::formats::Json)?;
    let first: Vec<Record> = nextjson::formats::Json.decode(&json_bytes)?;
    let last: Vec<Record> = nextjson::formats::Json.decode(&json_again)?;
    println!(
        "json {:<6} -> msgpack {:<6} -> cbor {:<6} -> json {:<6}  data_equal = {}",
        json_bytes.len(),
        msgpack.len(),
        cbor.len(),
        json_again.len(),
        first == last
    );

    // 二进制格式对同一数据的体积优势（对比演示）。
    println!("\n== 体积对比（同一条 128 条记录数据集） ==");
    let sizes = [
        ("json", nextjson::formats::Json.encode(&data)?.len()),
        ("cbor", nextjson::formats::Cbor.encode(&data)?.len()),
        ("msgpack", nextjson::formats::MsgPack.encode(&data)?.len()),
        ("pickle", nextjson::formats::Pickle.encode(&data)?.len()),
    ];
    let base = sizes[0].1 as f64;
    for (name, size) in sizes {
        println!(
            "  {name:<8} {size:>8} B  ({:.1}% of json)",
            size as f64 / base * 100.0
        );
    }

    // 自检：native 路径严格往返。
    assert_eq!(nextdecode::<Vec<Record>>(&nextencode(&data)?)?, data);
    // 顺带演示从字符串直接解析。
    let one: Record =
        from_str(r#"{"id":1,"active":true,"score":1.25,"name":"r","tags":[],"samples":[]}"#)?;
    println!("\n单条 JSON 解析: id = {}", one.id);

    Ok(())
}

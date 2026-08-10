# NextJson

## English Documentation - [中文文档](https://github.com/blueokanna/NextJson/blob/main/README_CN.md)

A production-oriented, zero-third-party-crate Rust JSON/CBOR library with
`no_std + alloc` support.

### Guarantees

- The workspace contains only the local `nextjson` and `nextjson-derive` crates.
- The only dependency entry is the local workspace derive crate. There are no
  registry, Git, or external path dependencies.
- The derive crate uses only Rust's standard `proc_macro` API.
- The core enables `no_std`, denies unsafe code, and denies missing docs.
- Native contracts are named `nextencode`, `nextdecode`, and `nextdecode_into`.
- Unescaped JSON strings and definite CBOR text strings can borrow input bytes.
- JSON/CBOR conversion relays events without constructing an intermediate tree.

Audit the complete build graph directly:

```text
cargo tree --workspace --all-features --edges normal,build,dev
```

The output must contain only the two local packages:

```text
nextjson
└── nextjson-derive (local workspace proc-macro)
nextjson-derive
```

### Installation

Default `std + derive` configuration:

```toml
[dependencies]
nextjson = "0.1"
```

Core `no_std + alloc` configuration:

```toml
[dependencies]
nextjson = { version = "0.1", default-features = false }
```

Enable the repository-owned derive macros without enabling `std`:

```toml
[dependencies]
nextjson = { version = "0.1", default-features = false, features = ["derive"] }
```

| Feature  | Default | Purpose                                                        |
| -------- | ------: | -------------------------------------------------------------- |
| `std`    |     yes | Standard I/O adapters and standard-library-specific types      |
| `derive` |     yes | Repository-owned `NsonSerialize` and `NsonDeserialize` derives |

### Native API

`nextencode(&value)` returns compact JSON bytes. `nextdecode(input)` decodes one
complete JSON value and rejects trailing data. Writer, reader, pretty-print,
dynamic `Value`, JSON macro, schema inspection, and JSON Schema APIs remain
available as focused helpers.

```rust
use nextjson::{NsonDeserialize, NsonSerialize};

#[derive(Debug, PartialEq, NsonSerialize, NsonDeserialize)]
#[njson(rename_all = "camelCase")]
struct User {
    user_id: u64,
    name: String,
    #[njson(default)]
    tags: Vec<String>,
}

let expected = User {
    user_id: 7,
    name: "Ada".into(),
    tags: vec!["compiler".into()],
};

let bytes = nextjson::nextencode(&expected)?;
let actual: User = nextjson::nextdecode(&bytes)?;
assert_eq!(actual, expected);
# Ok::<(), nextjson::Error>(())
```

### Cross-format architecture

`cross_format::EventSink` is NextJson's own dependency-free structural
protocol. `json_into` and `cbor_into` are sources; `JsonSink` and `CborSink` are
destinations. Both directions validate event order and nesting. The built-in
CBOR implementation supports an RFC 8949 JSON-compatible profile, including
128-bit bignums and finite IEEE floats.

```rust
use nextjson::cross_format;

let json = br#"{"name":"NextJson","values":[1,2,3],"ok":true}"#;
let cbor = cross_format::json_to_cbor(json)?;
let json_again = cross_format::cbor_to_json(&cbor)?;

let left: nextjson::Value = nextjson::nextdecode(json)?;
let right: nextjson::Value = nextjson::nextdecode(&json_again)?;
assert_eq!(left, right);
# Ok::<(), nextjson::Error>(())
```

| API                                    | Purpose                                                |
| -------------------------------------- | ------------------------------------------------------ |
| `json_into`                            | Relay JSON input into any repository-owned `EventSink` |
| `cbor_into`                            | Relay CBOR input into any repository-owned `EventSink` |
| `json_to_cbor` / `json_to_cbor_writer` | Stream JSON into CBOR                                  |
| `cbor_to_json` / `cbor_to_json_writer` | Stream CBOR into JSON                                  |
| `cbor_to_json_pretty`                  | Stream CBOR into formatted JSON                        |

The CBOR profile accepts definite and indefinite arrays, maps, and text;
`u64`/`i64` major types; tag 2/tag 3 bignums for exact `u128`/`i128` values;
and finite half-, single-, and double-precision floats. Map keys must be UTF-8
text.

The profile intentionally rejects values that JSON cannot preserve: arbitrary
byte strings, non-text map keys, non-finite floats, and unknown semantic tags.
No lossy fallback is performed.

### Zero-copy scope

Zero-copy applies when source bytes are already the target UTF-8 string:
unescaped JSON strings and definite CBOR text. Escaped JSON strings and
indefinite CBOR text require materialization. Output encoding necessarily writes
new bytes to its destination. These boundaries are tested with pointer-range
assertions.

### Derives and schemas

The repository-owned derives support structs, tuple structs, generics, const
generics, and external, internal, adjacent, or untagged enum representations.
Container attributes include `rename_all`, `tag`, `content`, `untagged`,
`deny_unknown_fields`, `default`, `transparent`, `crate`, and `bound`. Field
attributes include `rename`, `alias`, `default`, `skip`, directional skips,
`skip_serializing_if`, `flatten`, `borrow`, `with`, `serialize_with`, and
`deserialize_with`. Variant attributes include `rename`, `alias`, `skip`, and
`other`.

Every derived type also exposes a `const SCHEMA: TypeSchema`:

```rust
# use nextjson::{NsonDeserialize, NsonSerialize};
#[derive(NsonSerialize, NsonDeserialize)]
struct Point { x: i32, y: i32 }

let schema = nextjson::schema_of::<Point>();
let json_schema = nextjson::to_json_schema::<Point>();
# let _ = (schema, json_schema);
```

### Safety and limits

The library contains no unsafe Rust. Checked decode slots prevent an invalid
custom implementation from exposing uninitialized memory. Nesting is bounded;
numeric conversions are checked; malformed UTF-8, syntax, trailing input, and
unrepresentable cross-format values are errors. Applications must still impose
deployment-specific byte, collection, time, and output limits.

Reader APIs buffer the complete input, so servers must enforce an input-byte
limit at the transport boundary. The default JSON and CBOR nesting limit is 128. See the [Safety Model](https://github.com/blueokanna/NextJson/blob/main/docs/SAFETY.md) for the auditable invariants and remaining
application responsibilities.

### Benchmark

The repository-owned benchmark runs four paths over the same 128-record
fixture: native JSON `nextencode`, native JSON `nextdecode`, JSON to CBOR, and
CBOR to JSON. It imports no comparison library and does not claim universal
superiority.

```text
cargo bench --locked -p nextjson --bench format_comparison
```

See the [Reproducible Benchmark](https://github.com/blueokanna/NextJson/blob/main/docs/BENCHMARKS.md) for the fixture, measurement method, output
format, and reporting requirements.

### Reproducibility

Use the committed lock file and run:

```text
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check -p nextjson --no-default-features --locked
cargo doc --workspace --all-features --no-deps --locked
cargo tree --workspace --all-features --edges normal,build,dev
```

The lock file must contain only the two local packages. Benchmark reports must
include CPU, OS, Rust version, measurement duration, and every output row.
Results from one fixture or machine are not evidence of universal performance.

## License

Apache-2.0

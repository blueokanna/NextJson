# NextJson

## English Documentation - [中文文档](https://github.com/blueokanna/NextJson/blob/main/README_CN.md)

## Wiki

The repository Wiki is published from the `/wiki` directory:
[GitHub Wiki](https://github.com/blueokanna/NextJson/wiki)

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

The design is **data-model-first** (not AST-first): typed decode streams
directly into your fields with zero intermediate tree, and `Value` is an
opt-in consumer of the same decoder. Two encoder policies are exposed:
`Encoder` validates the event protocol on every call, while `FastEncoder`
(the `nextencode` / `to_vec` / `to_string` / writer entry points) trusts the
derive-verified call sequence and skips per-value checks for ~2x encoding
throughput. See [Design](docs/DESIGN.md) for the fork analysis, the
borrowing model (transient / owned / borrowed), and the attribute / policy
layers.

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

### Multi-format engine

`nextjson::formats` is a dependency-free, format-neutral codec engine. The
crate's own `NsonSerialize` / `NsonDeserialize` contracts are generic over
`FormatEncoder` / `FormatDecoder`, so one implementation serves every format
whose wire model can represent that value. Most encoders emit directly;
document-shaped TOML and YAML collect a `Value` first so tables can be ordered
correctly. Unsupported combinations return errors listed in the matrix below.

Event-order validation is centralized: the format encoders and the
cross-format sinks drive one shared protocol state machine, parameterized
only by whether the wire format has explicit array separators (JSON does,
CBOR does not). On the decode side, the byte lexer serves typed scalar reads
directly from the source byte, so the unified token stream stays available
for content replay without taxing the hot path.

```rust
use nextjson::formats;

let value = ("NextJson", vec![1_u64, 2, 3], true);

let json = formats::encode_with(&value, formats::Json)?;
let msgpack = formats::encode_with(&value, formats::MsgPack)?;
let yaml = formats::encode_with(&value, formats::Yaml)?;

let back: (String, Vec<u64>, bool) = formats::decode_with(&json, formats::Json)?;
assert_eq!(back, formats::decode_with(&msgpack, formats::MsgPack)?);
assert_eq!(back, formats::decode_with(&yaml, formats::Yaml)?);
# Ok::<(), nextjson::Error>(())
```

Sixteen formats are registered. Formats are first-class `Format` values with a
canonical name, MIME type, file extensions, and binary/text classification, so
they can be passed around, stored, or selected dynamically:

```rust
use nextjson::formats::{FormatKind, self};

let kind: Option<FormatKind> = formats::by_extension("toml");
let detected: Option<FormatKind> = formats::detect(br#"{"a":1}"#);
let json = formats::encode_with(&42_i64, formats::Json)?; // format by value
# let _ = (kind, detected, json);
```

| Group              | Formats                                                            |
| ------------------ | ------------------------------------------------------------------ |
| Text, self-descr.  | `json`, `json5`, `hjson`, `yaml`, `toml`, `ron`, `sexpr`, `csv`, `urlform` |
| Binary, self-descr.| `cbor`, `msgpack`, `bson`, `bencode`, `pickle`                     |
| Binary, schema-light | `postcard`                                                       |
| Environment        | `envy` (deserialization only, requires `std`)                      |

Transcoding between compatible format models needs no typed value:

```rust
use nextjson::formats;
let json = br#"{"name":"NextJson","values":[1,2,3]}"#;
let msgpack = formats::transcode(json, formats::Json, formats::MsgPack)?;
let json2 = formats::transcode(&msgpack, formats::MsgPack, formats::Json)?;
assert_eq!(json2, json);
# Ok::<(), nextjson::Error>(())
```

#### Capability matrix (honest limits)

Every format implements the unified contract. Wire-model limits and deliberate
codec-subset limits are reported as errors instead of silent lossy fallback:

| Format | Scalars | Containers | Notes |
| ------ | ------- | ---------- | ----- |
| `json` | null/bool/int/float/str | array/object | RFC 8259; full model |
| `json5` | as JSON + `Infinity`/`NaN` | + comments, unquoted keys, single quotes, trailing commas | encoder emits strict JSON |
| `hjson` | as JSON | + unquoted keys/strings, comments | encoder emits strict JSON |
| `yaml` | null/bool/int/float/str | block + flow subset | block maps/sequences, `key: value`, `- `, `---`, `{…}`/`[…]`, block scalars `|`/`>` (with `-`/`+` chomping and indentation indicator) |
| `toml` | bool/int/float/str (no null) | tables, arrays, inline tables, multi-line strings | document-shaped: a bare scalar root is rejected; `"""`/`'''` multi-line strings with `\` continuation |
| `ron` | bool/int/float/str/char | map/seq/tuple/struct/enum | `Some(...)` wrappers round-trip |
| `sexpr` | atoms, quoted strings, numbers, `#t`/`#f`, `nil` | lists; maps as alists | schema-less nested-map `Value` decoding is ambiguous; use typed targets |
| `csv` | int/float/bool/str | rows; object rows with header | RFC 4180 |
| `urlform` | int/float/bool/str | flat key/value map only | RFC 3986 percent-encoding |
| `cbor` | null/bool/int/float/str | array/map | RFC 8949 JSON-compatible profile via event relay |
| `msgpack` | nil/bool/int/float/str | array/map | JSON-compatible scalar/container families; no bin/ext; 128-bit integers rejected when they do not fit 64-bit |
| `bson` | null/bool/int32/int64/double/str | document/array | document-shaped: a bare scalar root is rejected |
| `bencode` | int, UTF-8 strings | list/dict | canonical sorted keys; no null/float; bool maps to 1/0 |
| `postcard` | null/bool/unsigned int/str | seq/map | **non-self-describing**: signed integers, floats, `Option`, `Value`, and peek are rejected |
| `pickle` | `None`/bool/int/float/str | list/dict/tuple | CPython protocol 2 subset; 128-bit via `LONG1` |
| `envy` | int/float/bool/str | flat map (the environment) | deserialization only; `std` required |

`detect()` is heuristic and intentionally conservative: it claims only strong
structural signatures (pickle protocol header, bencode intro, BSON length
prefix, text-format ASCII starts, MessagePack/CBOR binary signatures) and
returns `None` for ambiguous input.

#### Cross-language compatibility

The codecs are verified with explicit foreign-wire fixtures, not only
self-round-trips: MessagePack bytes matching Python `msgpack`, CBOR bytes
matching Python `cbor2`, CPython 3 protocol-2 pickle bytes, canonical bencode,
MongoDB-style BSON documents, and hand-written TOML/YAML/RON/S-expression/
JSON5/Hjson inputs. See the `formats` integration tests for the exact bytes.

### Zero-copy scope

Zero-copy applies when source bytes are already the target UTF-8 string:
unescaped JSON strings and definite CBOR text. Escaped JSON strings and
indefinite CBOR text require materialization. Output encoding necessarily writes
new bytes to its destination. These boundaries are tested with pointer-range
assertions.

### Derives and schemas

The repository-owned derives support structs, tuple structs, generics, const
generics, and external, internal, adjacent, or untagged enum representations.
Container attributes include `rename_all` (including the directional
`serialize`/`deserialize` form), `tag`, `content`, `untagged`,
`deny_unknown_fields`, `default`, `transparent`, `crate`, `bound` (including
directional `bound(serialize=…, deserialize=…)`), `into`, `from`, `try_from`,
`remote`, and `expecting` (overrides the type description used in
  deserialization type-mismatch / length-mismatch error messages; the default
  is the type's fully qualified path). Field attributes include `rename`, `alias`,
`default`, `skip`, directional skips, `skip_serializing_if`, `flatten`,
`borrow`, `with`, `serialize_with`, `deserialize_with`, and `getter`. Variant
attributes include `rename`, `rename_all`, `skip`, and directional skips.
Attributes are accepted in `#[njson(...)]`, `#[nextjson(...)]`, or
`#[serde(...)]` form, so existing serde types migrate without rewriting their
attributes.

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

`from_slice` / `from_str` operate on a complete in-memory input; `from_reader`
(std) pulls incrementally from any `std::io::Read` source (see
`StreamDecoder`). The default JSON and CBOR nesting limit is 128. See the [Safety Model](https://github.com/blueokanna/NextJson/blob/main/docs/SAFETY.md) for the auditable invariants and remaining
application responsibilities.

### Benchmark

The repository-owned benchmark compares encode/decode throughput and encoded
size across the 14 wire formats that can represent the fixture (of the 16
registered: `envy` reads the process environment rather than a wire format,
and `urlform` only represents a flat map). It imports no comparison library
in the workspace and does not claim universal superiority.

```text
cargo bench --locked -p nextjson --bench format_comparison
```

An out-of-workspace crate (`benchmarks/serde-comparison/`) additionally
benchmarks the same fixture against `serde`/`serde_json` on shared hardware;
it keeps its own Cargo.lock so the workspace dependency audit stays intact.

```text
cd benchmarks/serde-comparison && cargo run --release
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

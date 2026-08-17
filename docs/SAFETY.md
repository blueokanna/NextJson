# Safety Model

NextJson enables `#![deny(unsafe_code)]` at the crate root. This is a
compiler-enforced code property, not a claim that arbitrary input can never
exhaust memory or CPU.

## Initialization and destruction

`NsonDeserialize::nextdecode_into` receives checked `DecodeSlot<T>` storage,
not a public uninitialized-memory contract. An implementation must call
`DecodeSlot::write` before it returns success; `nextdecode` checks this state.
Derived field slots use normal `Option<T>` drop semantics on errors, missing
fields, and duplicate-field replacement.

## Input guarantees

- JSON strings validate UTF-8, escapes, and surrogate pairs.
- CBOR text validates UTF-8, and map keys must be text.
- JSON and CBOR reject trailing root values and trailing bytes.
- Checked integer parsing covers the complete Rust `i128`/`u128` domain.
- CBOR tag 2/tag 3 values wider than 128 bits are errors.
- Non-finite JSON and CBOR floats are errors.
- Text decoders (YAML, TOML, RON, Hjson, S-expression, CSV, urlform) reject
  non-finite floats, including overflow spellings such as `1e999` that
  `str::parse::<f64>()` would accept as infinity.
- MessagePack is the one deliberate exception: its wire format can represent
  NaN / infinity losslessly, so non-finite floats pass through on the wire
  (matching the format's own semantics) and error only when relayed to a
  format that cannot represent them. Typed `f64` decode rejects them.
- Decoders reject nesting deeper than 128 levels by default.

## Multi-format guarantees

Every format in `nextjson::formats` serves the same checked event contract.
Format-specific guarantees:

- Length-prefixed binary formats (MessagePack, BSON, bencode, pickle) reject
  truncated headers and buffers that do not consume the declared length.
- BSON documents validate element type bytes and null-terminated field names,
  and reject a document whose declared length differs from the consumed input.
- Pickle executes a bounded stack-machine subset: `MARK` framing, stack depth,
  and long-integer (`LONG1`/`LONG4`) sizes are bounds-checked; unknown opcodes
  are errors.
- Postcard is non-self-describing; schema-less peeking is rejected because the
  wire cannot classify the next token without a target type.
- Bencode, TOML, and BSON reject values their wire models cannot represent
  (a bare scalar root for TOML/BSON; `null`/float for bencode). Bencode maps
  booleans explicitly to canonical integers `1` and `0`.
- The YAML, TOML, JSON5, and Hjson text parsers validate UTF-8 and reject
  unterminated strings, quotes, and block constructs.
- URL-form decoding validates percent-encoding (`%XX`) pairs; malformed
  escapes are errors.

No format performs a silent lossy fallback: a value that a wire format cannot
preserve is an explicit error on the `encode` or `decode` side.

## Cross-format guarantees

JSON and CBOR relay events through `EventSink` without building a document
tree. Structural state rejects multiple roots, mismatched containers, missing
object values, and keys outside objects. CBOR byte strings, non-text keys,
non-finite floats, and unknown tags fail instead of being converted lossily.
`formats::transcode` decodes to a `Value` and re-emits; the same no-lossy-
fallback rule applies to every source/destination pair.

## Zero-copy boundary

Unescaped JSON strings and definite CBOR text borrow their source range.
Escapes and indefinite text require owned materialization. Output always writes
new bytes.

## Resource limits

The nesting limit is not a total resource limit. The library does not impose a
single global limit on input bytes, string length, collection length, output
bytes, or execution time. Applications handling untrusted input must enforce
those quotas at transport and application boundaries. `from_reader` buffers its
complete reader and therefore requires a caller-provided bounded reader.

## Verification

```text
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check -p nextjson --no-default-features --locked
cargo doc --workspace --all-features --no-deps --locked
cargo tree --workspace --all-features --edges normal,build,dev
```

The regression suite covers malformed input, depth limits, integer boundaries,
partial-drop behavior, invalid custom decoders, writer failures, RFC 8949
fixtures, 128-bit bignums, structural event errors, pointer-level borrowing,
all registered format entry points, and explicit foreign-wire fixtures for the
formats named in the README. These checks are evidence for the covered
contracts, not a replacement for deployment-specific quotas or continuous
fuzzing.

## The opt-in `simd` feature

`simd` is the only feature that introduces `unsafe` into the crate. Its safety
model is deliberately narrow:

- **Containment.** The `unsafe` code lives in exactly one module,
  `nextjson/src/scan.rs`, and only under `cfg(feature = "simd")`. The module
  carries `#![cfg_attr(feature = "simd", allow(unsafe_code))]`; without the
  feature the crate keeps `#![deny(unsafe_code)]` crate-wide and the module is
  fully safe (portable SWAR only).
- **Bounds.** Every vector load is guarded by an explicit length check
  (`i + 16 <= len` for SSE2/NEON, `i + 32 <= len` for AVX2) before the load;
  shorter tails always fall back to the scalar reference scan. An
  out-of-bounds read is not possible.
- **Feature detection.** The AVX2 path is only entered after
  `std::is_x86_feature_detected!("avx2")` (a CPUID-backed check, `std` only).
  SSE2 and NEON are baseline on their respective architectures and need no
  detection. Unsupported targets simply use the portable path.
- **Correctness.** Every accelerated path is tested against a scalar reference
  implementation over all 256 byte values, all two-byte pairs, all lengths
  `0..=80` with every interesting byte at head/middle/tail, 1 MiB clean
  buffers, and 2000 deterministic pseudo-random buffers across every prefix
  length. The aarch64 NEON path resolves hit positions with the scalar
  reference (correct by construction) and asserts that the SIMD compare never
  disagrees with it.
- **Honest boundary.** The `unsafe` blocks are hand-written intrinsics, not
  `unsafe`-wrapped safe APIs from a third party. Review scope is the single
  module above.

## Safety comparison with serde

This section compares safety-relevant properties honestly. It is a *property*
comparison, not a claim that one library is universally safer — both are memory
safe under Rust's rules, and both rely on applications enforcing deployment
quotas.

| Property | serde / serde_json | nextjson |
| --- | --- | --- |
| `unsafe` code | serde uses internal `unsafe` (reflection, `RawValue`); serde_json float parsing historically used `unsafe` | default build is `#![deny(unsafe_code)]` with no `unsafe` in the crate; the opt-in `simd` feature confines hand-written intrinsics to the `scan` module (see above) |
| Compiler-enforced unsafe gate | none (unsafe is allowed) | `#![deny(unsafe_code)]` makes any future `unsafe` a compile error (with `simd` enabled, only the `scan` module is exempt) |
| Error model | `serde_json::Error` carries line/column; serde `Error` is opaque | `Error` carries line/column/offset and a coarse `classification()` |
| Recursion limits | serde_json has a recursion limit (128); serde core relies on serializer | all decoders cap nesting at 128 by default |
| Number overflow | serde_json returns overflow errors | checked `i128`/`u128` parsing with overflow errors |
| Non-finite floats (JSON) | serde_json emits `null` for `NaN`/`Infinity` unless feature flags | explicit error (no silent lossy fallback) |
| UTF-8 / surrogate validation | serde_json validates | validated in every string path |
| Partial-drop safety on derive errors | serde visitor pattern keeps state in locals | `InitSlot<T>` uses normal `Option<T>` drop semantics; duplicate fields are rejected and already initialized values are dropped |
| `no_std` | serde `no_std`; serde_json is `std`-only | core is `no_std + alloc`; only streaming IO is `std` |
| Zero-dependency build graph | serde is one dependency; ecosystem formats add many | the whole workspace has only the two local crates |
| Format-specific strictness | serde format crates vary (e.g., serde_json `RawValue`, YAML quirks) | every format rejects values its wire model cannot preserve — no silent lossy fallback |

What this table does *not* claim: it does not assert nextjson has fewer
long-tail bugs than a decade-old, community-fuzzed ecosystem, and it does not
replace external fuzzing or deployment quotas. The `unsafe`-free property (in
default builds) and the `deny(unsafe_code)` gate are the concrete, verifiable
differences.

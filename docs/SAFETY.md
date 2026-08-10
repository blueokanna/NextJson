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
- The default nesting limit is 128.

## Cross-format guarantees

JSON and CBOR relay events through `EventSink` without building a document
tree. Structural state rejects multiple roots, mismatched containers, missing
object values, and keys outside objects. CBOR byte strings, non-text keys,
non-finite floats, and unknown tags fail instead of being converted lossily.

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
fixtures, 128-bit bignums, structural event errors, and pointer-level borrowing.
These checks are evidence for the covered contracts, not a replacement for
deployment-specific quotas or future continuous fuzzing.

# Safety model

NextJson's default build denies unsafe Rust. The optional `simd` feature permits
intrinsics only in `src/scan.rs`; vector loads have length guards and AVX2 uses
runtime detection. These are reviewable implementation properties, not a claim
that the library has no bugs or needs no deployment testing.

## Decoding

`DecodeSlot<T>` and derived field slots use ordinary initialized Rust values.
Returning success without writing a slot is an error. Partially decoded values
are dropped through normal Rust ownership; no raw uninitialized storage is
exposed to a custom deserializer.

JSON validates UTF-8, escapes, surrogate pairs, number syntax, container syntax,
and the end of the document. Ignored fields still undergo syntax checks. Integer
parsing uses checked arithmetic through `i128` and `u128`. JSON and the CBOR
JSON-compatible profile reject non-finite floats. Other formats have distinct
data models: JSON5 accepts non-finite literals, MessagePack can carry non-finite
values, and CSV or URL-form cells can remain text. Consult the format matrix
before assuming a conversion preserves types or supports every wire feature.

Decoders default to a 128-level nesting limit. Raising a configurable decoder
limit requires evaluating stack use on the deployment target. Schema policies
are enforced by the validation APIs on decoded values; they do not automatically
limit allocations during parsing.

Lookahead is part of a saved parser position. JSON and streaming JSON save the
start of a pending token; native text and binary codecs also preserve required
container metadata. BSON saves a field's already-consumed type tag. Regressions
cover optional untagged variants and failed variant retries.

## Encoding

Public JSON helpers use the validated `Encoder`. CBOR and other format
helpers use the checked event wrapper. Invalid root counts, missing values,
mismatched containers, and incomplete event streams return errors. The JSON
encoder and checked wrapper reject container nesting beyond 128 levels before
descending further into a recursive serializer.

`FastEncoder` remains an explicit opt-in that omits per-value protocol checks.
It requires verifying every nested serializer and custom hook. Deriving the
outer type alone does not establish that condition. Historical unchecked
benchmarks do not measure the validated default API.

After an encoding or writer error, discard the encoder and any partial output.
Writing to an external sink is not transactional: bytes already delivered cannot
be rolled back. The nesting guard bounds container recursion, not arbitrary
recursion or work performed inside user-provided trait implementations.

## Streaming and resource limits

`from_reader` reads incrementally but retains consumed bytes for backtracking.
It is not a constant-memory API. Use `from_reader_with_limit(reader, max_bytes)`
or `StreamDecoder::with_input_limit` when input size is not already bounded.

The limit includes whitespace. At the boundary the reader may consume one
additional byte to distinguish EOF from oversized input; that byte is not
retained. Exceeding the limit returns `input byte limit exceeded` and cannot be
undone by restoring a parser mark. This avoids accepting a valid prefix from a
truncated stream. Interrupted reads are retried; other I/O failures propagate.

An input byte quota does not bound decoded allocations, output size, CPU time,
reader-internal buffering, or network wait time. Applications need limits for
their actual protocol and payloads. Allocation failure is not universally
recoverable through this library's `Result` type.

## Verification

```text
cargo test --workspace --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo check -p nextjson --no-default-features --locked
cargo +1.78.0 test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
```

`tests/production_regressions.rs` exercises chunk boundaries, malformed ignored
values, interrupted I/O, input quotas, lookahead/backtracking, invalid custom
serializers, deep encoding, and map consistency after a predicate panic. The
rest of the suite includes wire fixtures, numeric boundaries, partial drops,
and scalar-versus-SIMD comparisons. Passing these checks is evidence for the
tested cases; it is not an independent security audit or a substitute for
continuous fuzzing and deployment-specific load testing.

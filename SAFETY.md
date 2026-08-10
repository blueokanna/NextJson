# Safety model

NextJson forbids unsafe Rust in the library with `#![deny(unsafe_code)]`. This
is a compile-time property, not a claim that arbitrary input can never exhaust
time or memory.

## Decode contract

`NsonDeserialize::nextdecode_into` receives a `DecodeSlot<T>`, not a public
`MaybeUninit<T>`. A third-party implementation must call `DecodeSlot::write`
before returning success. `NsonDeserialize::nextdecode` checks this state and
returns an error when the contract is violated, so an incorrect implementation
cannot make safe library code read uninitialized memory.

Derived structs keep partially decoded fields in `InitSlot<T>`, which is backed
by `Option<T>`. Normal Rust drop semantics clean up initialized fields on every
error path and replace duplicate fields without leaks.

## Input guarantees

- JSON strings are validated as UTF-8. Unescaped strings borrow the source;
  escaped strings allocate decoded UTF-8.
- Integer parsing uses checked arithmetic through `u128`/`i128`; overflow is an
  error and is never rounded silently.
- Non-finite floats and finite literals outside `f64` range are rejected.
- Nesting is bounded by `DecodeConfig::max_depth` (128 by default).
- Trailing input, trailing commas, unescaped controls, invalid escapes, lone
  surrogates, and malformed numbers are rejected.

## Resource limits

Maximum depth does not limit total input bytes, collection length, string
length, or output size. Services handling untrusted traffic must enforce those
limits at the transport or application boundary. `from_reader` buffers the
whole reader and therefore requires an externally bounded reader.

## Verification

The regression suite contains fault-injection, partial-drop, malformed-input,
integer-boundary, recursion-limit, invalid custom nextdecode, and pointer-level
zero-copy tests. Run the same gates as CI:

```text
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check -p nextjson --no-default-features
cargo check -p nextjson --no-default-features --features serde
```

These checks are evidence for the covered contracts. They are not a substitute
for deployment-specific memory limits or future continuous fuzzing.

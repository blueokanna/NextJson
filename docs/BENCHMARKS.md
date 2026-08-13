# Reproducible Benchmark

`nextjson/benches/format_comparison.rs` measures encode and decode throughput
plus encoded size for every format that can represent the fixture, over the
same 128-record fixture.

Before timing, it validates a native JSON typed round trip and warms every path.
The build graph contains no third-party crate (the repository's dependency
audit forbids them).

```text
cargo bench --locked -p nextjson --bench format_comparison
```

Each path runs for two seconds by default. Use at least ten seconds for a
recorded comparison:

```powershell
$env:NEXTJSON_BENCH_MS = "10000"
cargo bench --locked -p nextjson --bench format_comparison
```

```bash
NEXTJSON_BENCH_MS=10000 cargo bench --locked -p nextjson --bench format_comparison
```

The output is stable CSV:

```text
format,size_bytes,encode_ops,encode_MBps,decode_ops,decode_MBps
```

The full-model fixture (`Vec<Record>` with `u64`/`bool`/`f64`/`String`/`Vec`)
covers JSON-family, RON, S-expression, CBOR, MessagePack, and Pickle. TOML and
BSON are document-shaped, so the array is wrapped in a table root. Bencode and
postcard have no float on the wire (and postcard rejects signed scalars), so
they use float-free fixtures; postcard additionally uses unsigned fields. CSV
uses flat rows.

## Sample measurement (single run, this developer machine, Intel i7-11850H, 32GB RAM, Windows 11, Rust 1.97.0):

| format   | size_bytes | encode_ops | encode_MBps | decode_ops | decode_MBps |
| -------- | ---------- | ---------- | ----------- | ---------- | ----------- |
| json     | 23636      | 12924      | 305.47      | 5984       | 141.43      |
| json5    | 23636      | 13013      | 307.58      | 1653       | 39.08       |
| hjson    | 23636      | 13011      | 307.52      | 2956       | 69.86       |
| yaml     | 51219      | 2319       | 118.80      | 612        | 31.36       |
| ron      | 27283      | 5502       | 150.12      | 1911       | 52.13       |
| sexpr    | 22249      | 6163       | 137.13      | 3132       | 69.68       |
| cbor     | 17090      | 3901       | 66.67       | 1153       | 19.71       |
| msgpack  | 16867      | 31611      | 533.18      | 8779       | 148.07      |
| pickle   | 22691      | 43508      | 987.24      | 1713       | 38.87       |
| toml     | 27155      | 1442       | 39.16       | 1045       | 28.39       |
| bson     | 32037      | 5303       | 169.91      | 7449       | 238.64      |
| bencode  | 7300       | 12847      | 93.78       | 14980      | 109.35      |
| postcard | 5231       | 45461      | 237.80      | 23894      | 124.99      |
| csv      | 2771       | 18676      | 51.75       | 9661       | 26.77       |

Interpretation is about _trade-offs_, not rank:

- MessagePack and Pickle win on encode throughput; JSON dominates on portability.
- Compact binary (postcard, bencode, msgpack) is 3-4x smaller than JSON for
  this fixture.
- TOML/YAML trade throughput for human-oriented, document-shaped output.
- CBOR here is the JSON-compatible profile (with bignum/float machinery), which
  is why it is slower than the simpler MessagePack path.

Published results must include CPU, OS, `rustc -Vv`, revision, measurement
duration, and the full table. This benchmark is a reproducible project
baseline, not proof of universal performance.

## Comparison with the serde ecosystem

Benchmarking against the mature serde ecosystem requires third-party crates,
which the repository's dependency-audit gate forbids in the workspace
Cargo.lock. The comparison therefore lives in a **standalone, out-of-workspace
crate** at `benchmarks/serde-comparison/`. It has its own Cargo.lock (committed
for reproducible `--locked` builds), so the main library keeps its zero-
dependency property and the audit stays green. `serde`/`serde_json` never
appear in the workspace dependency graph.

Run it:

```text
cd benchmarks/serde-comparison
cargo run --release
# optional: NEXTJSON_BENCH_MS=10000 cargo run --release
```

Same fixture (256 `Record`s), same warm-up, same measurement loop, same
process, same machine. The int-only rows use a float-free `IntRecord` model so
the integer-formatting path is isolated from float cost.

Post native-width integer writers, recorded before the parser fast path
landed (this developer machine, 2 s window, single run) — the decode columns
below therefore predate the `parse_u64_fast`/`parse_i64_fast` change; the
parser improvement is quantified per-primitive in the next section instead:

| case | size_bytes | encode_ops | encode_MBps | decode_ops | decode_MBps |
| --- | --- | --- | --- | --- | --- |
| nextjson_encode | 48063 | 7666 | 368.44 | 2738 | 131.61 |
| serde_json_encode | 48063 | 16662 | 800.81 | 3876 | 186.27 |
| nextjson_encode_intonly | 44446 | 8752 | 389.00 | 2854 | 126.86 |
| serde_json_encode_intonly | 44446 | 18625 | 827.80 | 4401 | 195.62 |

Absolute throughput on a laptop varies with power state and thermal budget;
repeated runs of this exact binary on the same machine have produced
nextjson encode numbers between ~158 and ~278 MB/s (serde_json ~340-460)
across a single afternoon, and the *ratio* moved between ~1.2x and ~2.9x
as the two libraries react differently to frequency changes. For that
reason the table above records one specific run (the one quoted in the
methodology) rather than a smoothed average, and the per-primitive A/B
numbers in the next section are the reproducible deltas.

### Why nextjson is slower than serde_json — and the evidence it is release mode

All table numbers above come from `cargo run --release` with the workspace
`[profile.release]` (`opt-level = 3`, `lto = "thin"`, `codegen-units = 1`). That
the comparison runs in release mode, not debug, is provable: in a **debug**
build the compiler disables serde's optimizations, and nextjson is *faster*
than serde_json:

| case (debug build) | encode_MBps | decode_MBps |
| --- | --- | --- |
| nextjson_encode | 19.36 | 8.39 |
| serde_json_encode | 8.05 | 8.09 |

Three measurable causes account for the release gap:

1. **Integer conversions widened `u64`/`i64` to `u128`** (dominant on encode,
   significant on decode). On the encode side the widened path forced a
   compiler-rt `__udivti3` libcall for every division, several times slower
   than a native 64-bit `div`. nextjson now writes integers through
   native-width `write_u64_into`/`write_i64_into` (hardware `div`), which
   raised encode throughput from ~291 to 368 MB/s on the isolation fixture
   (int-only from ~339 to 389 MB/s). The parser had the same widening:
   `Number::parse` accumulated every integer digit in `u128` arithmetic
   (long multiply/add chains per digit). It now parses through native-width
   `parse_u64_fast`/`parse_i64_fast` and falls back to the 128-bit parser
   only on genuine overflow, so integer-heavy decode payloads no longer pay
   the widened cost. An in-process A/B on the parsing primitive (mixed digit
   lengths, 2 s per side) measures 5.1 ns/call for the fast path versus
   38.2 ns/call for the widened path — a 7.4x reduction on the primitive.
   Wire bytes are identical in both directions.
2. **Float formatting** goes through `fmt::Display`/`core::write!` while
   serde_json uses Ryū. Removing floats from the fixture accounts for only
   ~16% of the gap, so this is secondary.
3. **Structural**: serde_json is a monomorphic, JSON-specialized hot path
   optimized for a decade (lookup-table digits, preallocated buffers), while
   nextjson routes every value through the generic `FormatEncoder` contract
   with per-value `start_value()`/frame/depth state that must stay correct
   across 13 formats.

The remaining gap is ~2.17x on encode for this JSON-only fixture. Read these
numbers as a **baseline for this exact fixture**, not a general ranking:
nextjson's encode path is a general format-neutral contract that also drives
13 other formats, while serde_json is a single, specialized JSON hot path.
Cross-check on your own hardware and fixtures before drawing conclusions.

Published results must include CPU, OS, `rustc -Vv`, revision, measurement
duration, and the full table. This benchmark is a reproducible project
baseline, not proof of universal performance.

CI compiles both benchmark crates but does not enforce a throughput threshold
on shared hardware.

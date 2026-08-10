# Reproducible comparison benchmark

`nextjson/benches/serde_comparison.rs` compares six paths over the same 128
record fixture:

- native NextJson nextencode and nextdecode;
- Serde types through NextJson's direct compatibility adapter;
- `serde_json` nextencode and nextdecode.

The benchmark first parses both encoded documents with `serde_json::Value` and
asserts semantic equality. It then warms every path before measurement. No
intermediate `Value` is used by the NextJson Serde adapter.

## Run

Use a quiet machine, release mode, the committed `Cargo.lock`, and the same CPU
power profile for every comparison:

```text
cargo bench --locked -p nextjson --features serde --bench serde_comparison
```

The default measurement window is 2 seconds per case. Increase it for reported
results:

```powershell
$env:NEXTJSON_BENCH_MS = "10000"
cargo bench --locked -p nextjson --features serde --bench serde_comparison
```

```bash
NEXTJSON_BENCH_MS=10000 cargo bench --locked -p nextjson --features serde --bench serde_comparison
```

Output is CSV:

```text
case,iterations,operations_per_second
nextjson_native_nextencode,...,...
nextjson_serde_nextencode,...,...
serde_json_nextencode,...,...
nextjson_native_nextdecode,...,...
nextjson_serde_nextdecode,...,...
serde_json_nextdecode,...,...
```

Record CPU model, OS, Rust version (`rustc -Vv`), git revision, duration, and
all six rows with any published result. Shared CI runners compile the benchmark
but do not enforce noisy throughput thresholds.

The current benchmark is a production baseline, not proof that one library is
universally faster. Additional datasets should be added before making claims
about small messages, escape-heavy text, deeply nested data, or large maps.

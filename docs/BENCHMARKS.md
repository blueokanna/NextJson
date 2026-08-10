# Reproducible Benchmark

`nextjson/benches/format_comparison.rs` measures four paths over the same
128-record fixture: native JSON nextencode, native JSON nextdecode, streaming
JSON-to-CBOR, and streaming CBOR-to-JSON.

Before timing, it validates a complete JSON -> CBOR -> JSON -> typed-value
round trip and warms every path. The build graph contains no third-party crate.

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
case,iterations,operations_per_second
nextjson_native_nextencode,...,...
nextjson_native_nextdecode,...,...
nextjson_json_to_cbor,...,...
nextjson_cbor_to_json,...,...
```

Published results must include CPU, OS, `rustc -Vv`, revision, measurement
duration, and all four rows. This benchmark is a reproducible project baseline,
not proof of universal performance. CI compiles it but does not enforce a
throughput threshold on shared hardware.

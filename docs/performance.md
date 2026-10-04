# Performance harness

The dependency-free, fixed-input microbenchmarks track large regressions while
the API is young. They are not substitutes for end-to-end H1/H2/H3 throughput,
latency, peak-memory, and cancellation testing.

```powershell
. ./scripts/dev-env.ps1
cargo run --locked --release -p rustymiddle-core --example hooks_benchmark
cargo run --locked --release -p rustymiddle-content --example content_benchmark
```

The initial Windows LLVM/Ninja release baseline recorded on 2026-10-03 was:

| Scenario | Indicative local result |
|---|---:|
| empty chain lifecycle | 666 ns/exchange |
| one no-op interceptor lifecycle | 189,647 ns/exchange |
| four no-op interceptor lifecycle | 720,212 ns/exchange |
| one 4 KiB streaming transform | 182,328 ns/exchange |
| one bounded observer delivery | 1,460 ns/event |

Numbers vary with hardware, scheduler load, and security software. They establish
a reproducible shape, not a release threshold. Record the host and investigate a
material change before accepting it; do not compensate by enlarging queues or
buffers. The lifecycle benchmark intentionally includes the panic-containment
task boundary used in production.

The content benchmark measures the actual neutral `ContentBodyPipeline` path
for coded pass-through and the owned codec APIs for decode-only and
decode/re-encode work. Its deterministic input repeats a 4 KiB pseudo-random
block, remains within the production default expansion policy, and uses the
production default content limits. The initial Windows LLVM/Ninja release
throughput baseline recorded on 2026-10-03 was:

| Scenario | 4 KiB | 256 KiB |
|---|---:|---:|
| coded neutral pass-through | 59.2 MiB/s | 2,029.7 MiB/s |
| gzip decode-only | 128.2 MiB/s | 3,364.5 MiB/s |
| gzip decode/re-encode | 11.7 MiB/s | 774.6 MiB/s |
| Brotli decode-only | 535.0 MiB/s | 882.3 MiB/s |
| Brotli decode/re-encode | 1.2 MiB/s | 7.1 MiB/s |
| deflate decode-only | 1,754.8 MiB/s | 5,307.2 MiB/s |
| deflate decode/re-encode | 94.7 MiB/s | 721.0 MiB/s |
| zstd decode-only | 51.0 MiB/s | 1,443.9 MiB/s |
| zstd decode/re-encode | 5.1 MiB/s | 502.3 MiB/s |

The small-input rows are dominated by setup and finalization. Brotli encoding
is intentionally not replaced by a weaker compression mode to improve these
numbers; output policy and codec tuning require explicit interoperability and
resource review.

Future transport benchmarks should measure throughput, time to first byte, and
memory return after cancellation for H1, H2, and H3 with no hooks, no-op hooks,
bounded transforms, and saturated observers.

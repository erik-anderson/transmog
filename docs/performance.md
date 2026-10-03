# Performance harness

`hooks_benchmark` is a dependency-free, fixed-input microbenchmark for tracking
large regressions while the API is young. It is not a substitute for end-to-end
H1/H2/H3 throughput and latency testing.

```powershell
. ./scripts/dev-env.ps1
cargo run --locked --release -p rustymiddle-core --example hooks_benchmark
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

Future transport benchmarks should measure throughput, time to first byte, and
memory return after cancellation for H1, H2, and H3 with no hooks, no-op hooks,
bounded transforms, and saturated observers.

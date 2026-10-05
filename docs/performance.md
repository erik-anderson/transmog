# Performance harness

The dependency-free, fixed-input microbenchmarks track large regressions while
the API is young. They are not substitutes for end-to-end H1/H2/H3 throughput,
latency, peak-memory, and cancellation testing.

```powershell
. ./scripts/dev-env.ps1
cargo run --locked --release -p transmog-core --example hooks_benchmark
cargo run --locked --release -p transmog-content --example content_benchmark
./scripts/benchmark-content.ps1
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

## End-to-end content-pipeline harness

`content_pipeline_benchmark` measures a complete content-layer exchange path:
Hooks v2 request/body planning, incremental decoding, a streaming decoded-byte
hook, and either identity output or restoration of the original coding. It uses
1 MiB deterministic pseudo-random bodies, 4 KiB source frames, and the
production default content and work limits. Fixture encoding and semantic
validation are outside timed throughput regions.

The harness records integer nanoseconds and bytes per second as JSON, measures
time and encoded bytes consumed before first output, and cancels sixteen live
8 MiB Brotli preserve-original pipelines. Cancellation must complete within one
second. The PowerShell runner also launches baseline and sixteen-pipeline child
processes, samples their working sets every 5 ms, and fails above a default
512 MiB ceiling. The reported incremental value subtracts the fixture-bearing
baseline. This is a portable process-level regression signal, not exact
allocator accounting; record host load and investigate material changes.

The report is written to the ignored
`artifacts/performance/content-pipeline.json` path and includes the source Git
revision, dirty-tree state, OS, architecture, workload dimensions, active work
limits, and the memory sampling interval and ceiling. Override the output path,
sampling interval, or ceiling explicitly through script parameters rather than
changing production limits. A proposed `content-performance` hosted workflow
is parked under `ci/github-actions/` while hosted automation is intentionally
deferred. It is design material for a future explicit CI-enablement decision;
the local Windows LLVM/Ninja gate is authoritative in the meantime.

The initial Windows LLVM/Ninja baseline recorded on 2026-10-03 was:

| Coding | Identity throughput | Preserve-original throughput | Identity first output | Preserve-original first output |
|---|---:|---:|---:|---:|
| gzip | 47.0 MiB/s | 28.1 MiB/s | 4 KiB consumed | 8 KiB consumed |
| Brotli | 46.0 MiB/s | 3.3 MiB/s | 4 KiB consumed | complete body consumed |
| deflate | 98.1 MiB/s | 34.6 MiB/s | 4 KiB consumed | 32 KiB consumed |
| zstd | 41.9 MiB/s | 18.3 MiB/s | 4 KiB consumed | 132 KiB consumed |

The cancellation p95 was 0.251 ms. The sixteen-pipeline workload had an
80.5 MiB sampled peak working set versus a 27.9 MiB fixture baseline, a
52.6 MiB incremental sampled peak. These are indicative values, not release
thresholds; the semantic checks, cancellation deadline, and 512 MiB memory
ceiling are the automated gates. The checked-in PowerShell runner currently
provides peak-working-set sampling on the supported Windows LLVM/Ninja profile;
the Rust benchmark executable itself remains cross-platform.

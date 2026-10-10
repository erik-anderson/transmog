# Performance harness

Transmog keeps deterministic microbenchmarks and one process-level content
pipeline benchmark in source control. Their purpose is to expose large
regressions in lifecycle overhead, streaming latency, cancellation, and bounded
memory use. Results vary with hardware, scheduler load, and security software,
so observed numbers are artifacts, not permanent documentation or release
thresholds.

Run the harnesses from the Windows LLVM/Ninja environment on an otherwise idle
host:

```powershell
. ./scripts/dev-env.ps1
cargo run --locked --release -p transmog-core --example hooks_benchmark
cargo run --locked --release -p transmog-content --example content_benchmark
./scripts/benchmark-content.ps1
```

## Microbenchmarks

`hooks_benchmark` measures an empty lifecycle, no-op interceptor chains, a 4 KiB
streaming transform, and bounded observer delivery. It includes the same panic-
containment task boundary used in production.

`content_benchmark` measures the neutral coded fast path and decode-only and
decode/re-encode work for gzip, Brotli, deflate, and zstd at fixed input sizes.
The input is deterministic and remains inside production expansion limits.
Fixture construction and correctness checks are outside the timed region.

Investigate a material regression instead of compensating by increasing queues,
body limits, or timeouts. Codec tuning is a compatibility and resource-policy
decision, not merely a benchmark optimization.

## End-to-end content pipeline

`content_pipeline_benchmark` exercises request/body planning, incremental
decoding, a streaming decoded-byte interceptor, and either identity output or
restoration of the original coding. It reports integer nanoseconds, bytes per
second, encoded bytes consumed before first output, cancellation latency, and
active work-limit configuration as JSON.

The PowerShell runner also compares baseline and sixteen-pipeline child-process
working sets, sampled every 5 ms by default. The incremental value subtracts the
fixture-bearing baseline. The default 512 MiB ceiling and one-second
cancellation deadline are correctness gates; peak working set is a portable
regression signal, not exact allocator accounting.

The ignored report is written to
`artifacts/performance/content-pipeline.json` and includes the source Git
revision, dirty-tree state, OS, architecture, workload dimensions, and sampling
configuration. Override output, interval, or ceiling through script parameters
rather than changing production limits.

The current harness is cross-platform Rust, but process working-set sampling is
implemented by the Windows runner. A hosted performance workflow is parked
under `ci/github-actions/` and remains inactive until hosted automation is
explicitly enabled.

## Browser capture scale

The [local Chromium load harness](load-testing.md) qualifies CLI and real desktop
capture with more than 2 GiB of payloads and 10,000 exchanges per target. It
includes retained traffic, native trace integrity, memory sampling and desktop
virtualization/save/clear checks using a synthetic loopback website.

## Remaining coverage

A future transport benchmark should compare H1, H2, and H3 throughput,
time-to-first-byte, cancellation, and memory return for no interceptors, no-op
interceptors, bounded transforms, and saturated observers. This is tracked in
the [roadmap](roadmap.md); it is not implied by the content harness.

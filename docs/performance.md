# Performance harness

Transmog keeps deterministic microbenchmarks and process-level content and TLS
handshake benchmarks in source control. Their purpose is to expose large
regressions in lifecycle overhead, streaming latency, cancellation, and bounded
memory use. Results vary with hardware, scheduler load, and security software,
so observed numbers are artifacts, not permanent documentation or release
thresholds.

Run the harnesses from the Windows LLVM/Ninja environment on an otherwise idle
host:

```powershell
python scripts/dev-env.py --check
python scripts/dev-env.py --run cargo run --locked --release -p transmog-core --example hooks_benchmark
python scripts/dev-env.py --run cargo run --locked --release -p transmog-content --example content_benchmark
python ./scripts/benchmark-content.py
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

The Python runner also compares baseline and sixteen-pipeline child-process
working sets, sampled every 5 ms by default. The incremental value subtracts the
fixture-bearing baseline. The default 512 MiB ceiling and one-second
cancellation deadline are correctness gates; peak working set is a portable
regression signal, not exact allocator accounting.

The ignored report is written to
`artifacts/performance/content-pipeline.json` and includes the source Git
revision, dirty-tree state, OS, architecture, workload dimensions, and sampling
configuration. Override output, interval, or ceiling through script parameters
rather than changing production limits.

The Rust harness and Python runner support Windows, Linux and macOS memory
sampling through their host APIs. A hosted performance workflow is parked
under `ci/github-actions/` and remains inactive until hosted automation is
explicitly enabled.

## Post-quantum TLS handshakes

```powershell
python ./scripts/benchmark-pqc.py --operations 500
```

The runner builds the release `pqc_benchmark` fixture and compares fresh TLS 1.3
handshakes across non-PQC, ML-DSA-44/65/87 and automatic credential selection,
each with X25519 or hybrid X25519MLKEM768 exchange. The fixture uses
the production context factories and validates the negotiated algorithms and
an application-byte round trip. Keys stay in memory and its root is used only
by the fixture's verifier.

Each case reports p50/p95 client handshake latency, first ClientHello record
size including its five-byte TLS record header, process CPU and peak resident
memory. Latency includes TLS certificate verification but excludes fixture
construction, TCP connect and application I/O. CPU and memory describe the
whole fixture process, including construction. Windows reports lifetime CPU;
Linux reports the last sampled CPU value; unsupported CPU measurements are
explicitly unavailable. Resident memory is sampled every 5 ms and can miss
short peaks. These are loopback TLS measurements, not browser page latency or
QUIC/H3 throughput.

The ignored report at `artifacts/performance/pqc-handshakes.json` records the
source revision, dirty-tree state, OS and architecture. Compare on an otherwise
idle host and retain the report with qualification evidence; do not turn one
machine's timings into a permanent performance threshold.

## Browser capture scale

The [local Chromium load harness](load-testing.md) qualifies CLI and real desktop
capture with more than 2 GiB of payloads and 10,000 exchanges per target. It
includes retained traffic, native trace integrity, memory sampling and desktop
virtualization/save/clear checks using a synthetic loopback website.

## Remaining coverage

A future transport benchmark should compare H1, H2, and H3 throughput,
time-to-first-byte, cancellation, and memory return for no interceptors, no-op
interceptors, bounded transforms, and saturated observers. The content and browser-scale harnesses do not establish those results.

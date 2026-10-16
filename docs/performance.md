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

## TMCap capture benchmarks

Use the capture benchmark to compare compression, writer throughput, append
latency and file size. Keep results with the measured commit, workload and host
configuration in generated evidence. Results from earlier experimental layouts
are not a baseline for the current format.

### Run the capture benchmark

From the repository root, with Python 3.11 or newer:

```text
python scripts/dev-env.py --check
python scripts/benchmark-capture.py --mib 256 --repeats 3 --output artifacts/performance/compression.jsonl
python scripts/benchmark-capture.py --no-build --container --kinds text,binary,mixed --mib 256 --repeats 3 --output artifacts/performance/container.jsonl
python scripts/benchmark-capture.py --no-build --container --encrypted --kinds text,binary,mixed --mib 256 --repeats 3 --output artifacts/performance/encrypted.jsonl
```

The first invocation builds the release fixture. The driver constrains affinity
to one, two and four available CPUs and alternates worker counts across repeats.
It supports Windows process affinity and Linux `sched_setaffinity`; on other
platforms, run the `capture_compression_benchmark` executable directly. Its
`--sink null` option separates filesystem costs.

### Capture workloads and interpretation

- `text`: deterministic JSON-like content, with independent 128 KiB chunks.
- `binary`: pseudorandom incompressible chunks.
- `mixed`: 25% text and 75% binary bytes.
- `small`: independent 2 KiB text bodies for the isolated compression experiment.

Default mode measures adaptive zstd jobs, bounded channels and buffered output;
all jobs pass through a worker. `--container` measures the actual `CaptureWriter`,
including digests, metadata, indexing, integrity protection and sealing. One
worker means inline compression in container mode; larger counts allow its lazy
pool. Password derivation occurs before the timed interval.

Reports contain throughput, p95/p99 append or admission duration and file size.
Writes use the OS cache without fsync, so these are not sustained storage-media
measurements. CPU affinity limits parallelism but does not model an older CPU's
cache or memory bandwidth. Repeat on an idle host and with realistic concurrent
traffic before changing production defaults. More throughput does not guarantee
lower individual-request latency.

The production writer uses a lazy two-worker pool when at least two CPUs are
available. Initial probes, metadata, bypassed bodies and chunks smaller than 4 KiB
stay on the writer thread. Eight pending records preserve order and bound
speculation. See [native capture encoding](native-capture-format.md#compression-and-scheduling)
for policy and backpressure.

### Storage and lazy-read regressions

These deterministic tests check structural savings and read bounds, not fixed
machine-dependent timings:

```text
python scripts/dev-env.py --run cargo test --locked -p transmog-capture persistent_sse_ids_grow_once -- --nocapture
python scripts/dev-env.py --run cargo test --locked -p transmog-capture shared_connection_storage_comparison -- --nocapture
python scripts/dev-env.py --run cargo test --locked -p transmog-capture --release --lib large_trace_metadata_io_measurement -- --ignored --nocapture
```

They cover persistent SSE ID sharing, reused connection metadata, metadata-only
indexing and individual body reads. Related capture tests cover paired body
sharing, encrypted/plain checkpoints, compaction, malformed references and
circular eviction. Storage savings depend on reuse and workload; large bodies
can dominate metadata savings.

For a private local dataset, the opt-in `repack_local_records_and_measure_metadata_open`
test accepts `TMCAP_RECORDS` JSONL and a `TMCAP_REPACKED` destination. Keep private
captures and raw results out of source control.

## Browser capture scale

The [local Chromium load harness](load-testing.md) qualifies CLI and real desktop
capture with more than 2 GiB of payloads and 10,000 exchanges per target. It
includes retained traffic, native trace integrity, memory sampling and desktop
virtualization/save/clear checks using a synthetic loopback website.

## Portable archive composition

Build and run the synthetic local codec harness with the configured tools:

```powershell
python scripts/dev-env.py --run cargo build --release --locked -p transmog-portable --example package_codec
python scripts/benchmark-portable.py
```

The ignored `artifacts/case-transfer-performance.json` records ZIP creation and
wrapper pack/open/validation times, process CPU, sampled peak Windows working
set, and physical bytes/latency for mid-entry 64 KiB probes. It compares Stored ZIP
with whole-stream/member-boundary TMCap sampling against Deflate levels 1 and 6
with outer compression disabled. A protected mixed case reports password-derived
writer initialization separately. Native inputs are 32 MiB synthetic captures,
including an encrypted capture; other inputs are an encoded asset, 16 MiB text,
small configuration output, and both mixed member orders. No real traffic or
machine configuration is collected.

The initial Windows warm-cache qualification observed:

| Workload | Stored + member-boundary TMCap | Deflate 6 + raw TMCap |
| --- | --- | --- |
| Native capture | 32.157 MiB; mid-entry probe 0.35 ms | 32.141 MiB; probe 10.85 ms |
| Mixed, captures first | 72.290 MiB; largest probe 0.14 ms | 72.336 MiB; largest probe 13.59 ms |
| Plain response text | 0.047 MiB | 0.094 MiB |
| Small context | 0.047 MiB | 0.047 MiB |

Whole-stream sampling produced 80.141 MiB for the captures-first mix; resetting
at member boundaries recovered text compression and produced 72.290 MiB in both
orders. This supports the boundary reset policy. It does not imply a size advantage
for incompressible evidence. Three fixed blocks create a roughly 48 KiB floor for
small native transfers. Mid-entry native-capture reads used about 128 KiB of
physical reads versus about 16 MiB for the Deflate prefix; raw data records each
probe rather than treating a cached archive tail as a random read.

The codec process used roughly 6–7 MiB peak working set on these inputs, or about
70 MiB for the encrypted mix including the existing 64 MiB KDF. ZIP creation is
measured separately as another process. A reproducible roughly 40-second stored
text ZIP creation wall-time stall had only about 0.08 CPU seconds, so combined
wall-time throughput is not inferred from this sample. These are synthetic codec
composition results with warm OS caches, not desktop end-to-end pack performance,
cold-storage latency, maximum-index memory, or a cross-machine throughput promise.
Application preparation/semantic validation and signed distribution need their
own qualification.

## Remaining coverage

A future transport benchmark should compare H1, H2, and H3 throughput,
time-to-first-byte, cancellation, and memory return for no interceptors, no-op
interceptors, bounded transforms, and saturated observers. The content and browser-scale harnesses do not establish those results.

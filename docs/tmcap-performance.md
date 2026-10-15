# TMCap performance verification

Use the capture benchmark to compare compression, writer throughput, append
latency and file size. Keep results with the measured commit, workload and host
configuration in generated evidence. Results from earlier experimental layouts
are not a baseline for the current format.

## Run the benchmark

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

## Workloads and interpretation

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

## Storage and lazy-read regressions

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

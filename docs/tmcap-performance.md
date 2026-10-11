# TMCap compression measurements

The synthetic test supports a lazy two-worker pool for compressible large
bodies. One available CPU uses the writer thread. Initial probes, bypassed data,
metadata and chunks smaller than 4 KiB stay on that thread. The live capture
writer itself runs separately from asynchronous request handling. Eight pending
records bound compression work and preserve output order.

## Method

Measurements were collected on 2026-10-11 on Windows, on an Intel Core Ultra 7
265K with eight logical processors available to the environment. The driver pins
each child process to one, two or four of those processors before starting its
workers. Affinity models constrained parallelism; it does not reproduce an older
processor's instruction throughput, cache, memory bandwidth or storage.

Each configuration processes 256 MiB three times, alternating worker-count order.
Tables show median input MiB/s. Timings include buffer copies and OS file writes
to a temporary file, with no fsync. They do not measure sustained media throughput
after exhausting the OS cache, HTTP throughput or request latency. No concurrent
builds or tests ran during these measurements. Other host activity and short run
times still introduce variability.

The deterministic workloads are:

- `text`: varied JSON-like URLs, numbers and identifiers, approximately 16% of
  original size after zstd level 1; independent 128 KiB chunks across 64 bodies.
- `binary`: pseudorandom, incompressible 128 KiB chunks across 64 bodies. This
  models compression futility, not a particular video codec.
- `mixed`: 25% text and 75% binary bytes, with stable body identities.
- `small`: independent 2 KiB text bodies, used in the compression-stage experiment.

Two modes answer different questions. The default mode isolates adaptive zstd
jobs, bounded channels and a buffered output consumer. Every job in this mode
passes through a worker, including with `--workers 1`. `--container` uses the
actual `CaptureWriter`: one means inline compression; two/four permit its lazy
pool. This mode includes body digests, record metadata, index construction,
block checksums or encryption, output batching and final sealing. Password
derivation occurs before the timed steady-state interval. The benchmark can
checkpoint every 8192 records; the 256 MiB large-body runs contain only 2048
records and therefore measure final publication without an intermediate checkpoint.
The live worker's timed checkpoints are covered separately by integration tests.

The output records throughput, p95/p99 append or admission duration and file
size. The isolated mode also counts bytes offered to zstd. Its binary workload
compresses only 3.125% of input, the initial probe for each body, and stores every
byte. A separate regression test covers re-probing after 8 MiB of bypassed data.

## Encrypted container results

| Available CPUs | Workload | Inline | Two workers | Four workers |
| --- | --- | ---: | ---: | ---: |
| 1 | Text | 532 | 546 | 570 |
| 1 | Binary | 436 | 453 | 445 |
| 1 | Mixed | 395 | 516 | 569 |
| 2 | Text | 492 | 935 | 843 |
| 2 | Binary | 414 | 438 | 419 |
| 2 | Mixed | 382 | 531 | 631 |
| 4 | Text | 498 | 1041 | 1267 |
| 4 | Binary | 422 | 438 | 433 |
| 4 | Mixed | 423 | 581 | 639 |

With two CPUs, two workers improve text throughput by 90% and mixed throughput
by 39%. Text runs ranged from 312–492 MiB/s inline and 817–1017 MiB/s with two
workers. Binary runs overlap substantially and do not establish a gain; this
workload never starts the production pool. Small file-size differences between
repeats come from the randomized JSON bootstrap, not different retained bodies.

More throughput does not imply lower individual append latency. Median measured
percentiles on two CPUs, in microseconds:

| Workload | Inline p95 | Two workers p95 | Inline p99 | Two workers p99 |
| --- | ---: | ---: | ---: | ---: |
| Text | 279 | 323 | 512 | 705 |
| Mixed | 607 | 458 | 1063 | 766 |

The dedicated live writer and bounded queues absorb ordinary bursts. When they
fill, capture deliberately backpressures forwarding. This benchmark does not
prove a request-latency bound.

## Supporting measurements and default selection

The unencrypted container also benefits with two CPUs:

| Workload | Inline MiB/s | Two workers MiB/s | Four workers MiB/s |
| --- | ---: | ---: | ---: |
| Text | 480 | 786 | 895 |
| Binary | 407 | 401 | 400 |
| Mixed | 399 | 519 | 621 |

That is a 64% text gain and 30% mixed gain for two workers. Encryption and plain
checksums use different code paths; differences between those tables should not
be interpreted as the cost of encryption in isolation.

The isolated compression-stage experiment explains why a universal pool is a
poor default. On one CPU, text falls from 814 to 654 MiB/s with two workers, and
binary falls from 1225 to 724 MiB/s. On two CPUs, text rises from 775 to 1217 MiB/s,
but binary does not benefit. Four workers sometimes improve the full writer,
especially with four CPUs available, but have inconsistent results across the
isolated workloads and consume more CPU resources. Two is a conservative default
that already gives a substantial benefit with constrained parallelism.

The isolated small-body experiment can benefit from parallelism on multiple
CPUs, but the actual format also pays metadata/index costs and does not accumulate
an entire response for classification. The production policy deliberately keeps
small/new bodies on the writer thread; these measurements do not justify a claim
that the chosen policy accelerates many independent tiny responses. Batching such
responses for compression would need its own full-pipeline latency/memory study.

## Reproduce

Run from the repository root with Python 3.11 or newer:

```text
python scripts/dev-env.py --check
python scripts/benchmark-capture.py --mib 256 --repeats 3 --output compression.jsonl
python scripts/benchmark-capture.py --no-build --container --kinds text,binary,mixed --mib 256 --repeats 3 --output container.jsonl
python scripts/benchmark-capture.py --no-build --container --encrypted --kinds text,binary,mixed --mib 256 --repeats 3 --output encrypted.jsonl
```

The first command through the benchmark driver builds its release executable.
The executable also accepts `--sink null` to separate filesystem costs. The
driver supports Windows process affinity and Linux `sched_setaffinity`; run the
executable directly on other platforms. Raw JSONL from the review accompanies
the review deliverable. Repeat on actual lower-end hardware when available,
using longer runs and realistic concurrent traffic before tuning worker counts.

## Shared connection metadata

Run the deterministic storage comparison with:

```text
python scripts/dev-env.py --run cargo test -p transmog-capture shared_connection_storage_comparison -- --nocapture
```

For 1,000 snapshots of one reused connection, with changing byte counters and a
TLS context containing 32 offered cipher names, the compressed indexed file was
443,576 bytes with shared context versus 919,172 bytes with inline transport
metadata: 51.7% smaller. Both paths use the same container, zstd policy, index and
seal. The control stores the former expanded payload under an opaque control
kind so the writer does not normalize it; its kind name differs slightly.
Certificates and bodies are excluded to isolate context storage. This is a
synthetic metadata-heavy case, not a predicted saving for every workload: large
bodies dominate file size, and connections used once offer little reuse.

The comparison and regression fixtures also cover changing connection facts,
request-relative measurements, encrypted and plain checkpoints/compaction,
independent random frame reads, metadata-only opening, dictionary bounds,
malformed references, data/index mismatch and circular eviction. Reader and
crypto fuzzers include connection definitions and pinned reference contexts.

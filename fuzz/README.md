# Content and capture fuzzing

This independent, non-published workspace keeps libFuzzer tooling out of the
shipping dependency graph. `cargo-fuzz` uses LLVM sanitizer support and runs on
Linux with the pinned nightly toolchain; it is not a Windows build prerequisite.

## Targets and oracles

- `content_encoding` combines raw malformed values with structured synthesis.
  The structured seed can generate sixteen fields and near-16-KiB values from a
  tiny input. Successful parses must canonicalize and reparse identically;
  failures must be deterministic.
- `decode_stream` drives arbitrary encoded bytes through every supported codec,
  variable budgets, raw-DEFLATE compatibility, fragmentation, trailers, early
  completion, duplicate trailers, and terminal-state checks.
- `codec_roundtrip` generates valid identity and one-to-four-layer gzip, Brotli,
  zlib, and zstd bodies at important size boundaries. Encoding and reverse-order
  decoding must reproduce the original bytes across independent chunk sizes.
- `content_pipeline` runs valid multi-layer bodies through a decoded
  interceptor transform, header/trailer repair, optional modification, and
  identity or preserve-original output. Re-decoding preserved output must produce the
  expected modified representation.
- `websocket_frames` combines raw hostile streams with synthesized valid
  client/server frames, both mask roles, arbitrary transport chunking, text and
  binary messages, and `permessage-deflate`. Successful events must remain
  within their declared allocation bounds and preserve UTF-8 invariants. It also
  exercises the uncapped production defaults, huge incomplete length declarations,
  and fragmented compressed messages. Valid synthesized messages must decode
  completely and reproduce the input bytes.
- `sse_stream` compares whole-buffer observation with arbitrary transport chunks,
  both directly and through gzip, deflate, Brotli, and zstd. It covers persistent
  IDs, resets, CR/LF boundaries, BOMs, malformed UTF-8, heartbeats, empty events,
  and incomplete tails. Event content and fields must be independent of chunking.
  Raw input is bounded to 8 KiB; synthesized compressed input to 512 bytes so
  campaigns spend their time on SSE behavior rather than compression setup.

- `capture_reader` combines arbitrary files and headers with checksum-repaired
  mutations of compact and normal checkpointed captures, structured footers,
  branches, index references and records, malformed zstd bodies/index objects,
  and public X.509 certificates. It checks bounded iteration, final seal
  consistency, and agreement between indexed, full, and random readers, including
  record fields, body bytes, and hashes. Body corruption
  can be detected lazily; metadata-only opening need not authenticate unread bodies.
- `capture_crypto` caches password-derived contexts for normal 16 KiB and compact
  1 KiB blocks once per process, then
  exercises encrypted block/tag bit flips, incorrect ordinals and frame lengths.
  The selected block can belong to the metadata page, independent body, or
  dictionary base of a dependent body; mutations cover the complete block and
  tag. Every single-bit mutation in a selected authenticated block must fail. A
  separate oracle verifies that changing public bootstrap bytes cannot replace
  an already pinned reader's key/context. This target does not rerun Argon2 or
  claim to fuzz password-based opening on each iteration; deterministic tests
  cover bootstrap authentication and KDF parameter rejection.

The capture reader harness limits input to 64 KiB, files to 1 MiB, records to
256, and JSON metadata records to at most 128 KiB. Stream content exercises the
production u32 object bound, including false large expanded-size declarations.
This bounds synthesized input, without adding a production stream-size cap;
the normal test suite separately covers larger files, metadata trees, 128 KiB
body splitting, short writes, and encrypted checkpoint recovery. Semantic seeds
retain the malformed branch-level and missing-seal regressions.

All campaigns disable libFuzzer's gradual length ramp. This is important for
short CI campaigns: configured maximum lengths and synthetic boundary classes
are available immediately instead of remaining capped at the largest seed.

## Windows with Docker Desktop and WSL2

The reproducible local smoke path is:

```powershell
python scripts/fuzz-content-docker.py --runs 10000
```

The runner builds a digest-pinned Debian image with Clang, Ninja, LLVM coverage
tools, the pinned stable and nightly toolchains, and the pinned `cargo-fuzz`.
It mounts the checkout read-only, caches downloaded crates and compiled fuzz
targets in named Docker volumes, and copies both semantic seeds and the
minimized coverage corpus to ephemeral storage. Crash reproducers are written
to the ignored `fuzz/artifacts` directory, so removing the container does not
discard failures. Ordinary campaigns do not change source files or corpora. The same image backs `scripts/test-linux-docker.py`; that
runner clears fuzz-only sanitizer flags before executing the normal workspace
matrix with Clang, `llvm-ar`, and Ninja.

To deliberately evolve, minimize, and replace the checked-in coverage corpus:

```powershell
python scripts/update-content-fuzz-corpus-docker.py --runs 100000
```

Review corpus changes like source changes. All selected campaigns and
minimizations must succeed before any existing corpus is replaced; a crash
leaves every prior corpus intact and its reproducer under `fuzz/artifacts`.
Docker Desktop's internal `docker-desktop` distribution is not a development
shell and should not be modified.

## Linux or a normal WSL distribution

Install the pinned tools and native build prerequisites:

```bash
rustup toolchain install nightly-2026-10-01 --profile minimal \
  --component llvm-tools-preview
cargo +nightly-2026-10-01 install cargo-fuzz --version 0.13.2 --locked
export CC=clang CXX=clang++ CMAKE_GENERATOR=Ninja
export CFLAGS='-fsanitize=address -fno-omit-frame-pointer -fsanitize-coverage=inline-8bit-counters,pc-table,trace-cmp'
export CXXFLAGS="$CFLAGS"
```

Run the bounded smoke suite used by the deferred CI definition:

```bash
FUZZ_RUNS=10000 bash fuzz/run-container.sh
```

Run time-bounded sustained campaigns, update the minimized corpus, verify native
zstd instrumentation, and generate first-party LLVM coverage reports:

```bash
FUZZ_MAX_TOTAL_TIME=300 bash fuzz/update-corpus.sh
bash fuzz/verify-native-sanitizers.sh
bash fuzz/report-coverage.sh
```

The coverage command reports only the first-party source surfaces owned by each
target and fails if their aggregate line coverage drops below its checked-in
floor. These floors are regression guards, not claims that a percentage alone
measures fuzzing quality; the semantic oracles and sustained corpus evolution
remain equally important.

`fuzz/seeds` contains small, named semantic examples and is maintained by hand.
The TMCap seeds are also replayed by ordinary capture-crate tests against the
exact fixture builders and mutation oracles used by libFuzzer:

```text
python scripts/dev-env.py --run cargo test -p transmog-capture --test fuzz_seeds
```

[`capture-seed-expectations.json`](capture-seed-expectations.json) records whether
each reader seed should open, which metadata/full records it must reach, and its
intentional error, if any. The test also checks the complete seed inventory and
all 64 crypto selector combinations for block geometry, body/context/stream selection,
mutation type, and dictionary-dependent bodies. A valid fixture silently becoming
an early parser rejection fails this test. When the format changes, update
the semantic seeds and review their expected outcomes together; do not blindly
replace the expectations with actual output. These tests need no libFuzzer or
Linux toolchain and run with the normal capture suite.

Malformed seeds deliberately remain malformed: `raw-header` has an excessive
header length; `plain-bootstrap` omits the footer; `encrypted-bootstrap` exercises
the password-required path; `excessive-kdf` exercises parameter rejection.
`index-reference` has a readable index and deliberately invalid data.
`oversized-index` tests expanded-size limits, while `zstd-index` uses a bounded
size and truncated zstd bytes so it actually reaches decompression.
`context-missing` uses a well-typed but unresolved digest, and
`context-forward-reference` puts a valid definition after its use. The populated
`context-tls-reference` seed covers a public certificate definition, TLS context,
and resolved performance snapshot together. Compact/checkpoint and encrypted
fixtures are generated by the current writer, not stored as obsolete file blobs.

`fuzz/corpus` contains content-addressed, coverage-bearing inputs and is updated
only by the explicit maintenance command. The deferred weekly workflow is configured to upload
the evolved minimized corpus, crash artifacts, merged LLVM profiles, and the
human-readable coverage summary; it never commits changes automatically.

Rust sanitizer flags do not automatically instrument C built by dependency
build scripts. The fuzz environment therefore supplies Clang `CFLAGS` for the
bundled zstd implementation and `CXXFLAGS` for BoringSSL X.509 parsing.
`verify-native-sanitizers.sh capture` checks native archives in the active release
fuzz build, excluding Rust wrapper objects. When adding sanitizer flags to an
existing cache, clean the affected native package before rebuilding; some native
build scripts do not track changes to these environment variables.

The normal workspace suite separately runs deterministic malformed-stream,
decompression-bomb, WebSocket masking/fragmentation/close, chunk-boundary, and
cancellation cases on every platform.
The fuzz-only lockfile has its own `cargo-deny` policy, including the NCSA
license used by LLVM's libFuzzer runtime. Fuzz-only components are deliberately
excluded from distribution notices and the shipping SBOM.

## Targeted TMCap campaigns

On Windows, the portable Docker runner can select just these targets:

```powershell
python scripts/fuzz-content-docker.py --runs 10000 --target capture_reader capture_crypto
python scripts/update-content-fuzz-corpus-docker.py --runs 100000 --target capture_reader capture_crypto
```

The second command deliberately updates the checked-in corpus; the first does not.

After installing the tools above (including Clang's ASan runtime), run:

```bash
mkdir -p /tmp/tmcap-reader /tmp/tmcap-crypto
cp -a fuzz/seeds/capture_reader/. /tmp/tmcap-reader/
cp -a fuzz/seeds/capture_crypto/. /tmp/tmcap-crypto/
cargo +nightly-2026-10-01 fuzz run capture_reader /tmp/tmcap-reader -- \
  -len_control=0 -max_len=65537 -rss_limit_mb=2048 -timeout=10 \
  -max_total_time=300 -seed=314159 -print_final_stats=1
cargo +nightly-2026-10-01 fuzz run capture_crypto /tmp/tmcap-crypto -- \
  -len_control=0 -max_len=256 -rss_limit_mb=2048 -timeout=10 \
  -runs=100000 -seed=161803 -print_final_stats=1
bash fuzz/verify-native-sanitizers.sh capture
```

Use disposable corpus directories for ordinary campaigns. Preserve failures
from `fuzz/artifacts/<target>/` and replay with
`cargo +nightly-2026-10-01 fuzz run <target> <reproducer>`. The general Docker
smoke runner includes both capture targets. To evolve only their checked-in
corpora, use `FUZZ_TARGETS="capture_reader capture_crypto"` with
`fuzz/update-corpus.sh`. The existing source-coverage floors in
`report-coverage.sh` apply to the original content/WebSocket targets; no SSE or TMCap coverage floor
is claimed yet. Execution counts and sanitizer coverage are not a proof that
the format has no defects.


Shared connection context coverage: reader mode `12` accepts an array of stored
records. It recomputes each `connection-context` digest after mutations and
replaces integer `context` references in performance transports with the
corresponding definition digest. This reaches schema validation and reference
resolution beyond the hash check; missing and forward references stay testable.
Production compact/checkpoint fixtures also contain a referenced performance
snapshot. Named seeds cover valid reuse, duplicates, missing definitions and
forward references. The crypto target uses bit 2 of its selector to exercise a
performance frame with pinned immutable context as well as a body frame.


Packed-object mode `13` constructs a paired dictionary-compressed body. Named
seeds cover valid decoding, changed base/result digests, forward references and
a forbidden dictionary chain. Metadata lives only in leaf objects; mode `11`
uses explicit body references. Codec/expanded-size seeds use the ten-byte packed
header.

Metadata allocation regressions include dense record quotas, excessive JSON
nesting, and over-wide branches. Deterministic tests additionally cover the
4096-entry leaf ceiling, pre-materialization record bounds, and viewer retained
JSON allocation budgets. The fuzz targets exercise the capture library, not the
entire desktop import transaction; import publication and cancellation are
covered by application regression tests.

Reader mode `14` passes raw leaf JSON through the production envelope parser;
mode `15` mutates a valid two-level index tree and repairs block checksums. The
`multilevel-index` seed must visit all four leaves successfully, keeping branch
traversal and random reads reachable rather than testing only branch rejection.

The `object-amplification` seed is byte-bounded but exceeds the structural
allocation budget with thousands of tiny objects. It must fail before the
reader constructs a `Value` tree. Writer/reader symmetry and escaped string
punctuation are covered by deterministic tests.

Stream-message records in reader mode 12 have independently indexed content,
so the same oracle checks persistent-ID resolution alongside lazy and random
body reads. Modes 5 and 6 with bit 7 set exercise malformed stream content
prefixes, UTF-8 raw fields, and zstd expanded lengths. Crypto bit 5 selects a
stream definition or reference (bit 4), covering content and metadata integrity.

The SSE semantic seeds also replay in the ordinary content crate tests across
all four content codings and four chunk sizes. Large-session integration tests
cover 24 MiB messages through both memory and encrypted disk circular storage;
those checks complement the deliberately smaller fuzz inputs.

# Content fuzzing

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
- `content_pipeline` runs valid multi-layer bodies through a decoded Hooks v2
  transform, header/trailer repair, optional modification, and identity or
  preserve-original output. Re-decoding preserved output must produce the
  expected modified representation.

All campaigns disable libFuzzer's gradual length ramp. This is important for
short CI campaigns: configured maximum lengths and synthetic boundary classes
are available immediately instead of remaining capped at the largest seed.

## Windows with Docker Desktop and WSL2

The reproducible local smoke path is:

```powershell
./scripts/fuzz-content-docker.ps1 -Runs 10000
```

The runner builds a digest-pinned Debian image with Clang, Ninja, LLVM coverage
tools, the pinned stable and nightly toolchains, and the pinned `cargo-fuzz`.
It mounts the checkout read-only, caches downloaded crates and compiled fuzz
targets in named Docker volumes, and copies both semantic seeds and the
minimized coverage corpus to ephemeral storage. Ordinary campaigns therefore
do not dirty the checkout. The same image backs `test-linux-docker.ps1`; that
runner clears fuzz-only sanitizer flags before executing the normal workspace
matrix with Clang, `llvm-ar`, and Ninja.

To deliberately evolve, minimize, and replace the checked-in coverage corpus:

```powershell
./scripts/update-content-fuzz-corpus-docker.ps1 -Runs 100000
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
```

Run the same bounded smoke suite used by pull requests:

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
`fuzz/corpus` contains content-addressed, coverage-bearing inputs and is updated
only by the explicit maintenance command. The weekly sustained workflow uploads
the evolved minimized corpus, crash artifacts, merged LLVM profiles, and the
human-readable coverage summary; it never commits changes automatically.

Rust sanitizer flags do not automatically instrument C built by dependency
build scripts. The fuzz environment therefore supplies Clang `CFLAGS` for the
bundled zstd implementation, and `verify-native-sanitizers.sh` rejects builds
where sanitizer symbols appear only in the Rust wrapper.

The normal workspace suite separately runs deterministic malformed-stream,
decompression-bomb, chunk-boundary, and cancellation cases on every platform.
The fuzz-only lockfile has its own `cargo-deny` policy, including the NCSA
license used by LLVM's libFuzzer runtime. Fuzz-only components are deliberately
excluded from distribution notices and the shipping SBOM.

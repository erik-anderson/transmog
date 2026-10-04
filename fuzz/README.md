# Content fuzzing

This independent, non-published workspace keeps libFuzzer tooling out of the
shipping dependency graph. `cargo-fuzz` uses LLVM sanitizer support and is run
on Linux with a nightly Rust toolchain; it is not a Windows build prerequisite.

On Windows, Docker Desktop's WSL2 Linux engine is the reproducible local path:

```powershell
./scripts/fuzz-content-docker.ps1 -Runs 10000
```

The runner builds a digest-pinned Debian image with Clang, Ninja, the pinned
nightly, and the pinned `cargo-fuzz`. It mounts the checkout read-only, keeps
downloaded crates and the compiled fuzz target in named Docker volumes, and
copies seed corpora to ephemeral storage so ordinary campaigns do not dirty the
checkout. A normal WSL development distribution can instead use the direct
Linux commands below; Docker Desktop's internal `docker-desktop` distribution
is not a development shell and should not be modified.

On Linux or inside a normal WSL distribution, install the pinned driver and run
bounded local sessions:

```bash
rustup toolchain install nightly-2026-10-01 --profile minimal
cargo +nightly-2026-10-01 install cargo-fuzz --version 0.13.2 --locked
export CC=clang CXX=clang++ CMAKE_GENERATOR=Ninja
cargo +nightly-2026-10-01 fuzz run content_encoding fuzz/corpus/content_encoding -- \
  -dict=fuzz/dictionaries/content_encoding.dict -max_len=16384 -max_total_time=60
cargo +nightly-2026-10-01 fuzz run decode_stream fuzz/corpus/decode_stream -- \
  -max_len=8192 -max_total_time=60
```

Keep minimized regressions in the corresponding checked-in corpus directory.
The normal workspace suite separately runs deterministic malformed-stream,
decompression-bomb, chunk-boundary, and cancellation cases on every platform.
The fuzz-only lockfile has its own `cargo-deny` policy, including the NCSA
license used by LLVM's libFuzzer runtime. Fuzz-only components are deliberately
excluded from distribution notices and the shipping SBOM.

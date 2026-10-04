# Content fuzzing

This independent, non-published workspace keeps libFuzzer tooling out of the
shipping dependency graph. `cargo-fuzz` uses LLVM sanitizer support and is run
on Linux with a nightly Rust toolchain; it is not a Windows build prerequisite.

Install the pinned driver and run bounded local sessions:

```bash
cargo install cargo-fuzz --version 0.13.2 --locked
cargo +nightly fuzz run content_encoding fuzz/corpus/content_encoding -- \
  -dict=fuzz/dictionaries/content_encoding.dict -max_len=16384 -max_total_time=60
cargo +nightly fuzz run decode_stream fuzz/corpus/decode_stream -- \
  -max_len=8192 -max_total_time=60
```

Keep minimized regressions in the corresponding checked-in corpus directory.
The normal workspace suite separately runs deterministic malformed-stream,
decompression-bomb, chunk-boundary, and cancellation cases on every platform.
The fuzz-only lockfile has its own `cargo-deny` policy, including the NCSA
license used by LLVM's libFuzzer runtime. Fuzz-only components are deliberately
excluded from distribution notices and the shipping SBOM.

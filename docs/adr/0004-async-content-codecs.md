# ADR 0004: Async content codec engine

Status: accepted for gzip and Brotli; later algorithms require separate review

## Context

The content-processing layer needs incremental, bounded decoding and encoding.
It must fit Tokio body pumps without buffering complete representations, keep
codec implementation types out of public contracts, and eventually support
gzip, deflate, Brotli, and zstd behind one rustymiddle-owned interface.

The dependency choice is deliberately narrower than the planned product
surface. Enabling a codec is a supply-chain and runtime decision of its own;
accepting gzip and Brotli does not implicitly accept every backend supported by
the same crate.

## Decision

Use [`async-compression`](https://docs.rs/async-compression/latest/async_compression/)
through its Tokio streaming adapters. Pin the exact reviewed release (currently
0.4.50) and disable default features. Initially enable only `tokio`, `gzip`, and
`brotli`; the crate publishes
[no default feature set](https://docs.rs/crate/async-compression/latest/features),
so the selected codec surface is explicit.

Keep every dependency type private. The public API accepts and returns
rustymiddle's canonical `BodyFrame` values, reports rustymiddle-owned typed
errors, and enforces rustymiddle-owned byte and expansion budgets before bytes
are retained. This lets the implementation change without coupling transport,
hook, or application layers to a codec crate.

The gzip feature currently uses `flate2`. Its default backend is the pure-Rust
`miniz_oxide` implementation, documented by
[`flate2`](https://docs.rs/flate2/latest/flate2/), so this phase adds no native
compression toolchain or second TLS/native-library stack. Brotli is enabled
through the library's separately gated feature and exercised through the same
rustymiddle-owned API and bounds. The upstream project is dual
MIT/Apache-2.0 licensed; see the
[`async-compression` repository](https://github.com/Nullus157/async-compression).

The Brotli backend's normal-format decoder does not opt into the nonstandard
large-window extension, bounding its history window to the format's 24-bit
maximum (16 MiB). Decoded and retained output remains subject to the smaller
application limits where configured. Gzip uses its format-defined 32 KiB
history window. The gzip decoder validates and emits every concatenated member,
as required by the [RFC 1952 file format](https://www.rfc-editor.org/rfc/rfc1952.html#section-2.2),
and rejects trailing bytes that do not form another valid member rather than
ambiguously ignoring them.

## Consequences

- Decode and encode can make progress incrementally as body frames arrive.
- Resource limits and frame sequencing remain stable library policy rather
  than incidental codec behavior.
- Dependency features and versions are auditable and reproducible.
- Deflate and zstd remain disabled until their implementation, dependency
  graph, interoperability corpus, and operational limits are reviewed in their
  respective phases.
- A dependency upgrade is intentional work: update the exact pin, review the
  changelog and graph, and rerun the corruption, truncation, limit, and
  interoperability suites.

## Alternatives considered

- Use `flate2` directly. This is viable for gzip but would require maintaining
  different streaming adapter machinery for later algorithms.
- Buffer complete bodies and use synchronous convenience functions. Rejected
  because it violates bounded streaming and increases latency and memory use.
- Select a native zlib backend now. Rejected because gzip does not require it,
  and native ABI/toolchain complexity should only be accepted with measured
  benefit.

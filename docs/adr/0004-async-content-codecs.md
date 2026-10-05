# ADR 0004: Async content codec engine

Status: accepted for gzip, Brotli, deflate, and zstd

## Context

The content-processing layer needs incremental, bounded decoding and encoding.
It must fit Tokio body pumps without buffering complete representations, keep
codec implementation types out of public contracts, and eventually support
gzip, deflate, Brotli, and zstd behind one Transmog-owned interface.

The dependency choice is deliberately narrower than the planned product
surface. Enabling a codec is a supply-chain and runtime decision of its own;
accepting gzip, Brotli, deflate, and zstd does not implicitly accept every backend
supported by the same crate.

## Decision

Use [`async-compression`](https://docs.rs/async-compression/latest/async_compression/)
through its Tokio streaming adapters. Pin the exact reviewed release (currently
0.4.50) and disable default features. Initially enable only `tokio`, `gzip`,
`brotli`, `zlib`, `deflate`, and `zstd`; the crate publishes
[no default feature set](https://docs.rs/crate/async-compression/latest/features),
so the selected codec surface is explicit.

Keep every dependency type private. The public API accepts and returns
Transmog's canonical `BodyFrame` values, reports Transmog-owned typed
errors, and enforces Transmog-owned byte and expansion budgets before bytes
are retained. This lets the implementation change without coupling transport,
hook, or application layers to a codec crate.

The gzip and deflate features currently use `flate2`. Its default backend is
the pure-Rust `miniz_oxide` implementation, documented by
[`flate2`](https://docs.rs/flate2/latest/flate2/). Brotli is enabled through the
library's separately gated feature and exercised through the same
Transmog-owned API and bounds. The upstream project is dual
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

HTTP `deflate` encoding always uses the RFC 9110 zlib wrapper. Decoding is
strict by default. Applications can explicitly enable raw-DEFLATE compatibility
for legacy senders; this selects raw decoding only when the first two bytes,
even when split across body frames, cannot form an RFC 1950 zlib header. This
bounded signature policy avoids speculative decode-and-rewind buffering. A raw
stream whose first two bytes accidentally form a valid zlib header remains a
strict decode failure rather than triggering an ambiguous retry.

Zstd uses `zstd` 0.14.0 and its bundled `libzstd` 1.5.7 sources. The existing
Clang/LLVM native toolchain compiles those sources; this introduces no second
TLS or networking stack and requires no system zstd installation. Each decoder
sets `windowLogMax` from `ContentLimits::max_decoder_window_bytes`, rounding
down for non-power-of-two limits so the effective bound never exceeds policy.
The default is 16 MiB and the smallest accepted configuration is 1 KiB.
Concatenated frames are enabled and fully validated. Dictionaries and zstd
background workers are not enabled.

## Consequences

- Decode and encode can make progress incrementally as body frames arrive.
- Resource limits and frame sequencing remain stable library policy rather
  than incidental codec behavior.
- Dependency features and versions are auditable and reproducible.
- All four initially planned HTTP content codings use one private adapter
  surface with explicit byte, ratio, and decoder-window bounds.
- A dependency upgrade is intentional work: update the exact pin, review the
  changelog and graph, and rerun the corruption, truncation, limit, and
  interoperability suites.

## Alternatives considered

- Use `flate2` directly. This is viable for gzip but would require maintaining
  different streaming adapter machinery for later algorithms.
- Buffer complete bodies and use synchronous convenience functions. Rejected
  because it violates bounded streaming and increases latency and memory use.
- Select a native zlib backend. Rejected because gzip and deflate do not require
  it. Zstd's native implementation is accepted because it provides the format's
  maintained reference implementation and exposes the required decoder-window
  control through the chosen Rust wrapper.

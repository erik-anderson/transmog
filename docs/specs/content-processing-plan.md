# Plan: Production-grade content processing

Status: active; Phases 1-5 and the local Phase 6 deterministic hardening,
fuzz-target, benchmark, and dependency gates are implemented on Windows;
cross-platform fuzz/codec observation and preemptible codec CPU deadlines remain
Audience: implementation agents and reviewers
Depends on: Hooks v2 and the layered monorepo roadmap
Primary scope: new `rustymiddle-content` crate and runtime body-pipeline
composition

## Mission

Allow interceptors, rules, and interactive tools to inspect or modify decoded
HTTP representation bytes while preserving protocol correctness and bounded
resource use. Add gzip, Brotli, deflate, and zstd decoding/re-encoding without
placing codec policy in transport adapters or forcing all traffic through a
lossy recompression path.

The content layer operates after transfer framing has produced canonical body
frames and before semantic body hooks. It is independent of H1 chunking, H2
DATA frames, and H3 streams.

## Non-goals

This milestone does not implement:

- character-set transcoding, HTML parsing, JSON editing, or media inspection;
- capture storage, saved rules, an external control protocol, or a UI;
- transfer-coding support beyond the runtime's existing HTTP framing;
- archive extraction or content-type sniffing;
- WebSocket per-message compression;
- cache storage or an HTTP cache;
- dictionary distribution for Brotli or zstd;
- transparent browser-to-proxy QUIC interception.

## Placement and ownership

`rustymiddle-content` depends on `rustymiddle-core` and owns:

- content-coding tokens and ordered coding stacks;
- content-processing policy and resource limits;
- decode/re-encode filters and codec-specific failures;
- representation header invalidation and repair;
- codec conformance and decompression-bomb tests.

`rustymiddle-core` continues to own canonical body frames, hook actions,
bounded body-pipeline contracts, cancellation, and terminal outcomes.
`rustymiddle-runtime` decides whether content processing is enabled for a
listener/exchange and composes content filters around hook body plans.
Transport crates remain unaware of compression algorithms.

## Required semantics

### Coding order

For `Content-Encoding: gzip, br`, the sender applied gzip and then Brotli.
Decoding therefore runs in reverse order (`br`, then `gzip`); re-encoding runs
in declared order (`gzip`, then `br`). Multiple field lines are combined in
wire order. Empty elements, parameters, non-token bytes, and ambiguous use of
`identity` are rejected.

Unknown codings are never silently presented to a hook as decoded content. A
policy may either leave the complete representation untouched or fail the
exchange before decoded-content hooks run. It may not partially decode a stack
and label the result identity.

### Fast path and modification path

Untouched traffic retains a zero-codec pass-through path. If no registered
consumer requires decoded content and no content-aware transformation is
selected, original encoded bytes and representation headers pass through.

When decoded processing is selected:

```text
canonical encoded frames
  -> count/limit encoded bytes
  -> decode codings in reverse order
  -> count/limit decoded bytes and expansion
  -> Hooks v2 body pipeline
  -> identity output OR encode codings in declared order
  -> count/limit output bytes
  -> canonical output frames
```

Trailers remain terminal frames and are not passed into codec byte streams.
Codec completion occurs before trailers are forwarded. A codec must reject
truncated streams, checksum failures, invalid trailing bytes under the selected
policy, and output after terminal trailers. Representation digests or validators
carried in trailers are invalidated by the same rules as head fields.

### Hook representation selection

Hooks v2 currently describes body mechanics but does not distinguish raw
content-coded bytes from decoded representation bytes. Runtime integration must
add a typed body-representation requirement alongside each body plan:

- `Neutral` for pass-through plans that do not inspect bytes;
- `Raw` for a transform that deliberately operates on coded bytes;
- `DecodedRequired` for semantic transforms that must see identity bytes;
- `DecodedIfSupported` for optional inspection that may decline an unsupported
  stack without changing traffic.

The chain aggregates these requirements before constructing the body pump.
Neutral plans do not trigger codecs. Any decoded transform activates the
content plan. A raw transform and a decoded transform on the same body are a
typed composition error unless a future explicit conversion stage separates
them; registration order must not silently decide representation.

Head hooks run before the body pump is committed. A decoded body event receives
the repaired identity head, while original/effective encoded heads remain
available as immutable evidence. Once decoded processing is selected, the
content layer owns `Content-Encoding`, `Content-Length`, validators, and range
metadata on the outgoing head. Conflicting direct edits are rejected or
normalized with an explicit diagnostic, never silently trusted.

`Replace` and `Discard` may drain an encoded input without decoding it because
they do not inspect the original bytes. A replacement is treated as decoded
output and encoded only if the selected output policy requires it. This avoids
wasting CPU on a body that will be discarded while preserving transport flow
control and encoded-input byte limits.

### Header view and repair

Decoded-content hooks receive a head representing identity body bytes. The
content layer removes `Content-Encoding` and stale `Content-Length` from that
view and invalidates representation metadata that can no longer describe the
bytes:

- `Content-MD5`;
- `Digest`, `Content-Digest`, and `Repr-Digest`;
- `ETag`;
- `Accept-Ranges` and `Content-Range`.

For identity output, those fields remain absent. For preserved coding output,
the original normalized coding stack is restored but validators are still
absent because recompression need not reproduce the original octets. A known
buffered output length may restore `Content-Length`; streaming output omits it
and lets the transport select framing.

Request `Accept-Encoding` negotiation is not rewritten merely because the
proxy can decode. A separate policy may deliberately force identity upstream,
but it must remain explicit and auditable.

### Bounds and denial-of-service resistance

Every plan has finite limits for:

- encoded input bytes;
- decoded bytes;
- output bytes;
- decoder history-window bytes;
- coding-stack depth;
- expansion ratio with a finite small-input allowance;
- frames and bytes emitted per body-filter call;
- codec callback duration and whole-exchange lifetime.

Counters use checked or saturating arithmetic and fail before allocation based
on attacker-controlled declared sizes. The decoded-byte limit is authoritative
even when the ratio allowance would permit more. Cancellation drops codec work
and releases body-channel capacity. Dictionary lookup, unbounded window sizes,
and detached blocking work are forbidden.

### Deflate compatibility

The standard `deflate` coding means a zlib-wrapped stream. Real servers have
historically emitted raw DEFLATE. Compatibility fallback, if implemented, must
be an explicit policy with tests; it must not retry after releasing decoded
bytes from a failed first interpretation.

### Errors and observation

Errors use stable categories: malformed coding header, unsupported coding,
stack-depth exceeded, encoded limit, decoded limit, expansion limit, truncated
stream, checksum failure, codec failure, timeout, and cancellation. Messages
must not include body bytes or credentials.

Observers may receive coding names, byte counts, ratios, elapsed time, and the
stable failure category. Body samples remain separately opt-in and bounded.

## Public API direction

The initial API is expected to include:

- `ContentCoding` and ordered `ContentCodingStack`;
- `ContentLimits` and a per-body `ContentBudget`;
- `ContentOutput` (`Identity` or `PreserveOriginal`);
- a typed raw/decoded representation requirement on Hooks v2 body actions;
- immutable `ContentPlan` derived from validated headers and policy;
- helpers that produce the decoded hook head and repaired output head;
- streaming decoder and encoder filters compatible with canonical
  `BodyFrame`s.

Codec implementations remain private behind these protocol-neutral types.
Public APIs do not expose library-specific decoder structs. The crate is
pre-1.0 and may make coordinated monorepo breaking changes.

## Implementation phases

### Phase 1: Semantics, plans, and budgets

- Create `rustymiddle-content` with no codec dependency.
- Parse duplicate/comma-separated `Content-Encoding` fields strictly.
- Model encode/decode order and identity handling.
- Add finite limit validation and incremental encoded/decoded/output counters.
- Implement decoded-head and output-head repair.
- Add exhaustive unit tests for order, syntax, limit boundaries, overflow,
  default ports-independent header handling, and validator removal.

Exit gate: public types are documented, strict Clippy passes, malformed or
unsupported stacks cannot be mistaken for identity, and header repair is
deterministic.

### Phase 2: Codec engines

- Select maintained codec libraries after license, advisory, native-build,
  cancellation, and streaming API review.
- Implement gzip with checksum/truncation/concatenated-member tests.
- Implement zlib-wrapped deflate and separately controlled raw fallback.
- Implement Brotli with bounded window/output behavior.
- Implement zstd without remote dictionaries or unbounded windows.
- Adapt blocking codec APIs through owned, cancellable bounded workers only if
  truly necessary; prefer incremental in-task state machines.
- Add official-format vectors, corrupted vectors, truncation tests, empty
  streams, tiny frames, and randomized chunk boundaries.

Exit gate: each codec round-trips deterministic vectors and fails closed under
all configured bounds on Windows, Linux, and macOS.

### Phase 3: Pipeline composition

- Add typed body-representation requirements and deterministic chain conflict
  detection before any pump starts.
- Add a content-aware wrapper that sandwiches the Hooks v2 body pipeline
  between decoding and output encoding.
- Preserve trailer ordering and bodyless response semantics.
- Decide decoded processing before committing the outgoing head.
- Keep untouched encoded bodies on the pass-through fast path.
- Ensure local responses and replacement bodies use an explicit output coding
  decision rather than inheriting stale headers.
- Propagate typed content failures into exchange terminal outcomes and
  observers.

Exit gate: request and response modifications see decoded bytes, unchanged
traffic is byte-preserving, and cancellation at every codec/hook boundary does
not leak tasks or flow-control credit.

### Phase 4: Runtime policy and embedding

- Add immutable content policy to `ProxyComponents`/runtime configuration.
- Support disabled, inspect-to-identity, and preserve-original-output modes.
- Define unknown-coding behavior explicitly.
- Allow applications to provide policy, not arbitrary unbounded codec objects.
- Add an embedding example that modifies compressed content without CLI state.

Exit gate: an embedding application can opt in per listener and observe typed
results without transport-private types.

### Phase 5: Protocol matrix and live proof

- Exercise gzip, Brotli, deflate, and zstd on request and response paths across
  H1/H1, H1/H2, H2/H1, H2/H2, H1/H3, and H2/H3 where supported.
- Test streaming first-byte behavior and unrelated-stream isolation.
- Test declared and actual size-limit failures before and during streaming.
- Test Auto H3 fallback with replayable encoded and decoded bodies.
- Add local browser fixtures so correctness does not depend on public sites.
- Retain the five-case public Chromium compatibility gate.

Exit gate: deterministic local fixtures prove coding correctness and the live
gate proves no regression in normal verified browser traffic.

### Phase 6: Hardening and documentation

- Add fuzz targets for header parsing, chunk boundaries, and corrupt streams.
- Add deterministic decompression-bomb and cancellation stress tests.
- Benchmark untouched pass-through, decode-only, and decode/re-encode paths for
  each codec and representative sizes.
- Review dependency licenses, advisories, duplicate graphs, notices, and SBOM.
- Document interoperability choices and remaining limitations.

Exit gate: the full workspace gate, optimized Clang/Ninja/Windows SDK build,
cross-platform CI, and live browser suite pass from a clean checkout.

Implementation note: checked-in Linux libFuzzer targets cover coding-header
parsing and bounded corrupt/chunked decoder input. The normal suite owns
deterministic decompression-bomb and cancellation stress, while the content
release harness records pass-through, decode-only, and decode/re-encode
baselines for every codec. Shipping and fuzz-only dependency graphs remain
separate. Cross-platform CI observation and a preemptible per-codec work/deadline
mechanism are still open.

## Dependency acceptance criteria

Codec dependencies are accepted only after checking:

- active maintenance and security response history;
- compatible MIT/Apache/BSD-style licensing;
- no second TLS stack or unrelated networking runtime;
- predictable Windows Clang/Ninja and Unix Clang builds;
- streaming APIs that do not require unbounded buffers;
- control over decoder window, dictionary, checksum, and trailing-data policy;
- no hidden global pools or threads;
- acceptable duplicate dependency and binary-size impact.

Exact versions are pinned through the workspace lockfile. Notices and the SBOM
are regenerated in the same change that accepts a dependency.

## Definition of done

- [x] Phase 1 typed plans, budgets, and header repair pass.
- [x] gzip decode and encode pass corruption and round-trip tests.
- [x] deflate decode and encode pass strict and compatibility-policy tests.
- [x] Brotli decode and encode pass bounded streaming tests.
- [x] zstd decode and encode pass bounded streaming tests.
- [x] Multiple codings compose in correct reverse-decode/forward-encode order.
- [x] Unsupported or malformed coding never reaches decoded hooks as identity.
- [x] Untouched encoded traffic remains byte-preserving.
- [x] Modified traffic has correct coding, framing, and validator headers.
- [x] Requests and responses preserve trailers and bodyless semantics.
- [ ] All byte, ratio, depth, frame, time, and cancellation bounds are tested.
- [x] One paused codec stream does not block unrelated H2/H3 streams.
- [x] All ingress/egress protocol matrix rows pass local coding fixtures.
- [x] Public embedding example and rustdoc are complete.
- [x] Formatting, strict Clippy, locked tests, dependency policy, notices, SBOM,
      and the one-BoringSSL check pass.
- [x] Windows LLVM/Ninja/Windows SDK release build passes.
- [ ] Linux and macOS CI pass.
- [x] Live Chromium verification passes without certificate bypass or route
      interception.
- [x] Header and codec fuzz targets, deterministic decompression-bomb and
      cancellation stress, and per-codec release benchmarks are checked in.
- [x] Shipping dependency licenses, advisories, duplicate graph, notices, SBOM,
      and native-code ownership are reviewed by the automated gate.

## Next implementation slice

Observe the new Linux fuzz smoke job and the existing Linux/macOS codec matrix,
then add a preemptible codec work-quantum/deadline design before checking the
remaining all-bounds item. Preserve the deterministic protocol matrix as the
behavioral baseline; do not weaken finite limits or move content policy into
transport adapters to improve a benchmark.

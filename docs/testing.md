# Testing

Local deterministic fixtures are the primary correctness gate. Public sites
and hosted CI are not required to validate ordinary changes.

## Repository gate

On Windows, enter the pinned LLVM/Ninja environment and run:

```powershell
. ./scripts/dev-env.ps1
./scripts/test.ps1
```

The gate checks formatting, strict Clippy, locked workspace tests, dependency
policy, the single-TLS-family graph, and generated supply-chain artifacts. It
does not install certificates or modify system proxy settings.

Useful narrower commands are:

```powershell
cargo test --locked --workspace --all-features --all-targets
./scripts/check-crypto-graph.ps1
./scripts/test-linux-docker.ps1
```

The Linux runner uses a digest-pinned Docker image with Clang, `llvm-ar`, and
Ninja, mounts the checkout read-only, and keeps Cargo and target data in named
volumes. A manually dispatched standard Windows runner now executes the unsigned
installer's application/host/desktop test gate. The broader hosted workflow
definitions remain inactive under `ci/github-actions/`; see
[`ci/README.md`](../ci/README.md).

## Deterministic coverage

The workspace suites cover these contracts:

- all H1/H2 ingress and H1/H2/H3 origin-egress combinations, including
  streaming, pool reuse, multiplexing, per-stream cancellation, route evidence,
  and safe automatic fallback;
- TLS chain and DNS/IP identity validation, immutable trust generations,
  certificate reload, and fail-closed certificate errors;
- interceptor factory isolation, forward/reverse ordering, short circuits,
  exactly-once terminal delivery, body plans, panic/timeout/drop containment,
  observer redaction and saturation, and destination authorization;
- strict and legacy-compatible content-coding parsing, gzip, Brotli, zlib and
  raw deflate, zstd, multi-layer decode/modify/re-encode, trailers, malformed
  input, bombs, byte/ratio/window/time bounds, cooperative scheduling, and
  cancellation;
- experimental control correlation, stale and duplicate replies, saturation,
  controller loss, and fail-closed interactive errors;
- native capture recovery and sealing, TMCap caller identity, SAZ conversion,
  automation ordering and audit identities, auto-response fidelity, sandboxed
  scripts, retained-body eviction, and safe raster/SVG preview behavior;
- session catalog admission/eviction, lossy subscription recovery, replay risk
  acknowledgement, transactional host restoration, product-state recovery,
  support-data redaction, and concurrent/idempotent shutdown;
- autoresponse gate persistence and immutable hook snapshots, anonymous/named
  patterns, repeated query constraints, saved matcher examples, atomic batch
  creation, saved compressed-response editing after source loss, and replay;
- desktop rule/Traffic multiselection across pages, keyboard deletion and Undo,
  bulk states and duplicate warnings, unsaved-draft choices, shared pause
  controls, matcher tooling, persisted pane splits and small-window editors
  under enforced CSP and Trusted Types;
- selected breakpoint editors and retained replacement drafts, real decision
  deadlines, Composer draft replacement and late responses, conditional replay
  acknowledgements, script candidate invalidation, capture task/picker behavior,
  and Settings save/revert and certificate recovery;
- populated desktop lists, visible bulk actions, control and text alignment,
  and wide/narrow tool layouts under production templates;
- WebSocket handshake validation, masking, fragmentation, control frames,
  UTF-8, close behavior, permessage-deflate, hook ordering, and transparent
  byte-copy mode.

Tests use deterministic clocks and injected providers where practical. The
operating system is touched only by explicitly named integration or packaging
gates.

## Standalone interoperability

`scripts/test-interop.ps1` places independently distributed clients and servers
on opposite sides of the release proxy:

| Client | Origin | Coverage |
|---|---|---|
| curl | Nginx | H1 traffic, native/scripted mutations, break/edit, auto-response, retained bodies, preview, capture/export, and restart persistence |
| curl | Apache HTTP Server | H1 proxy modification and terminal evidence |
| Chromium | Nginx and Apache | navigation, response modification, DOM proof, and terminal evidence |
| curl | Caddy over TLS | forced H2 and H3 egress with exact-IP certificate verification |
| Chromium | Nginx encoded fixtures | gzip, Brotli, deflate, zstd, and a four-layer coding stack |
| Chromium WebSocket | standalone Node server | browser-style CONNECT, echo, upgrade evidence, and terminal relay counts |

Each HTTP request carries a unique value that only the proxy can add to both
the response and captured evidence, so localhost bypass cannot produce a false
pass. Nginx, Apache, and Caddy are digest-pinned and exposed only on ephemeral
loopback ports. The runner creates temporary certificate material and removes
its containers, network, state, bodies, captures, and keys in cleanup.

Run with Docker Desktop available:

```powershell
./scripts/test-interop.ps1
```

The headless product portion launches release copies of the proxy, script host,
and preview worker and requires the production AppContainer/Job Object policy.
It does not mutate the OS trust store, system proxy, or durable user state.

## Live Chromium gate

The network-enabled gate uses Playwright Chromium with a fresh profile, QUIC
disabled, no certificate-error bypass, no request interception, and a durable
current-user test CA. Certificate installation/removal is deliberately a
one-time interactive operation; ordinary runs only verify the exact SHA-256
root read-only.

```powershell
./scripts/setup-live-test-ca.ps1
./scripts/test-live.ps1
./scripts/remove-live-test-ca.ps1
```

The run proves intercepted HTTPS navigation and H1/H2/H3 origin egress and
writes redacted protocol evidence to `verification/live-report.json`. A stopped
proxy must make navigation fail instead of falling back to a direct connection.

## Desktop and release qualification

The real WebView2 gate checks hydration, commands, CSP, accessibility, theme and
forced-color behavior, scaling, long labels, bounded memory growth, fixed-root
layout, and single-instance handoff:

```powershell
./scripts/test-windows-desktop.ps1
./scripts/test-windows-desktop.ps1 -SoakMinutes 30
```

The second form is the release soak. Installer and clean-machine checks are in
[Windows release and qualification](windows-release.md).

## Fuzzing, performance, and Miri

Coverage-guided content and WebSocket targets live in the independent `fuzz/`
workspace so libFuzzer is absent from shipping artifacts. See
[`fuzz/README.md`](../fuzz/README.md) for the pinned Linux/Docker workflow and
corpus policy.

Run performance checks with [`performance.md`](performance.md). The
transport-neutral core is intended to remain Miri-compatible; on a host with
the nightly component installed:

```powershell
cargo +nightly miri test -p transmog-core --lib
```

Miri is an additional diagnostic and is not installed or silently skipped by
the repository gate.

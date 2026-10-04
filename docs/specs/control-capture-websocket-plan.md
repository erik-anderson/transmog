# Plan: Control, capture, export, automation, and WebSockets

Status: active; Phases 1-2 complete; hosted automation is intentionally disabled
Audience: implementation agents and reviewers
Depends on: Hooks v2 and the content-processing plan

## Mission

Build upward from the protocol-neutral proxy library to the seams required by
a Charles/Fiddler-style developer tool without putting product state in the
network core. The same layers must remain useful to embedding applications and
a future headless command-line tool.

This plan is deliberately pre-stabilization. The control protocol may break
indefinitely until a human maintainer explicitly records that stabilization now
makes sense. Capture files have their own compatibility policy and never embed
live control envelopes as their durable representation.

## Non-negotiable boundaries

- `rustymiddle-core` owns canonical exchanges, Hooks v2, attributed hook
  effects, observation boundaries, cancellation, and finite resource limits.
- Control DTOs and delivery live above core. They do not become hook APIs.
- Native capture is append-oriented and independently versioned from control.
- SAZ is an export/import adapter, not the authoritative storage model.
- Reusable automation compiles to ordinary identified hooks. Callers that do
  not need it install only their own hooks.
- WebSocket messages and frames are not modeled as HTTP body frames.
- Every queue, pause, retained body, file, parser, and transform has a declared
  finite bound. Failure of tooling never weakens TLS or routing policy.
- Hosted CI remains parked under `ci/github-actions/` until a human explicitly
  asks to enable it.

## Phase 1: Attributable capture substrate

Add stable interceptor IDs distinct from display names. Centrally record every
traffic-affecting head/body action with hook ID, chain position, phase, order,
and a redacted structural summary. Expose explicit client-request,
upstream-request, upstream-response, and client-response observation
boundaries. Body bytes remain opt-in; full streaming observation, trailers,
route attempts, sequence gaps, and delivery loss are explicit.

Tests must prove forward/reverse attribution order, unchanged replacements,
short circuits, aborts, plan selection, redaction, trailer handling, bounded
queues, and both buffered and streaming boundary order.

Exit gate: a higher layer can reconstruct what entered and left the proxy and
which hook caused every represented change without inspecting hook internals.

## Phase 2: Experimental control v0

Create separate control-model and control-transport crates. Define owned DTOs
for capability negotiation, exchange events, pause notices, bounded edits,
continue/abort commands, and typed rejections. The first transport is an
in-process bounded broker with correlation IDs, deadlines, cancellation, and a
revision/build handshake. Incompatible peers fail closed; no compatibility or
rolling-upgrade promise exists.

Tests must cover malformed, duplicate, stale, late, saturated, disconnected,
cancelled, and out-of-order commands. Fuzz all untrusted envelope decoding once
serialized transport is introduced.

Exit gate: an external controller can drive Hooks v2 decisions without a
global session map or control types in core.

## Phase 3: Native streaming capture

Create a capture crate with an independently versioned append log. Store
redacted exchange metadata, boundary heads, attributed effects, attempts,
trailers, terminal state, and opt-in body segments while traffic is live. Use
checksummed records, explicit completeness/loss markers, crash-tail recovery,
atomic sealing, quotas, spool thresholds, retention policy, and deterministic
reader validation.

Tests must cover arbitrary write chunking, truncated/corrupt tails, unknown
record kinds, quota exhaustion, observer loss, cancellation, concurrent
exchanges, large bodies, and crash recovery. Capture failures are observable
but cannot fail proxied traffic unless an embedding caller explicitly selects
a fail-closed policy.

Exit gate: a process may be killed during capture and the readable prefix is
recoverable, bounded, and honest about missing data.

## Phase 4: Headless capture and export boundary

Expose native recording, inspection, validation, sealing, and export through
library services and CLI commands. Define exporter/importer traits over the
native reader so formats are adapters. Support streaming native output to a
file or standard output where seek semantics permit; use a bounded temporary
artifact when a destination format requires finalization.

Tests must exercise exit codes, interrupted output, stdout/backpressure,
overwrite refusal, deterministic summaries, redaction defaults, and round
trips through an in-memory exporter.

Exit gate: a future UI and the CLI use the same capture APIs, and no CLI state
is required by the proxy runtime.

## Phase 5: SAZ compatibility

Add an optional SAZ adapter. Export a standards-compatible ZIP session archive
with deterministic session numbering and canonical request/response wire
rendering. Preserve information SAZ cannot represent in optional namespaced
metadata while strict compatibility mode emits only conventional entries.
Conversion consumes a sealed or recovered native capture; native recording
never waits on ZIP central-directory finalization.

Use golden archives and at least one independent consumer for compatibility.
Test duplicate headers, trailers, binary and absent bodies, CONNECT/TLS
metadata, incomplete exchanges, timestamps, ZIP64 thresholds, path safety,
corrupt archives, and deterministic output.

Exit gate: compatible tools open strict exports and native-to-SAZ conversion
does not claim fidelity for information the format cannot carry.

## Phase 6: Optional reusable automation

Create a rules/automation crate that compiles a small declarative model into
ordinary Hooks v2 registrations with stable rule/hook IDs. Direct callers may
continue to install custom hooks and take no dependency on this crate. Record
matched rule ID, produced hook effect, and deterministic conflict/order
resolution. Keep persistence, authoring UX, and project state outside core.

Tests must cover precedence, multiple matches, conflicts, no-op edits,
content-representation requirements, replay safety, budgets, cancellation,
redaction, and deterministic compilation.

Exit gate: common repeatable edits are convenient and auditable without making
a rules engine mandatory or privileged.

## Phase 7: WebSocket inspection

Add a protocol-neutral WebSocket crate and runtime upgrade lifecycle. Model
handshake, direction, frames, fragmented messages, control frames, close state,
and attributed hook decisions separately from HTTP bodies. Enforce masking,
reserved bits, UTF-8, control-frame, message/frame size, decompression, queue,
pause, idle, and close-handshake bounds. Preserve byte-transparent tunneling
when no WebSocket hook is installed.

Tests must include RFC vectors, arbitrary TCP chunking, fragmentation with
interleaved control frames, masking in each direction, close races, malformed
input, permessage-deflate negotiation and bombs, backpressure, cancellation,
and unrelated-connection isolation.

Exit gate: upgraded traffic can be observed and modified through explicit
hooks, while disabling the feature preserves the existing tunnel fast path.

## Stabilization trigger

There is exactly one trigger for control-protocol stabilization: a human
maintainer says that stabilizing it now makes sense and records that decision.
Neither completion of these phases nor external use triggers stability. Until
that decision, documentation labels control v0 experimental, same-build peers
are expected, and incompatibility is rejected rather than guessed.

## Per-phase delivery discipline

Each phase lands in at least one separate commit. Before each phase is marked
complete, run formatting, strict Clippy, locked workspace tests, and the
phase-specific adversarial suite using the configured Clang/LLVM and Ninja
toolchain. Do not activate hosted automation as part of these phases.

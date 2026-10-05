# Plan: Layered monorepo roadmap

Status: active; milestones 0-9 and the standalone interoperability expansion
are locally complete; the product shell is next and hosted Linux/macOS
automation remains intentionally deferred
Audience: maintainers, implementation agents, and reviewers  
Scope: the initial Transmog monorepo and the future repository boundaries it
must preserve

## Mission

Build a Charles/Fiddler-style inspection and modification system from a
library-first proxy engine without moving product state into the networking
core. The code starts in one monorepo so cross-layer changes can be atomic. The
layers must nevertheless have explicit dependency direction, owned data, and
test contracts so they can be split into separately released repositories when
there is a concrete reason to do so.

The monorepo is an implementation convenience, not permission to erase
boundaries.

## Decisions

1. Hooks v2 remains the protocol-neutral exchange lifecycle and modification
   boundary.
2. Content decoding and re-encoding is a separate library layer above core body
   framing and below product rules or interactive editing.
3. Capture, saved rules, UI state, projects, and replay collections do not live
   in `transmog-core`.
4. A control-model boundary may be introduced before it is stable, but core
   structs and incidental Serde representations are never the wire protocol.
5. The control protocol remains experimental and breakable until a human
   explicitly states that stabilizing it now makes sense. Time elapsed, test
   coverage, release count, a repository split, or the existence of multiple
   consumers does not trigger stabilization automatically.
6. If no human makes that decision, compatibility may be deferred
   indefinitely. During that period, producers and consumers are built from
   the same revision and reject incompatible peers.
7. Repository splitting happens for ownership or release reasons, not merely
   because a crate boundary exists.

## Intended layers and dependency direction

```text
transmog-core
  canonical messages, bounded bodies, Hooks v2, routing contracts

transmog-content -> core
  content-coding plans, bounded codecs, representation metadata repair

transmog-tls
  trust snapshots, downstream certificates, verified TLS contexts

transmog-http -> core + tls
transmog-h3   -> core + tls
  transport adapters and upstream pools

transmog-runtime -> core + content + tls + http + h3
  listener, exchange orchestration, retries, shutdown, provider assembly

transmog-control-model -> core
transmog-control-transport -> control-model
  experimental commands/events and in-process or IPC delivery

transmog-capture -> core + control-model
transmog-rules   -> core + content
  durable capture/spooling and reusable automated decisions

CLI / desktop / web products -> public library layers
```

Dependencies point downward. Transport, storage, rule, and UI types do not
appear in core public signatures. Higher layers convert deliberately at their
boundary rather than exporting another layer's internal model as their own
contract.

## Cross-layer invariants

- All queues, buffers, decompression, capture, and pause points have finite
  configured bounds.
- Cancellation and shutdown propagate downward and do not leave detached
  exchange work.
- Original client intent, effective logical messages, authorized destinations,
  and transport attempts remain distinct.
- Credentials and body bytes are excluded from routine telemetry by default.
- A higher-layer failure cannot weaken TLS validation, destination
  authorization, replay safety, or no-DIRECT behavior.
- Persisted data and live control messages have separate schemas and separate
  compatibility decisions.
- Protocol adapters share behavioral contract suites rather than merely
  exposing similarly named methods.
- Public APIs use transport-neutral types unless their crate explicitly owns a
  transport.

## Milestones

### 0. Proxy core and Hooks v2

Status: locally complete; hosted Linux and macOS CI observation is explicitly
deferred.

The result is the typed exchange engine, body plans, observers, routing and
upstream seams, provider interfaces, bounded decision bridge, embedding
example, and H1/H2/H3 compatibility matrix described by the Hooks v2 plan.

Exit gate: every local Hooks v2 definition-of-done item passes. Hosted
Linux/macOS observation remains a deferred acceptance gate after automation is
explicitly enabled.

### 1. Content processing

Status: locally complete; hosted automation observation is deferred.

Implement strict content-coding parsing, bounded streaming decode/re-encode,
representation header repair, and composition around body hooks. Support gzip,
Brotli, deflate, and zstd under explicit policy. Preserve a raw pass-through
path that does not recompress untouched bodies.

Exit gate: the dedicated [content-processing plan](content-processing-plan.md)
is complete across H1/H2/H3 requests and responses.

### 2. Attributable capture substrate

Status: complete.

Expose stable hook identity, attributed traffic effects, explicit observation
boundaries, opt-in full body streams, trailers, route attempts, and visible
loss. This is the core-owned vocabulary that enables storage and control
without making either a core responsibility.

Exit gate: higher layers can reconstruct observable exchange evolution and
attribute represented changes to hooks.

### 3. Experimental control model and transport

Status: complete as breakable same-build v0; stabilization is not requested.

Define product-facing v0 commands and events for exchange discovery,
breakpoint pause/resume, bounded body access, modification, cancellation, and
capability discovery. Keep the semantic source of truth in Hooks v2 and adapt
through `DecisionBridge`-style correlation.

The first transport may be in-process. If a process boundary becomes useful,
add a handshake containing an experimental protocol revision and build
identity. Peers from incompatible builds fail closed with a clear diagnostic;
there is no rolling-upgrade promise in v0.

Exit gate: malformed, duplicate, stale, late, saturated, disconnected, and
cancelled commands have deterministic behavior, and no transport DTO leaks
into core.

### 4. Native streaming capture

Status: complete.

Add append-oriented capture interfaces, metadata indexing, opt-in body
retention, memory-to-disk spooling thresholds, retention/deletion policy, and
redaction. The first store may live in the monorepo, but storage records own a
schema independent from control messages.

Exit gate: crashes, quota exhaustion, cancellation, and observer loss cannot
corrupt traffic or create unbounded storage growth.

### 5. Headless capture and export boundary

Status: complete.

Expose recording, inspection, validation, sealing, and format adapters through
library services and a CLI. Native output streams directly; formats requiring
finalization may use bounded temporary storage.

Exit gate: the headless and future graphical products share the same capture
API and exporters do not leak into the proxy core.

### 6. SAZ compatibility

Status: complete.

Convert native captures into a conventional Session Archive Zip. Keep strict
compatibility distinct from optional namespaced metadata, and never make SAZ
the live persistence format.

Exit gate: golden archives open in an independent compatible consumer and
unsupported fidelity is reported explicitly.

### 7. Optional rules and replayable automation

Status: complete.

Build a deterministic rule layer over Hooks v2 and content processing. Rules
match immutable/effective metadata explicitly, declare whether they need body
content, and compile to typed actions. Saved-rule persistence belongs here, not
in core.

Exit gate: rule ordering, conflicts, resource budgets, redaction, and replay
safety pass deterministic suites, with no hidden global session map.

### 8. WebSocket inspection

Status: complete for HTTP/1.1 Upgrade; advanced transports remain optional.

Add an upgrade/tunnel lifecycle and bounded frame model distinct from HTTP body
frames. Preserve fragmentation, control-frame constraints, masking semantics,
close handshakes, compression negotiation, and per-direction flow control.

Exit gate: HTTP upgrade handling and WebSocket frame processing are separately
testable and a paused connection cannot block unrelated traffic.

### 9. Headless application/session service

Status: complete.

Compose runtime, control, capture, automation, and WebSocket evidence into a
bounded application-owned service. It owns proxy run state, a finite searchable
live-session catalog, visible event loss, controller attachment, capture
lifecycle, and validated replay/composer requests. Host certificate and system
proxy changes remain explicit adapters so library consumers do not acquire
desktop side effects.

Exit gate: the dedicated
[application/session service plan](application-session-service-plan.md) is
complete, can be exercised without a UI, and an idle or disconnected consumer
cannot stall or silently weaken proxy traffic.

### 10. Product shell

Status: active; dependency and delivery spike complete.

Build the Windows-first desktop shell with Tauri, the operating-system WebView,
Rust-rendered Microsoft WebUI templates, and TypeScript only for authored
browser-side behavior. Add session browsing, breakpoint controls,
inspectors/editors, search, export, and diagnostics against the application
service. UI state never becomes proxy correctness state. The dedicated
[Tauri and WebUI product shell plan](tauri-webui-product-shell-plan.md) defines
the layering, security model, platform gates, and implementation phases. The
follow-on [traffic inspection, automation, and sandboxed scripting plan](traffic-inspection-automation-scripting-plan.md)
defines the Fiddler-style workspace, default-on circular response retention,
conditional rules, autoresponse, isolated V8 execution, Monaco authoring, and
safe preview work.

Exit gate: closing or restarting the UI cannot silently disable proxy safety,
and headless embedding remains fully supported.

### 11. Optional advanced transports

Evaluate native downstream HTTP/3, CONNECT-UDP, MASQUE, WebTransport, blind TCP
tunneling, and reverse-listener products as separate milestones. None is
implied by origin HTTP/3 support or provider seams.

## Experimental control-protocol policy

Before stabilization:

- the protocol is named experimental v0;
- breaking changes are allowed and coordinated atomically in the monorepo;
- producer and consumer normally ship from the same build;
- a revision/build handshake rejects mismatches instead of guessing;
- no compatibility adapter, deprecation window, or migration is promised;
- persisted captures do not store live protocol envelopes as their durable
  schema;
- documentation states that third-party clients assume breakage.

Stabilization has exactly one trigger: a human maintainer explicitly records
that stabilizing the protocol now makes sense. This decision should normally be
captured in an ADR, but the human decision—not completion of an automated
checklist—is authoritative. Until that happens, v0 may remain breakable
indefinitely.

Once triggered, stop feature work on the protocol long enough to:

1. inventory every producer, consumer, persisted reference, and deployment
   topology;
2. write compatibility and support-window policy;
3. define canonical serialization and unknown-field behavior;
4. add conformance fixtures and cross-version tests;
5. separate protocol version from product/build version;
6. publish a reviewed v1 schema and migration policy.

## Repository split policy

A crate is a candidate for a separate repository only when independent
ownership, security review, release cadence, distribution, or external reuse
justifies the operational cost. Before a split:

- dependency direction is already acyclic;
- public APIs and feature flags are documented;
- shared contract tests can run from either side of the boundary;
- fixtures do not rely on unpublished sibling internals;
- versioning and coordinated-release expectations are explicit;
- license, notices, and security-policy ownership are assigned.

A split does not implicitly stabilize the experimental control protocol. The
same-build rule may continue across coordinated repositories until a human
separately triggers stabilization.

## Immediate sequence

1. Complete Phase 1 of the
   [Tauri and WebUI product shell plan](tauri-webui-product-shell-plan.md): add
   the UI-neutral application facade and independent WebUI renderer crate,
   then establish the shell/status/diagnostics foundation and graceful close.
2. Keep the standalone interoperability, live Chromium, and local Linux gates
   passing. When a human explicitly enables hosted automation, activate and
   observe the parked Linux/macOS templates and close platform-specific
   findings.

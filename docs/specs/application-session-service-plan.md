# Plan: Headless application/session service

Status: active
Audience: maintainers, embedders, CLI authors, and future product-shell authors
Depends on: Hooks v2, content processing, experimental control v0, native
capture, automation, and WebSocket inspection

## Mission

Provide one UI-independent application boundary that composes the completed
library layers into a running developer-tool session. The service owns
application lifecycle and bounded live state; it does not move product state,
storage policy, or operating-system side effects into the proxy core.

The same API must support a desktop shell, a headless command, and an embedding
application. Control v0 remains same-build and breakable until a human
explicitly requests stabilization.

## Ownership and boundaries

The new `rustymiddle-session` crate may depend on runtime, core, content,
control-model, control-transport, capture, and WebSocket public APIs. None of
those lower layers may depend on it.

The service owns:

- start, stop, status, and terminal failure reporting for one proxy run;
- a finite live-session catalog assembled from immutable observer events;
- deterministic filtering, ordering, pagination, and point lookup;
- bounded body retention selected explicitly by the caller;
- visible observer sequence gaps, catalog eviction, and subscriber lag;
- dynamic native-capture start, stop/seal, status, and failure reporting;
- same-build controller attachment and the Hooks v2 control adapter;
- validated replay/composer requests through a caller-supplied executor;
- explicit host-integration transactions through caller-supplied adapters.

The service does not own:

- graphical widgets, navigation, editor state, or layout;
- a stable network control protocol;
- system proxy or certificate-store mutation implementations;
- saved project/rule persistence or an analytics database;
- unbounded body history or an implicit global singleton;
- reverse-proxy, ACME, load-balancing, or advanced tunnel behavior.

## Phase 1: Bounded live catalog

Create immutable session snapshots keyed by core exchange ID. Track original
metadata, observed request/response heads, byte counts, trailers, attributed
hook effects, route attempts, and terminal outcome. Configuration must bound
session count, retained body bytes per exchange, event subscribers, and query
page size.

Eviction is deterministic: terminal sessions are oldest-first candidates;
active sessions are never silently discarded to admit a new session. Rejected
admission, observer sequence gaps, and broadcast lag have monotonic counters.
Queries use stable ordering and an opaque continuation cursor rather than
returning the complete catalog.

Exit gate: concurrent and out-of-order events cannot corrupt a snapshot,
credentials remain redacted by the observer boundary, and every memory owner
has a finite configured limit.

## Phase 2: Capture and subscriptions

Add a service observer that updates the catalog, publishes bounded session
deltas, and optionally appends native capture records. Capture start uses
create-new semantics, capture stop seals and flushes, and writer failures become
visible service state without failing traffic. Starting a second capture,
stopping an idle capture, quota exhaustion, and shutdown races are typed.

Exit gate: a shell may subscribe for hints and recover authoritative state by
query after lag; capture interruption never corrupts catalog state or proxy
traffic.

## Phase 3: Proxy and controller lifecycle

Bind the service observer into `ProxyComponents` without replacing caller
observers. Own the runtime task and expose explicit `Stopped`, `Running`,
`Stopping`, and `Failed` states. Start-twice and concurrent stop are
deterministic; stop waits for the runtime's bounded drain and seals an active
capture.

Adapt experimental control DTOs to an identified Hooks v2 interceptor. Head
and body edits remain phase-typed, bounded, authority-safe, cancellable, and
auditable. Controller timeout, saturation, disconnect, stale reply, and invalid
action fail closed for only the affected exchange.

Exit gate: a headless controller can observe, pause, continue, edit, or abort
traffic through the service without control types leaking into core.

## Phase 4: Replay/composer and host integration seams

Define an owned, validated replay request with finite headers/body, normalized
target, explicit replay-risk acknowledgement for non-idempotent methods, and no
ambient credentials. Execute through a caller-supplied asynchronous executor so
the service does not invent a second network stack or bypass route policy.

Define transactional host-integration hooks for system-proxy/certificate setup.
The service records an opaque restore token after apply and invokes exact
restore during normal stop. Apply failure prevents proxy publication; restore
failure is visible and retryable. The crate ships no OS-mutating implementation.

Exit gate: validation, executor timeout/cancellation, start rollback, exact
restore, and retryable restore failure pass deterministic tests.

## Phase 5: Hardening and handoff

Add concurrency, saturation, cancellation, shutdown, capture-quota, and catalog
eviction stress tests. Document embedding, product-shell consumption, privacy
defaults, and remaining limitations. Run format, strict Clippy, locked workspace
tests, dependency policy, one-BoringSSL graph, the standalone interoperability
suite, and the local Linux matrix where applicable. Hosted automation remains
disabled.

## Definition of done

- [ ] Catalog bounds, deterministic eviction, queries, and loss counters pass.
- [ ] Bounded delta subscriptions recover through authoritative queries.
- [ ] Dynamic native capture is create-new, sealable, and failure-isolated.
- [ ] Proxy lifecycle is explicit, idempotent, and drains on stop.
- [ ] Control v0 drives identified Hooks v2 head/body decisions safely.
- [ ] Replay/composer validation and executor isolation pass.
- [ ] Host integration apply/restore is transactional and retryable.
- [ ] Public APIs contain no UI, transport-private, or OS-specific types.
- [ ] Rustdoc and embedding documentation are complete.
- [ ] Windows LLVM/Ninja and local interoperability gates pass.
- [ ] Hosted CI remains inactive until a human explicitly enables it.

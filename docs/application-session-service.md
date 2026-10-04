# Application/session service

`rustymiddle-session` is the UI-independent application boundary above the
proxy runtime. It composes a bound proxy, immutable observer evidence,
experimental same-build control, streaming native capture, replay/composer, and
caller-owned host setup without moving product state or operating-system policy
into the lower libraries.

The crate is suitable for a desktop shell, a headless command, or an embedding
application. It contains no window, navigation, database, or OS-specific types.

## Layering

The dependency direction is one way:

```text
desktop / CLI / embedding application
                 |
       rustymiddle-session
        /       |        \
 runtime   control/capture  core evidence
    |
HTTP, H3, TLS, content, WebSocket adapters
```

Lower crates do not depend on the session service. A product may bypass this
crate and compose the lower layers directly, or may use the service while still
installing its own hooks, observers, route selector, upstream service, content
policy, certificate resolver, and WebSocket hooks.

## Embedding flow

1. Create an `ApplicationSessionService` with finite `ServiceConfig` limits.
2. Build the application's `ProxyComponents` as usual.
3. Call `prepare_components` exactly once. It appends the service observer and
   the stable `rustymiddle.session.interactive-control` hook; it does not
   replace caller registrations.
4. Bind `ProxyServer` with the returned components.
5. Call `start`, or `start_with_host` with a caller-owned transactional adapter.
6. Query or subscribe to the catalog, attach an experimental controller, and
   start or stop captures as needed.
7. Call `stop` and inspect its result. Stop drains the runtime, then seals an
   active capture and restores host state.

`start` consumes an already-bound server so listener, trust, content, and route
policy remain runtime concerns. `proxy_control` exposes the runtime's existing
generation-scoped trust-reload and observer-statistics handle while the run is
owned. Start-twice is typed; concurrent stops serialize and become idempotent.
An unexpected runtime failure remains visible until the failed run is stopped.

Dropping the last service owner cancels its task, closes the bounded capture
worker, and invokes an armed host restore guard. Explicit `stop` remains the
required normal path because it waits for drain and reports capture or restore
failure instead of making it best effort.

## Live catalog and subscriptions

The catalog stores immutable snapshots in admission order under a configured
session-count limit. A new exchange evicts the oldest terminal session. It
never silently removes an active exchange; admission is rejected and counted
when all retained entries are active.

Each snapshot may contain:

- immutable original metadata and redacted boundary heads;
- per-boundary observed byte counts and a bounded retained prefix;
- terminal trailers, optional-hook diagnostics, attributed hook effects,
  route selection, and bounded route attempts;
- completed or failed HTTP outcome and optional terminal WebSocket evidence.

Observer redaction happens below the service. Authorization, proxy
authorization, cookies, and set-cookie values therefore cannot enter the
catalog through the runtime observer path. Body retention is opt-in through a
finite per-session limit.

Queries use stable admission order, deterministic filters, a capped page size,
and an opaque continuation cursor. The bounded broadcast is only a change hint.
A slow subscriber receives `Lagged(count)` and must page authoritative state;
subscriber lag, observer sequence gaps, stale events, rejected admission,
eviction, bounded-detail loss, and WebSocket evidence lag are monotonic and
visible.

## Dynamic capture

`CaptureManager` owns one dedicated bounded writer queue. Starting capture uses
create-new file semantics and never overwrites an artifact. Stopping appends the
native seal and flushes. Starting twice and stopping while idle are typed.

The service observer never performs file I/O. It updates the catalog and tries
to enqueue the already-redacted event. Queue saturation, quota exhaustion, and
writer errors move capture to a visible failed state without changing proxy
traffic or corrupting catalog state. The native append format retains a
recoverable valid prefix after interruption. Capture policy independently
controls body-sample retention.

## Interactive control

The service adapts experimental control v0 into one identified Hooks v2
interceptor. The caller selects an exact set of request-head, request-body,
response-head, and response-body phases; there is no built-in rules engine.
Body phases use decoded-required buffering under the negotiated finite limit.

Every replacement or abort is attributed to the interactive hook by the core
audit trail and therefore appears in observers and capture. Request-head edits
may change method, path, query, and fields, but cannot silently change scheme or
authority. Invalid phase actions, malformed heads, timeout, saturation,
disconnect, and stale replies fail closed for the affected exchange. Other
exchanges and catalog/capture observation remain independent.

Only one controller lease may attach at a time. Dropping the lease permits a
new attachment. Revision zero requires an exact build ID and remains breakable
indefinitely until a human maintainer explicitly records that stabilization now
makes sense.

## Replay/composer

`ReplayRequest` owns its method, normalized target, ordered headers, and bounded
body. Validation rejects malformed or inconsistent targets, invalid methods,
oversized headers or bodies, non-idempotent methods without explicit risk
acknowledgement, and credential-bearing headers without explicit confirmation.
The service never copies ambient credentials into a replay.

Execution goes through a caller-supplied `ReplayExecutor`. An application
should implement it over the same route policy and upstream stack used for
normal traffic. The service does not create a second network stack. Execution
is contained by cancellation, timeout, panic/task failure, and finite response
header/body validation.

## Host integration

`HostIntegration` is a transactional seam, not an implementation. The caller
may use it for system-proxy or certificate-store setup. `apply` receives the
already-bound endpoint and returns an opaque restore token containing the exact
prior state. Apply completes before the runtime is published and must roll back
its own partial work on error.

The service retains the token and calls `restore` on normal stop. Restore must
be safe to retry. If it fails, the same token is retained, restart is blocked,
and `retry_host_restore` retries exact restoration. The armed token also has a
last-owner drop guard. No Windows, macOS, or Linux mutation implementation is
shipped by this crate, and no process-global singleton is assumed.

## Deliberate exclusions

The service is not a capture database, saved-rule store, project model, stable
IPC server, authentication vault, or UI state container. Its catalog is finite
live state, subscriptions are lossy hints, native capture is the streaming
durable record, and SAZ remains a finalized compatibility export. Durable
indexing, project persistence, command-line option design, and product UX
belong in layers above it.

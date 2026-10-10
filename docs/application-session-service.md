# Application/session service

`transmog-session` is the UI-independent application boundary above the
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
       transmog-session
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

1. Create an `ApplicationSessionService` with the chosen `ServiceConfig`
   retention, per-entry and queue limits.
2. Build the application's `ProxyComponents` as usual.
3. Call `prepare_components` exactly once. It appends the service observer and
   the stable `transmog.session.interactive-control` hook; it does not
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

The catalog stores immutable snapshots in admission order. There is no live-entry
count cap by default. An optional maximum evicts the oldest completed live entries;
active requests may temporarily exceed it and imported entries are preserved.
Changing the maximum applies immediately. Per-entry samples and details, query
pages and subscription queues retain their separate bounds. See
[the catalog model](../crates/proxy-session/src/catalog.rs) for configuration.

Each snapshot may contain:

- immutable original metadata and boundary heads under the selected privacy policy;
- per-boundary observed byte counts and a bounded retained prefix;
- terminal trailers, optional-hook diagnostics, attributed hook effects,
  route selection, and bounded route attempts;
- completed or failed HTTP outcome and optional terminal WebSocket evidence.

The service defaults to redacting sensitive header values before catalog and
capture publication. `set_redact_sensitive_headers` selects the caller's policy;
the desktop and guided CLI use persistent preferences and collect sensitive
headers by default. Runtime registrations separately select sensitive-header and
body interest. Catalog body samples are opt-in with a per-session limit; product
body storage sits above this metadata catalog.

Queries use stable admission order, deterministic filters, a capped page size,
and an opaque continuation cursor. The bounded broadcast is only a change hint.
A slow subscriber receives `Lagged(count)` and must page authoritative state;
subscriber lag, observer sequence gaps, stale events, eviction,
bounded-detail loss and WebSocket evidence lag are monotonic and
visible.

The service observer waits for capacity in its bounded event queue. Storage
pressure slows request processing instead of discarding authoritative evidence.
Application-owned retention callbacks have no time deadline while waiting for
storage; panic and cancellation containment remain active. Whole-catalog
presentation queries select admission order first, then snapshot entries one at
a time and format them outside the writer lock, allowing ingestion to continue.

An event reserves queue capacity across its observer registrations before
publishing or advancing exchange state. Cancelling a capacity wait releases
the reservations and leaves sequence and terminal state unchanged, so a retry
or failure cleanup can still deliver evidence. A cancelled batch preserves its
already committed prefix without leaving a sequence gap for its pending event.

## Dynamic capture

`CaptureManager` owns one dedicated bounded writer queue. Starting capture uses
create-new file semantics and never overwrites an artifact. Stopping appends the
native seal and flushes. Starting twice and stopping while idle are typed.

The service observer never performs file I/O. It enqueues the privacy-filtered
event and updates the catalog. A full writer queue waits for capacity off the
async executor and propagates pressure back to requests. Configured quota
exhaustion and writer errors move capture to a visible failed state. The native
append format retains a recoverable valid prefix after interruption. Capture policy independently
controls body-sample retention.

## Interactive control

The service adapts experimental control v0 into one identified interceptor. The
caller selects an exact set of request-head, request-body,
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

Execution goes through a caller-supplied `ReplayExecutor`, which explicitly
implements both owned-body and file-stream dispatch or reports that streaming
is unavailable. Product file bodies stay outside the bounded in-memory request
editor. Implement this over the same route policy and upstream stack used for
normal traffic. The service does not create a second network stack. Execution
is contained by cancellation, timeout, panic/task failure, and finite response
header/body validation.

## Host integration

`HostIntegration` is a transactional seam, not an implementation. The caller
may use it for system-proxy or certificate-store setup. `apply` receives the
already-bound endpoint and returns an opaque restore token containing the exact
prior state. Apply completes before the runtime is published and must roll back
its own partial work on error.

For the desktop's reversible off transition, `begin_drain` restores host state
first and lets admitted work finish without a deadline. `resume_drain` reapplies
host configuration and invalidates that pending shutdown while preserving the
same run. Explicit `stop` uses the runtime's bounded shutdown and seals capture.
The [service implementation](../crates/proxy-session/src/service.rs) owns transition
serialization and restoration failures.

The service retains the token and calls `restore` on normal stop. Restore must
be safe to retry. If it fails, the same token is retained, restart is blocked,
and `retry_host_restore` retries exact restoration. The armed token also has a
last-owner drop guard. No Windows, macOS, or Linux mutation implementation is
shipped by this crate, and no process-global singleton is assumed.

## Deliberate exclusions

The service is not a capture database, saved-rule store, project model, stable
IPC server, authentication vault or UI state container. Its catalog is live
metadata with configurable retention, subscriptions are lossy hints and native
capture is the durable record. Saved-file indexing, project organization,
command-line policy and product UX belong above it.

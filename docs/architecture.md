# Architecture

Transmog is a desktop web-debugging application built over a layered exchange
engine and explicit-proxy listener.
Browser traffic currently enters as plain absolute-form HTTP or as HTTP/1.1 or
HTTP/2 inside an intercepted `CONNECT`. HTTP/3 is an origin egress protocol;
the project does not claim to intercept a browser's native QUIC packets.

```text
explicit H1 listener / CONNECT TLS (H1 or H2)
                    |
                    v
          canonical exchange engine
       metadata, cancellation, limits
                    |
          request: A -> B -> C
                    |
        bounded request body pipeline
                    |
     route selector + destination policy
                    |
       +------------+-------------+
       |                          |
 Hyper H1/H2 or quiche H3    application UpstreamService
       |                          |
       +------------+-------------+
                    |
        response: C -> B -> A
                    |
       bounded response body pipeline

HTTP/1.1 101 upgrade
        |
        +-- no WebSocket hooks --> byte-transparent bidirectional copy
        |
        +-- hooks installed ----> bounded frame/message/compression relay
```

Observers receive redacted immutable events beside the HTTP path through finite
queues; they cannot mutate traffic.

`transmog-session` sits above this complete path. It turns observer evidence
into a finite live catalog, owns one runtime task and dynamic capture worker,
adapts experimental control decisions into an identified hook, and exposes
injected replay and transactional host-integration seams. The layer is
UI-independent and no lower crate depends on it.

## Layer map

The workspace keeps dependency direction explicit even though all layers share
one repository:

- `transmog-core`: canonical messages and bodies, interception, observation,
  routing contracts, and exchange metadata;
- `transmog-content`: content-coding plans, codecs, bounds, and representation
  metadata repair;
- `transmog-tls`, `transmog-http`, `transmog-h3`, and `transmog-websocket`:
  trust/certificate policy and protocol adapters;
- `transmog-client-identity`: bounded operating-system caller attribution;
- `transmog-runtime`: listener, exchange orchestration, upstream pools,
  retries, shutdown, and provider assembly;
- `transmog-control-model` and `transmog-control-transport`: breakable
  same-build interactive commands and delivery;
- `transmog-capture` and `transmog-saz`: native streaming records and finalized
  compatibility export;
- `transmog-automation`, `transmog-script`, `transmog-script-supervisor`, and
  `transmog-script-host`: declarative and sandboxed programmable behavior;
- `transmog-session`: UI-independent live application lifecycle;
- `transmog-app` and `transmog-app-webui`: persisted product workspaces, safe
  presentation models, and Rust-rendered UI;
- `transmog-host-windows`, `transmog-process-sandbox`, and
  `transmog-preview-worker`: explicit OS integration and isolated helpers; and
- `transmog`, `transmog-desktop`: headless command-line and Windows desktop
  products.

Dependencies point toward lower layers. Transport objects do not appear in core
interception contracts, and product persistence or UI types do not appear in
runtime or protocol APIs.

## Interception lifecycle

`transmog-core` exposes transport-neutral, typed lifecycle callbacks. An
`InterceptorFactory` creates one `ExchangeInterceptor` per exchange. Request
callbacks run in registration order and response and terminal callbacks unwind
in reverse order. A short-circuit at interceptor B never enters C; a local
response from B still passes through the response hooks for B and A. Completion
or failure is delivered once to every entered interceptor.

Body behavior is selected before a pump starts. `PassThrough`, `Transform`,
`Buffer`, `Replace`, and `Discard` all use canonical frames and explicit bounds.
Hyper, quiche, Tokio channels, and TLS connection objects do not appear in hook
signatures. Pass-through streams before producer completion; complete-body
editing is available only through an explicit nonzero limit.

`transmog-content` is the next layer above canonical body framing. It owns
content-coding plans, representation header repair, decompression budgets, and
bounded streaming gzip/Brotli/deflate/zstd codec engines. Its content-aware
pipeline composes those engines around the interception body pipeline without
moving compression policy into Hyper, quiche, or core lifecycle types.
Interceptors declare neutral, raw, required-decoded, or optional-decoded
representation requirements; raw/decoded conflicts fail before body processing
begins.
Codec operations use content-layer byte quanta, cooperative executor yields,
per-call deadlines, and cumulative active-work deadlines. Timeout is a typed
terminal content failure; transports neither schedule nor reinterpret it.

The immutable `OriginalTarget` records client intent. Head edits are logical
message edits and cannot silently redirect a socket. A `Reroute` action is fed
to a `RouteSelector`, which applies a `DestinationAuthorizer` and returns an
auditable `UpstreamPlan`. Pool keys include destination, HTTP policy, trust
generation, TLS policy, and connector policy.

## Embedding boundaries

`ProxyComponents` freezes the application-owned pieces before bind:

- an immutable interceptor chain;
- an immutable, finite content-processing policy (disabled by default);
- a bounded `ObserverHub`;
- an optional custom `RouteSelector`;
- an optional canonical streaming `UpstreamService`;
- an optional protocol-neutral `WebSocketHookFactory` (empty by default);
- a downstream `DownstreamCertificateResolver`;
- a bounded, best-effort downstream `ClientIdentityResolver`;
- clock and ID providers for deterministic tests or host integration.

The convenience `ProxyServer::bind` adapts a `ProxyCa` through the bounded MITM
certificate resolver. `ProxyServer::bind_with_components` has no CA parameter,
so a future reverse listener can use supplied certificate material without
weakening CONNECT/SNI identity validation. The default network upstream keeps
trust snapshots generation-scoped. Reload builds complete replacement Hyper and
quiche clients; in-flight exchanges retain their previous generation.
`HyperUpstreamService` and `H3UpstreamService` also expose those pooled network
clients through the same canonical boundary used by application services. They
validate the authorized destination against the complete normalized request
target before any network I/O; fallback policy stays above the individual
adapter.

The default client-identity resolver snapshots the process associated with a
new loopback TCP connection on Windows and Linux. Non-loopback peers are marked
remote, lookup failures do not fail traffic, and embedders can replace the
resolver. The snapshot is part of immutable exchange metadata and is therefore
available to hooks, observers, the application session catalog, and native
capture. See [client process attribution](client-process-attribution.md) for
platform behavior and security constraints.

The core owns protocol correctness state: exchange IDs, framing and flow
control, route attempts, retry eligibility, connection pools, deadlines,
cancellation, body bounds, pause permits, trust generation, certificate and
Alt-Svc caches, and exactly-once terminal state. Capture databases, saved rules,
UI state, IPC schemas, projects, replay collections, and scripting runtimes
belong above the core.

The headless application/session service owns only live application lifecycle:
bounded searchable snapshots, lossy delta hints, capture start/seal state,
same-build controller attachment, and proxy run status. Durable databases,
saved projects, command-line policy, and UI selection/editor state remain above
that service. See [the service guide](application-session-service.md). The
desktop-specific facade, sandbox helpers, and presentation layer sit above that
service; see [the product shell](product-shell.md).

## Resource and failure model

`RuntimeLimits`, `HookLimits`, `ContentLimits`, and `H3TransportLimits` make
every queue, buffer, deadline, connection count, and paused-callback count
finite. A slow observer uses one declared delivery policy: bounded
backpressure, drop-newest with a visible counter, or disconnect. Body bytes and
credential fields are excluded or redacted by default.

Hook, route-selector, upstream-service, body-filter, and observer callbacks run
behind panic, timeout, cancellation, and drop containment. Dropping a boundary
future aborts its spawned task. A failing observer never changes traffic.
Traffic-affecting failures are typed and fail closed; trust or certificate
errors are never converted to success by a hook default.

WebSocket inspection uses a separate session-local hook chain. Its two traffic
directions are independently scheduled and backpressured. Disabling those
hooks selects an exact byte-copy path, so merely linking the WebSocket crate
does not reinterpret upgraded traffic.

Automatic routing streams directly when no HTTP/3 alternative is cached. With
a cached alternative it uses an explicitly bounded replay buffer. Fallback is
allowed only for replayable requests before response commitment. Hyper pools
HTTP/1.1 and HTTP/2 connections; the quiche adapter multiplexes bounded streams
and segregates pools by origin, advertised peer, and trust generation.

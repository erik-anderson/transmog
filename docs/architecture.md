# Architecture

rustymiddle is a library-first exchange engine with an explicit-proxy listener.
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

Observers receive redacted immutable events beside this path through finite
queues; they cannot mutate traffic.
```

## Hooks v2

`rustymiddle-core` exposes transport-neutral, typed lifecycle callbacks. An
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

The immutable `OriginalTarget` records client intent. Head edits are logical
message edits and cannot silently redirect a socket. A `Reroute` action is fed
to a `RouteSelector`, which applies a `DestinationAuthorizer` and returns an
auditable `UpstreamPlan`. Pool keys include destination, HTTP policy, trust
generation, TLS policy, and connector policy.

## Embedding boundaries

`ProxyComponents` freezes the application-owned pieces before bind:

- an immutable interceptor chain;
- a bounded `ObserverHub`;
- an optional custom `RouteSelector`;
- an optional canonical streaming `UpstreamService`;
- a downstream `DownstreamCertificateResolver`;
- clock and ID providers for deterministic tests or host integration.

The convenience `ProxyServer::bind` adapts a `ProxyCa` through the bounded MITM
certificate resolver. `ProxyServer::bind_with_components` has no CA parameter,
so a future reverse listener can use supplied certificate material without
weakening CONNECT/SNI identity validation. The default network upstream keeps
trust snapshots generation-scoped. Reload builds complete replacement Hyper and
quiche clients; in-flight exchanges retain their previous generation.

The core owns protocol correctness state: exchange IDs, framing and flow
control, route attempts, retry eligibility, connection pools, deadlines,
cancellation, body bounds, pause permits, trust generation, certificate and
Alt-Svc caches, and exactly-once terminal state. Capture databases, saved rules,
UI state, IPC schemas, projects, replay collections, and scripting runtimes
belong above the core.

## Resource and failure model

`RuntimeLimits`, `HookLimits`, and `H3TransportLimits` make every queue, buffer,
deadline, connection count, and paused-callback count finite. A slow observer
uses one declared delivery policy: bounded backpressure, drop-newest with a
visible counter, or disconnect. Body bytes and credential fields are excluded
or redacted by default.

Hook, route-selector, upstream-service, body-filter, and observer callbacks run
behind panic, timeout, cancellation, and drop containment. Dropping a boundary
future aborts its spawned task. A failing observer never changes traffic.
Traffic-affecting failures are typed and fail closed; trust or certificate
errors are never converted to success by a hook default.

Automatic routing streams directly when no HTTP/3 alternative is cached. With
a cached alternative it uses an explicitly bounded replay buffer. Fallback is
allowed only for replayable requests before response commitment. Hyper pools
HTTP/1.1 and HTTP/2 connections; the quiche adapter multiplexes bounded streams
and segregates pools by origin, advertised peer, and trust generation.

# Architecture

Transmog is a desktop web-debugging application built over a layered exchange
engine and explicit-proxy listener.
Browser traffic currently enters as plain absolute-form HTTP or as HTTP/1.1 or
HTTP/2 inside an intercepted `CONNECT`. HTTP/3 is an origin egress protocol;
native downstream QUIC is unsupported. With HTTPS decryption disabled, CONNECT
relays opaque bytes and records only tunnel metadata.

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

H1 Upgrade / H2 extended CONNECT
        |
        +-- no hooks --> byte-transparent relay, passive observation if requested
        |
        +-- hooks ----> inspected frame/message/compression relay
```

Observers receive immutable events beside the HTTP path through finite queues;
they cannot mutate traffic. Each registration selects body interest and sensitive
header access, with redaction as the library default.

`transmog-session` sits above this complete path. It turns observer evidence
into a live catalog with configurable retention, owns one runtime task and dynamic
capture worker, adapts experimental control decisions into an identified hook, and exposes
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

`transmog-core` exposes transport-neutral lifecycle callbacks. Factories create
one interceptor per exchange; request hooks run in registration order, while
response and terminal hooks unwind through entered interceptors in reverse.
Body plans select bounded pass-through, transform, buffer, replace or discard
behavior before pumping. See [interception lifecycle](interception-lifecycle.md)
for short circuits, exactly-once cleanup, observation and cancellation.

`transmog-content` composes gzip/Brotli/deflate/zstd processing around canonical
body frames. It owns representation negotiation, header repair, decompression
budgets and cooperative work limits; transports do not reinterpret codec errors.
[Embedding](embedding.md) explains the opt-in content policies and limits.

Client intent stays in immutable `OriginalTarget`. Editing a message head cannot
silently redirect a socket: rerouting goes through `RouteSelector` and
`DestinationAuthorizer` to an auditable `UpstreamPlan`. Pool identities include
destination, HTTP policy, trust generation, TLS and connector policy.

## Embedding boundaries

`ProxyComponents` fixes the interceptor chain, content policy, observers and
providers before bind. Hosts may supply route selection, a canonical streaming
`UpstreamService`, WebSocket hooks, certificate/client-identity resolvers, clocks
and IDs. Hyper, quiche and TLS connection objects stay outside hook signatures.
See [embedding](embedding.md) for construction and provider obligations.

Built-in network adapters validate authorized destinations before I/O. The
runtime owns replay/fallback policy and [connection racing](networking.md).
Trust reload builds replacement clients; in-flight exchanges retain their old
generation. [Caller attribution](client-process-attribution.md) is a bounded,
best-effort metadata snapshot and never authorizes traffic.

Core/runtime/protocol layers own framing, flow control, retries, pools, deadlines,
body bounds, pause permits, trust generation and terminal state. The
[session service](application-session-service.md) adds the live catalog, capture,
controller and host-integration lifecycle. Durable workspaces, UI selection,
IPC schemas and scripting belong above those layers in the application and
[desktop shell](product-shell.md).

## Resource and failure model

`RuntimeLimits`, `HookLimits`, `ContentLimits` and `H3TransportLimits` bound
transport queues, connections, processing and paused callbacks. Product
[retention and privacy](product-state-and-support.md#retention-and-privacy) are
separate policies, not a total process-memory limit.

Observers choose finite delivery policies. Desktop retention and CLI recording
wait for storage capacity, slowing traffic rather than dropping evidence under
pressure. Library observers omit bodies and redact sensitive headers by default;
products explicitly choose their capture privacy settings.

Hook, route-selector, upstream-service, body-filter, and observer callbacks run
behind panic, timeout, cancellation, and drop containment. Application-owned
retention observers explicitly omit callback deadlines so a slow storage write
does not disconnect retention. Dropping a boundary future aborts its spawned
task. Observer failures do not rewrite traffic.
Traffic-affecting failures are typed and fail closed; trust or certificate
errors are never converted to success by a hook default.

WebSocket inspection uses a separate session-local hook chain. Its two traffic
directions are independently scheduled and backpressured. Disabling those
hooks preserves the wire bytes. Observers can passively decode messages without
changing forwarding; without observers the relay performs an exact byte copy.

Automatic routing streams directly when no HTTP/3 alternative is cached. With
a cached alternative it uses an explicitly bounded replay buffer. Fallback is
allowed only for replayable requests before response commitment. Hyper pools
HTTP/1.1 and HTTP/2 connections; the quiche adapter multiplexes bounded streams
and segregates pools by origin, advertised peer, and trust generation.

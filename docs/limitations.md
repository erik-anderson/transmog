# Initial limitations

- HTTP/3 is proxy-to-origin egress, not transparent browser QUIC interception.
- No general blind CONNECT, arbitrary TCP tunnel, CONNECT-UDP, MASQUE, or
  WebTransport support. CONNECT payloads are limited to intercepted TLS and
  browser-style HTTP/1.1 WebSocket handshakes whose authority remains fixed to
  the CONNECT target.
- Certificate-pinned clients will fail; pinning is never bypassed.
- BoringSSL verification over OS-enumerated roots does not reproduce every OS or
  Chrome Root Store constraint, revocation mechanism, CT rule, or metadata feed.
- 0-RTT and QUIC migration are disabled.
- The content-processing layer provides strict coding plans, header repair,
  resource budgets, transport-neutral gzip/Brotli/deflate/zstd engines, and
  multi-layer Hooks v2 composition. Runtime decoding is disabled by default;
  embedding applications must choose identity output or restoration of the
  original coding stack with an explicit finite `ContentPolicy`.
- Content byte, expansion, stack-depth, decoder-window, work-quantum, and active
  wall-time bounds are enforced inside codec operations. Preemption is
  cooperative at bounded input/output progress boundaries: a dependency call,
  including native zstd work, cannot be forcibly interrupted halfway through
  an instruction sequence. Applications handling hostile traffic should keep
  byte, ratio, window, and work limits conservative together.
- Content processing does not transcode character sets, parse semantic media
  formats, or negotiate dictionaries. WebSocket `permessage-deflate` is owned
  by the separate WebSocket layer. Raw-DEFLATE HTTP-content compatibility is
  opt-in and selected only from the initial zlib-header probe; it never retries
  after releasing decoded bytes.
- Automatic routing with a cached HTTP/3 alternative uses bounded whole-body
  buffering so an unsuccessful QUIC attempt can be replayed safely over HTTP/2.
  Forced-protocol routes and Auto without a cached HTTP/3 alternative stream.
- A body-phase hook cannot replace an exchange with a synthetic response
  after the upstream response head has been committed. Synthetic responses are
  supported at the request- and response-head phases.
- WebSocket inspection currently supports HTTP/1.1 Upgrade. HTTP/2 extended
  CONNECT, HTTP/3 WebSockets, application-owned upstream byte streams, and
  inspected recompression for explicitly negotiated windows below 15 remain
  future adapter work. The no-hook path is byte-transparent for frame semantics
  it does not need to understand.
- Experimental control v0 and the headless session controller are same-build,
  in-process bounded adapters. There is no stable external control protocol;
  stabilization remains deferred until a human explicitly requests it. There
  is no capture database, saved-rule engine, credential vault, or UI.
- The live session catalog is deliberately finite and in-memory. Delta
  subscriptions are lossy hints; consumers recover by paging authoritative
  state. Native capture is the streaming durable format, while SAZ is a
  finalized compatibility export.
- Host integration is an injected transaction seam. The crate ships no system
  proxy or certificate-store mutation implementation. Normal callers must use
  explicit service stop to observe restoration errors; the last-owner drop path
  can only make a best-effort restore attempt.
- Reverse-proxy listener configuration, ACME, load balancing, and health checks
  remain above/beyond the current explicit-proxy runtime. The provider seams do
  not imply those features are implemented.
- The proxy binds only to loopback unless a deliberately unsafe option is set.

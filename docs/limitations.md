# Current limitations

This page states constraints of the checked-in implementation. Desired
extensions belong in the [roadmap](roadmap.md), not in completed phase plans.

## Transport and routing

- HTTP/3 is proxy-to-origin egress, not transparent browser QUIC interception.
- There is no general blind CONNECT, arbitrary TCP tunnel, CONNECT-UDP, MASQUE,
  or WebTransport support. CONNECT payloads are limited to intercepted TLS and
  browser-style HTTP/1.1 WebSocket handshakes whose authority remains fixed to
  the CONNECT target.
- WebSocket inspection supports HTTP/1.1 Upgrade. HTTP/2 extended CONNECT,
  HTTP/3 WebSockets, and application-owned upstream byte-stream upgrades are
  not implemented. The no-hook path is byte-transparent for frame semantics it
  does not need to understand.
- QUIC 0-RTT and connection migration are disabled.
- Automatic routing with a cached HTTP/3 alternative uses bounded whole-body
  buffering so an unsuccessful QUIC attempt can be replayed safely over
  HTTP/2. Forced routes and automatic routing without a cached alternative
  stream directly.
- Reverse-proxy listeners, ACME, load balancing, and health checks are not
  implemented. Provider seams do not imply those product features.

## TLS and identity

- Certificate-pinned clients fail; Transmog never bypasses pinning.
- BoringSSL verification over roots enumerated from the operating system does
  not reproduce every OS or Chrome Root Store constraint, revocation mechanism,
  certificate-transparency rule, or metadata feed.
- Caller-process attribution is diagnostic and best effort. Windows and Linux
  have bounded system resolvers. Local callers on macOS and other unsupported
  platforms are reported as unknown unless an embedder supplies a resolver;
  non-loopback callers are identified only as remote.

## Content and interception

- Runtime content decoding is disabled by default for library embedders. They
  must choose identity output or restoration of the original coding stack with
  an explicit finite `ContentPolicy`.
- Content processing does not transcode character sets, parse semantic media
  formats, or negotiate dictionaries. Raw-DEFLATE compatibility is opt-in and
  selected only from the initial zlib-header probe; it never retries after
  releasing decoded bytes.
- Byte, expansion, coding-depth, decoder-window, work-quantum, and active-time
  limits are cooperative. A dependency call, including native zstd work,
  cannot be forcibly interrupted halfway through an instruction sequence.
- A body-phase interceptor cannot replace an exchange with a synthetic response
  after the upstream response head has been committed. Synthetic responses are
  available at request- and response-head phases.
- Inspected WebSocket recompression supports the RFC default 15-bit DEFLATE
  window. Explicitly negotiated smaller windows are rejected in inspected mode.

## Product and persistence

- The desktop is Windows-first and relies on Evergreen WebView2. Linux and
  macOS desktop products have not been qualified.
- The live session catalog is deliberately finite and in memory. Delta
  subscriptions are lossy hints; consumers recover by querying authoritative
  state. Native TMCap is the streaming durable format, while SAZ is a finalized
  compatibility export.
- Persisted automation, scripts, response assets, and preferences are bounded
  product workspaces, not a durable indexed traffic database or full project
  model.
- Auto-response URL matching is currently case-sensitive exact matching.
  Request-header criteria are exposed for POST; request-body criteria and URL
  regular expressions are not yet available in the desktop workflow.
- Experimental control revision 0 is a same-build, in-process contract. There
  is no stable external control protocol, and stabilization is deferred until a
  human explicitly requests it.
- `transmog-session` exposes host integration as an injected transaction seam.
  It does not mutate platform state itself. The Windows desktop supplies the
  concrete current-user proxy and certificate-store integration. Normal callers
  must stop explicitly to observe restoration errors; last-owner cleanup is
  necessarily best effort.
- The proxy binds only to loopback unless a deliberately unsafe option is set.

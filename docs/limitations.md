# Current limitations

This page states constraints of the checked-in implementation. Desired
extensions belong in the [roadmap](roadmap.md).

## Transport and routing

- HTTP/3 is proxy-to-origin egress, not transparent browser QUIC interception.
- Disabling HTTPS decryption permits opaque CONNECT tunnels with destination and
  byte-count evidence only. With decryption enabled, CONNECT accepts intercepted
  TLS or browser-style H1 WebSocket handshakes bound to the CONNECT target.
  CONNECT-UDP, MASQUE and WebTransport are unsupported.
- WebSocket inspection supports HTTP/1.1 Upgrade and HTTP/2 extended CONNECT.
  HTTP/3 WebSockets and application-owned upstream byte-stream upgrades are not
  implemented. Passive inspection preserves forwarded wire bytes.
- QUIC 0-RTT and connection migration are disabled.
- Automatic routing with a cached HTTP/3 alternative uses bounded whole-body
  buffering so an unsuccessful QUIC attempt can be replayed safely over
  HTTP/2. Forced routes and automatic routing without a cached alternative
  stream directly.
- Reverse-proxy listeners, ACME, load balancing, and health checks are not
  implemented. Provider seams do not imply those product features.

## TLS and identity

- Certificate-pinned clients fail; Transmog never bypasses pinning.
- The shared BoringSSL PKI verifier enforces public-certificate policy, CT,
  CRLSet/stapled-OCSP checks and bounded AIA issuer retrieval. Remaining Chrome
  differences include EV, optional online revocation, browser clock recovery,
  MTC and QWAC support. See [the verifier contract](chromium-verifier.md).
- Caller-process attribution is diagnostic and best effort. Windows and Linux
  have bounded system resolvers. Local callers on macOS and other unsupported
  platforms are reported as unknown unless an embedder supplies a resolver;
  non-loopback callers are identified only as remote.

## Content and interception

- Runtime content decoding is disabled by default for library embedders. They
  must choose identity output or restoration of the original coding stack with
  an explicit finite `ContentPolicy`.
- Content processing does not transcode character sets, parse semantic media
  formats, or negotiate dictionaries. Raw-DEFLATE compatibility is opt-in for
  runtime content processing and enabled for retained-body inspection. It is
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
- Live metadata stays in memory with no entry-count cap by default. An optional
  maximum evicts completed live entries; active requests can temporarily exceed
  it and imported entries are preserved. Body retention has a separate circular
  budget, not a total application-memory ceiling. Delta subscriptions are lossy
  hints; consumers recover by querying authoritative state. Native TMCap is the
  durable format and reads saved bodies on demand; SAZ is a finalized
  compatibility archive.
- Persisted automation, scripts, response assets, and preferences are bounded
  product workspaces, not a durable indexed traffic database or full project
  model.
- Saved responses support exact URLs, address patterns and bounded regular
  expressions, with request-header criteria for every method. Request-body
  matching is not yet available in the auto-response editor.
- Experimental control revision 0 is a same-build, in-process contract. There
  is no public raw decision transport. [Local MCP access](mcp.md) exposes scoped
  product operations above that internal contract; it does not expose script
  execution, arbitrary files or an unauthenticated remote listener.
- `transmog-session` exposes host integration as an injected transaction seam.
  It does not mutate platform state itself. The Windows desktop supplies the
  concrete current-user proxy and certificate-store integration. Normal callers
  must stop explicitly to observe restoration errors; last-owner cleanup is
  necessarily best effort.
- Captured-page preview is Windows-only. It reconstructs captured resources,
  not browser state from the recording device, and blocks uncaptured requests.
  Preview/search processing budgets remain separate from capture and replay.
- Saved traces have no fixed file-size or record-count ceiling, but opening them
  requires bounded metadata indexes. Recording can fail on storage errors or an
  oversized metadata record; unlimited capture does not remove these constraints.
- The proxy binds only to loopback unless a deliberately unsafe option is set.

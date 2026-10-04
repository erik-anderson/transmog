# Initial limitations

- HTTP/3 is proxy-to-origin egress, not transparent browser QUIC interception.
- No general blind CONNECT, arbitrary TCP tunnel, CONNECT-UDP, MASQUE, or
  WebTransport support.
- Certificate-pinned clients will fail; pinning is never bypassed.
- BoringSSL verification over OS-enumerated roots does not reproduce every OS or
  Chrome Root Store constraint, revocation mechanism, CT rule, or metadata feed.
- 0-RTT and QUIC migration are disabled.
- The content-processing layer provides strict coding plans, header repair,
  resource budgets, transport-neutral gzip/Brotli/deflate/zstd engines, and
  multi-layer Hooks v2 composition. Runtime decoding is disabled by default;
  embedding applications must choose identity output or restoration of the
  original coding stack with an explicit finite `ContentPolicy`.
- Automatic routing with a cached HTTP/3 alternative uses bounded whole-body
  buffering so an unsuccessful QUIC attempt can be replayed safely over HTTP/2.
  Forced-protocol routes and Auto without a cached HTTP/3 alternative stream.
- A body-phase hook cannot replace an exchange with a synthetic response
  after the upstream response head has been committed. Synthetic responses are
  supported at the request- and response-head phases.
- WebSocket handshake heads can pass through HTTP handling, but WebSocket frame
  inspection and modification are not implemented.
- The included `DecisionBridge` is an in-process bounded adapter. There is no
  stable external control protocol, capture database, saved-rule engine, or UI.
- Reverse-proxy listener configuration, ACME, load balancing, and health checks
  remain above/beyond the current explicit-proxy runtime. The provider seams do
  not imply those features are implemented.
- The proxy binds only to loopback unless a deliberately unsafe option is set.

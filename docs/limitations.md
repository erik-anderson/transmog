# Initial limitations

- HTTP/3 is proxy-to-origin egress, not transparent browser QUIC interception.
- No general blind CONNECT, arbitrary TCP tunnel, CONNECT-UDP, MASQUE, or
  WebTransport support.
- Certificate-pinned clients will fail; pinning is never bypassed.
- BoringSSL verification over OS-enumerated roots does not reproduce every OS or
  Chrome Root Store constraint, revocation mechanism, CT rule, or metadata feed.
- 0-RTT and QUIC migration are disabled.
- Body modification begins with identity content encoding. Compressed body
  rewriting is deferred until it has explicit round-trip tests.
- Automatic routing with a cached HTTP/3 alternative uses bounded whole-body
  buffering so an unsuccessful QUIC attempt can be replayed safely over HTTP/2.
  Forced-protocol routes and Auto without a cached HTTP/3 alternative stream.
- A body-phase breakpoint cannot replace an exchange with a synthetic response
  after the upstream response head has been committed. Synthetic responses are
  supported at the request- and response-head phases.
- The proxy binds only to loopback unless a deliberately unsafe option is set.

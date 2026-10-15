# WebSocket inspection

WebSocket support is split across three clean boundaries:

1. `transmog-http` retains an H1 upgraded stream or an H2 extended CONNECT stream,
   returning the actual canonical request and response heads.
2. `transmog-runtime` applies the ordinary HTTP interceptor chain to the
   opening request and response, authenticates the server accept proof, and
   connects the two upgraded streams.
3. `transmog-websocket` owns protocol-neutral handshake validation, frame
   parsing, message assembly, compression, message/control hooks, effect
   attribution, and the bounded bidirectional relay.

WebSocket frames are never represented as HTTP body frames. The HTTP exchange
finishes when the validated opening response (H1 `101` or H2 `200`) is committed;
the upgraded session then has its own cancellation, close, framing, and hook
lifecycle.

## Transparent and inspected paths

`ProxyComponents` has no WebSocket hooks by default. Without observers the runtime
uses `relay_transparent`, a byte-for-byte bidirectional copy. With observers it
uses passive, incremental frame/message decoding while forwarding the original bytes.
Decoder errors disable inspection with a visible diagnostic; forwarding continues.

Installing a `WebSocketHookFactory` selects `relay_inspected`. Each session gets
isolated interceptor instances. Complete decompressed messages and control
frames pass through the registered hooks in client-to-server order and unwind
in reverse order server-to-client. Every decision records the stable hook ID,
chain position, direction, order, and input/output lengths. Hooks may continue,
replace, drop, or close. Text replacements must remain UTF-8 and all callbacks,
replacements, and concurrent pauses have finite limits.

`ProxyServer::subscribe_websocket_evidence` publishes a bounded terminal report
for each runtime session. A successful inspected report contains the centrally
attributed effects; failures carry a redaction-safe reason. Consumers that need
message bytes while traffic is live use sequenced stream lifecycle and message
observer events. The desktop catalog, native capture, retention and privacy
policies consume these events; see [stream inspection](stream-inspection.md).

## Protocol and resource policy

The inspected path enforces:

- client masking and server non-masking;
- canonical payload lengths and valid opcodes/reserved bits;
- control-frame finality and the 125-byte control limit;
- correct continuation state with interleaved control frames;
- complete-message and close-reason UTF-8;
- valid close codes and bounded close handshakes;
- frame, assembled-message, unread-buffer, read, write, callback, replacement,
  concurrency, idle, and close-handshake limits;
- cryptographically generated fresh masks when forwarding toward an origin;
- `permessage-deflate` inflation and negotiated context takeover.

Default inspection preserves complete messages without a fixed payload cap.
Explicit decoder limits remain available to library callers; protocol validity
checks and bounded queues still apply.

The inspected encoder currently supports the RFC default 15-bit DEFLATE
window. If a peer explicitly selects a smaller window, inspected mode rejects
the handshake before accepting the downstream upgrade. Transparent mode remains
compatible because it never recompresses the stream.

## Current transport scope

The runtime handles RFC 6455 HTTP/1.1 upgrades for direct absolute-form `ws://`
proxy requests, browser-style plaintext HTTP/1.1 carried inside a `CONNECT` to
the WebSocket origin, and `wss://` requests inside intercepted CONNECT TLS. With
HTTPS decryption enabled, the CONNECT preface detector preserves the first byte,
accepts only TLS or a WebSocket `GET` preface, and retains inner-authority
validation. Disabling HTTPS decryption selects opaque CONNECT forwarding.

Intercepted TLS H2 connections advertise RFC 8441 support. Extended CONNECT with
`:protocol = websocket` uses the same authority checks, hooks, observation and
relay as H1; ordinary or unsupported nested CONNECT requests remain rejected.
H1/H2 translation generates an H1 nonce and accept proof only where required,
removes connection-specific headers on H2, and preserves subprotocol/compression
negotiation. Response hooks cannot change negotiated wire semantics.

Origins advertise extended CONNECT through SETTINGS before an opening request
is sent. A bounded PING exchange establishes that the initial SETTINGS have been
processed. Auto routing falls back to H1 if ALPN selects H1 or H2 lacks this setting;
it never retries an opening request after it was sent. Forced H2 reports the lack
of capability. Concurrent WebSockets can share a dedicated origin H2 connection,
partitioned by the existing trust, caller, isolation and certificate authorization
rules. Weak pool entries release idle origin connections after the last stream.
Flow control applies per stream; closure or reset never closes unrelated streams.

An origin that declines the handshake is returned through the bounded ordinary
HTTP response pipeline. An H1 2xx decline becomes a downstream H2 502, since an H2
2xx CONNECT response would incorrectly establish a tunnel. Captured origin heads
retain the original status. HTTP/3 WebSockets and application-owned
`UpstreamService` byte-stream upgrades remain unsupported.

Locally authored HTTP responses can reject an opening request. A synthetic 2xx
response to extended CONNECT becomes 502 because no origin relay owns that
tunnel; its authored body is preserved for inspection.

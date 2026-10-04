# WebSocket inspection

WebSocket support is split across three clean boundaries:

1. `rustymiddle-http` retains Hyper's upgraded origin stream while returning a
   canonical handshake head.
2. `rustymiddle-runtime` applies the ordinary HTTP Hooks v2 chain to the
   opening request and response, authenticates the server accept proof, and
   connects the two upgraded streams.
3. `rustymiddle-websocket` owns protocol-neutral handshake validation, frame
   parsing, message assembly, compression, message/control hooks, effect
   attribution, and the bounded bidirectional relay.

WebSocket frames are never represented as HTTP body frames. The HTTP exchange
finishes when the validated `101 Switching Protocols` response is committed;
the upgraded session then has its own cancellation, close, framing, and hook
lifecycle.

## Transparent and inspected paths

`ProxyComponents` has no WebSocket hooks by default. In that configuration the
runtime uses `relay_transparent`, a byte-for-byte bidirectional copy which does
not parse, buffer, remask, or otherwise normalize frames. This remains useful
for extensions or traffic the inspection layer does not understand.

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
message bytes while traffic is live should use an observation-only hook and
their own bounded storage or delivery policy.

An application that only needs observation can install a hook that records the
message/control events and always returns `Continue`. This is also the seam for
a future native-capture WebSocket record stream; capture state does not belong
in the frame engine.

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
- bounded `permessage-deflate` inflation and negotiated context takeover.

The inspected encoder currently supports the RFC default 15-bit DEFLATE
window. If a peer explicitly selects a smaller window, inspected mode rejects
the handshake before sending `101` downstream. Transparent mode remains
compatible because it never recompresses the stream.

## Current transport scope

The runtime handles RFC 6455 HTTP/1.1 upgrades for direct absolute-form `ws://`
proxy requests, browser-style plaintext HTTP/1.1 carried inside a `CONNECT` to
the WebSocket origin, and `wss://` requests inside intercepted CONNECT TLS. The
CONNECT preface detector preserves the first byte, accepts only TLS or a
WebSocket `GET` preface, and retains the usual inner-authority validation; this
is not a general blind TCP tunnel. HTTP/2 extended CONNECT, HTTP/3 WebSockets, and
application-owned `UpstreamService` byte-stream upgrades are separate future
adapters. An origin that declines the upgrade is returned through the ordinary
bounded HTTP response pipeline.

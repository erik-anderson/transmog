# Architecture

Browser traffic reaches a loopback explicit HTTP proxy. Plain HTTP arrives in
absolute form; HTTPS arrives through HTTP/1.1 `CONNECT`. After CONNECT, the
proxy issues a short-lived leaf from its local CA and serves HTTP/1.1 or HTTP/2
according to downstream ALPN.

Every exchange is translated into `rustymiddle-core` types. Breakpoints operate
on that model before adapters translate it to Hyper HTTP/1.1, Hyper HTTP/2, or
quiche HTTP/3. HTTP/3 is origin egress only: this design does not claim to
intercept a browser's native QUIC packets.

```text
Chromium --HTTP proxy/CONNECT--> Hyper ingress
                                  |
                           canonical session
                                  |
                         bounded breakpoints
                                  |
                    +-------------+-------------+
                    |             |             |
                 Hyper H1      Hyper H2      quiche H3
                    +-------------+-------------+
                              BoringSSL
```

Protocol adapters own framing and flow control. Request and response bodies move
through bounded `BodyStream` channels, so a slow consumer applies backpressure
instead of causing unbounded buffering. Forced HTTP/1.1, HTTP/2, and HTTP/3
routes stream in both directions. A paused callback owns a permit for only its
stream; it has a deadline and cannot accumulate unbounded body bytes. Header
translation is centralized in the core crate.

`RuntimeLimits` makes the server-side resource envelope explicit: connection
count, H2 concurrent streams, H1 header count/buffer size and read deadline,
per-frame body idle deadline, body byte limits, bounded channel depth, TLS
handshake timeout, breakpoint concurrency/deadline, leaf-cache size, and
shutdown drain timeout. HTTP/3 has its corresponding queue, pool, stream,
flow-control, and transport-idle bounds in `H3TransportLimits`.

Automatic routing streams directly when no HTTP/3 alternative is cached. When
an HTTP/3 alternative is cached, Auto uses an explicitly bounded replay buffer:
falling back safely from a failed QUIC attempt requires the request body to be
replayable. This is a deliberate exception to the streaming default, governed
by the same request and response byte limits.

Hyper maintains separate HTTP/1.1, HTTP/2, and automatic-selection pools. The
quiche adapter pools by origin host, advertised peer port, and immutable trust
generation. Each H3 pool entry is a single driver that multiplexes request
streams over one QUIC connection; both the number of live entries and the
per-connection pending queue are bounded by `H3TransportLimits`. A failed or
expired connection is removed lazily and recreated without mixing trust
generations. `ProxyServer::control` exposes generation-safe trust reloads: it
builds a complete replacement set of Hyper and quiche clients, atomically makes
that set active for new exchanges, and lets in-flight exchanges finish through
the `Arc`-retained prior generation. Reused or decreasing generation numbers
are rejected.

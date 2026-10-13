# HTTP/2 ALPS and Client Hints

The Chromium 154 network recipe offers HTTP/2 Application-Layer Protocol
Settings (ALPS) at the measured `0x44cd` codepoint. **More handshake options →
HTTP/2 ALPS** is a separate choice in custom profiles. It applies only when
HTTP/2 is offered; disabling HTTP/2 also removes the ALPS offer. A server that
does not negotiate ALPS uses the ordinary HTTP/2 settings exchange.

Offering ALPS means accepting transport settings, not promising additional
browser headers. Transmog sends an empty client ALPS value, as Chromium does,
and sends its local HTTP/2 SETTINGS normally. It validates peer ALPS framing
and setting values, then applies those settings before sending a request.
Settings delivered in TLS are already acknowledged; Transmog sends no HTTP/2
ACK for them. Later wire SETTINGS retain their ordinary acknowledgements.

The current h2 crate has no ALPS bootstrap API. A private, bounded connection
adapter feeds one local SETTINGS frame into its existing decoder and consumes
that frame's ACK locally. Requests wait for these settings before encoding.
This keeps flow control, concurrency, HPACK and extended CONNECT in h2's
existing state machine without maintaining a dependency fork.

## Why Client Hint processing is deferred

Servers may include an `ACCEPT_CH` extension frame in their ALPS value.
Transmog records these preferences for inspection but does not act on them.
The proxy cannot reliably route this
connection-level information back to the caller before its request, and does
not own the caller's browser state, device values, permissions policy or
consent. Generating hints itself could misrepresent the caller and change
the request being debugged. Existing caller-authored hints and ordinary
response headers continue through the normal forwarding and header-rule paths.

The [Client Hint Reliability draft, section 4.3](https://www.ietf.org/archive/id/draft-davidben-http-client-hint-reliability-03.html#section-4.3)
permits ignoring or forgetting `ACCEPT_CH` entries. ALPS therefore does not
promise that every requested hint will be sent. This is a deliberate limitation
of our proxy behavior, rather than a claim of complete Chromium behavior.

The [HTTP ALPS draft, section 3](https://www.ietf.org/archive/id/draft-vvv-httpbis-alps-01.html#section-3)
defines TLS-delivered settings as acknowledged and permits those settings to
replace the server's initial wire SETTINGS. Merely advertising the extension
and discarding all received settings would be unsafe: a server could select
flow-control or concurrency limits that the client never applies.

Previously saved custom profiles retain ALPS disabled. The built-in Chromium
154 recipe advances to revision 2 to include ALPS; the browser reference stays
154.0.8037.99. Older captures retain ALPS disabled in their recorded provenance.

## Captured server contents

Traffic → Details → Connection shows **HTTP/2 ALPS** as an observed negotiation
fact: negotiated with a byte count, not negotiated, or unavailable when it was
not recorded or the TLS handshake did not reach that point. The configured
profile's offer is separate from this evidence. An empty negotiated server
value still means ALPS was used.

**Server ALPS contents** decodes the original server value into ordered H2
frames, named SETTINGS parameters, and each `ACCEPT_CH` origin and requested
hint list. The Client Hint lists are read-only and ignored when forwarding.
Unknown settings retain their numeric identifiers; unknown frame payloads have
a hex preview. **Original ALPS bytes (Hex)** shows the complete value, including
frame headers. Malformed payloads retain their bytes and show parse diagnostics.
The readable view limits large content and explains truncation; Hex remains complete.

Native captures store the server value with the shared physical connection
context. Both SAZ export profiles preserve it, including encrypted archives;
strict SAZ uses bounded hex chunks in conventional SessionFlags. Earlier
captures containing only the status text show that text and explain that
the original payload was not recorded. Conflicting imported status text remains
available alongside the typed negotiation evidence.

## Qualification

`cargo test --locked -p transmog-http --test alps` uses a local BoringSSL TLS
origin to exercise the offer and toggle, HPACK and concurrency before the first
request, a zero receive window, real SETTINGS acknowledgements, fallback, and
malformed settings. Unit tests also exercise fragmented writes and backpressure.
The Docker interoperability suite verifies that Caddy declines ALPS and still
serves verified H2. Its nginx and Apache fixtures use plain HTTP.

The external Google check is explicitly ignored in ordinary offline tests:
`cargo test --locked -p transmog-http --test alps google_negotiates -- --ignored --nocapture`.
It sends one HEAD request to `https://www.google.com/`, verifies the certificate
and H2 response, and requires recorded ALPS negotiation. This supplements the
deterministic fixture; it is not a dependency of ordinary development tests.

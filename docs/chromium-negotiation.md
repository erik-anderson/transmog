# Chromium negotiation contract

The reference profile is Chromium 154.0.8037.99. Both upstream transports use
the shared native certificate verifier described in [chromium-verifier.md](chromium-verifier.md).

## Resumption

Stored peer chains are reverified on TLS 1.2, TLS 1.3 and QUIC resumption, using
current time and the connection's frozen trust inputs. Captures distinguish a
restored verification path from a chain newly sent by the server.

The TCP adapter owns its session cache rather than relying on hyper-boring's
unbounded origin map. Caches hold at most 1024 origin partitions and two tickets
per partition, prefer the newest ticket, consume native single-use tickets,
check expiry on lookup, and sweep expired entries every 256 lookups. TLS 1.2
session lifetime is one hour; TLS 1.3 uses the native/server lifetime.

TCP contexts belong to an immutable caller/run pool. QUIC ticket keys additionally
include origin and alternative ports, caller isolation, destination address,
trust epoch and client-authentication epoch. Address racing cannot consume a
ticket intended for a different peer. Replacing TLS inputs creates a fresh cache.
Post-handshake QUIC tickets are retained after establishment and packet progress.
Quiche exposes the latest serialized ticket after each receive; two tickets
processed in the same receive may collapse to the latest one.

Prompt and automatic client-certificate selection require a fresh CertificateRequest
and disable ticket caching, preserving their existing authorization contract.
The default explicit no-certificate policy permits anonymous tickets; installing
a controller alone does not disable normal resumption. Authorization epochs and
caller partitions still separate tickets, and anonymous reuse never queries keys.
Physical connection reuse and TLS session resumption are separate policies.
Zero-RTT remains disabled: enabling it requires a separate request/replay policy.

## TLS 1.2 False Start

TCP contexts enable BoringSSL's False Start mode. The native library permits it
only for eligible full TLS 1.2 handshakes (ECDHE, AEAD, negotiated ALPN), after
certificate verification. Writes may begin before the server's Finished;
reads still wait for it. TLS 1.3, CBC and connections without ALPN do not use it.
Capture details record **TLS False Start: Used** and **Handshake completion:
Waiting for server Finished**, then change completion to **Complete** when the
native handshake-done callback fires. Connection setup timing ends when the
connection becomes usable for writes, as it does in Chromium.

## Buffered certificates

Production TCP and QUIC contexts use `TLS_with_buffers_method`, as Chromium does.
The shared PKI verifier parses the leaf and candidate issuers rather than having
the TLS compatibility layer eagerly parse every wire entry. An irrelevant malformed
issuer can be ignored when another valid trusted path exists; a malformed leaf or
unusable required issuer still fails. H3 telemetry hashes raw certificate bytes and
does not impose a second parse requirement on unused entries.

Credentials and requested CA names use buffer APIs. QUIC connections are created
without quiche's legacy hostname setter; SNI and expected identity are installed
separately before the first packet. The older `hyper_connector*` embedding helpers
retain their legacy return types; the production adapter uses `tcp_context_builder`.

## Encrypted Client Hello

Built-ins enable real ECH independently of ECH GREASE. Custom profiles can disable
either under **More handshake options**. Old captures missing the real-ECH field
retain false. TLS 1.2-only profiles do not attempt ECH.

The pinned Hickory resolver reads the configured system DNS servers. HTTPS records
are bounded by a three-second discovery deadline, eight aliases, 64 service records
and native configuration size limits. Service priority, ALPN, no-default-ALPN,
mandatory keys, target name and port select compatible endpoints in priority order.
For a `.` target,
discovery uses the actual HTTPS record owner, including after CNAME
resolution and for port-prefixed owners. The original origin remains the
certificate and HTTP authority.
Bindings that change the requested destination port are ignored, matching the
pinned Chromium recipe. An explicit Alt-Svc destination port remains supported.
A/AAAA resolution uses the existing system resolver and Happy Eyeballs policy. DNS-cache TTLs apply;
configuration changes refresh the resolver on subsequent lookups. This supplies
ECH discovery, rather than Chromium's complete browser secure-DNS/DoH policy.

If an eligible endpoint cannot be reached, TCP and QUIC try the remaining service
alternatives in order, each with its own ECH configuration. Exhausting ECH
alternatives fails without falling back to a plaintext origin connection.
Certificate failures, client-certificate authorization failures and a failed
authenticated TLS retry stop endpoint fallback. The selected endpoint remains
visible in the connection evidence.

TCP and QUIC install the configuration before sending their first flight; ticket
keys include its digest. The certificate verifier uses the ECH public name only
for a rejection handshake. HTTP/3 also resolves HTTPS metadata for Alt-Svc
destinations, using their destination port while authenticating the logical origin.
Retry configs are copied only when BoringSSL reports
`SSL_R_ECH_REJECTED` after authenticating both the certificate and Finished. A new
connection retries once with those configs, or without ECH after authenticated
rollback. Its certificate must verify for the original origin. Origin client
identities and HTTP requests are never sent on a rejection connection. Untrusted
public-name certificates, damaged handshakes and repeated rejections fail closed.
Both attempted connections and their actual ECH outcomes remain in captures.

## Missing issuers

The single PKI path builder consults supplied issuers and the intermediate cache
first, then retrieves AIA CA-issuer URLs when it needs additional candidates.
Like Chromium's issuer source, it starts at most five valid URLs per certificate,
waits for their results in order, accepts individual DER, CMS/PKCS#7, or the first
PEM certificate, limits decoded responses to 64 KiB, and bounds each batch to 10
seconds. HTTP content encodings are decoded within that bound; encoded input is
additionally limited to 256 KiB.
Only HTTP is supported, including after redirects. Fetches send no cookies,
caller headers or authentication and bypass the proxy's TLS machinery to avoid
recursive verification. The path builder retains its 60-second and 20-iteration
limits. Cancellation stops outstanding fetches.

Production TCP and QUIC verification runs on bounded workers. Only owned DER,
expected identity, SCT/OCSP evidence and frozen trust inputs cross threads; the
TLS object and capture publication stay on their owning thread. Low-level
synchronous embedding contexts retain synchronous verification; async adapters
install the worker callback and must supply a TLS task waker.

Only intermediates from fully accepted paths enter the bounded cache: up to 256
certificates and 4 MiB, with seven days of inactivity expiry. Leaf certificates
and anchors are excluded. Cached certificates remain untrusted candidates;
every new path rechecks trust, expiry, constraints, hostname, CT and revocation.
Capture details report AIA retrieval attempts and whether a cached candidate was
used. This is a process-local intermediate cache, not Chromium's complete HTTP
cache or browser-level certificate-verification cache.

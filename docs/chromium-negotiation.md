# Chromium negotiation contract

Use [network profiles](network-profiles.md) to select and customize the offer.
The reference profile is Chromium 154.0.8037.99. Both upstream transports use
the shared native certificate verifier described in [chromium-verifier.md](chromium-verifier.md).

## Algorithms and ClientHello

Built-in upstream offers target Chromium 154.0.8037.99. The shared
[algorithm policy](../crates/proxy-tls/src/algorithms.rs) defines key exchange and
transport-specific signature preferences. A profile roll must review the pinned
Chromium [group policy](https://github.com/chromium/chromium/blob/154.0.8037.99/net/ssl/ssl_config_service.cc)
and [TCP signature preferences](https://github.com/chromium/chromium/blob/154.0.8037.99/net/socket/ssl_client_socket_impl.cc).

| Purpose | Offer and behavior |
| --- | --- |
| TLS 1.3 key exchange | `X25519MLKEM768` (`0x11ec`), followed by X25519, P-256 and P-384. Initial shares cover the hybrid group and X25519. |
| TCP TLS 1.3 authentication | ML-DSA-44/65/87 (`0x0904`–`0x0906`) precede the non-PQC signature preferences; RSA/SHA-1 is excluded. |
| QUIC TLS authentication | Chromium's nine BoringSSL signature preferences, without ML-DSA. TLS 1.3 cannot select the advertised RSA PKCS#1 schemes. |

QUIC's handshake offer does not disable ML-DSA issuer signatures in certificate
paths. An RSA/ECDSA leaf issued by a locally trusted ML-DSA CA can verify through
the shared PKI verifier. Installing a root does not change the ClientHello offer.

TLS 1.2 uses non-PQC algorithms. Each TLS leg negotiates independently; see
[interception credentials](certificate-model.md#interception-credentials) for
downstream selection and trust-chain limits.

TCP ClientHellos enable ordinary GREASE and prepend a reserved GREASE signature
scheme. QUIC omits those offers under the built-in recipe. Signature GREASE is
independent of ordinary GREASE and does not change CertificateRequest algorithms.
Both transports randomize extension order while preserving TLS ordering rules.
Custom profiles expose GREASE choices separately from real algorithm preferences.

TCP requests stapled OCSP and signed certificate timestamps; QUIC follows its
separate Chromium offer. The [verifier contract](chromium-verifier.md) defines
CT/CRLSet/stapled-OCSP enforcement and recorded outcomes for every source.

Upstream TLS 1.3 clients also advertise Brotli certificate decompression
(`compress_certificate`, algorithm `0x0002`). Decoding uses BoringSSL's existing
certificate size limit and exact output buffer, rejects malformed, truncated
or trailing data, and passes the decoded certificate through the same trust
and identity verification. Origins may still send an uncompressed certificate.
Client certificates and downstream certificates remain uncompressed.

Built-in profiles enable ECH GREASE on TCP and QUIC. When no real ECH
configuration is used, BoringSSL sends a fresh dummy `encrypted_client_hello`
extension when TLS 1.3 is offered, including when an origin negotiates TLS 1.2. This
exercises extension handling and matches Chromium's ECH GREASE behavior; it
does not encrypt the server name. Real ECH discovery and authenticated retry are
separate settings; see [Encrypted Client Hello](#encrypted-client-hello).

The default TLS 1.2 cipher list uses Chromium's expression
`ALL:!aPSK:!ECDSA+SHA1:!3DES`, including its CBC and static-RSA compatibility
suites after the ECDHE/AEAD suites. RSA/SHA-1 server handshake signatures are
excluded on TCP, matching its separate signature policy. Custom profiles can
disable compatibility ciphers; embedders can supply an explicit cipher list.

## Resumption

Stored peer chains are reverified on TLS 1.2, TLS 1.3 and QUIC resumption, using
current time and the connection's frozen trust inputs. Captures distinguish a
restored verification path from a chain newly sent by the server.

The TCP adapter owns a bounded session cache with at most 1024 origin partitions
and two tickets per partition. It prefers the newest ticket, consumes native
single-use tickets, checks expiry on lookup, and sweeps expired entries every
256 lookups. TLS 1.2 session lifetime is one hour; TLS 1.3 uses the native/server
lifetime.

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

## Encrypted Client Hello

Built-ins enable real ECH independently of ECH GREASE. Custom profiles can disable
either under **More handshake options**. TLS 1.2-only profiles do not attempt ECH.

The pinned Hickory resolver reads the configured system DNS servers. HTTPS records
are bounded by a three-second discovery deadline, 64 service records
and native configuration size limits. Service priority, ALPN, no-default-ALPN,
mandatory keys, target name and port select compatible endpoints in priority order.
Matching the pinned Chromium resolver, AliasMode disables service metadata;
targets must match the original hostname or canonical record owner, and at least
one eligible service must allow default ALPN. That service need not offer ECH or
one of the caller's enabled protocols.
For a `.` target,
discovery uses the actual HTTPS record owner, including after CNAME
resolution and for port-prefixed owners. The original origin remains the
certificate and HTTP authority.
Bindings that change the requested destination port are ignored, matching the
pinned Chromium recipe. An explicit Alt-Svc destination port remains supported.
A/AAAA resolution uses the existing system resolver and Happy Eyeballs policy.
DNS-cache TTLs apply; configuration changes refresh the resolver on subsequent
lookups. This supplies ECH discovery, rather than Chromium's complete browser
secure-DNS/DoH policy.

If an eligible endpoint cannot be reached, TCP and QUIC try the remaining service
alternatives in order, each with its own ECH configuration. Exhausting ECH
alternatives fails without falling back to a plaintext origin connection.
Certificate failures, client-certificate authorization failures and a failed
authenticated TLS retry stop endpoint fallback. The selected endpoint remains
visible in the connection evidence.
The bounded transport history retains the selected successful connection and
recent failed attempts, preserving the client leg even when many alternatives
were tried.

TCP and QUIC install the configuration before sending their first flight; ticket
keys include its digest. The certificate verifier uses the ECH public name only
for a rejection handshake. HTTP/3 also resolves HTTPS metadata for Alt-Svc
destinations, using their destination port while authenticating the logical origin.
Retry configs are copied only when BoringSSL reports
`SSL_R_ECH_REJECTED` after authenticating both the certificate and Finished. A new
connection retries once with those configs, or without ECH after authenticated
rollback. Its certificate must verify for the original origin.
The retry retains the selected service metadata and connected peer address,
without another HTTPS or A/AAAA lookup; capture display strings never control
routing. Origin client identities and HTTP requests are never sent on a rejection
connection. Untrusted
public-name certificates, damaged handshakes and repeated rejections fail closed.
Both attempted connections and their actual ECH outcomes remain in captures.

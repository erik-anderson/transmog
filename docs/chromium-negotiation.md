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

Request-driven client-certificate controllers require a fresh CertificateRequest
and disable ticket caching, preserving their existing authorization contract.
Physical connection reuse and TLS session resumption are separate policies.
Zero-RTT remains disabled: enabling it requires a separate request/replay policy.

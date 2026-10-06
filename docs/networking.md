# Origin connection racing

Transmog applies one validated `HappyEyeballsConfig` to its built-in origin
transports. The default starts the preferred candidate immediately, permits an
alternate-family attempt after 250 milliseconds, and retains at most 16 unique
candidates for one connection. A caller may construct another finite policy
with a delay from 1 millisecond through 2 seconds and a candidate bound from 1
through 64.

`ProxyConfig::happy_eyeballs` controls new HTTP/1.1, HTTP/2, and HTTP/3 origin
connections created by the proxy runtime. Trust reloads build replacement
connection pools with the same immutable policy. Direct users of
`HyperOriginClient` or `H3OriginClient` can use their
`with_happy_eyeballs` constructors. The default constructors apply the default
policy rather than disabling connection racing.

## TCP and TLS

The Hyper adapter bounds and family-balances the system resolver's output, then
passes the configured delay to its `HttpConnector`. The connector races the
preferred and alternate address families; failed addresses within a family are
tried in resolver order. TLS, certificate verification, and ALPN run on the
winning TCP connection. Restricting the allowed HTTP protocol set does not
disable address-family racing.

## QUIC and HTTP/3

The quiche adapter resolves a bounded candidate list, removes duplicates, and
interleaves address families while preserving resolver order within each
family. Each candidate owns a separate UDP socket and complete QUIC/TLS
handshake. A later candidate starts after the configured delay, or immediately
when every in-flight candidate has already failed. The first fully established,
certificate-verified connection wins; dropping the race cancels and closes all
losing candidate state.

The selected UDP peer remains visible in `H3Telemetry`. Pool identity continues
to use the logical origin, peer port, and trust generation rather than pinning
future connections to a stale DNS address.

## Bounds and failure behavior

- DNS resolution is delegated to the operating system and awaited once per new
  pooled connection.
- Duplicate results do not consume the candidate bound.
- At most the configured number of handshake attempts can exist for one race.
- Existing transport deadlines still bound individual attempts and the outer
  upstream deadline bounds the complete exchange.
- If every candidate fails, the final typed transport error is returned. No
  failed address-family attempt weakens TLS verification or protocol policy.

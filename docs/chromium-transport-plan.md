# Remaining Chromium transport alignment

The built-in profiles target Chromium 154.0.8037.99. Chromium 155 uses the same
wire targets covered here. Use the [local comparison harness](chromium-comparison.md)
to qualify changes; a matching JA4 hash alone does not establish transport or
certificate-verification equivalence.

Current offers and connection policy are documented in
[Chromium negotiation](chromium-negotiation.md), [ALPS](http2-alps.md), and
[certificate verification](chromium-verifier.md). Dependency patches and their
reproduction commands live in the [transport inventory](../third_party/rust-transport/README.md).

## Expand comparison coverage

The harness compares fresh GET requests over H1.1, H2 and H3. Extend it to cover
connection reuse, HEAD/POST, duplicate headers and cookies, edits and reroutes,
accepted ALPS, and resumption. Keep protocol fallback and missing evidence as
hard errors. Compare randomized ECH padding and QUIC GREASE across samples;
a single random value difference is not a compatibility defect.

## HTTP/2 startup ordering

The initial SETTINGS values, header order, priority and HPACK output match the
target in the fresh-request case. A real SETTINGS ACK still precedes the initial
WINDOW_UPDATE and HEADERS. A peer-settings bootstrap API must preserve flow
control, concurrency limits and ALPS settings before request allocation. Prefer
an upstreamable encoder/connection API; do not rewrite encoded HPACK bytes.

Qualify uploads, trailers, connection reuse and extended CONNECT alongside any
startup-order change.

## HTTP/3 dynamic QPACK

The pinned quiche decoder has incomplete dynamic-table paths. Keep advertised
capacity and blocked-stream limits at zero until bounded updates, insertions,
evictions, blocked streams, acknowledgements, cancellation and malformed-input
handling work against an independent peer. Prefer upstream support; assess a
maintained implementation or adapter migration if necessary.

## HTTP/3 datagrams

H3_DATAGRAM and the QUIC datagram transport parameter remain absent. Supporting
them requires an application consumer, routing and ownership rules, capture
semantics and bounded queues. Review that product scope before implementation.
HTTP/3 extended CONNECT and WebTransport likewise need working end-to-end behavior
before their capabilities can be advertised.

For each change, run the affected interoperability tests and local comparison.
Keep qualification reports and review transcripts in generated evidence, not in
this plan.

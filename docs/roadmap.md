# Roadmap

This roadmap lists remaining work. Current behavior is documented in the
[topic guides](README.md).

Priorities are intentionally not tied to dates. A feature moves out of this
file when its durable contract is documented and its implementation and tests
are checked in.

## Desktop and traffic workspace

- Add a searchable capture library and project-level organization across saved
  captures, persisted automation, scripts, response assets and preferences.
- Add desktop file actions for the implemented bounded rule/asset interchange,
  conflict review and disabled-import APIs in [portable investigations](portable-investigations.md).
- Add request-body auto-response conditions only with explicit retention,
  privacy, resource, and replay semantics.
- Extend headless tooling for persisted automation and script-workspace
  management. Guided support recording, circular capture, native streaming
  output and password-protected export already exist in the CLI.

## Capture and inspection fidelity

- Continue the revision-0 [portable investigation contract](portable-investigations.md)
  beyond the shared rule/working-case APIs and desktop ZIP64/TMCap transfer:
  binary-only validation, matching, inspection and packaging (K4), followed by
  script/run/preview/comparison integration (K5).
  Broader project organization and any future format stability decision remain
  separate from the experimental live control protocol.
- Expand safe previews or semantic viewers only after a format-specific parser,
  sandbox, CSP, resource-limit, and hostile-corpus review.

## Platforms and distribution

- Extend platform qualification beyond the Linux container gate to native Linux
  and macOS host integration, networking and process sandboxes.
- Decide whether a Linux or macOS desktop shell has a supportable system WebView
  and process-sandbox story before enabling those products.
- Add macOS caller-process attribution only if it can use a supported and
  testable operating-system contract; the available `libproc` socket APIs are
  currently private and subject to change.
- Review activation of the deferred hosted platform, browser, fuzz, and
  performance workflows in the [CI guide](../ci/README.md).

## Protocol expansion

- HTTP/3 WebSockets.
- Application-owned upstream byte streams for upgraded connections.
- Native downstream HTTP/3, CONNECT-UDP, MASQUE, and WebTransport.
- Reverse-proxy listener products, including certificate provisioning, load
  balancing, and health checks. Existing provider seams do not imply these
  features.
- QUIC 0-RTT and connection migration only after replay and identity policy is
  explicit.

## Chromium transport alignment

The built-in profiles target Chromium 154.0.8037.99. Chromium 155 uses the same
wire targets covered here. Use the [local comparison harness](chromium-comparison.md)
to qualify changes; a matching JA4 hash alone does not establish transport or
certificate-verification equivalence.

Current offers and connection policy are documented in
[Chromium negotiation](chromium-negotiation.md), [ALPS](alps.md), and
[certificate verification](chromium-verifier.md). Dependency patches and their
reproduction commands live in the [transport inventory](../third_party/rust-transport/README.md).

### Expand comparison coverage

The harness compares fresh GET requests over H1.1, H2 and H3. Extend it to cover
connection reuse, HEAD/POST, duplicate headers and cookies, edits and reroutes,
accepted ALPS, and resumption. Keep protocol fallback and missing evidence as
hard errors. Compare randomized ECH padding and QUIC GREASE across samples;
a single random value difference is not a compatibility defect.

### HTTP/2 startup ordering

The initial SETTINGS values, header order, priority and HPACK output match the
target in the fresh-request case. A real SETTINGS ACK still precedes the initial
WINDOW_UPDATE and HEADERS. A peer-settings bootstrap API must preserve flow
control, concurrency limits and ALPS settings before request allocation. Prefer
an upstreamable encoder/connection API; do not rewrite encoded HPACK bytes.

Qualify uploads, trailers, connection reuse and extended CONNECT alongside any
startup-order change.

### HTTP/3 dynamic QPACK

The pinned quiche decoder has incomplete dynamic-table paths. Keep advertised
capacity and blocked-stream limits at zero until bounded updates, insertions,
evictions, blocked streams, acknowledgements, cancellation and malformed-input
handling work against an independent peer. Prefer upstream support; assess a
maintained implementation or adapter migration if necessary.

### HTTP/3 datagrams

H3_DATAGRAM and the QUIC datagram transport parameter remain absent. Supporting
them requires an application consumer, routing and ownership rules, capture
semantics and bounded queues. Review that product scope before implementation.
HTTP/3 extended CONNECT and WebTransport likewise need working end-to-end behavior
before their capabilities can be advertised.

For each change, run the affected interoperability tests and local comparison.
Keep qualification reports and review transcripts in generated evidence, not in
the roadmap.

## Post-quantum trust

- Evaluate PQ interception issuers and Merkle Tree Certificate support as
  Chromium's PKI requirements develop. Define trust installation, protected key
  storage, migration and rollback before changing the interception root.
  Current hybrid exchange and ML-DSA handshake support are documented in the
  [certificate model](chromium-negotiation.md#algorithms-and-clienthello).

## Control protocol stabilization

Control revision 0 is a same-build, breakable internal contract. It may remain
that way indefinitely. Stabilization begins only when a human maintainer
explicitly records that it now makes sense; elapsed time, repository splits,
consumer count, or test coverage do not trigger it.

Once triggered, stop protocol feature work long enough to inventory producers,
consumers, persisted references, and deployment topologies; define compatibility
and support windows; specify canonical serialization and unknown-field behavior;
add conformance and cross-version fixtures; and record the v1 decision in an
ADR before promising compatibility.

## Repository boundaries

The monorepo keeps cross-layer changes atomic while APIs are evolving. Split a
crate into another repository only when independent ownership, release cadence,
security review, or distribution warrants the operational cost. A split must
retain acyclic dependency direction, runnable contract tests, explicit release
coordination, and clear license/security ownership. It does not stabilize the
control protocol by itself.

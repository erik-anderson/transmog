# Remaining Chromium transport alignment

## Verified starting point

The local harness compared transport commit `ac9d6ea` with Chrome
155.0.8059.79 using three fresh direct/proxied pairs for each of H1.1, H2 and H3.
All 18 navigations proved their browser and origin protocols and proxy routing.
The production recipe remains Chromium 154.0.8037.99; the implemented numeric
defaults were separately checked against that release's source and pinned
Google QUICHE revision.

Completed corrections:

- H2 SETTINGS match: HPACK table 64 KiB, push disabled, stream receive credit
  6 MiB, header-list limit 256 KiB, with the default frame size implicit.
  Initial connection credit is 15 MiB, yielding WINDOW_UPDATE `15663105`.
  Auto, forced H2 and the separate WebSocket extended-CONNECT client share the
  policy. Wire fixtures cover Auto/forced H2; runtime WebSocket tests pass.
- H3 field-section limit is 256 KiB. Its 15 MiB connection credit, three 6 MiB
  stream credits, and peer stream counts 100/103 match the browser capture.
- All normalized H3 HTTP request fields match, including pseudo-header and
  regular-header order. H1/H2 JA4 hashes still match; H3 JA4 still differs.

The full core, HTTP and H3 suites, ALPS tests, seven runtime H2 WebSocket tests,
formatting and strict Clippy passed. Per-change reviews and a final combined
review completed; the final review found no additional actionable issues.

See [the comparison guide](chromium-comparison.md) for the repeatable command,
normalization, artifacts and limitations. The report counts differing fields,
not independent bugs: one header-order change affects several reported fields.

## 1. Qualify the remaining wire targets

Chromium 154 and 155 use the same wire targets for this work. Use the recorded
Chrome 155 binary for comparison while keeping the named recipe fixed. A future
recipe roll should record the browser binary digest and qualify changed targets.

Expand the local cases beyond a fresh GET: repeated requests and connection
reuse, HEAD/POST, duplicate headers and cookies, edits/reroutes, ALPS accepted
and declined, and resumption. Normalize reserved QUIC version codes inside the
version-information parameter while retaining their presence and order.
Keep ECH GREASE padding lengths as sampled observations; a single different
random padding length does not establish a compatibility gap.

Acceptance: each remaining difference has a pinned source or repeatable wire
expectation, with protocol fallback and missing evidence remaining hard errors.

## 2. Align QUIC's TLS offer and packet policy

The first increment uses existing APIs: QUIC has Chromium's nine signature
schemes, no ordinary TLS/signature GREASE or OCSP/SCT request, a zero-length
client source CID, eight-byte initial destination CID, and advertised UDP
receive size 1472. Explicit custom `NoSignature` mode still enables ordinary
GREASE. Observation IDs are independent random values so zero wire CIDs cannot
collapse separate physical connections. ECH GREASE and the common verifier are
unchanged. Review round 1 corrected regression-test assumptions and CID
identity coverage; round 2 found no further significant issues. The complete
H3 suite, TLS PQC/wire suite and strict Clippy passed.

Signature distinction: the pinned Chromium QUIC configuration does not add
ML-DSA to BoringSSL's default handshake verification preferences; TCP explicitly
does. Installing a root does not change that ClientHello list. Certificate path
verification separately supports ML-DSA issuer signatures, including an
RSA/ECDSA leaf issued by a locally trusted ML-DSA CA. Do not describe the QUIC
offer restriction as disabling all ML-DSA certificate chains.

The packet increment adds bounded quiche APIs for 1250-byte Initial datagrams,
omitting effective ACK defaults, extension transport parameters and RFC 9368
version information. Chosen-version mismatches fail with VERSION_NEGOTIATION_ERROR;
malformed encodings, duplicate/standard extension IDs and oversized values are
rejected. The reserved version's value and insertion position are randomized,
and the harness retains its position while normalizing only the reserved code.
The zero-CID production driver omits disable_active_migration; its connected
UDP socket remains pinned. ORIG permits ignored ORIGIN hints without granting
additional routing or connection-reuse authority. Datagram support is absent.
Three review rounds corrected test assumptions, version GREASE placement, FFI
coverage and snapshot publication under transient Windows directory locks.
H3 tests, isolated quiche regressions, strict Clippy and local comparison pass.

Keep the common certificate verifier and trust inputs. Introduce a
transport-specific offer policy at the shared TLS-context boundary, so QUIC
does not inherit TCP-only GREASE, signature preferences and OCSP/SCT requests.
Preserve custom-profile override meanings and report the effective offers.

Handle H3 ALPS as a real integration: negotiate its correct application label,
apply the server's HTTP/3 settings before dependent requests, and capture the
raw/decoded contents. Extending the existing H2-only ALPS offer just to change
the JA4 hash would leave settings semantics unqualified. Determine the required
quiche settings-injection API before advertising it.

Then align initial datagram size, client/server connection-ID lengths, advertised
UDP receive size, ACK parameters, QUIC reserved transport parameters and version
information. Prefer existing quiche APIs. Where a value is fixed internally,
propose a small upstream API and use the existing pinned-dependency/patch
workflow only for the necessary change. Preserve working Retry, version
negotiation, IPv4/IPv6 racing, ECH retry, resumption and connection isolation.

Acceptance: QUIC ClientHello inputs/JA4 and stable transport parameters match the
pinned browser; authenticated retry and certificate rejection tests still pass.

## 3. Align HTTP serialization

The second increment adds opt-in h2 request extensions for pseudo-header order
and root priority. The normal origin client and extended CONNECT use them.
Completed body channels expose end-of-stream without discarding queued errors
or trailers, allowing HEADERS to carry END_STREAM. In the local Chrome run all
H2 headers and HEADERS bytes match; the remaining startup difference is the
early real SETTINGS ACK. Keep the initial concurrency gate until an alternative
can prove ALPS limits are applied before request allocation. Round 1 review
added extended-CONNECT and terminal-error coverage and resolved-graph checks;
round 2 found no further significant issues.

H1 now preserves original case and interleaved duplicate ordering through the
Hyper parser, canonical model and outgoing serializer. Host comes from the
authorized target; Connection is generated from reuse/upgrade policy after
hop-by-hop filtering. Auto leaves Host synthesis to protocol selection so H2
does not receive a redundant Host beside `:authority`. Tests cover forced/Auto
H1, reuse, duplicates, stale Host, connection-nominated headers and injection.
The local Chrome run has no H1 HTTP differences, only sampled ECH padding.
Three review rounds fixed feature gating, Windows Git line-ending effects,
and the H2 Host regression. Minimal Hyper client/server/http1 feature builds,
HTTP tests, runtime checks and strict Clippy pass. The QUIC runtime PQC fixture
now distinguishes hybrid exchange from ML-DSA signatures, matching the real
Chromium QUIC offer without changing verifier coverage.

Transport registry archives are checksum-pinned and preserved with their
licenses under `third_party/rust-transport`. The complete patches and file
receipts are reproducible with `scripts/vendor-transport.py`; portable build
preflight checks both bytes and the resolved Cargo sources. Regeneration forces
LF independently of Git's user settings. Quiche's patch additionally exposes
the bounded packet and transport-parameter APIs described above.

For H2, add focused encoder controls for Chromium's pseudo-header order and the
HEADERS priority fields. Investigate startup-frame ordering alongside the ALPS
bootstrap: the current SETTINGS ACK precedes some initial frames. Preserve
correct peer settings, flow control and concurrency before sending requests.
Prefer upstreamable controls over rewriting encoded HPACK bytes in an I/O shim.
The numeric SETTINGS list itself is already aligned.

For H1, carry original header name/order metadata through the canonical request
and serializers, including user edits and protocol translation. Preserve Chrome's
mixed casing rather than applying blanket title casing. Generate Host and the
outgoing connection-management headers from the authorized destination and our
actual upstream reuse policy. Retain hop-by-hop filtering, framing repair and
protection against conflicting lengths; never forward a caller's Connection
header as a shortcut.

Acceptance: fresh/reused H1 and H2 request ordering and casing match across the
expanded cases, without breaking uploads, trailers, WebSockets or interception.

## 4. Implement missing H3 capabilities before advertising them

The pinned quiche 0.30 decoder still has unimplemented QPACK dynamic-table
paths. Its configuration setters can advertise nonzero capacities even though
those paths are incomplete. Keep capacity and blocked-stream promises at zero
until bounded table updates, inserts/evictions, blocked streams,
acknowledgements/cancellation and malformed-input handling are implemented and
tested against an independent peer. Prefer upstream support; assess a maintained
implementation or an adapter migration behind the existing interface if needed.

QUIC datagrams require an application consumer, routing/ownership rules, capture
semantics and bounded queues. Review that product scope before implementing or
advertising H3_DATAGRAM. Extended CONNECT/WebTransport promises likewise need
working end-to-end behavior. These are substantive capabilities, not fingerprint
switches.

Acceptance: interoperable QPACK compression and any approved datagram feature
work correctly under load and cancellation before their Chrome-like SETTINGS
are enabled. Unsupported capabilities remain absent from offers.

For each implementation stage, run a review/fix cycle of at most three rounds,
stopping early when no issues remain; commit the completed stage and rerun the
local comparison plus the affected interoperability tests.

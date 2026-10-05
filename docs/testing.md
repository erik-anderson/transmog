# Testing

Local fixtures are the correctness gate. The deterministic suite covers H1/H1,
H1/H2, H2/H1, H2/H2, H1/H3, and H2/H3 without depending on public content. It
also exercises streaming before producer completion, head/body replacement,
synthetic responses, aborts, timeouts, trust failures/reloads, concurrency,
bounded channels and bodies, HTTP/3 connection reuse, multiplexing, and
per-stream backpressure/cancellation isolation. It also rejects ambiguous
transfer codings and suppresses bodies for HEAD and bodyless response statuses.
The shared upstream verifier is exercised with valid DNS identities, unknown
roots, and wrong DNS names on H1/H2 and H3; H3 additionally proves an exact IP
SAN succeeds while all other certificate errors remain fail-closed.
Test H3 origins retain ownership of their bound UDP sockets until the client
has consumed the response. This prevents Linux connected-UDP `ECONNREFUSED`
delivery after a fixture task exits, without sleeps, retries, or weaker client
error handling; it also prevents negative certificate cases from passing for
the wrong transport reason.
The protocol matrix is expanded alongside
new failure handling rather than relying on public sites for correctness.
Slow partial HTTP/1 headers and stalled request bodies have deterministic
deadline tests; oversized declared bodies are rejected before contacting an
origin. Auto routing is exercised against an unreachable cached H3 alternative:
a safe GET falls back before response start with ordered attempt evidence, while
a POST is not replayed onto the TCP origin.

Hooks v2 has deterministic core coverage for factory isolation, forward/reverse
chain order, short circuits, exactly-once terminal outcomes, callback panic and
timeout containment, drop cancellation, all bounded body plans, trailer-order
model enumeration, observer redaction and saturation, route authorization,
application-owned streaming upstreams, and a 256-exchange concurrency stress
case. The bounded `DecisionBridge` tests correlation, duplicate and stale
replies, disconnect, timeout, cancellation, and queue saturation.
One shared upstream contract suite runs against an application-owned service,
the Hyper H1 adapter, and the quiche H3 adapter. It verifies canonical bounded
request/response streaming and typed plan propagation; adapter unit tests also
reject destination/authority mismatches before network I/O.

The content-processing Phase 1 suite strictly parses ordered and duplicate
`Content-Encoding` fields, proves reverse decode order, rejects ambiguous or
unsupported stacks, repairs representation headers/trailers, and exercises
absolute, expansion-ratio, exact-boundary, and arithmetic-overflow accounting.
The Phase 2 codec suite decodes independent gzip, Brotli, zlib, raw-DEFLATE,
and zstd vectors,
round-trips empty and fragmented representations, proves encoder output is
independent of input frame boundaries, keeps trailers terminal, rejects
malformed, truncated, checksum-invalid, and trailing-junk inputs, validates
every concatenated gzip member, and exercises encoded, decoded,
expansion-ratio, and output limits for every enabled codec. Deflate tests prove
strict zlib framing is the default and the raw compatibility policy is both
explicit and stable when its two-byte probe crosses frame boundaries.
Zstd tests validate concatenated frames and prove an encoded window larger than
the configured decoder-history bound is rejected before decoded bytes escape.
The Phase 3 suite proves deterministic raw/decoded requirement aggregation,
rejects mixed raw and decoded chains before pumping, and exercises reverse
multi-layer decoding plus forward re-encoding. It also proves exact neutral
pass-through, optional unsupported-coding bypass without invoking the hook,
required unsupported-coding failure, header and trailer repair, whole-stack
limits, and replacement of corrupt encoded input without decoding discarded
source bytes.
The Phase 4 runtime suite sends a gzip request through an application-owned
streaming upstream and returns a Brotli response. Required decoded hooks modify
both identity representations; the runtime restores each original coding,
repairs stale length and validator fields before head commitment, and exposes
only canonical frames to the application boundary. Policy tests also prove
disabled-mode required failure, optional exact bypass, and identity-body
compatibility.
The Phase 5 runtime suite composes gzip, Brotli, deflate, and zstd into one
ordered coding stack on both requests and responses across H1/H1, H1/H2,
H2/H1, H2/H2, H1/H3, and H2/H3. It proves decoded hook modification,
forward-order re-encoding, and stale length/validator repair in every row.
Separate fixtures prove output is streamed after the first complete gzip member,
a paused decoder does not block another stream on the same H2 or H3 connection,
an actual decoded-size overflow terminates the upstream body, and Auto fallback
replays the already processed coded request exactly once before response start.
The existing declared-size rejection test complements the in-stream limit test.
Phase 6 adds deterministic highly compressible inputs for every codec and
asserts that no bytes beyond the decoded limit escape, regardless of one-byte,
seven-byte, or whole-body input framing. A 64-task cancellation stress case
drops independently owned live encoders, verifies every task is cancelled, and
proves unrelated round trips still complete. These tests stay in the ordinary
cross-platform workspace gate.

The codec work-bound suite validates configuration and exact monotonic-time
boundaries without sleeping, then exercises the real async engines to prove
large encode and decode calls cooperatively yield. A deliberately exhausted
deadline must return typed `ContentCodecError::Timeout`, make the codec
terminal, and leave independently owned tasks unaffected. Fuzz targets use
smaller finite work quanta so their normal mutation paths also exercise the
decoder's quantum-resume state machine.

Coverage-guided targets live in the independent, non-published `fuzz/`
workspace. `content_encoding` mutates duplicate field boundaries and coding
syntax while checking canonical reparse invariants. `decode_stream` mutates
coding selection, frame boundaries, corrupt bytes, and valid seeded vectors
under fixed encoded, decoded, ratio, window, output, and stack-depth limits.
The separate workspace keeps `libfuzzer-sys` out of the shipping dependency
graph and SBOM. The checked-in Docker/WSL2 runner supplies the pinned nightly
and `cargo-fuzz` driver with Clang/Ninja, mounts the source read-only, and
isolates generated corpus entries from the checkout. Longer local campaigns
and corpus handling are documented in `fuzz/README.md`. Prepared hosted
automation is parked under `ci/github-actions/` and is intentionally inactive;
see `ci/README.md`.

The headless application/session suite covers deterministic terminal eviction,
active-session admission refusal, stable cursor paging and filters, credential
redaction before storage, cross-boundary body retention, subscriber lag,
observer sequence gaps, 512 concurrent exchange updates, create-new capture,
sealing and quota failure isolation, concurrent/idempotent stop, unexpected
runtime failure, controller exclusivity, capability negotiation, authority-safe
head edits, phase-invalid actions, bounded decoded body replacement with stable
hook attribution, replay risk/credential validation, replay timeout and
cancellation, host apply-before-publication, exact restore retry, and final-owner
cleanup. The service tests use injected runners, replay executors, and host
adapters; no test mutates the operating system.

## Standalone interoperability

The local interoperability gate runs the release proxy between independently
distributed clients and servers:

| Client | Origin | Assertions |
|---|---|---|
| curl | Nginx 1.28.0 | HTTP status, pinned server identity, proxy-added header and HTML marker, terminal exchange evidence |
| curl | Apache HTTP Server 2.4.65 | HTTP status, pinned server identity, proxy-added header and HTML marker, terminal exchange evidence |
| Chromium | Nginx 1.28.0 | navigation, response headers, DOM marker, terminal exchange evidence |
| Chromium | Apache HTTP Server 2.4.65 | navigation, response headers, DOM marker, terminal exchange evidence |
| curl | Caddy 2.10.2 over TLS | forced H2 and H3 origin egress, exact-IP SAN and chain verification, adapter/ALPN evidence |
| Chromium | Nginx 1.28.0 encoded fixtures | gzip, Brotli, zlib-deflate, zstd, and `gzip, br, deflate, zstd` decode/modify/re-encode with coding preservation |
| Chromium WebSocket | standalone Node echo server | browser-style plaintext HTTP/1.1 inside `CONNECT`, echo bytes, upgrade evidence, terminal relay byte counts |

Nginx, Apache, and Caddy use exact multi-platform image-manifest digests. Compose
publishes each origin on a random loopback-only port and waits for its health
check. Caddy binds the same loopback port for TCP and UDP and advertises H1, H2,
and H3; the proxy is forced to use H2 or H3 and reports the selected adapter and
ALPN. Every HTTP client request carries a unique proof value that only the
running proxy can add to both the response headers and HTML. The suite also
requires the corresponding exchange evidence record, so an implicit localhost
proxy bypass cannot look successful.

The runner creates an ephemeral CA for downstream proxy signing, then issues a
short-lived Caddy leaf containing the exact `127.0.0.1` IP SAN. Curl receives
the CA as its downstream trust anchor and the proxy augments its BoringSSL
upstream trust snapshot with the same public certificate. Verification remains
strict in both directions; no certificate-error bypass is used. Nothing is
installed into an operating-system trust store. Containers, the per-run Compose
network, generated encoded bodies, and private material are removed in a
`finally` block. Run it from PowerShell with Docker Desktop running:

```powershell
pwsh ./scripts/test-interop.ps1
```

Pass `-SkipBrowserInstall` only when the package's pinned Playwright Chromium
is already cached. This local, deterministic matrix complements rather than
replaces the HTTPS live-browser gate below: the latter proves verified CONNECT
interception and H1/H2/H3 Internet egress, while the standalone matrix proves
compatibility with exact third-party client and server distributions without
depending on public-site behavior. The standalone cases deliberately use H1
ingress. The deterministic in-process matrix remains responsible for H2
ingress and for every H1/H2/H3 ingress/egress combination; together the two
layers cover the protocol and coding matrix without making Docker fixtures the
sole correctness oracle.

The final compatibility gate uses Playwright-managed Chromium with a fresh
profile, browser QUIC disabled, no certificate-error bypass, and no Playwright
route interception. Its durable local test CA is generated and installed once
by `scripts/setup-live-test-ca.ps1`; the private key remains under `.local/`
with a user-only ACL and is never checked in. Each test run verifies that exact
SHA-256 certificate in the current-user Root store without modifying the store.
This avoids interactive certificate dialogs during automation while preserving
normal Chromium verification. Run `scripts/remove-live-test-ca.ps1` to remove
the exact root plus local certificate and key. The gate asserts proxy-added
response headers and DOM proof, records egress telemetry, and verifies that a
stopped-proxy navigation fails rather than going DIRECT.

`verification/live-report.json` is the machine-readable record. It includes the
per-case timestamp, ephemeral proxy address, proof and legacy-named hook event IDs,
downstream connection and stream IDs, ingress/egress protocols and ALPN,
adapter, trust generation, route-attempt history, and H3 peer/certificate-chain
fingerprints. The report also records that Chromium QUIC was disabled, service
workers were blocked, Playwright route interception and certificate bypasses
were not used, and that the durable CA was verified before and after the run.

One-time Windows setup and explicit teardown:

```powershell
pwsh ./scripts/setup-live-test-ca.ps1
pwsh ./scripts/test-live.ps1
pwsh ./scripts/remove-live-test-ca.ps1
```

Run `scripts/dev-env.ps1 -Check` before native builds and
`scripts/check-crypto-graph.ps1` after dependency resolution.

Additional hardening commands are:

```powershell
# Full Linux Clang/LLVM/Ninja matrix through Docker Desktop's WSL2 backend.
./scripts/test-linux-docker.ps1

# Deterministic model/fuzz smoke and concurrency tests are part of this suite.
cargo test --locked -p transmog-core --all-features

# Compile every example and benchmark harness.
cargo test --locked --workspace --all-features --all-targets

# Dependency-free fixed-input microbenchmarks; run on an otherwise idle host.
cargo run --locked --release -p transmog-core --example hooks_benchmark
cargo run --locked --release -p transmog-content --example content_benchmark

# End-to-end decoded content throughput, first-byte, cancellation, and sampled
# peak-working-set report. The JSON artifact is ignored by Git.
./scripts/benchmark-content.ps1
```

The transport-neutral core is intended to remain Miri-compatible. On a host
with the nightly component installed, run `cargo +nightly miri test -p
transmog-core --lib`. Miri is an additional diagnostic and is not installed
or silently skipped by `scripts/test.ps1`.

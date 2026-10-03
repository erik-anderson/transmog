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
The protocol matrix is expanded alongside
new failure handling rather than relying on public sites for correctness.
Slow partial HTTP/1 headers and stalled request bodies have deterministic
deadline tests; oversized declared bodies are rejected before contacting an
origin. Auto routing is exercised against an unreachable cached H3 alternative:
a safe GET falls back before response start with ordered attempt evidence, while
a POST is not replayed onto the TCP origin.

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
per-case timestamp, ephemeral proxy address, proof and breakpoint event IDs,
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

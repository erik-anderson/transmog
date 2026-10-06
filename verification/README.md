# Verification artifacts

The network-enabled Playwright gate writes ignored `live-report.json` and
`live-report.md` files here. They are per-run evidence, not maintained project
documentation or a source of truth. Failure screenshots, traces, and videos
remain under `e2e/playwright/test-results`. Reports contain protocol metadata
and proof identifiers only; they must not contain private keys, cookies,
authorization values, or response bodies.

Each report captures the proxy address, browser version, per-case completion
time, connection/stream/session and breakpoint event IDs, protocol/ALPN,
adapter, trust generation, route attempts, and HTTP/3 peer certificate
fingerprints. Root-level flags record the browser isolation and verification
settings used by the run.

The browser run uses the durable local CA created once by
`scripts/setup-live-test-ca.ps1`. Test execution only verifies that exact root
read-only, so repeated automation is non-interactive and uses no certificate
bypass. Its user-only private key lives under ignored `.local/`, never in the
repository. `scripts/remove-live-test-ca.ps1` is the explicit teardown.

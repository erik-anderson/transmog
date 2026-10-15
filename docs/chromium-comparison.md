# Local Chromium fingerprint comparison

Run from the repository root with Python 3.11+ and Docker Desktop using Linux
containers:

```powershell
python scripts/dev-env.py --check
python scripts/compare-chromium.py
```

The default performs three fresh direct/proxied pairs for **each of HTTP/1.1,
HTTP/2 and HTTP/3**. It builds the production proxy transports and local TLS/QUIC
origins, launches isolated headless Chrome profiles, and saves JSON evidence plus
an HTML comparison under `.local/chromium-comparison/<run>/`. It does not contact
ja4checker.com or any other public measurement service.

Docker image construction and Cargo dependency downloads need network access.
The measurement container runs with `--network none`: only its private loopback
is available. Container-scoped `NET_RAW` captures TCP SYN packets without host
capture software, host networking or privileged mode. Repository sources are
mounted read-only. Keys exist only in fixture memory; the exported PEM files are
public certificates. No host trust store or proxy setting changes.

For a quicker run or a parity gate:

```powershell
python scripts/compare-chromium.py --samples 1
python scripts/compare-chromium.py --protocols h2 h3 --samples 3 --require-match
```

`--output-dir PATH` selects a **new** evidence directory; existing directories are
never overwritten. `--refresh-browser` rebuilds the browser image with the
available Chrome release. Otherwise Docker may reuse its installed version.
The report records the actual browser product/version, image ID, repository
revision, tracked diff hash, fixture source hashes and effective proxy profile.
Changes during the build abort the run. Do not edit the checkout during a run.

An exit status of 0 means all requested measurements completed; differences are
reported without failing the command. `--require-match` exits 2 when normalized
differences exist. Missing evidence, protocol fallback, proxy bypass, capture or
decoding failures exit 1. A runtime failure writes `failure.json` and retains
fixture logs and completed per-leg captures; it does not produce a passing
report. The gate compares observed fields, not a compatibility allowlist.

## Measurement and coverage

Every navigation must return a unique proof token. Browser DevTools identifies
the negotiated browser protocol and certificate issuer. The origin independently
records the actual protocol, ClientHello and request; proxied cases also require
a completed production proxy exchange with the expected egress protocol. A
silent H3-to-H2 fallback fails the measurement.

For H1.1 and H2, both legs use that protocol to the origin. For H3, direct Chrome
uses QUIC; the proxied browser uses H2 CONNECT interception and Transmog uses its
production H3 origin transport. This tests **origin-facing H3 fingerprinting**;
Transmog does not provide a QUIC proxy ingress. Chrome's loopback bypass is
explicitly disabled for proxied navigations.

| Surface | Measured signals |
| --- | --- |
| JA4 / TLS | Published JA4 hash and its inputs: TLS version, SNI presence, cipher/extension counts, ALPN, sorted cipher/extension lists and ordered signature algorithms. Full public ClientHello bytes retain extension order and payloads. Decoded comparisons also include groups, key-share algorithms/lengths, TLS versions, compression, GREASE presence/positions, padding, ECH offer shape, and negotiated TLS version/cipher/ALPN. |
| JA4H inputs / HTTP | Method, HTTP version, complete ordered header names **and values**, original casing for H1.1, pseudo-header order, cookie presence/names/values, Referer and Accept-Language. A separately named SHA-256 digest covers the normalized HTTP signals. |
| JA4T inputs / TCP | Actual origin connection SYN: receive window, ordered options, MSS and window scale, plus IP TTL, option values and raw packet. Matched using both endpoints' addresses and ports. H3 explicitly reports TCP as not applicable because QUIC uses UDP. |
| H2 additional evidence | Decrypted client preface and frame bytes through the first request, ordered SETTINGS, connection WINDOW_UPDATE, frame order/flags/lengths, HEADERS priority fields and decoded HPACK header order. |
| H3 additional evidence | Ordered peer SETTINGS including unknown entries, decoded QPACK header order, raw/decoded QUIC transport parameters, initial datagram size/version and connection-ID lengths. |
| Attribution | Actual browser product/version, User-Agent/client hints in the request, effective proxy profile, direct/proxied certificate issuer and production egress evidence. |

These are the observable TLS, HTTP and TCP inputs behind the site's fingerprint
surfaces, with additional protocol detail. Proprietary bot-risk scores and
browser similarity estimates are not reconstructed; public-IP, ASN and
geolocation are not meaningful for loopback traffic. The report never treats
these unavailable fields as zero-risk or as evidence of parity.

The JA4 algorithm follows the [published TLS specification](https://github.com/FoxIO-LLC/ja4/blob/main/technical_details/JA4.md).
HTTP and TCP inputs are reported directly; this tool does not bundle the
separately licensed JA4H/JA4T implementations or label its own hashes as theirs.
See [upstream licensing](https://github.com/FoxIO-LLC/ja4#licensing).
Python HPACK decoding is a development-only distro package in the comparison
image. The fixture adds no shipping dependency.

## Interpretation and limits

GREASE code points, TLS random/session bytes, public key-share bytes, random
QUIC connection IDs and ephemeral listener ports are normalized where needed;
their raw values remain in the evidence. TLS extension order is retained but
excluded from stable comparison because Chromium randomizes it. Algorithm and
HTTP header order remain significant. Reserved H3 SETTINGS values are normalized
while their position and presence remain visible. The report lists fields that
varied within one client across samples; a single-pair delta in such a field is
not sufficient evidence of a systematic difference.

Chrome uses an SPKI-scoped allowance for the two ephemeral fixture CAs. Transmog
still verifies the origin using the shared production verifier and only the
fixture root. Its custom profile inherits the Chromium transport recipe, then
uses isolated OS-mode fixture trust with local-certificate overlays disabled and
the requested H1/H2/H3 protocol switches. Trust-anchor-ID metadata is the bundled
catalog, not the host's installed WebView2 component. The full effective profile
is saved in each `browser.json`. Interpret trust-anchor offer differences in
that context; this is not a comparison of Windows installed trust components.

The browser version can differ from the repository's pinned Chromium recipe;
the harness records this rather than claiming a version match. Both clients run
on the same Linux kernel, so TCP results do not qualify Windows TCP behavior.
The origin accepts a simple successful GET, declines ALPS, and uses no public
DNS. This run does not cover session resumption, real ECH DNS discovery, ALPS
server payload handling, certificate-verifier equivalence, connection reuse or
timing-based classifiers. Existing interoperability and verifier tests cover
separate aspects of those behaviors.

`results.json` and `report.html` summarize comparisons. Each leg retains
`browser.json`, `connection-*.json`, `signals.json`, `fixture.log`, public CA
certificates and (for proxied legs) `proxy-evidence.json`. `tcp-syn.json` retains
the run's local SYN observations. No TLS key log or private CA key is exported.

Parser regression checks run without Docker:

```powershell
python -m unittest discover -s scripts/tests -p test_browser_fingerprint.py -v
```

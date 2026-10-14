# Chromium session review, October 2026

Reviewed the 22 session commits from `52f5e68` through `2b3aa7b`, using
`86a6950` as the comparison base. Scope included ClientHello changes, network
profiles and pool admission, ALPS bootstrap/capture, PKIMetadata discovery,
Windows local trust monitoring and certificate verification. References were
the measured Chromium 154.0.8037.99 sources and its BoringSSL revision.

Three review/fix rounds replaced the upstream legacy X.509 verifier with the
single BoringSSL PKI implementation. Significant verification findings fixed:

- Public anchor expiry/constraint flags were ignored, including root path-length
  and certificate-policy constraints. Public name constraints could also be
  applied when their enforcement flag was off.
- Exact public-anchor matching could bypass the target certificate's signature.
  PKI instead builds a verified path, including a self-signature when the target
  is the same self-signed public anchor.
- CN-only certificates and SHA-1-signed issued certificates were accepted.
- Windows ROOT entries denied server trust by usage properties were also omitted
  from issuer search. Chromium still searches them as untrusted candidates.
- Recovery could follow a different verification path from normal handshakes.
  It now requires the same frozen PKI policy, including distinct trust roles and
  prospective issuer path-length constraints.

The following review also checked native handle ownership and concurrency,
duplicate public/local trust precedence, policy-positive controls, expected
names independent of SNI, and bounded captures. Captures use raw DER and the
actual PKI path, preserving duplicates rather than quietly changing the path.
The callback uses PKI-parsed SANs instead of a second hostname parser.

The last round reproduced a transport compatibility gap: the current Rust TLS
method eagerly parses the wire chain before the PKI callback. PKI can ignore a
malformed unused intermediate, while this method rejects it. Switching to
buffered TLS needs adaptation of the crate integration's X.509 APIs and remains
a maintenance decision, as requested. Other differences from Chrome's complete
browser verifier are listed in [the verifier contract](chromium-verifier.md).
This review does not claim full Chrome acceptance parity.

Validation completed:

- Full Windows workspace tests with all features, and strict workspace Clippy.
- Affected TLS/native/HTTP/QUIC/local-trust suites rerun after the raw-buffer
  cleanup; seven live TLS regressions plus policy-positive controls passed.
- Linux native bridge linkage, hostname/native tests and live TLS regressions.
- Desktop UI build and populated browser workspace/profile/capture checks.
- Python tooling fixtures, PSL download/reproducibility/offline checks, pinned
  PKI source hashes and the one-BoringSSL graph on Windows/Linux/macOS.
- Verified Google HTTP/2 request with ALPS through the new verifier.

The PSL and native source licenses are included in desktop credits,
third-party notices and generated supply-chain artifacts. Signed release
qualification was not dispatched as part of this review.

# Client certificate implementation

The first two phases provide a verified-request handshake adapter and a Windows
current-user software CNG identity source. Production proxy/Composer connection
ownership, pool/session isolation, pending decisions and desktop settings remain
the next integration phase. No desktop configuration or defaults change yet.

Use `PolicyClientAuthenticationResolver` with
`WindowsCurrentUserCertificateSource` and a connection-owned
`ClientAuthenticationAttempt`. Unconfigured origins continue without a
certificate. The source is queried after an actual verified CertificateRequest;
there is no anonymous probe or signing challenge during inventory. Keep the
inventory monitor only while a controller needs identity-change invalidation.

## Windows source

The supported store is CurrentUser/My. The supported provider is Microsoft
Software Key Storage Provider, with RSA and ECDSA P-256/P-384/P-521. Hardware,
third-party providers, machine keys, legacy CAPI and PFX import are outside this
phase. The accepted first-provider scope is Windows user store only.

Both curve-specific CNG identifiers and generic `ECDSA` keys are supported.
Generic keys are classified by `ECCCurveName`, including the Windows names for
the same NIST curves; key length alone never establishes curve compatibility.
Unknown curves remain ineligible.

The native boundary opens the store read-only, checks provider metadata before
opening keys, and uses silent CNG calls. Each acquisition checks the exact public
SHA-256 certificate, provider/container association, signing usage and matching
public key. SHA-1 is only a Windows store lookup index; the exact SHA-256 DER
check determines acceptance. Key handles stay inside synchronous native calls
and are released before their providers. No cached handle survives removal, no
private key is exported, and no underlying Windows UI is permitted.

Chain construction uses local issuers with AIA fetching and automatic trusted-root
updates disabled. It does not establish server-side trust in a client identity.
The complete local path remains available for issuer matching and monitoring.
The request-driven TLS adapter omits a terminal self-signed CA from transmission,
after checking its CA role and verifying its own signature. It preserves the leaf,
required intermediates, and same-name cross-signed CAs. Capture evidence contains
only the certificates actually queued on the wire.
Bounds are 4,096 store entries, 1,024 supported identities, 32 certificates per
path, 1 MiB per public certificate and 32 MiB of inventory public paths. Failures
expose generic explanations, with no provider/container names or native error text.

Inventory and final eligibility checks run on blocking workers. Certificate
installation repeats public eligibility on the SSL thread; signing reacquires
the exact identity on a worker. Cancellation and authorization checks reject
stale results. A two-second monitor detects certificate/path/key disappearance
and inventory failure transitions, then invalidates the resolver generation.
Dropping the monitor aborts it, and its weak resolver reference avoids an ownership
cycle. Future pool owners must act on that generation; the source alone does not
retire a production pool.

Inventory, candidate filtering and final eligibility jobs are owned by their
awaiting futures. Dropping an owner aborts queued work and signals running
inventory/filter batches to stop between lookups. Native calls that already
started may finish. Cancellation and authorization are rechecked before final
eligibility; a parked decision uses a weak connection reference so it cannot
retain an abandoned owner.

## Chromium comparison

Reviewed Chromium's [Windows platform key implementation](https://raw.githubusercontent.com/chromium/chromium/main/net/ssl/ssl_platform_key_win.cc),
[platform key worker](https://raw.githubusercontent.com/chromium/chromium/main/net/ssl/ssl_platform_key_util.cc),
[certificate-store acquisition](https://raw.githubusercontent.com/chromium/chromium/main/net/ssl/client_cert_store_win.cc)
and [algorithm preferences](https://raw.githubusercontent.com/chromium/chromium/main/net/ssl/ssl_private_key.cc).
This adapter follows its CNG signature format: hash the complete TLS input once,
use the requested RSA hash identifier, use digest-length PSS salt, retain CNG's
RSA byte order, and encode raw ECDSA r/s as DER. Native work runs on workers, and
ordinary RSA/ECDSA preferences match Chromium before shared version/curve filtering.
The adapter also follows Chromium's omission of terminal self-signed roots, which
avoids servers rejecting an unnecessary root's signature algorithm.

Chromium also accommodates legacy CAPI and hardware providers and can retain an
acquired key. This source deliberately uses fresh, silent acquisitions for its
tested software scope. It does not copy Chromium's legacy hardware capability
signing probes. Windows documents the certificate property's legacy key spec
separately from the acquired CNG handle tag in
[CRYPT_KEY_PROV_INFO](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/ns-wincrypt-crypt_key_prov_info),
and the padding/prehash contract in
[NCryptSignHash](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptsignhash).

## Verification and review

Run the portable environment check before native builds, then:

```text
python scripts/dev-env.py --check
python scripts/dev-env.py --run cargo test --locked -p transmog-core -p transmog-tls -p transmog-tls-native -p transmog-client-certificates
python scripts/dev-env.py --run cargo test --locked -p transmog-tls --test windows_client_certificates -- --ignored --nocapture
```

The Windows test is explicitly ignored in ordinary runs because it creates owned
non-exportable keys in CurrentUser/My. It proves PKCS#8 private export is rejected,
then verifies real TLS 1.2/1.3 CertificateVerify signatures and different
authenticated/anonymous application responses. It covers RSA PKCS#1 SHA-1/256/384/512,
RSA-PSS SHA-256/384/512, ECDSA SHA-1 and all supported curves, including TLS 1.2
P-384/P-521 with SHA-256. Locally available issuer paths retain their roots while
the live server and queued public evidence confirm leaf-only transmission for
directly issued identities. Key deletion rejects stale providers, invalidates the
monitor generation and permits anonymous connections without falling through to
another identity. Certificate removal is also checked. Owned keys and public
issuers are removed on normal completion and test unwinding; no root trust is added.
The Windows release gate invokes this test explicitly before signing credentials.

The real Windows suite also covers generic named-curve P-256/P-384/P-521 keys,
verifies their `ECDSA` identifier, and performs TLS 1.2/1.3 signing with each.
Gated live handshakes cover queued filtering/final eligibility during explicit
cancellation, owner abort, deadline expiry and policy-generation changes.
Worker tests distinguish aborting queued jobs from allowing a started call to
finish and observe cancellation before continuing its batch.

Live TLS 1.2/1.3 chain tests verify root omission with SHA-256 and SHA-1 root
self-signatures, preservation of necessary intermediates (including a same-name
cross-signed CA), and preservation of self-signed leaf identities with or without
CA constraints. A terminal CA with an invalid self-signature is retained rather
than classified as a verified self-signed root. Each scene validates possession,
authenticated application bytes, the exact server-observed chain, matching capture
evidence, and the unchanged complete local path. The QUIC acceptance scene also
checks that its root is omitted.

The post-implementation review covered native pointer/buffer lifetimes, property
sizes and string termination, handle ownership, key pairing, algorithms, thread
placement, cancellation, redaction, inventory changes and fixture cleanup. It
corrected legacy-key-spec handling and bounded strings to Windows' returned byte
count, including a regression that rejects a terminator found only in allocation
padding. The reviewed source has no remaining actionable findings within this
phase. Hardware/device behavior and complete desktop/proxy acceptance remain
separate gates.

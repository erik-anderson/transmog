# Chromium verifier contract

Transmog maintains one upstream certificate verifier. TCP, QUIC and Trust
Anchor ID recovery use BoringSSL's PKI path builder, the implementation used by
Chromium. Root-source selection changes its inputs; it does not select a
different verifier. Future relaxations belong in explicit settings on this
same implementation, rather than in alternate verification paths.

The unmodified PKI target is pinned to BoringSSL
`427ec40cc8edc545253289232e885708c409e5ea`, matching our boring-sys submodule.
The thin [native bridge](../crates/tls-native/native/verifier.cc) links it to the
existing crypto instance. Its snapshot, licenses and source hashes are in
`third_party/boringssl-pki`; `scripts/vendor-boringssl-pki.py --check` verifies
them. Update the PKI snapshot together with the pinned Rust boring family.

The comparison release is Chromium **154.0.8037.99**, whose BoringSSL revision
is `ac39ea6853833c1f18fd23614091d11855e71752`. Comparing the two revisions found
no traditional X.509 path-building/verification changes: the relevant later
changes concern MTC error handling and OCSP parsing. Transmog does not enable
MTC anchors. This is shared PKI behavior, not a claim of complete Chrome browser
certificate-verification parity.

The shared pin is stricter than Chromium 154's OCSP parser about the required
empty payload of GOOD/UNKNOWN status NULLs. Transmog retains the unmodified
upstream parser: malformed status encodings soft-fail as invalid/unknown under
the default policy. Valid stapled responses use the same signature, issuer and
freshness checks. This malformed-input difference is not full byte-for-byte
browser parity.

## Trust and verification contexts

[Root-source selection](root-stores.md) supplies an immutable `TrustSnapshot`.
OS roots are enumerated as DER; the operating system does not build upstream
verification paths. Chromium/WebView2 profiles use public component roots plus
the optional monitored Windows local-trust overlay.

Expected peer names are independent of SNI and capture collection. Embedders
using raw TCP SSL objects should call `configure_ssl_metadata_for_peer`,
particularly for IP origins or when disabling SNI.

Live reload is exposed through `ProxyServer::control().reload_trust(...)`. A
reload must use a strictly increasing generation number. The runtime constructs
new Hyper HTTP/1.1/HTTP/2 and quiche HTTP/3 clients before atomically publishing
the generation; existing exchanges retain their original clients until they
finish, so connection pools never mix trust generations.

## Buffered certificates

Production TCP and QUIC contexts use `TLS_with_buffers_method`, as Chromium does.
The shared PKI verifier parses the leaf and candidate issuers rather than having
the TLS compatibility layer eagerly parse every wire entry. An irrelevant malformed
issuer can be ignored when another valid trusted path exists; a malformed leaf or
unusable required issuer still fails. H3 telemetry hashes raw certificate bytes and
does not impose a second parse requirement on unused entries.

Credentials and requested CA names use buffer APIs. QUIC connections are created
without quiche's legacy hostname setter; SNI and expected identity are installed
separately before the first packet. The production TCP adapter uses
`tcp_context_builder`; worker and synchronous embedding APIs are described below.

## Missing issuers

The single PKI path builder consults supplied issuers and the intermediate cache
first, then retrieves AIA CA-issuer URLs when it needs additional candidates.
Like Chromium's issuer source, it starts at most five valid URLs per certificate,
waits for their results in order, accepts individual DER, CMS/PKCS#7, or the first
PEM certificate, limits decoded responses to 64 KiB, and bounds each batch to 10
seconds. HTTP content encodings are decoded within that bound; encoded input is
additionally limited to 256 KiB.
Only HTTP is supported, including after redirects. Fetches send no cookies,
caller headers or authentication and bypass the proxy's TLS machinery to avoid
recursive verification. The path builder retains its 60-second and 20-iteration
limits. Cancellation stops outstanding fetches.

Production TCP and QUIC verification runs on bounded workers. Only owned DER,
expected identity, SCT/OCSP evidence and frozen trust inputs cross threads; the
TLS object and capture publication stay on their owning thread. Low-level
synchronous embedding contexts retain synchronous verification; `hyper_connector*`
and async adapters install the worker callback and require a TLS task waker.
Use `synchronous_connector*` for a blocking TLS stream. Verification rules
are identical; only execution differs.

Only intermediates from fully accepted paths enter the bounded cache: up to 256
certificates and 4 MiB, with seven days of inactivity expiry. Leaf certificates
and anchors are excluded. Cached certificates remain untrusted candidates;
every new path rechecks trust, expiry, constraints, hostname, CT and revocation.
Capture details report AIA retrieval attempts and whether a cached candidate was
used. This is a process-local intermediate cache, not Chromium's complete HTTP
cache or browser-level certificate-verification cache.

## Implemented core policy

- Chromium parse options, RSA minimum 1024 bits and `kStrong` certificate
  signature policy; supported curves and signatures are decided by PKI.
- RFC 5280 path building and validation, including alternate paths, name
  constraints, path length, key usage, EKU, critical extensions, certificate
  policies and policy constraints. Twenty path-building iterations and a
  sixty-second deadline match Chromium's bounded search.
- Public anchor-only trust with metadata expiry/constraint flags. A public
  anchor in the target position must build a verified path, including its own
  signature if the path returns to that same self-signed anchor.
- Windows ROOT anchor-or-leaf trust with enforced expiry/constraints and
  required leaf self-signatures. Machine TrustedPeople is exact leaf trust;
  its collection is not searched for issuers. ROOT and CA supply untrusted
  issuer candidates. Exact Disallowed wins at every path depth.
- SAN-only hostname checks, exact IP SANs, whole-first-label DNS wildcards and
  the ICANN registry boundary. Private PSL entries are excluded, matching
  Chromium's hostname check. Captures preserve PKI's chosen path and bounded
  failure diagnostics; capture observers cannot grant trust.
- Public-program leaf lifetime limits (398/200/100/47 days by issuance date),
  non-unique names and Chromium's fixed SPKI domain restriction.
  SANs are host-canonicalized before the fixed restriction's registry lookup,
  including escaped bytes and IDNA mappings. That lookup includes private PSL
  registries, while TLS hostname wildcard matching retains ICANN-only rules.
  Root-program identity is separate from local trust; public-only restrictions
  do not apply to private roots. Chromium's compiled SPKI blocklist applies at
  every path depth, including locally trusted paths. Test fixture catalogs are
  explicitly private test roots; production component catalogs retain public classification.
- Root metadata constraint groups are evaluated on candidate paths, including
  version bounds, leaf issuance-time bounds and RFC 5280 permitted DNS subtrees.
  Every condition within a group must pass; any passing group suffices. Eligible
  local ROOT trust bypasses metadata constraints. MTC serial bounds are ignored
  for ordinary X.509. Conflicting duplicate anchor policies fail decoding.
- SCT signatures from certificate and TLS extensions, with reconstructed
  precertificate TBS, log-key binding, SHA-256 RSA/ECDSA signatures and Chromium's
  60-second future-timestamp grace. CT policy applies to public roots, with
  operator diversity, lifetime-based embedded quorum, retirement and Static CT
  API leaf-index rules. SCT-based metadata constraints use verified timestamps
  from currently qualified logs, and are skipped when CT is disabled or stale.
- CRLSet SPKI, raw-subject and issuer/serial checks across the selected candidate
  path, including anchors. Expired sets keep blocking records; only positive
  leaf coverage expires. A valid revoked stapled OCSP response rejects the path
  even with online checks disabled. Missing, invalid or unknown staples soft fail
  under the normal Chromium policy; optional online checking is not enabled.

Recovery uses these same trust inputs and path builder. A candidate CA must
terminate at anchor trust and be capable of issuing a TLS leaf; it cannot gain
CA authority from TrustedPeople. Its issuer position is included when checking
path-length limits. The fresh TLS retry still performs full server verification.
Recovery checks root version eligibility, trust and revocation without applying
leaf issuance, DNS or SCT metadata conditions to the candidate CA itself. Those
conditions are evaluated against the retry's actual TLS leaf and verified SCTs.

## Remaining Chrome policies

Remaining work on the shared verifier:

- Chrome's optional online-revocation modes and EV-specific hard-fail checks.
  The ordinary policy matches Chrome's default offline CRLSet/stapled-OCSP use.
- EV classification, Chrome's clock-recovery retry, MTC and QWAC policy.

These gaps must stay visible when describing Chromium matching. Do not treat
the verifier name or a successful PKI path as evidence that CT/revocation was
checked.

## CT and revocation components

Installed Chrome and WebView2 use the same parser and policy. The root-source
profile selects the browser's `PKIMetadata/ct_config.pb` and
`CertificateRevocation/crl-set` directories beside its root-store directory.
OS and bundled root profiles use the bundled verification data. An explicit
root protobuf changes roots; it does not implicitly replace CT or revocation
inputs. Embedders can supply explicit directories with
`VerificationDataProvider::new` and `with_verification_data`.

CT accepts compatibility versions through 4 and lists no older than the
compiled baseline; the emergency disable switch precedes those checks. The
newest supported installed component is considered first. A list is timely
while its age is less than 70 days. Stale lists soft-disable CT enforcement and
SCT-based root constraints, matching Chromium; they never masquerade as a
successful CT check. Valid empty newer lists remain empty and fail CT for public
paths. Chromium 154 ignores OCSP-delivered SCTs by default and does not require
an RFC6962 log alongside Static CT API logs; these pinned feature defaults are
shared across sources.

CRLSet freshness follows its embedded sequence, not its folder version. Each
browser keeps its own selected component and last-good data; a newer Chrome
sequence cannot replace WebView2's list. Missing, corrupt, partial or unsupported
updates retain the last-good snapshot. Background workers scan once per minute
without filesystem IO in the SSL callback. New physical connections freeze both
datasets; a data change retires TCP/QUIC pool admission and cached TLS sessions,
while previously admitted work finishes with its original evidence.

The bundled CTConfig is generated from the licensed public Chromium log JSON;
the built-in CRLSet is sequence 0 plus Chromium's compiled key lists. Signed
browser updater packages are not redistributed or downloaded. Captures retain
source, digest, UTC timestamps, CRLSet sequence and actual per-path CT/CRLSet/OCSP
outcomes. The certificate inspector's existing **Connection and recorded policy**
disclosure presents these facts. Resumed upstream handshakes reverify the stored
chain and security evidence against current policy and time, as Chromium does;
the path is labeled as restored from the session rather than newly sent by the
server. Native captures and both SAZ profiles preserve the evidence.

To roll the baseline, run `python scripts/update-chromium-certificate-policy.py
--chromium-version FOUR.PART.RELEASE.VERSION`, then the same script with `--check`
and `python scripts/test-chromium-certificate-policy.py`. Review the source receipt,
generated protobuf and verifier feature/policy changes; this data roll does not
automatically change the static network profiles. Requalify the cryptographic,
pool-refresh, saved-capture, Google interoperability and UI checks.

## Rolling the Public Suffix List

The exact PSL snapshot, generated ICANN rules and MPL-2.0 license live in
`third_party/chromium-suffix-list`. The generated full rule set also supplies
private registries to the fixed CA domain restriction. To update to an exact
Chromium release:

```powershell
python scripts/update-chromium-psl.py --chromium-version 154.0.8037.99
python scripts/update-chromium-psl.py --check
python scripts/test-chromium-psl.py
python scripts/dev-env.py --run cargo test -p transmog-tls-native hostname
```

Replace the version with the desired four-part release and review the diff and
`snapshot.json` receipt. The updater fetches the release's source snapshot,
regenerates ICANN rules and records hashes. `--input PATH` supports an already
downloaded source. `--check` is offline and read-only. Profile recipes remain
static; this data roll does not itself roll the TLS offer or BoringSSL pin.

Reference code for the comparison:
[Chromium verifier](https://github.com/chromium/chromium/blob/154.0.8037.99/net/cert/cert_verify_proc_builtin.cc),
[outer certificate policy](https://github.com/chromium/chromium/blob/154.0.8037.99/net/cert/cert_verify_proc.cc),
[hostname matching](https://github.com/chromium/chromium/blob/154.0.8037.99/net/cert/x509_certificate.cc),
[Windows trust roles](https://github.com/chromium/chromium/blob/154.0.8037.99/net/cert/internal/trust_store_win.cc).

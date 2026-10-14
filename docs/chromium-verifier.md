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
  non-unique names and Chromium's fixed SPKI domain restriction. Root-program
  identity is separate from local trust; public-only restrictions do not apply
  to private roots. Chromium's compiled SPKI blocklist applies at every path
  depth, including locally trusted paths. Test fixture catalogs are explicitly
  private test roots; production component catalogs retain public classification.
- Root metadata constraint groups are evaluated on candidate paths, including
  version bounds, leaf issuance-time bounds and RFC 5280 permitted DNS subtrees.
  Every condition within a group must pass; any passing group suffices. Eligible
  local ROOT trust bypasses metadata constraints. MTC serial bounds are ignored
  for ordinary X.509. Conflicting duplicate anchor policies fail decoding.

Recovery uses these same trust inputs and path builder. A candidate CA must
terminate at anchor trust and be capable of issuing a TLS leaf; it cannot gain
CA authority from TrustedPeople. Its issuer position is included when checking
path-length limits. The fresh TLS retry still performs full server verification.

## Remaining Chrome policies

These are additional work on the same verifier, not reasons to maintain a
legacy fallback:

- Buffered TLS transport integration. The callback consumes raw TLS buffers and
  PKI-parsed SANs, but the current Rust connector/quiche integration uses
  BoringSSL's legacy TLS method, which eagerly caches X.509 objects before the
  callback. It can reject malformed unused wire intermediates that Chromium's
  buffered TLS method lets PKI ignore. Changing methods requires adapting the
  crate integration's X.509-dependent configuration and accessors. This is a
  deferred maintenance decision, not a second certificate verifier.
- CT signature/log verification and Chrome's CT policy, which need a maintained
  log list and policy data. Requesting SCTs is not CT enforcement.
- CRLSet, built-in blocked-key/name policy and revocation processing, including
  stapled OCSP and Chrome's optional online-revocation policy.
- SCT-based root metadata constraints await validated CT timestamps. They fail
  conservatively until CT integration; the other constraint groups are evaluated
  per path rather than excluding constrained anchors from the catalog.
- AIA issuer fetching and intermediate caching. Path building currently uses
  peer-supplied certificates, metadata helpers and the monitored local stores.
- EV classification, Chrome's clock-recovery retry, MTC and QWAC policy.

These gaps must stay visible when describing Chromium matching. Do not treat
the verifier name or a successful PKI path as evidence that CT/revocation was
checked.

## Rolling the Public Suffix List

The exact PSL snapshot, generated ICANN rules and MPL-2.0 license live in
`third_party/chromium-suffix-list`. To update to an exact Chromium release:

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

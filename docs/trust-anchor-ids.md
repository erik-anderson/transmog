# Trust Anchor IDs and root-store selection

The Chromium 154 recipe offers certificate-based Trust Anchor IDs in the TLS
`trust_anchors` extension (0xca34), on TCP and QUIC. These describe X.509 anchors,
not a choice of signature algorithm, so “non-PQC” would be misleading. Merkle
Tree Certificate identifiers are not advertised. The bundled root-store version
41 reproduces all 28 IDs observed in Chromium 154.0.8037.99, including raw-byte
sorting and TLS-trusted additional certificates. IDs are assigned relative OIDs,
not certificate hashes.

## Profiles and trust sources

**WebView2 (Chromium 154)** is the default profile. Its transport settings match
**Chromium 154**; only the root-store source differs. Chromium uses installed
Chrome metadata and WebView2 uses installed WebView2 metadata. Both fall back to
the bundled public Chromium snapshot. Customize exposes four root-store choices:

| Choice | Certificate verification and metadata |
| --- | --- |
| Public OS Root Store | Configured OS roots, including locally installed roots. Embedders retain explicitly configured trust. Bundled Chromium metadata supplies ID offers and untrusted chain-building helpers. |
| Bundled Chromium Root Store | Public TLS anchors from the shipped snapshot; no component discovery. |
| Installed Chrome Root Store (fall back to bundled) | Newer valid `crs.pb` from `%LocalAppData%\Google\Chrome\User Data\PKIMetadata\<component-version>`. |
| Installed WebView2 Root Store (fall back to Chromium bundled) | Newer valid `crs.pb` from `%LocalAppData%\Microsoft\Windows\SharedWebView\EBWebView\PKIMetadata\<component-version>`. |

The root-store setting changes actual origin certificate trust independently of
whether Trust Anchor IDs are enabled. Public Chromium stores do not merge private
OS roots. Loading a store never installs certificates into Windows or modifies
Chrome/WebView2 files. Transmog does not download Google's updater package or
claim to verify a local component's CRX signature.

Each installed source has a separate background resolver. It scans numeric
component folders at startup and every minute, with bounded file/count limits.
Freshness uses the protobuf's `version_major`, rather than mtime or folder name.
Only a strictly newer usable root store replaces the last-good snapshot. Missing,
partial, malformed or older files retain that copy, initially the bundle.
Fresh connections freeze their metadata and trusted store together. Established
connections finish normally. TCP sessions authenticated under an older store
cannot resume across a discovered trust change without certificate verification.

**Customize → More handshake options** also allows disabling Trust Anchor IDs
and providing an optional full path to `crs.pb`. That file replaces the bundled
baseline; a newer copy from the chosen installed source takes precedence. With
OS trust, it supplies only ID metadata/helpers. Save only preserves a definition;
Save and use loads a file before activating fresh proxy/Composer pools. Invalid
files prevent activation when used for trust or IDs. If a saved file disappears,
inspection and Settings remain usable while sends/proxy startup explain repair.
Save and use reloads a repaired file even if the recipe is unchanged.

Schema 9 state migrates to schema 10: the previous built-in default becomes
WebView2. Authored profiles retain OS trust and previous offers. Historical
captures missing the root-store field retain OS trust; missing Trust Anchor IDs
remain disabled. Imported evidence is never rewritten to today's defaults.

## Verification limits and recovery

Transmog uses BoringSSL's X.509 verifier, including hostname, purpose, validity,
signature and certificate constraints. Chromium metadata's additional TLS anchors
can terminate a verified path, enabling omitted-intermediate handling. OS mode
keeps metadata certificates as untrusted helpers.

This is root-store selection, not a complete implementation of Chrome's verifier.
An anchor with metadata ConstraintSets is admitted only if at least one set is
unrestricted for the measured Chromium version. Sets requiring Chrome's SCT/CT,
DNS, serial or issuance-time policy are conservatively excluded. This can reject
some certificates Chrome would accept. Root certificate expiry and X.509
constraints remain enforced by BoringSSL, including for additional TLS anchors.
Chrome's EV, CT log policy, revocation lists and MTC verification remain gaps.
Excluded-anchor counts and the applied policy are retained in connection evidence.

A server matching marker never authorizes a chain. After certificate rejection,
available server IDs can trigger one retry before HTTP data is sent. Only catalog
IDs whose CA verifies through selected trust are requested, in server preference
order. Unknown IDs, untrusted CAs, an unchanged offer and a second failure do not
cause further recovery. Both physical attempts remain captured.

## Recorded evidence and embedding

Traffic → Details → Connection reports the server's matching marker. Original
connection settings retain the effective trust source, root-store version/hash,
policy/exclusions, PKI provenance and requested IDs, even when ID offers are off.
Diagnostic metadata retains available server IDs and recovery status. ID lists
display OIDs alongside wire hex. Resumed sessions identify the absence of a new
certificate. Native captures and SAZ preserve these bounded public facts; missing
historical evidence remains unavailable.

Embedders can use `PkiMetadata::from_file`, `PkiMetadataProvider::new` and
`UpstreamTlsContextFactory::with_pki_metadata`. The network profile determines
whether a provider supplies trust or only metadata. `refresh` performs discovery
off transport threads; `current` returns an immutable snapshot without file IO.

The bundle is generated from pinned public Chromium source, with its full BSD
notice in Credits. See [snapshot provenance and generator](../third_party/chromium-root-store/README.md).

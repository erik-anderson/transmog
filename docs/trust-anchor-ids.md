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
whether Trust Anchor IDs are enabled. Both built-ins also enable **Trust locally
installed Windows certificates**. Customize can disable it to test the public
store alone. OS mode already includes local roots, so its switch is disabled.
Loading a store never installs certificates into Windows or modifies
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

A valid installed component can contain roots without any supported Trust Anchor
IDs. In that case Transmog keeps those roots and omits the `trust_anchors`
extension; it does not borrow IDs from the Chromium bundle or send an empty
offer. The bundled fallback applies when no newer usable root store is found.
This follows [Chromium's nonempty-offer policy](https://github.com/chromium/chromium/blob/154.0.8037.99/net/ssl/ssl_config_service.cc)
and its [component ID replacement](https://github.com/chromium/chromium/blob/154.0.8037.99/chrome/browser/component_updater/pki_metadata_component_installer.cc).
A local check of Edge
155.0.4283.45 through the proxy also found the extension absent in all eight
ClientHellos, including isolated profiles seeded with its installed version 46
component. No TLS feature flags or certificate-error overrides were used.

**Customize → More handshake options** also allows disabling Trust Anchor IDs
and providing an optional full path to `crs.pb`. That file replaces the bundled
baseline; a newer copy from the chosen installed source takes precedence. With
OS trust, it supplies only ID metadata/helpers. Save only preserves a definition;
Save and use loads a file before activating fresh proxy/Composer pools. Invalid
files prevent activation when used for trust or IDs. If a saved file disappears,
inspection and Settings remain usable while sends/proxy startup explain repair.
Save and use reloads a repaired file even if the recipe is unchanged.

Schema 9 selects WebView2 for the previous built-in default. Schema 10 migrates
to schema 11 with Chromium revision 5 and WebView2 revision 2, adding local trust
to the pre-release built-ins. Authored settings remain fixed; custom support has
not shipped, so no custom-default updater is introduced. Built-ins declare fixed
settings explicitly; OS and PKIMetadata updates only refresh certificate data.
Historical
captures missing the root-store field retain OS trust; missing Trust Anchor IDs
remain disabled. Imported evidence is never rewritten to today's defaults.

## Windows local trust

Local trust follows Chromium's read-only `CERT_STORE_PROV_SYSTEM_REGISTRY_W`
selection. ROOT, CA and Disallowed use machine/user, group-policy and
machine-enterprise locations. TrustedPeople uses machine locations only.
This excludes the aggregate SYSTEM provider and automatically managed AuthRoot
public roots. An explicitly installed ROOT entry remains a local trust choice,
even when its certificate is also public.

ROOT uses Chromium's anchor-or-leaf trust with enforced anchor expiry and
constraints and required leaf self-signatures. TrustedPeople uses exact leaf
trust with a required self-signature and is not searched for issuers. The PKI
verifier decides whether a ROOT entry can authorize an issuer path. CA entries
and all ROOT entries, including entries ineligible for server trust, remain
untrusted issuer candidates. Eligibility intersects
DER EKU and Windows usage properties, requiring serverAuth or any-purpose when
restricted. Disallowed rejects a certificate at any path depth regardless of
purpose, including public anchors. Normal hostname, expiry, server usage and
certificate constraints still apply.
Direct ROOT leaf trust requires a verified self-signature even for CA entries.
Subject and issuer names use canonical X.509 comparison. Local ROOT path-length
constraints count non-self-issued intermediate CAs, while allowing self-issued
key rollovers and direct issuance within the constraint.

A worker owns certificate collections and a shared manual-reset event.
`CertControlStore(NOTIFY_CHANGE)` registers notifications; reset, `RESYNC` and
rearming precede enumeration. UI/transport threads never enumerate the registry.
A signaled event invalidates trust before refresh. New TLS requests wait
asynchronously for a current bounded inventory or report a monitoring/read
failure, without using stale local trust. H1/H2/H3 and extended-CONNECT pools
retire old generations for new work; admitted streams finish. Cached TLS sessions
cannot bypass changed trust. Tests use temporary registry stores outside system
certificate locations and never modify the computer's real trusted roots.

Automatic local Windows trust is unavailable on other OSes. Embedders can supply
`LocalTrustMonitor::fixed` or `controlled`; automatic monitors remain read-only.
No private-root Trust Anchor IDs are invented: offers still come only from the
selected public PKIMetadata.

## Verification limits and recovery

Transmog uses one Chromium BoringSSL PKI path builder, including purpose,
validity, strong certificate signatures and RFC 5280 constraints. Hostnames
follow Chromium's SAN-only policy and ICANN wildcard boundary. Metadata's additional TLS anchors
can terminate a verified path, enabling omitted-intermediate handling. OS mode
keeps metadata certificates as untrusted helpers.

This is root-store selection, not a complete implementation of Chrome's verifier.
Metadata ConstraintSets are evaluated on candidate paths. Conditions within a
set are combined with AND; any passing set suffices. Version bounds, DNS subtrees
and leaf issuance-time bounds are enforced. MTC serial bounds are ignored for
ordinary X.509, as in Chromium. SCT-dependent sets use cryptographically verified
timestamps from currently qualified logs, and are skipped when CT is disabled or
the selected log list is stale. Public anchors use the protobuf's
`enforce_anchor_expiry` and `enforce_anchor_constraints` flags (both default to
false), including additional TLS anchors. Eligible public trust overrides
duplicate local trust flags; exact local distrust still wins. Windows local ROOT
enforces expiry and X.509 constraints, including certificate policies.
Chrome's additional policies remain tracked in [the verifier contract](chromium-verifier.md).
The applied policy is retained in connection evidence; constrained anchors are
no longer excluded as a group during loading. Conflicting duplicate policies
and malformed version bounds prevent a component update from replacing last-good.

A server matching marker never authorizes a chain. After certificate rejection,
available server IDs can trigger one retry before HTTP data is sent when the
initial connection offered IDs. Only catalog
IDs whose CA verifies through selected trust are requested, in server preference
order. Unknown IDs, untrusted CAs, an unchanged offer and a second failure do not
cause further recovery. Both physical attempts remain captured.
Recovery freezes current trust again and excludes leaf-only local trust from CA
selection, including locally trusted self-signed CA certificates in TrustedPeople.
It also enforces local anchor path-length limits, counting the requested CA as
an issuer because it must be able to authorize the server's leaf certificate.

## Recorded evidence and embedding

Traffic → Details → Connection reports the server's matching marker. Original
connection settings retain the effective trust source, root-store version/hash,
policy/exclusions and PKI provenance, even when ID offers are off. Requested IDs
are retained when offered.
An empty supported ID catalog is reported as **Not offered** with its reason;
there is no requested-ID list or selection recovery for that connection.
Local trust adds inventory generation/hash, role counts, monitoring source and
the verified path's trust role. Capture does not dump the private inventory;
observed verification-path certificates remain inspectable. Native captures and
SAZ retain these facts. Missing historical local-trust fields do not claim an overlay.
Diagnostic metadata retains available server IDs and recovery status. ID lists
display OIDs alongside wire hex. Resumed sessions identify the absence of a new
certificate. Native captures and SAZ preserve these bounded public facts; missing
historical evidence remains unavailable.

Embedders can use `PkiMetadata::from_file`, `PkiMetadataProvider::new` and
`UpstreamTlsContextFactory::with_pki_metadata`. The network profile determines
whether a provider supplies trust or only metadata. `refresh` performs discovery
off transport threads; `current` returns an immutable snapshot without file IO.
Pooled TCP adapters use `hyper_pool_connector_builder` and apply
`configure_ssl_metadata_for_peer` before each physical handshake. Both public
HTTP connector builders do this automatically. A frozen raw connector carrying
an old nonempty offer must be rebuilt to remove it; BoringSSL's public setters
cannot clear the offer. The native boundary rejects that stale configuration.

The bundle is generated from pinned public Chromium source, with its full BSD
notice in Credits. See [snapshot provenance and generator](../third_party/chromium-root-store/README.md).

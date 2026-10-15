# Root stores, local trust and Trust Anchor IDs

The [network profile](network-profiles.md#choose-certificate-trust) selects the
trust source and controls whether IDs are offered. The source supplies trust inputs;
all sources use the same [certificate verifier](chromium-verifier.md).

## Sources and component discovery

| Choice | Certificate verification and metadata |
| --- | --- |
| Public OS Root Store | Configured OS roots, including locally installed roots. Embedders retain explicitly configured trust. Bundled Chromium metadata supplies ID offers and untrusted chain-building helpers. |
| Bundled Chromium Root Store | Public TLS anchors from the shipped snapshot; no component discovery. |
| Installed Chrome Root Store (fall back to bundled) | Newer valid `crs.pb` from `%LocalAppData%\Google\Chrome\User Data\PKIMetadata\<component-version>`. |
| Installed WebView2 Root Store (fall back to Chromium bundled) | Newer valid `crs.pb` from `%LocalAppData%\Microsoft\Windows\SharedWebView\EBWebView\PKIMetadata\<component-version>`. |

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

## Trust Anchor ID offers

The Chromium 154 recipe offers certificate-based Trust Anchor IDs in the TLS
`trust_anchors` extension (0xca34), on TCP and QUIC. IDs are assigned relative
OIDs, not certificate hashes or signature-algorithm choices. Merkle Tree
Certificate identifiers are not advertised. The bundled root store and its
provenance are documented in the [snapshot guide](../third_party/chromium-root-store/README.md).

A valid installed component can contain roots without any supported Trust Anchor
IDs. In that case Transmog keeps those roots and omits the `trust_anchors`
extension; it does not borrow IDs from the Chromium bundle or send an empty
offer. The bundled fallback applies when no newer usable root store is found.
This follows [Chromium's nonempty-offer policy](https://github.com/chromium/chromium/blob/154.0.8037.99/net/ssl/ssl_config_service.cc)
and its [component ID replacement](https://github.com/chromium/chromium/blob/154.0.8037.99/chrome/browser/component_updater/pki_metadata_component_installer.cc).

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

All root sources use the same [BoringSSL PKI verifier](chromium-verifier.md),
including SAN-only hostname checks, metadata constraints, CT and revocation.
Additional TLS anchors can terminate a verified path; OS mode uses metadata
certificates only as untrusted helpers. Conflicting duplicate policies and
malformed version bounds prevent an update from replacing the last-good store.

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
SAZ retain these facts. Diagnostic metadata records available server IDs and
recovery status; the inspector shows OIDs alongside wire hex. Resumed sessions
identify the absence of a newly sent certificate. Missing evidence is unavailable.

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

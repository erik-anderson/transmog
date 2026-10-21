# Windows local certificate trust

This adapter follows Chromium's Windows local-trust selection: read-only
`CERT_STORE_PROV_SYSTEM_REGISTRY_W` stores in collections for ROOT, CA and
Disallowed from machine/user, group-policy and machine-enterprise locations.
TrustedPeople comes from machine locations only. It does not enumerate the
aggregate SYSTEM provider for trust grants or add AuthRoot public-program roots.
Distrust also includes CTLs in Disallowed stores and the full cached Windows
untrusted CTL in AuthRoot/AutoUpdate/DisallowedCertEncodedCtl. Frozen CTLs use
Crypt32 certificate-property hashing and exact subject-identifier lookup,
including Windows' signature-hash identifiers; no certificate DER inventory is
required for those entries. CTL expiry does not remove blocking records.
`LocalTrustMonitor::start_chromium` excludes all CTLs and retains Chromium's
user/enterprise certificate-entry scope, without reading the cached OS CTL.

ROOT and TrustedPeople eligibility uses `CertGetEnhancedKeyUsage` with flags 0,
including the intersection of certificate extensions and Windows properties.
CA certificates remain untrusted issuer material. Disallowed applies regardless
of usage. The TLS consumer must enforce the distinct certificate roles.

The worker registers `CertControlStore(NOTIFY_CHANGE)` on the collections with
a shared manual-reset event, resets and resynchronizes/rearms before reading,
and waits for either a notification or shutdown. Callers never enumerate stores.
A read-only registry watch covers CTL changes and newly created stores. A
notification invalidates the inventory before refresh; read failures never
return an old inventory as current. Owned immutable snapshots retain provenance
for connections that already started.

Tests create uniquely owned temporary registry stores outside Windows' system
certificate locations. They exercise real notifications, property changes,
removal and shutdown without modifying the computer's trusted roots.

References: [Chromium TrustStoreWin](https://github.com/chromium/chromium/blob/154.0.8037.99/net/cert/internal/trust_store_win.cc),
[CertControlStore](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-certcontrolstore).

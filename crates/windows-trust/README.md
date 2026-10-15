# Windows local certificate trust

This adapter follows Chromium's Windows local-trust selection: read-only
`CERT_STORE_PROV_SYSTEM_REGISTRY_W` stores in collections for ROOT, CA and
Disallowed from machine/user, group-policy and machine-enterprise locations.
TrustedPeople comes from machine locations only. It does not enumerate the
aggregate SYSTEM provider or AuthRoot public-root program.

ROOT and TrustedPeople eligibility uses `CertGetEnhancedKeyUsage` with flags 0,
including the intersection of certificate extensions and Windows properties.
CA certificates remain untrusted issuer material. Disallowed applies regardless
of usage. The TLS consumer must enforce the distinct certificate roles.

The worker registers `CertControlStore(NOTIFY_CHANGE)` on the collections with
a shared manual-reset event, resets and resynchronizes/rearms before reading,
and waits for either a notification or shutdown. Callers never enumerate stores.
A notification invalidates the inventory before refresh; read failures never
return an old inventory as current. Owned immutable snapshots retain provenance
for connections that already started.

Tests create uniquely owned temporary registry stores outside Windows' system
certificate locations. They exercise real notifications, property changes,
removal and shutdown without modifying the computer's trusted roots.

References: [Chromium TrustStoreWin](https://github.com/chromium/chromium/blob/154.0.8037.99/net/cert/internal/trust_store_win.cc),
[CertControlStore](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-certcontrolstore).

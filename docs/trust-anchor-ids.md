# Trust Anchor IDs and local Chrome metadata

Chromium 154 profile revision 3 offers the classical certificate-backed IDs in
Chrome's `trust_anchors` TLS extension (0xca34) on TCP and QUIC. The bundled
root-store version 41 reproduces all 28 IDs observed in Chromium 154.0.8037.99.
Identifiers are assigned relative OIDs, not hashes derived from certificates.
Chromium's raw-byte sorting and TLS-trusted additional-certificate selection are
preserved. Merkle Tree Certificate (MTC) identifiers are not advertised.

On Windows, a background worker checks
`%LocalAppData%\Google\Chrome\User Data\PKIMetadata\<component-version>\crs.pb`
at startup and once per minute. Numeric component folders are scanned with
bounded file/count limits. Freshness is the protobuf's `version_major`, rather
than modification time or component directory name. Only a strictly newer valid
root store replaces the last-good snapshot. Missing, partial, malformed and old
files retain the last-good copy, initially the bundle. No Chrome installation is
required. Transmog neither changes Chrome's files nor downloads its updater package.

Fresh physical connections use the latest discovered snapshot. Established and
pooled connections keep their original snapshot until retired normally.
Discovery does not interrupt requests or restart the proxy. The measured browser
profile stays Chromium 154; effective PKI source/version/hash is recorded separately.

**Customize → More handshake options** allows disabling Trust Anchor IDs or
specifying an optional full path to a `crs.pb`. An explicit file replaces the
bundled baseline; a newer discovered local Chrome root store takes precedence.
Save only preserves a definition. Save and use loads the enabled file before
activating fresh proxy/Composer pools, leaving the previous profile active if
loading fails. Disabled offers do not load the optional file. Old custom profiles
and historical captures retain the absent Trust Anchor IDs setting as disabled.
If a previously enabled file disappears, the app still opens for inspection and
Settings repair. Composer sends and proxy startup fail with a repair explanation
until a usable recipe is selected; they do not silently substitute another offer.
After repairing the file, Customize → Save and use reloads it even when the
profile settings are unchanged. Save only keeps the existing transport snapshot.

Embedders can load `transmog_tls::trust_anchors::PkiMetadata::from_file`, wrap
it in `PkiMetadataProvider::new`, and use `UpstreamTlsContextFactory::with_pki_metadata`.
Providers expose `refresh` for deterministic/off-thread discovery and `current`
for an immutable validated snapshot without file IO.

## Verification and recovery

Metadata supplies advertisement IDs and **untrusted** public issuer certificates.
These let a server omit an advertised intermediate while Transmog builds its
path to configured roots. Loading never installs roots, grants trust to additional
certificates, or applies Chrome's constraints/EV policy. A server's matching marker
does not bypass ordinary hostname, purpose, validity or chain verification.
Local files are not claimed to have a verified Google CRX signature.

After a rejected certificate chain, available server IDs can trigger one retry
of that endpoint before any HTTP data is sent. The retry requests only catalog
IDs whose associated CA verifies through configured trust, in server preference
order. Unknown IDs, untrusted CAs, an unchanged offer, or a second failure do not
trigger further recovery. Both physical attempts remain in connection evidence.

## Recorded evidence

Traffic → Details → Connection reports whether the server indicated a matching
chain. Original connection settings retain metadata source, major version,
SHA-256 and requested IDs. Diagnostic metadata retains server available IDs and
recovery status. Lists display human-readable OIDs alongside wire hex. Resumed
sessions explicitly indicate there was no new certificate. Native captures and
SAZ preserve these bounded public facts. Lists beyond the display limit are
marked; full server vectors remain available to live bounded recovery. Historical
missing evidence remains unavailable.

The bundle is generated from pinned public Chromium source, with its full BSD
notice in Credits. See [snapshot provenance and generator](../third_party/chromium-root-store/README.md).

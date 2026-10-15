# Client certificates

Proxy and Composer transports support request-driven certificates on HTTP/1.1,
HTTP/2 and HTTP/3, with connection ownership, pool isolation and a bounded pending
decision broker, advanced desktop host rules and a nonmodal selection interface.
Unconfigured hosts continue without a certificate.

## Desktop settings and selection

Open Settings → Preferences → Advanced network settings → Client certificates.
The global choice is No certificate or Prompt. Exact host/HTTPS-port rules add
No certificate, Prompt, or Automatic with an exact public identity or explicit
subject/issuer renewal attributes. The first enabled match wins, with drag handles
and Move up/down controls. Edits apply only after Save certificate settings;
missing, incompatible or ambiguous automatic matches continue anonymously without
trying another rule. Separate persistence preserves these rules during unrelated
settings, layout and window writes. Corrupt latest-state recovery retains rule
drafts disabled and restores No certificate instead of resurrecting authorization.
Recovery writes this safe generation before quarantining corrupt state. If that
write fails, the corrupt generations remain as evidence for subsequent launches.

Pending physical handshakes appear in the persistent connection indicator.
Its nonmodal panel shows public eligible choices and actual server constraints.
An answer applies only to the displayed live connections. Expired entries retain
their inspected context and disable actions; late answers are rejected. Viewer
Composer requests use separate caller scopes and refer selection to the main
window, whose command boundary enforces configuration and decision permissions.
Closing a viewer cancels its admitted Composer work without revoking other callers.
Closing the main window likewise preserves open viewers' pending choices and
authorization generation; final controller teardown cancels the shared broker.
Composer preserves received TLS/client-certificate evidence in its response
certificate viewer, and includes recorded certificate state in send-failure feedback.
With only HTTP/3 allowed, Auto Composer uses its verified QUIC transport.

Use `PolicyClientAuthenticationResolver` with
`WindowsCurrentUserCertificateSource` and a connection-owned
`ClientAuthenticationAttempt`. Unconfigured origins continue without a
certificate. Handshake selection queries the source after an actual verified CertificateRequest;
there is no anonymous probe or signing challenge during inventory. Explicit
settings inventory and monitoring are separate from TLS authorization. Keep the
inventory monitor only while a controller needs identity-change invalidation.
`WindowsClientCertificateController` owns that monitor and the decision broker
for one application run. Keep the controller alive while installing its shared
resolver with `ProxyComponents::with_client_certificate_resolver` and
`SystemReplayExecutor::with_client_certificate_resolver`. Composer requires its
own immutable caller/run identifier. `with_tls_factory` also supports embedding
trust without modifying operating-system roots. Dropping the controller closes
pending decisions and revokes its previous authorization generation.

## Production connections and decisions

The actual verified TLS handshake creates each attempt. There is no probe or
anonymous retry to discover whether a certificate changes the response. A
missing/ineligible pinned identity continues the same handshake anonymously,
without falling through to another rule or identity. A certificate decision
failure after a QUIC CertificateRequest does not trigger TCP fallback. Hyper's
automatic cancelled-request retries are disabled when a controller is installed.

Reusable connections are scoped by immutable caller, logical origin, trust and
authorization generation. Proxy authorization also includes its unique downstream
connection; best-effort process attribution and source IP never establish a shared
certificate grant between independent connections. Certificate isolation applies
even when the general outbound-isolation setting is disabled. HTTP/3 authorizes the logical
HTTPS port, independently of the Alt-Svc UDP port. Pinned rules reuse a connection
only after confirming the identity actually installed. An anonymous connection
made when a pinned certificate was expected never enters its authenticated pool.
Attribute-only renewal selectors and prompt decisions remain connection scoped;
different server constraints can select a different identity. Policy and inventory
changes replace Hyper pools and retire stale QUIC entries before new admissions.
Already admitted exchanges can finish. Prompt and automatic selection disable
resumption so authorization follows a fresh CertificateRequest. Explicit
No certificate policy permits anonymous resumption; see [negotiation](chromium-negotiation.md#resumption).

The broker exposes public identities and actual server constraints without key
material. Identical caller/origin/generation/constraints/choice sets coalesce into
one row with a connection count. Each answer applies only to those live physical
handshakes and does not create a saved rule or run grant. Bounds are 64 groups,
64 waiters per group, 1,024 choices per group and 32 MiB of retained public paths.
Dropped owners unregister; shutdown, deadlines and authorization changes cancel
pending work; late answers are rejected. TCP watches peer closure while paused,
preserving at most 1 MiB of encrypted handshake bytes for its TLS reader. QUIC
continues receiving packets and applying its actual timers while selection waits.
The QUIC registry is released before establishment, and its physical connection
bound includes ephemeral prompt handshakes. Composer bounds execution to ten
seconds and observes cancellation for both buffered and file-streamed requests.

## Windows source

The supported store is CurrentUser/My. The supported provider is Microsoft
Software Key Storage Provider, with RSA and ECDSA P-256/P-384/P-521. Hardware,
third-party providers, machine keys, legacy CAPI and PFX import are unsupported.

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
cycle. Inventory reads have a five-second deadline. Production pool owners act on
its authorization generation; using the source alone does not retire a pool.

Inventory, candidate filtering and final eligibility jobs are owned by their
awaiting futures. Dropping an owner aborts queued work and signals running
inventory/filter batches to stop between lookups. Native calls that already
started may finish. Cancellation and authorization are rechecked before final
eligibility; a parked decision uses a weak connection reference so it cannot
retain an abandoned owner.

## Chromium comparison

The reference implementations are Chromium's [Windows platform key implementation](https://raw.githubusercontent.com/chromium/chromium/main/net/ssl/ssl_platform_key_win.cc),
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

## Verification

Run the portable environment check before native builds, then:

```text
python scripts/dev-env.py --check
python scripts/dev-env.py --run cargo test --locked -p transmog-core -p transmog-tls -p transmog-tls-native -p transmog-client-certificates -p transmog-network
python scripts/dev-env.py --run cargo test --locked -p transmog-http -p transmog-h3 -p transmog-runtime -p transmog-app --test client_authentication
python scripts/dev-env.py --run cargo test --locked -p transmog-tls --test windows_client_certificates -- --ignored --nocapture
python scripts/dev-env.py --run cargo test --locked -p transmog-runtime -p transmog-app --test client_authentication -- --ignored --nocapture
# PowerShell 7, Windows/WebView2; builds an opt-in test binary in the development profile.
./scripts/test-windows-client-certificate-ui.ps1
```

The ignored Windows tests create owned, non-exportable keys in CurrentUser/My.
They verify real TLS signatures, authenticated and anonymous responses, chain
selection, key deletion and monitor invalidation through H1/H2/H3 proxy and
Composer paths. They remove their keys and certificates on normal completion
and unwinding; they do not install root trust. The Windows release gate runs them
before signing credentials are available.

Native desktop acceptance uses an isolated profile and local H1/H2/H3 peers to
exercise prompt selection, anonymous continuation, cancellation, automatic rules,
key deletion and restart persistence. Fixture commands and trust require the
`desktop-test-fixtures` feature and isolated-profile flag; ordinary packages
include neither. Browser checks cover rule ordering, draft/save separation,
errors, keyboard access and compact layouts under Trusted Types.

Portable integration tests cover caller and origin isolation, pool generations,
resumption suppression, prompt coalescing, deadlines, owner cancellation,
connection limits and exact wire/capture chains. Worker tests distinguish queued
cancellation from native calls already running. Hardware and third-party key
providers require separate implementation and acceptance coverage.

# Certificate and trust model

Upstream verification and downstream signing are deliberately separate.

Upstream root bytes are enumerated once from the operating system, parsed by
the Chromium PKI parser, and placed in an immutable `TrustSnapshot`. Each reload creates a new
generation, context factory, and connection-pool generation. Hyper and quiche
receive contexts from the same factory. The operating system does not perform
path building. One verifier based on Chromium's BoringSSL PKI path builder
handles TCP, QUIC and Trust Anchor ID recovery for every root-source choice.
The native bridge compiles the unmodified PKI target against the existing
BoringSSL crypto instance; the legacy X.509 verifier and hand-written parallel
certificate checks are removed from upstream verification. The adapters' TLS
method still pre-parses wire certificates; moving to buffered TLS remains a
separate integration gap described in the verifier contract.

The shared hostname check follows Chromium 154: SANs are mandatory; DNS
wildcards match only a whole first label and respect ICANN registry boundaries;
IP literals require the exact IP SAN. The pinned Chromium Public Suffix List
and its maintenance script are described in [the verifier contract](chromium-verifier.md).
Expected peer names are independent of SNI and capture collection. Embedders
using raw TCP SSL objects should call `configure_ssl_metadata_for_peer`,
particularly for IP origins or when disabling SNI.

Live reload is exposed through `ProxyServer::control().reload_trust(...)`. A
reload must use a strictly increasing generation number. The runtime constructs
new Hyper HTTP/1.1/HTTP/2 and quiche HTTP/3 clients before atomically publishing
the generation; existing exchanges retain their original clients until they
finish, so connection pools never mix trust generations.

## Post-quantum TLS algorithms

Transmog follows Chromium's supported post-quantum TLS algorithms without
pinning a Chromium release. The shared
[algorithm policy](../crates/proxy-tls/src/algorithms.rs) applies to upstream
HTTP/1.1, HTTP/2 and HTTP/3 and to downstream interception TLS. Chromium's
[group policy](https://github.com/chromium/chromium/blob/main/net/ssl/ssl_config_service.cc)
and [signature preferences](https://github.com/chromium/chromium/blob/main/net/socket/ssl_client_socket_impl.cc)
are the upstream references when that set changes.

| Purpose | Supported post-quantum algorithms | Compatibility |
| --- | --- | --- |
| TLS 1.3 key exchange | `X25519MLKEM768` (`0x11ec`), combining X25519 and ML-KEM-768 | Preferred ahead of X25519, P-256 and P-384. The initial ClientHello carries hybrid and X25519 shares; classical peers negotiate a shared classical group. |
| TLS 1.3 authentication | ML-DSA-44, ML-DSA-65 and ML-DSA-87 (`0x0904`–`0x0906`) | Upstream verification advertises all three followed by Chromium's classical signature preferences, excluding RSA/SHA-1. Downstream credential selection uses a compatible ML-DSA key or the ECDSA P-256 fallback. |

Each TLS leg negotiates independently. An origin's algorithm does not force the
client-facing leg to use the same algorithm, and downstream support does not
change upstream certificate verification. TLS 1.2 retains classical behavior.

Upstream TCP and QUIC ClientHellos prepend a reserved GREASE signature scheme
to Chromium's real verification preferences. BoringSSL selects the placeholder
for each handshake; peers ignore it and negotiate an existing real algorithm.
Signature-algorithm GREASE is a separate native setting from ordinary GREASE,
and does not change the server's CertificateRequest algorithm list.

The same upstream contexts enable ordinary GREASE in cipher suites, supported
groups, key shares, versions and reserved extensions, and randomize extension
order for each handshake. BoringSSL keeps the pre-shared-key extension last
when resuming a session and preserves the permutation across a retry. These
settings leave the real cipher, group and signature preferences unchanged.

Upstream clients request stapled OCSP responses and signed certificate
timestamps, matching Chromium's request extensions. These requests do not
introduce revocation or Certificate Transparency enforcement; certificate
acceptance continues to use the configured trust snapshot and identity checks.
Desktop network profiles select the effective trust source. The default WebView2
profile discovers its installed public roots, falling back to bundled Chromium;
custom profiles can select OS, bundled Chromium or installed Chrome roots.
Both built-ins include monitored Windows local trust, using registry-backed
ROOT, CA, machine TrustedPeople and Disallowed collections. The OS public-root
program is excluded. CA entries only help build chains; self-signed TrustedPeople
entries authorize only the exact server certificate, and explicit distrust wins.
Custom profiles can disable the overlay for public-only debugging.
Chrome's policies around this PKI core are not all implemented yet. The exact
remaining differences are listed in [the verifier contract](chromium-verifier.md);
root metadata constraints are evaluated on candidate paths. The same verifier
enforces CT and CRLSet/stapled-OCSP policies. Installed Chrome and WebView2 profiles
source their own component lists with identical freshness rules; other root
profiles use the bundled baseline. The connection-policy disclosure records
the selected datasets and actual checks, including stale or disabled CT.

Upstream TLS 1.3 clients also advertise Brotli certificate decompression
(`compress_certificate`, algorithm `0x0002`). Decoding uses BoringSSL's existing
certificate size limit and exact output buffer, rejects malformed, truncated
or trailing data, and passes the decoded certificate through the same trust
and identity verification. Origins may still send an uncompressed certificate.
Client certificates and downstream certificates remain uncompressed.

The shared upstream contexts enable ECH GREASE before each TCP or QUIC
handshake. BoringSSL sends a fresh dummy `encrypted_client_hello` extension
when TLS 1.3 is offered, including when an origin negotiates TLS 1.2. This
exercises extension handling and matches Chromium's ECH GREASE behavior; it
does not encrypt the server name or implement real ECH configuration discovery.

The default TLS 1.2 cipher list uses Chromium's expression
`ALL:!aPSK:!ECDSA+SHA1:!3DES`, including its CBC and static-RSA compatibility
suites after the ECDHE/AEAD suites. RSA/SHA-1 server handshake signatures are
excluded, matching Chromium's separate signature policy. Matching Chrome's
offer is the product default; advanced offer overrides may be added later.
Embedding applications can still supply an explicit cipher list.

The default interception resolver caches in-memory ML-DSA-44/65/87 and P-256
leaves per validated host. Capacity counts hosts, with at most four key variants
per host. Expired variants are removed before reuse; host eviction removes all
of its variants. BoringSSL
selects a credential from the client's actual offered signature schemes.
Existing custom resolvers may continue returning one certificate through
`resolve`; `resolve_credentials` allows ordered alternatives for one identity.
The standalone `ca issue` command continues to issue P-256 leaves.

Upstream client authentication accepts an ML-DSA credential only for TLS 1.3
when its exact key variant matches a requested signature scheme. It retains the
existing issuer, validity, identity, trust-generation and selection checks.
Native credential registration and callback access stay inside
[`transmog-tls-native`](../crates/tls-native/src/lib.rs), using the same BoringSSL
instance as the rest of the TLS stack.

The interception root remains classical. An ML-DSA leaf can sign the TLS
handshake while its issuer signs the certificate with ECDSA. Hybrid exchange
and an ML-DSA handshake therefore do not establish a fully post-quantum trust
chain. Chromium's [post-quantum authentication roadmap](https://www.chromium.org/Home/chromium-security/post-quantum-auth-roadmap/)
covers additional PKI work; PQ root and Merkle Tree Certificate migration are
outside the supported model.

## Recorded connection evidence

TLS evidence belongs to a physical connection and its client or upstream leg.
It also records whether HTTP/2 or HTTP/3 ALPS was negotiated and the exact server value.
The Connection inspector decodes SETTINGS and read-only `ACCEPT_CH` preferences,
retaining original bytes in Hex. Missing ALPS evidence is unavailable, rather
than inferred from the configured offer. See [HTTP/2 ALPS](http2-alps.md).
Successful handshakes record the negotiated group and ID, key-exchange type,
local and peer handshake signature schemes when available, session resumption,
HelloRetryRequest, protocol and cipher. Certificate inspection shows the
credential actually selected by BoringSSL, including its ML-DSA key variant.
Configured preferences alone never establish a negotiated algorithm.

On TLS 1.3 resumption, the group describes the new handshake. TLS 1.2 resumption
performs no fresh key exchange; its retained group is labeled **Original session
group** and **Original session group ID**, with **Not performed (TLS 1.2 session
resumed)** as the key-exchange status. A retained peer signature
is labeled **Original peer handshake signature**; the current peer signature is
**Not sent (session resumed)**. A chain restored from the TLS session is labeled
`resumed-session` rather than presented on the new connection. Reused HTTP
requests retain the evidence of their shared physical connection.

Native captures preserve these public facts through compression, encryption and
random-access loading without a schema change. Older or imported captures with
missing algorithm evidence show **Unavailable**, with no inference from the
certificate key or cipher name. See [testing](testing.md#post-quantum-tls) and
the [handshake benchmark](performance.md#post-quantum-tls-handshakes).

Both SAZ profiles retain each leg's exchange group, handshake signatures and
resumption facts in conventional SessionFlag metadata and readable connection
summaries. Transmog restores those recorded facts on import. Extended SAZ and
TMCap also preserve the complete public certificate paths and richer connection
evidence; compatibility SAZ does not include the public DER definitions.
XML-incompatible text is rendered as marked Unicode escapes in SAZ metadata and
generated negotiation summaries. Import restores the exact original values,
including NUL, rather than interpreting unmarked literal escape text.

The inspector defaults to the latest successful upstream connection, with the
latest upstream attempt as fallback when none succeeded. Selector labels include
the role, outcome and connection identity so failed attempts remain distinct.
When the same certificate fingerprint appears more than once, an available
complete certificate takes precedence over an earlier fingerprint-only reference.
Its connection view starts with a recorded protocol/group/cipher summary and an
ordered negotiation and verification list. Connection policy and trust provenance,
and additional imported metadata, have separate disclosures. Conflicting recorded
values produce a warning and retain the alternative in the metadata disclosure.
The inspector shows invisible controls, including NUL, as Unicode escapes, with
a note when presentation differs from the recorded text.
The original values remain unchanged in TMCap and SAZ exports.

## Interception root lifecycle

The proxy CA is generated or loaded independently and is never added to the
upstream store. Installing it is an explicit operator action. Test installers
use the current-user store, record the exact SHA-256 thumbprint, and remove only
that certificate. The local live-browser harness may retain its explicitly
installed test CA across runs to avoid repeated Windows consent dialogs; every
run verifies it read-only, and the dedicated teardown removes the exact root.

Routine telemetry records fingerprints and verification results, not private
keys or full subject names. There is no global accept-invalid-certificate mode.

Durable interception CA private-key files use a versioned, OS-protected format.
Windows encrypts the key with current-user DPAPI, with UI disabled and without
machine-wide decryption. macOS and Linux encrypt with AES-256-GCM using an
independent random wrapping key for each file, held in the user's macOS Keychain
or Linux Secret Service. The file contains ciphertext, a nonce and a credential
identifier; it contains no wrapping secret. Copying the key file alone to another
user or computer therefore does not give the recipient the signing key. This
mitigates accidental sharing, not access by software already running as that user.

New desktop and CLI roots are protected before any key bytes reach disk. The
desktop migrates its existing PEM key at startup and verifies protection before
starting the proxy. CLI persistent roots migrate when reused; `serve`, `ca issue`
and `ca protect --ca-cert ca.pem --ca-key ca.key` also validate and migrate legacy
keys. Migration atomically replaces only the private-key file, after checking
that it matches the public certificate, preserving the certificate fingerprint
and existing trust. Older copies of a plaintext key remain sensitive; migration
does not erase backups or guarantee physical erasure of previous disk blocks.

Transmog reads protected keys directly into memory for signing. Protected key
files are not PEM inputs for other tools. Credential-store failures, unsupported
formats, corruption or missing credentials fail without plaintext fallback or
silent CA replacement. Linux requires an available, unlocked Secret Service;
macOS requires access to the user's Keychain. A failed protection step retains an
existing key for retry. CLI root cleanup deletes its external wrapping secret
along with the key file; ephemeral CLI roots continue to keep keys only in memory.

When Windows DPAPI cannot decrypt the saved key, the desktop explains that the
key is unlikely to be recoverable and offers **Reset certificate and set up
again**. This explicit flow removes only the recorded root from current-user
trust, deletes its files, then creates a protected replacement and asks Windows
to trust it. Canceling or failure to remove the old root leaves its material and
ownership record intact; a failed trust prompt leaves the replacement available
for another setup attempt.

For private origins and hermetic tests, `transmog-cli serve
--upstream-ca-cert roots.pem` augments the OS-enumerated roots with one or more
PEM certificates before constructing the immutable BoringSSL snapshot. It does
not replace public roots, mutate an OS store, or enable invalid-certificate
acceptance; chain, identity, validity, and usage verification are unchanged for
Hyper and quiche. `transmog-cli ca issue --ca-cert ca.pem --ca-key ca.key
--identity HOST_OR_IP --cert leaf.pem --key leaf.key` issues a short-lived leaf
with the requested DNS or IP identity for controlled origins. The resulting
private key has the same handling requirements as any server key and should not
be checked in.

The guided CLI `record` command installs its public interception root with
explicit consent and removes it after an ephemeral run. Its fresh private key
stays in memory; `--persistent-root` opts into a protected key and reused root.
Windows installation targets current-user trust, macOS uses the login keychain,
and supported Linux trust stores use their normal update tools through `sudo`.
`--no-install-root` leaves trust setup manual. Cleanup retains exact public
identities when removal cannot be completed. See the
[support capture guide](cli-support-capture.md#certificates-and-interrupted-runs).

The lower-level `serve` and `ca` commands leave trust installation and removal
to the operator. With the release binary, generate a CA without repository
scripts:

```powershell
.\transmog-cli.exe ca generate --cert .\transmog-ca.pem --key .\transmog-ca.key
```

Install only the public certificate in the client's trust store, retaining the
printed `CA_SHA256` to identify it for removal. These operator-created CAs are
not owned by `roots cleanup`. See [manual proxy setup](cli.md#run-a-manually-configured-proxy).

For developers working in this checkout, the Windows scripts provide an
explicit, reversible test workflow with that generated public certificate:

```powershell
$install = pwsh ./scripts/install-ca-user.ps1 -CertificatePath ./transmog-ca.pem
# Save the CA_SHA256 value emitted above.
pwsh ./scripts/uninstall-ca-user.ps1 -Sha256 <saved-sha256>
```

The test installation script targets only `Cert:\CurrentUser\Root`. It requires
a CA Basic Constraints extension and verifies exactly one SHA-256 match after
installation. Uninstallation enumerates by that SHA-256 digest, refuses an
ambiguous match, removes only that certificate, and verifies that no match
remains. It never uses a subject-name wildcard and never opens the machine
store.

Installing an interception CA gives this process the ability to read and
modify TLS traffic for clients that trust it. Keep the OS-protected private key
accessible only to the owning user, do not log or transmit it, and
remove ordinary one-off roots immediately after use. For the durable
live-browser root, run `scripts/remove-live-test-ca.ps1` as soon as repeated
live testing is complete. Certificate-pinned applications are unsupported;
Transmog does not bypass pinning.

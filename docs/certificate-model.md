# Certificate and trust model

Upstream verification and downstream signing are deliberately separate.

Upstream root bytes are enumerated once from the operating system, parsed by
BoringSSL, and placed in an immutable `TrustSnapshot`. Each reload creates a new
generation, context factory, and connection-pool generation. Hyper and quiche
receive contexts from the same factory. The operating system does not perform
path building; v1 semantics are BoringSSL path and hostname verification over a
snapshot of OS-enumerated roots.

The shared verifier also handles literal-IP origins with BoringSSL's dedicated
IP-SAN matcher. This is intentionally narrow: only BoringSSL's hostname-mismatch
result for a syntactically valid IP is reconsidered, and it succeeds only when
the leaf has that exact IP SAN. Chain, validity, usage, and every other
verification failure remain fail-closed. DNS names continue through the normal
BoringSSL hostname verifier.

Live reload is exposed through `ProxyServer::control().reload_trust(...)`. A
reload must use a strictly increasing generation number. The runtime constructs
new Hyper HTTP/1.1/HTTP/2 and quiche HTTP/3 clients before atomically publishing
the generation; existing exchanges retain their original clients until they
finish, so connection pools never mix trust generations.

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

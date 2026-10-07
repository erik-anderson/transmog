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

The CLI does not modify an operating-system trust store. On Windows, the
checked-in scripts provide the explicit, reversible test workflow:

```powershell
pwsh ./scripts/new-proxy-ca.ps1 -CertificatePath ./transmog-ca.pem -PrivateKeyPath ./transmog-ca.key
$install = pwsh ./scripts/install-ca-user.ps1 -CertificatePath ./transmog-ca.pem
# Save the CA_SHA256 value emitted above.
pwsh ./scripts/uninstall-ca-user.ps1 -Sha256 <saved-sha256>
```

Installation targets only `Cert:\CurrentUser\Root`. The installer requires a
CA Basic Constraints extension and verifies exactly one SHA-256 match after
installation. Uninstallation enumerates by that SHA-256 digest, refuses an
ambiguous match, removes only that certificate, and verifies that no match
remains. It never uses a subject-name wildcard and never opens the machine
store.

Installing an interception CA gives this process the ability to read and
modify TLS traffic for clients that trust it. Keep the private key under the
user-only ACL applied by `new-proxy-ca.ps1`, do not log or transmit it, and
remove ordinary one-off roots immediately after use. For the durable
live-browser root, run `scripts/remove-live-test-ca.ps1` as soon as repeated
live testing is complete. Certificate-pinned applications are unsupported;
Transmog does not bypass pinning.

# Record a trace for support

Download the separate signed `transmog-cli.exe` from the release assets. It is
not installed with the desktop app. Download it from the project's official
release page. Right-click **transmog-cli.exe → Properties → Digital Signatures**,
open the signature's details and confirm Windows says it is valid. Compare the
signer with the publisher named in that release's notes. Resolve a missing or
invalid signature, or a different publisher, with your support contact first.

For a console check:

```powershell
$signature = Get-AuthenticodeSignature -LiteralPath .\transmog-cli.exe
$signature.Status
$signature.SignerCertificate.GetNameInfo(
  [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
```

The status should be **Valid**, with the release's expected publisher. If you
also downloaded the release checksums, compare SHA-256 using
`Get-FileHash -Algorithm SHA256 .\transmog-cli.exe`. A matching checksum checks
the downloaded bytes; the trusted signature identifies the publisher.

Save it in a folder you can write to, open PowerShell in that folder, and run:

```powershell
.\transmog-cli.exe record --output .\support-trace.tmcap
```

1. Answer **yes** when asked to install the public HTTPS interception root.
   Approve the Windows certificate prompt. If setup fails or you dismiss the
   prompt, the CLI offers another attempt in the same run.
2. When the console says **Recording**, reproduce the problem in the affected
   application. Windows proxy settings are configured automatically. Apps with
   their own proxy configuration need the address printed in the console; some
   apps also use a separate certificate store or certificate pinning.
3. Press **Ctrl+C once** in that console. Transmog restores the previous proxy
   settings, finishes requests already in progress, and saves the trace. A
   long-lived connection can keep the capture finishing; press **Ctrl+C again**
   to terminate unfinished work and save the available evidence instead.
4. Approve the certificate removal prompt if one appears. Wait for **Trace
   saved**, then open `support-trace.tmcap` through Transmog's **Import…** or
   **Files → Open in a separate viewer…**. Review the capture and share that one
   chunk-compressed file with your support contact.

The default captures complete headers, including cookies and credentials, and
retains up to 25 MB (25,000,000 bytes) per request body. Response bodies and
observed byte counts remain available within the overall trace file budget.
Large requests continue forwarding when their capture prefix reaches the cap. To redact `Authorization`, `Proxy-Authorization`, `Cookie` and
`Set-Cookie` values, add `--redact`. This choice persists for subsequent CLI
captures. Use `--retain-sensitive` to change it back. Bodies can still contain
private information, so review the trace before sharing.

To remove the per-request capture cap (trace output has no file-size ceiling):

```powershell
.\transmog-cli.exe record --unlimited-request-bodies --output .\large-trace.tmcap.gz
```

This preference persists for subsequent CLI runs. Use --request-body-limit
25000000 to restore the default, or supply another positive byte count. The
console prints the active limit before recording. Overall file limits and
separate bounded body-processing limits still apply.

Existing files are never overwritten. Choose another filename for the next
capture. TMCap compresses each record as it arrives and lets the desktop read body
chunks on demand. An optional `.tmcap.gz` adds an outer wrapper at stop; if that
wrapper fails, the chunk-compressed `.tmcap` remains available. Use an output ending in `.tmcap` when
you want an uncompressed trace.

## Certificates and interrupted runs

The default root is ephemeral: its private key stays in memory and is never
written to disk. Public recovery records include run identity, certificate validity,
trust-store scope, lifecycle state, key-storage mode and cleanup attempts/outcome.
They contain no private-key bytes. The CLI removes its trusted public certificate after the
capture, including after setup or recording failures. Public certificate and
ownership records live separately from the desktop in
`%LOCALAPPDATA%\Transmog-cli`. Windows proxy recovery also has its own CLI journal.

If the process is killed or you dismiss a removal prompt, the next run attempts
to remove the old root and restore interrupted proxy settings. Failed removal
keeps the exact identities needed for later cleanup. It does not require an old
ephemeral key: the next capture generates a fresh root and keeps its new key in
memory. To retry cleanup directly:

```powershell
.\transmog-cli.exe roots cleanup
```

For repeated captures on your own test machine, explicitly opt into persistence:

```powershell
.\transmog-cli.exe record --persistent-root --output .\next-trace.tmcap.gz
```

This retains a protected private key and reuses a valid CLI root on subsequent
`--persistent-root` runs. A root expiring within seven days, not yet valid, or
missing/mismatched key material is retired and replaced. Its key is removed;
its public identity stays available until OS cleanup succeeds. Ordinary
ephemeral runs still use a fresh root. Older pending/retired roots are retried at
startup and completion without immediately repeating a canceled prompt for the
root used by the current capture. To
remove persistent roots and their keys too:

```powershell
.\transmog-cli.exe roots cleanup --include-persistent
```

The CLI state directory is restricted to the current user before private
material is created. Do not share it with a support contact.

## Recover an interrupted capture

If the console or computer closed unexpectedly, keep the leftover native
`support-trace.tmcap` beside the intended `.tmcap.gz` output. A gzip file left
mid-compression may be incomplete; the native file is kept until compression
finishes successfully. Restore CLI-owned proxy/certificate state first:

```powershell
.\transmog-cli.exe roots cleanup
```

This restores an interrupted Windows proxy journal before certificate prompts.
Persistent roots require `--include-persistent` to remove them. If cleanup is
canceled, public recovery metadata stays available for another attempt.

In Transmog, use **Import…** to open the leftover `.tmcap`. The viewer recovers
the valid prefix without changing the original and explains incomplete evidence
in **Trace metadata**. Use **Save trace…** and choose a new filename such as
`recovered-trace.tmcap`; each chunk is already compressed. Choose password
encryption when the recovered copy also needs protection. Review it, then
share the recovered copy. Missing bytes cannot be reconstructed.

For support staff who prefer console recovery:

```powershell
.\transmog-cli.exe capture inspect --input .\support-trace.tmcap
.\transmog-cli.exe capture seal --input .\support-trace.tmcap --output .\recovered-trace.tmcap
```

Run `seal` only when inspection reports an unsealed capture. It writes a new
file from complete records and omits an interrupted tail. Already sealed native
files can be imported directly and saved as a compressed copy in the desktop.

## Other platforms and manual setup

Run `transmog-cli record --output support-trace.tmcap` in a terminal. Configure
the affected application's HTTP and HTTPS proxy with the printed address, then
remove that configuration after stopping. Automatic host proxy configuration
currently applies to Windows; `--no-system-proxy` keeps Windows setup manual too.

On macOS, root installation uses the current user's login keychain and its OS
consent flow; cleanup specifies the exact SHA-256 identity. On Linux, supported
Debian/Ubuntu and Fedora-family trust stores use their normal certificate update
tools through `sudo`. This affects the system trust store and may require your
password. Apps that use another trust store need their own setup. See the
[Ubuntu trust-store guide](https://documentation.ubuntu.com/server/how-to/security/install-a-root-ca-certificate-in-the-trust-store/index.html)
and [Apple's security tool reference](https://github.com/apple-oss-distributions/Security/blob/main/SecurityTool/macOS/security.1).

Use `--no-install-root` for manual certificate setup; the public root path is
printed. In a noninteractive console, specify `--install-root` to explicitly
authorize the OS workflow or `--no-install-root`. OS consent is still required
where applicable. A remote listener requires `--allow-remote`; on Windows also
use `--no-system-proxy` and configure that device with the listener address.

The lower-level `serve`, `ca` and `capture` commands remain available for existing
operator workflows. `transmog-cli help` lists their options.

To include network configuration for a support investigation, add
`--include-network-context`. The trace records `ipconfig /all` on Windows,
interface/DNS/route context on macOS, or `ip` and `resolvectl` output on Linux.
This is optional because adapter addresses, DNS configuration and computer names
may be private. Collection is bounded and failures remain in Trace metadata.
Open the saved trace and use **Trace metadata** before sharing.

## Password protection and circular recording

Use `record --encrypt --output support-trace.tmcap` to enable AES-256 encryption.
The console masks the password and asks you to confirm it before certificate
setup. Keep the password to reopen the file; Transmog cannot recover it. Share
the password separately from the trace. Passwords are never saved in CLI
preferences or certificate recovery metadata.

For unattended captures, add `--encrypt --password-file <protected-file>`. The
UTF-8 file is read into memory (up to 4096 bytes; a final newline is ignored).
Protect that file yourself; Transmog does not delete or remember it. Actual
password strings are never accepted as command-line options.

Ordinary recording streams every exchange to the trace. For just the latest
retained traffic, use `--circular-buffer auto`: the limit is half installed RAM
and retained encoded frames stay in memory until Ctrl+C saves them. An abrupt
process termination loses an unsaved memory buffer. Specify a custom limit such
as `--circular-buffer 512MiB`, `2GB`, or a positive byte count (at least 1 MiB).
Limits above half installed RAM use disk. `--circular-buffer unlimited` has no
buffer maximum and uses disk. Saved traces have no fixed file-size ceiling. Disk circular caches hold compressed frames and, for encrypted runs,
ciphertext. Completed older exchanges are evicted first; if active traffic alone
exceeds the quota, older active evidence can be dropped without interrupting
forwarding. The saved trace’s metadata records eviction counts.

`capture inspect` and `capture validate` ask for a password when needed, or
accept `--password-file <protected-file>` in an unattended run. They also accept
`.tmcap.gz`. `capture seal` preserves the input’s encryption in the recovered
copy. `capture export --format saz --encrypt` writes AES-256 encrypted SAZ; use
`--source-password-file` for encrypted native input and `--password-file` for
the encrypted output. JSONL intentionally has no encryption option.

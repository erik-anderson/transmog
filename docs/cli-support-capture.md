# Record a trace for support

Download the separate signed `transmog-cli.exe` from the release assets. It is
not installed with the desktop app. Save it in a folder you can write to, open
PowerShell in that folder, and run:

```powershell
.\transmog-cli.exe record --output .\support-trace.tmcap.gz
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
   saved**, then open `support-trace.tmcap.gz` through Transmog's **Import…** or
   **Files → Open in a separate viewer…**. Review the capture and share that one
   compressed file with your support contact.

The default captures complete headers, including cookies and credentials, and
retains up to 25 MB (25,000,000 bytes) per request body. Response bodies and
observed byte counts remain available within the overall trace file budget.
Large requests continue forwarding when their capture prefix reaches the cap. To redact `Authorization`, `Proxy-Authorization`, `Cookie` and
`Set-Cookie` values, add `--redact`. This choice persists for subsequent CLI
captures. Use `--retain-sensitive` to change it back. Bodies can still contain
private information, so review the trace before sharing.

To remove the per-request capture cap while retaining the 4 GiB file budget:

```powershell
.\transmog-cli.exe record --unlimited-request-bodies --output .\large-trace.tmcap.gz
```

This preference persists for subsequent CLI runs. Use --request-body-limit
25000000 to restore the default, or supply another positive byte count. The
console prints the active limit before recording. Overall file limits and
separate bounded body-processing limits still apply.

Existing files are never overwritten. Choose another filename for the next
capture. Compression streams the saved native file; if compression fails, the
uncompressed `.tmcap` remains available. Use an output ending in `.tmcap` when
you want an uncompressed trace.

## Certificates and interrupted runs

The default root is ephemeral: its private key stays in memory and is never
written to disk. The CLI removes its trusted public certificate after the
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

This retains a protected private key and reuses the same CLI root on subsequent
`--persistent-root` runs. Ordinary ephemeral runs still use a fresh root. To
remove persistent roots and their keys too:

```powershell
.\transmog-cli.exe roots cleanup --include-persistent
```

The CLI state directory is restricted to the current user before private
material is created. Do not share it with a support contact.

## Other platforms and manual setup

Run `transmog-cli record --output support-trace.tmcap.gz` in a terminal. Configure
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

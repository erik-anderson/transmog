# Windows release and qualification

Windows is the first supported Transmog desktop target. A release is a signed,
current-user NSIS installer that uses the Evergreen WebView2 runtime already
present on the supported Windows host. The app does not bundle a browser or
WebView runtime, require administrator installation, or fetch runtime UI assets.

## Release prerequisites

In addition to [building.md](building.md), the release workstation needs:

- a code-signing certificate available to the current user and usable by the
  Windows SDK `signtool.exe`;
- its 40-hex SHA-1 certificate thumbprint (Windows' signing-certificate
  selector, not a traffic-certificate identity);
- access to the configured timestamp service; and
- the Tauri NSIS artifacts in cache when the final build must run without
  network access.

No certificate identity, private key, token, or password belongs in this
repository. The release wrapper injects the certificate thumbprint in a
temporary ignored config overlay and removes that overlay after packaging.

For hosted releases, dispatch **Windows signed draft release** on `main` and
approve its protected `release-signing` job after the build passes. The workflow
uses Azure Artifact Signing through profile-scoped OIDC, signs the inner binaries
and NSIS bundle, verifies timestamps/publisher, tests permission rejection, and
creates a GitHub provenance attestation for the final signed installer. It creates
only a draft release. Its setup, permissions, artifacts, and verification command
are described in [`ci/README.md`](../ci/README.md). The separate unsigned installer
workflow remains available for development.

For the alternative local-certificate path, create a signed installer:

```powershell
pwsh ./scripts/package-windows.ps1 `
  -SigningCertificateThumbprint 0123456789ABCDEF0123456789ABCDEF01234567
```

The wrapper reproduces UI assets, runs the desktop/application/host test gate,
builds the NSIS bundle, requires a valid Authenticode result, and prints the
installer's SHA-256. For local installer development only:

```powershell
pwsh ./scripts/package-windows.ps1 -UnsignedDevelopment
```

Repeat the package command with Cargo offline and network blocked before
release to prove the cached build path. The packaging wrapper fails if the
Tauri configuration changes away from `skip`; this prevents accidentally
shipping a fixed runtime, bootstrapper, or offline WebView installer.

```powershell
pwsh ./scripts/package-windows.ps1 `
  -SigningCertificateThumbprint 0123456789ABCDEF0123456789ABCDEF01234567 `
  -Offline
```

## Installer and lifecycle policy

- Installation is per-current-user. Downgrades are blocked.
- Evergreen WebView2 is an operating-system prerequisite and is never bundled.
  Diagnostics report the actual runtime version.
- Tauri's single-instance plugin is registered first. A second launch focuses
  the existing main window and exits; it cannot create a competing proxy owner.
- **Prepare safe update handoff** detaches breakpoint control, seals/stops
  active capture, stops the proxy, and restores host state. The app must then
  be closed before the installer runs.
- The NSIS pre-uninstall hook refuses to continue while the app is running. In
  update mode it invokes `--prepare-update`; in removal mode it invokes
  `--uninstall-cleanup`. A nonzero maintenance result aborts removal so an
  unresolved system-proxy or trust-store state is never hidden by deleting the
  executable.
- Maintenance mode restores only an existing Transmog proxy journal. Uninstall
  removes only the exact SHA-256 certificate in
  `certificate-ownership-v1.json`; a similarly named or otherwise unrecorded
  root is never touched.

## Owned data

The installer owns its executable, shortcuts, and uninstall registration.
Runtime product state under `%LOCALAPPDATA%\Transmog` owns these names:

- `proxy-recovery-v1.json`;
- `certificate-ownership-v1.json`;
- `preferences.<generation>.json` and quarantined variants;
- `.transmog-state-*.tmp`; and
- `diagnostics.jsonl` plus its single rotation.

Uninstall removes only those regular files. Unknown files and directories are
left in place. User-selected CAs, private keys, captures, JSONL files, SAZ
files, and support bundles are user artifacts and are never deleted by the
uninstaller. Updates preserve product state and certificate ownership.

## Desktop qualification gates

Run the release WebView gate (the default fast soak uses 100 bounded refreshes):

```powershell
pwsh ./scripts/test-windows-desktop.ps1
```

For release qualification, use at least 30 minutes:

```powershell
pwsh ./scripts/test-windows-desktop.ps1 -SoakMinutes 30
```

The gate uses the packaged WebView technology and verifies startup, CSP and
browser-error absence, semantic landmarks and accessible names, the Chromium
accessibility tree, keyboard-focusable native controls, forced-colors behavior,
reduced motion, 200% device scale, long localized labels, bounded JavaScript
heap, bounded process working-set growth, and second-launch handoff. Tauri and
NSIS declare per-monitor-v2 DPI awareness.

Also run the complete repository gate and standalone interoperability matrix:

```powershell
pwsh ./scripts/test.ps1
pwsh ./scripts/test-interop.ps1
```

The interoperability matrix proves the proxy observed the exchange and covers
standalone curl/Chromium with Nginx, Apache, and Caddy across supported HTTP and
content-coding paths. Certificate validation remains enabled.

## Clean-machine release checklist

The maintainer has deferred this checklist for the initial pipeline integration.
Hosted Windows Server installer and WebView checks do not establish clean Windows
11 qualification. Draft release evidence records that limitation.

Use a disposable, fully updated Windows 11 VM with no Transmog state:

1. Confirm the supported Windows image has an updated Evergreen WebView2
   runtime, disconnect networking, and install the signed NSIS bundle. Confirm
   a valid Authenticode publisher and that no WebView payload is installed by
   Transmog.
2. Launch Transmog twice. Confirm one process/window and focus handoff.
3. Choose **Set up HTTPS interception**, verify the warning precedes the OS
   prompt, and approve current-user trust. Start the proxy and verify that the
   current-user proxy is applied, live traffic follows automatically, and HTTPS
   certificate validation remains on.
4. Exercise HTTP/1.1, HTTP/2, HTTP/3 egress, gzip, deflate, Brotli, zstd,
   WebSocket inspection, one request edit, one response edit, composer replay,
   native capture, JSONL export, and strict/extended SAZ export. For every case,
   verify the Transmog session/capture evidence so a direct path cannot pass.
5. Exercise keyboard-only navigation, a screen reader, 100%, 150%, and 200%
   scaling, Windows high contrast, reduced motion, and deliberately long text.
6. Start the proxy, terminate Transmog, and relaunch. Confirm startup recovery
   restores the exact previous registry values without requiring a manual
   recovery click. Also verify normal close and an OS-requested exit restore
   before process termination.
7. Run **Prepare safe update handoff**, install a newer signed build offline,
   and confirm preferences and the exact app-owned certificate survive.
8. Uninstall and approve exact certificate removal. Confirm the system proxy is
   not enabled, the ownership record and declared app data are gone, unrelated
   current-user roots remain, and user-created captures/exports remain.

Record OS build, WebView2 version, installer SHA-256, signer, test command
outputs, and the pre/post proxy and certificate inventories in the release
evidence. Certificate install/remove dialogs are intentionally manual; release
automation must wait for the human action rather than bypass validation.

# Transmog desktop shell

This Windows-first Tauri 2 application is the production shell over the
UI-neutral Transmog application facade. It provides proxy lifecycle, live
sessions, inspectors, breakpoints, replay, captures/exports, durable settings,
redacted support diagnostics, and hardened update/uninstall handoff while
keeping the WebView a bounded presentation client.

The delivery and security decisions are recorded in
[`docs/adr/0007-tauri-webui-delivery.md`](../../docs/adr/0007-tauri-webui-delivery.md).
Complete prerequisites and packaging commands are in
[`docs/building.md`](../../docs/building.md#windows-desktop-shell).

## Layout

- `src/lib.rs`: portable renderer, route allowlist, command DTOs, and tests;
- `src/windows.rs`: thin Tauri/WebView2 adapter;
- `ui/src`: WebUI HTML/CSS sources and authored TypeScript island;
- `ui/dist`: reviewed, reproducible ESM and state-projection outputs;
- `capabilities/main.json`: empty Tauri core/plugin permission set;
- `tauri.conf.json`: current-user Windows bundle metadata relying on the host's
  Evergreen WebView2 runtime, with no localhost server;
- `windows/installer-hooks.nsh`: fail-closed update/uninstall maintenance hook.

## Local verification

```powershell
Push-Location ./apps/desktop/ui
npm ci
npm run build
npm run check
Pop-Location

. ./scripts/dev-env.ps1
cargo test --locked -p transmog-desktop
cargo run --locked -p transmog-desktop
```

For the real WebView smoke check, start the binary with a loopback-only
WebView2 DevTools port, then run `npm run smoke:webview -- --port 9333` from
`apps/desktop/ui`. The DevTools switch is test-only. Hosted CI remains inactive
until a human explicitly enables it.

Use `scripts/test-windows-desktop.ps1` for accessibility, high contrast/DPI,
localization-length, memory/soak, and single-instance gates. Pass
`-ScreenshotPath C:\path\to\traffic.png` to capture the fixed-viewport traffic
workspace from the same real WebView2 run. Use
`scripts/package-windows.ps1` for unsigned development or signed release NSIS
bundles. See [`docs/windows-release.md`](../../docs/windows-release.md).

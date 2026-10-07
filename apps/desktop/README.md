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
- `ui/src`: composable WebUI workspaces, reactive templates, and shared initial state;
- `ui/dist`: ignored ESM and state-projection outputs generated during the build;
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
npm run test:workspaces
Pop-Location

. ./scripts/dev-env.ps1
cargo test --locked -p transmog-app-webui -p transmog-desktop
cargo run --locked -p transmog-desktop
```

The browser workspace checks use the locked Playwright installation in
`e2e/playwright`; run `npm ci` there and install its Chromium browser once.
They render the production WebUI templates under Trusted Types enforcement
and use fixture commands, covering lazy imports, keyed session rows, burst
refreshes, inspector races, preserved drafts, and editor disposal.

`app-shell` composes `traffic-workspace`, `settings-workspace`,
`automation-workspace`, `breakpoint-workspace`, `composer-workspace`, and
`capture-workspace`. Children communicate through typed bubbling events and
public `@attr`/`@observable` properties. HTML owns list and conditional rendering;
CSS owns visual state. Rust and TypeScript share `ui/src/initial-state.json` so
the first document and hydration use the same defaults.

Traffic and settings hydrate eagerly for live capture and proxy bootstrap.
Other workspace modules hydrate on first selection and stay mounted to preserve
drafts. Monaco loads after its Trusted Types environment and applies its CSS
only when an editor is used. Paused-exchange components reserve an 18-rem block
with WebUI lazy rendering; client-created rows additionally wait for visibility
before allocating their editor. Editors dispose owned models when removed.
The custom origin embeds the current bundler asset manifest, including shared
and dynamic chunks, with no filesystem fallback.

Workspace selection does not use URL routing. If URL navigation is introduced,
use `@microsoft/webui-router` after resolving the Trusted Types compatibility
constraint recorded in ADR 0007. The current custom-protocol response is
buffered, so progressive streaming hydration would need a transport change.

For the real WebView smoke check, start the binary with a loopback-only
WebView2 DevTools port, then run `npm run smoke:webview -- --port 9333` from
`apps/desktop/ui`. The DevTools switch is test-only. Hosted CI remains inactive
until a human explicitly enables it.
The localization check temporarily expands the Traffic heading eightfold at
200% DPI and restores the original text immediately after measuring it.

Use `scripts/test-windows-desktop.ps1` for accessibility, high contrast/DPI,
localization-length, memory/soak, and single-instance gates. Pass
`-ScreenshotPath C:\path\to\traffic.png` to capture the fixed-viewport traffic
workspace from the same real WebView2 run. Use
`scripts/package-windows.ps1` for unsigned development or signed release NSIS
bundles. See [`docs/windows-release.md`](../../docs/windows-release.md).

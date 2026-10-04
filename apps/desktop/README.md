# rustymiddle desktop shell

This Windows-first Tauri 2 application is the Phase 0 delivery spike for the
product shell. It proves Rust-native Microsoft WebUI SSR, a TypeScript-only
interaction island, embedded custom-protocol assets and fetch, a typed Tauri
command, and a bounded channel notification. It does not yet start or display
the proxy service.

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
- `tauri.conf.json`: Windows bundle metadata with no localhost server.

## Local verification

```powershell
Push-Location ./apps/desktop/ui
npm ci
npm run build
npm run check
Pop-Location

. ./scripts/dev-env.ps1
cargo test --locked -p rustymiddle-desktop
cargo run --locked -p rustymiddle-desktop
```

For the real WebView smoke check, start the binary with a loopback-only
WebView2 DevTools port, then run `npm run smoke:webview -- --port 9333` from
`apps/desktop/ui`. The DevTools switch is test-only. Hosted CI remains inactive
until a human explicitly enables it.

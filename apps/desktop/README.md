# Transmog desktop

The Windows desktop app provides a visual workspace for inspecting and modifying
HTTP traffic. It combines live capture, request/response previews, breakpoints,
replay, traffic scripts, auto-response rules, and capture exports.

The traffic table has configurable columns and resizable inspectors. Layouts are
saved, responses choose a suitable preview automatically, and editors can expand
into a larger working window.

## Architecture

Tauri hosts the WebUI interface in WebView2. Composable workspaces handle
presentation; the UI-neutral Rust application facade owns proxy lifecycle,
traffic data, persistence, and operations. Editors and secondary workspaces load
on demand. The interface is served from an embedded origin without a local web
server or filesystem fallback.

- `src/windows.rs`: Windows and Tauri integration.
- `ui/src`: WebUI components, templates, and styles.
- `crates/app`: application services shared with headless clients.
- `crates/app-webui`: embedded UI rendering and assets.

See [the delivery architecture decision](../../docs/adr/0007-tauri-webui-delivery.md)
for the framework and security choices.

## Build and run

Follow [the build prerequisites](../../docs/building.md#windows-desktop-shell), then:

```powershell
Push-Location ./apps/desktop/ui
npm ci
npm run check
Pop-Location

. ./scripts/dev-env.ps1
cargo run --locked -p transmog-desktop
```

The desktop Cargo package produces `transmog.exe` on Windows. The separate
headless CLI package produces `transmog-cli.exe`.

## Verification and packaging

UI checks use the repository's Playwright installation; native desktop checks
cover WebView2, accessibility, display scaling, and application lifecycle.
Development guidance is in [ui/AGENTS.md](ui/AGENTS.md).

Use [the Windows release guide](../../docs/windows-release.md) for packaging and
[the CI guide](../../ci/README.md) for signed draft releases and the manual unsigned
installer workflow. Current desktop behavior is described in
[the product guide](../../docs/product-shell.md).

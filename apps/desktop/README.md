# Windows desktop development

The desktop runs the WebUI interface in WebView2. Tauri serves embedded assets
and connects the interface to the Rust application services. See the [desktop
guide](../../docs/product-shell.md) for capture, replay, recording and viewer
windows.

| Source | Responsibility |
| --- | --- |
| [Windows host](src/windows.rs) | Tauri commands and Windows integration |
| [UI](ui/src) | Components, templates and styles |
| [Application](../../crates/app) | Proxy, traffic and persistence services shared with headless clients |
| [WebUI adapter](../../crates/app-webui) | Initial rendering and embedded assets |

Follow [Build and launch](../../docs/building.md#build-and-launch-the-windows-desktop)
to build `transmog.exe` and its helpers. For UI changes, follow the [UI
instructions](ui/AGENTS.md) and [desktop checks](../../docs/testing.md#desktop-and-capture-workflows).
The [delivery decision](../../docs/adr/0007-tauri-webui-delivery.md) explains the
embedded origin and its security controls. [Windows releases](../../docs/windows-release.md)
covers packaging and qualification.

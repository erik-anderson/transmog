# ADR 0007: Tauri/WebUI delivery uses a custom protocol and bounded commands

Status: accepted
Date: 2026-10-04

## Context

The desktop product needs Rust-rendered Microsoft WebUI documents, small
TypeScript interaction islands, and access to an existing UI-neutral
application/session service. It must not start a second proxy, expose the core
directly to hostile captured content, require a Node process at runtime, or
open an unauthenticated loopback HTTP listener.

Tauri maps a registered custom scheme to an HTTP localhost origin on Windows.
WebUI can render its compiled protocol into a fully buffered document and can
hydrate authored components from an external ESM bundle. WebUI also supports
JSON partial navigation, but its documentation currently identifies a conflict
between enforced Trusted Types and that client-side routing path.

Tauri's configured CSP injection applies to Tauri-managed frontend assets. It
does not rewrite a response produced by an application-registered custom
protocol. WebUI needs a fresh per-document nonce, so the custom protocol
renderer—not a static Tauri configuration string—must own the effective CSP.

## Decision

Use Tauri with WebView2 on Windows and Microsoft WebUI behind a local adapter.
[Dependency manifests](../dependencies.md) own the reviewed version pins.

- Register `transmog-ui` and serve only allowlisted embedded paths. On
  Windows, the observed origin is `http://transmog-ui.localhost/`. Do not
  open a TCP listener or use Tauri's localhost plugin.
- Point Tauri's frontend URL at that custom protocol. The WebUI adapter owns
  the only embedded resource inventory; Tauri must not also embed the build
  directory, which contains private build metadata as well as runtime assets.
  Brotli-compress resources at build time and verify their exact original bytes.
  Decode each resource on first GET within its recorded size and cache it for
  subsequent requests. HEAD serves metadata without decoding. Retain the existing
  path allowlist, MIME types, CSP, and offline delivery behavior.
- Render initial documents in Rust from the embedded WebUI protocol. Generate a
  fresh CSP nonce for each document, pass it through `RenderOptions`, and return
  the restrictive CSP as a response header. Keep Tauri's static `app.security.csp`
  unset because it does not govern custom-protocol responses and cannot carry
  the per-render nonce.
- Enforce `require-trusted-types-for 'script'` with the `webui` policy and one
  app-created `monaco` policy. The Monaco policy is shared across the editor's
  internal policy requests, permits HTML only for the pinned local editor
  renderer, and restricts worker script URLs to app-owned same-origin
  `/monaco-*` assets. Monaco's runtime-generated CSS and bundled data-font are
  the only reasons `style-src 'unsafe-inline'` and `font-src data:` are present;
  inline scripts and evaluation remain forbidden. Serve no remote
  subresources, allow navigation only to the custom application origin that
  Tauri exposes on the current platform, and deny WebView-initiated popups.
  App commands own separate capture viewers and isolated preview windows.
- Use same-origin custom-protocol fetch only for explicitly allowlisted,
  app-owned resources. Use typed Tauri
  commands for bounded authoritative queries and mutations. Use bounded
  channels for lossy revision/delta hints, never as authoritative state or a
  body-chunk stream.
- Defer WebUI partial routing. This preserves Trusted Types enforcement and
  avoids committing the product to a client navigation model before the real
  shell needs one. A future change requires a new compatibility and security
  proof; it does not silently weaken Trusted Types.
- Generate the deterministic ESM bundle and projection manifest during the
  build and ignore `ui/dist` in Git. `npm run check` type-checks and generates
  the client artifacts before Cargo consumes them. Compile the WebUI protocol
  and CSS from source in Rust's build script.
- Keep the Tauri dependency and binary Windows-only. The library/render slice
  remains portable, while Linux desktop support waits for the declared
  WebKitGTK qualification phase.

## Capability and data boundary

The main window capability contains no broad Tauri core/plugin permissions.
Each product command is explicitly registered, accepts a bounded typed DTO,
revalidates in Rust, and returns a stable bounded result. Live session
notifications are hint-only typed channels. Filesystem paths are accepted only
by narrow capture/CA/import/export and confirmed save operations. There is no
generic filesystem, shell, WebView HTTP-client or remote-domain IPC permission.

The custom protocol has no filesystem fallback. Unknown and traversal-shaped
paths return 404, methods other than GET/HEAD return 405, dynamic pages are
`no-store`, and all responses set `nosniff` and a no-referrer policy. Captured
traffic never enters raw HTML, script, style, component metadata, or a URL
loaded by the trusted application WebView. The explicit
[captured-page preview](../captured-page-preview.md) uses a separate profile and
resource-interception boundary without application-command access.

## Consequences

The desktop can be built and exercised without a localhost server and ships no
browser or Node runtime. Renderer tests do not need a window, and the WebView
boundary has a repeatable real-runtime probe.

Generated client assets must be deliberately refreshed when TypeScript or its
locked packages change. The renderer buffers a complete document and does not
provide soft navigation or progressive streaming. Tauri/WebUI are
same-build internal contracts and may change atomically while the monorepo is
still under the human-controlled stabilization policy.

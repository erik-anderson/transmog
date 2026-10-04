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

Use Tauri 2.12.1 with WebView2 on Windows and Microsoft WebUI 0.0.30 behind a
local adapter.

- Register `rustymiddle-ui` and serve only allowlisted embedded paths. On
  Windows, the observed origin is `http://rustymiddle-ui.localhost/`. Do not
  open a TCP listener or use Tauri's localhost plugin.
- Render initial documents in Rust from the embedded WebUI protocol. Generate a
  fresh CSP nonce for each document, pass it through `RenderOptions`, and return
  the restrictive CSP as a response header. Keep Tauri's static `app.security.csp`
  unset because it does not govern custom-protocol responses and cannot carry
  the per-render nonce.
- Enforce `require-trusted-types-for 'script'` with the sole `webui` policy.
  Serve no remote subresources and allow navigation only to the custom
  application origin that Tauri exposes on the current platform. Deny new
  windows.
- Use same-origin custom-protocol fetch only for explicitly allowlisted,
  app-owned resources such as the Phase 0 delivery probe. Use typed Tauri
  commands for bounded authoritative queries and mutations. Use bounded
  channels for lossy revision/delta hints, never as authoritative state or a
  body-chunk stream.
- Defer WebUI partial routing. This preserves Trusted Types enforcement and
  avoids committing the product to a client navigation model before the real
  shell needs one. A future change requires a new compatibility and security
  proof; it does not silently weaken Trusted Types.
- Check in the deterministic ESM bundle and projection manifest. Compile the
  WebUI protocol and CSS from source in Rust's build script. A clean Cargo build
  therefore needs no npm process; `npm run check` proves the checked-in client
  artifacts are reproducible.
- Keep the Tauri dependency and binary Windows-only. The library/render slice
  remains portable, while Linux desktop support waits for the declared
  WebKitGTK qualification phase.

## Capability and data boundary

The main window capability contains no Tauri core/plugin permissions. The only
native application operation is the explicitly registered
`phase_zero_probe` command. It accepts a deny-unknown-fields DTO, validates
bounded input in Rust, returns a stable result, and sends exactly one typed
channel notification. Later commands must follow the same allowlist and
validation pattern; broad filesystem, shell, HTTP-client, or remote-domain IPC
permissions are prohibited.

The custom protocol has no filesystem fallback. Unknown and traversal-shaped
paths return 404, methods other than GET/HEAD return 405, dynamic pages are
`no-store`, and all responses set `nosniff` and a no-referrer policy. Captured
traffic never enters raw HTML, script, style, component metadata, or a URL
loaded by the WebView.

## Phase 0 evidence

The spike was tested on Rust 1.97.1, Clang/LLVM 22.1.4, Ninja 1.13.2, Windows
SDK 10.0.26100, and WebView2 154.0.4258.53.

- Six Rust tests cover SSR state, fresh nonces, restrictive CSP, embedded
  asset/probe routing, traversal-shaped paths, method handling, and command DTO
  validation, including the exact custom-origin navigation allowlist.
- TypeScript type checking and deterministic esbuild/projection regeneration
  pass from the npm lockfile.
- Dev and optimized WebView2 runs load only the custom origin plus Tauri's IPC
  origin. The automated DevTools smoke check observes ESM, both hashed WebUI CSS
  assets, favicon, same-origin JSON, command IPC, one channel hint, no CSP
  violations, and no browser errors.
- The optimized binary imports Windows system libraries but no Node runtime.
  The app runs without an HTTP development server.
- Tauri produced a 1.79 MiB NSIS installer. After the first hash-verified NSIS
  tool download, a second full packaging run succeeded with Cargo offline and
  sandbox network access blocked.
- `cargo deny` accepts the reviewed MPL-2.0 and Apache-2.0-with-LLVM-exception
  licenses added by Tauri. RUSTSEC-2024-0370 is ignored only because Cargo
  metadata exposes Tauri's GTK 0.18 packages while target-specific `cargo tree`
  proves the unmaintained macro is unreachable from both configured Windows
  and Linux rustymiddle desktop targets. The exception must be removed or
  replaced before Linux desktop support is enabled.

## Consequences

The desktop can be built and exercised without a localhost server and ships no
browser or Node runtime. Renderer tests do not need a window, and the WebView
boundary has a repeatable real-runtime probe.

Generated client assets must be deliberately refreshed when TypeScript or its
locked packages change. The initial implementation buffers a complete document
and does not provide soft navigation or progressive streaming. Tauri/WebUI are
same-build internal contracts and may change atomically while the monorepo is
still under the human-controlled stabilization policy.

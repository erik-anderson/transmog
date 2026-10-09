# Key dependencies

Transmog keeps protocol and platform dependencies behind its own layered
interfaces. Application code and interception hooks operate on canonical
Transmog types rather than Hyper, quiche, BoringSSL, Tauri, or V8 objects. This
page calls out the significant third-party building blocks and the boundaries
where they are used; it is not an exhaustive dependency inventory.

Exact versions are intentionally not duplicated here. The workspace
[`Cargo.toml`](../Cargo.toml), [`Cargo.lock`](../Cargo.lock), desktop
[`package.json`](../apps/desktop/ui/package.json), and its lockfile are the
authoritative version records.

## Networking, HTTP, and cryptography

| Responsibility | Key dependencies | Transmog usage |
| --- | --- | --- |
| Asynchronous I/O | `tokio`, `bytes`, `futures-util`, `tower-service` | Sockets, tasks, timers, bounded channels, byte buffers, and service adapters throughout the transport layers. |
| Canonical HTTP vocabulary | `http`, `http-body`, `http-body-util` | Standard method, URI, header, status, body-frame, and trailer types at adapter boundaries. Transmog converts these to its own validated canonical exchange model before invoking hooks. |
| HTTP/1.1 and HTTP/2 | `hyper`, `hyper-util` | Downstream HTTP serving and pooled upstream clients, including HTTP/2 multiplexing and TCP Happy Eyeballs. The integration lives in `transmog-http`. |
| TLS and cryptography | `boring`, `boring-sys`, `tokio-boring`, `hyper-boring` | One BoringSSL family implements upstream TLS verification, downstream interception TLS, certificate issuance, ALPN, and Hyper's TLS transport. BoringSSL is compiled from source as part of the native build. |
| Operating-system trust roots | `rustls-native-certs` | Enumerates platform root certificates, which Transmog imports into its BoringSSL trust snapshot. Despite its name, it does **not** put rustls in the production TLS data path. |
| QUIC and HTTP/3 | `quiche` | Pooled HTTP/3 origin egress, QUIC transport, and Alt-Svc upgrades in `transmog-h3`. The crate is built with `boringssl-boring-crate` so HTTP/3 shares the same BoringSSL bindings and trust policy as HTTP/1.1 and HTTP/2. |

The shared `transmog-network` layer owns bounded DNS candidate ordering and
Happy Eyeballs policy. Hyper performs the TCP race; the quiche adapter races
complete, verified QUIC/TLS handshakes. See
[origin connection racing](networking.md).

Transmog deliberately rejects a second production TLS implementation. The
dependency policy bans OpenSSL, native-tls, rustls, and webpki data-plane
crates, while the crypto-graph check verifies exactly one `boring` and
`boring-sys` version in the normal dependency graph. See
[`deny.toml`](../deny.toml) and
[`check-crypto-graph.ps1`](../scripts/check-crypto-graph.ps1).

## Content, WebSockets, and file formats

| Responsibility | Key dependencies | Transmog usage |
| --- | --- | --- |
| Content coding | `async-compression` | Bounded streaming gzip, Brotli, DEFLATE/zlib, and zstd decoding and re-encoding in `transmog-content`. |
| WebSocket compression and handshake primitives | `flate2`, `sha1`, `base64`, `getrandom` | Per-message DEFLATE plus RFC handshake and masking primitives in the bounded `transmog-websocket` implementation. |
| Structured data | `serde`, `serde_json` | Control models, persisted application state, diagnostics, capture metadata, and narrow process/FFI messages. |
| Native capture compression and integrity | `flate2`, `crc32fast`, `sha2` | Independent DEFLATE frames, corruption checks and indexed body digests for lazy `.tmcap` reads. |
| Native capture encryption | `aes-gcm`, `argon2`, `getrandom`, `zeroize` | Optional per-frame AES-256-GCM after compression, password-derived keys, fresh randomness and secret cleanup in `transmog-capture`. |
| SAZ compatibility | `zip` | Indexed Fiddler-compatible import and finalized export, including supported password encryption. Native `.tmcap` remains the streaming capture format. |
| Safe raster previews | `image` | GIF, JPEG, PNG, and WebP decoding inside the isolated, resource-bounded preview worker. SVG previews use an opaque image URL rather than parsing active content in the application page. |

## Desktop application and scripting

| Responsibility | Key dependencies | Transmog usage |
| --- | --- | --- |
| Native desktop shell | `tauri`, Wry, Evergreen WebView2 | Window lifecycle, native commands, and the Windows system WebView. Transmog expects Evergreen WebView2 to be present rather than bundling a browser runtime. |
| UI component model | `@microsoft/webui-framework`, `@microsoft/webui` | Rust-oriented WebUI projection and the browser-side component runtime used by the desktop interface. |
| Code editing | `monaco-editor` | TypeScript automation editing and diagnostics in the desktop UI. |
| Sandboxed script execution | `deno_core` and V8 | Executes user automation in a separate capability-restricted process with explicit time and memory containment. |
| TypeScript parsing/transpilation | `deno_ast` | Validates and transpiles the supported TypeScript subset before isolated execution. |
| Browser-side code | TypeScript and `esbuild` | The small client-side layer that cannot be rendered or handled in Rust. Generated bundles are reproducibly verified from the npm lockfile. |
| Windows integration | `windows-sys` | Current-user proxy settings, process attribution, AppContainer and Job Object sandboxing, console suppression, and related host APIs. |

## Build and supply-chain controls

Native dependencies require LLVM/Clang, CMake, Ninja, NASM, and the Windows SDK
on Windows. These are build prerequisites rather than runtime libraries; see
[build prerequisites](building.md) for the supported toolchain.

Security-sensitive and native integration packages are pinned where review and
binary compatibility matter. Cargo and npm lockfiles make complete transitive
resolution reproducible. `cargo deny` enforces the approved license and source
policy, denies known-vulnerable or yanked packages subject to documented narrow
exceptions, and rejects wildcard dependency versions. Dependency changes are
expected to pass the normal workspace tests, crypto-graph check, and platform
build gates described in [testing](testing.md).

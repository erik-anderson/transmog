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
| HTTP/1.1 and HTTP/2 | `hyper`, `hyper-util`, `h2` | Downstream HTTP serving and pooled upstream clients, including HTTP/2 multiplexing and TCP Happy Eyeballs. The integration lives in `transmog-http`. |
| TLS and cryptography | `boring`, `boring-sys`, `tokio-boring` | One BoringSSL family implements upstream TLS verification, downstream interception TLS, hybrid ML-KEM exchange, ML-DSA handshake authentication, certificate issuance, ALPN, and Transmog's Hyper TLS transport. BoringSSL is compiled from source as part of the native build. |
| Operating-system trust roots | `rustls-native-certs` | Enumerates platform root certificates, which Transmog imports into its BoringSSL trust snapshot. Despite its name, it does **not** put rustls in the production TLS data path. |
| QUIC and HTTP/3 | `quiche` | Pooled HTTP/3 origin egress, QUIC transport, and Alt-Svc upgrades in `transmog-h3`. The crate is built with `boringssl-boring-crate` so HTTP/3 shares the same BoringSSL bindings and trust policy as HTTP/1.1 and HTTP/2. |

The shared `transmog-network` layer owns bounded DNS candidate ordering and
Happy Eyeballs policy. Hyper performs the TCP race; the quiche adapter races
complete, verified QUIC/TLS handshakes. See
[origin connection racing](networking.md).

The BoringSSL crate family is temporarily patched to one immutable upstream Git
revision for the unreleased signature-algorithm GREASE API. The patch applies to
all four crates, including quiche's transitive dependencies. The exact revision
lives in the workspace manifest and lockfile; remove the patch when a published
release provides the same native API. Dependency policy permits only this Git
repository, and ordinary registry dependencies remain required elsewhere.
`hyper-boring` remains a test-only comparison connector; production uses
Transmog's connector and stream adapter. The crypto-graph check enforces this.
Cargo applies patches only at the consumer's workspace root, so independent
embedding workspaces must repeat these Git entries and the vendored transport
patches described below, with paths resolving to the reviewed source trees.
The repository's Rust and .NET examples already use the patched root workspace.

`transmog-tls-native` contains the narrow native boundary for client certificate
requests, alternate server credentials and signature-algorithm GREASE omitted by
the safe wrapper. It owns native references and selected public-chain evidence
using that same BoringSSL instance. Algorithm and trust policy remain in
`transmog-tls`; see the
[negotiation policy](chromium-negotiation.md#algorithms-and-clienthello) and
[verifier contract](chromium-verifier.md).

Transmog deliberately rejects a second production TLS implementation. The
dependency policy bans OpenSSL, native-tls, rustls, and webpki data-plane
crates, while the crypto-graph check verifies exactly one package identity,
including source and version, for each of `boring` and `boring-sys` in the normal
dependency graph. See
[`deny.toml`](../deny.toml) and
[`check-crypto-graph.py`](../scripts/check-crypto-graph.py).

Portable Cargo build commands also validate the actual resolved Git source,
package versions, BoringSSL gitlink/submodule and active native features before
compilation. `scripts/dev-env.ps1 -Check` runs the same checks for PowerShell.
Source/header/precompiled archive overrides (`BORING_BSSL_*PATH` and
`*_ASSUME_PATCHED`, including target and FIPS forms) are rejected both by the
portable preflight and the native build script. FIPS, RPK, wildcard and relaxed
verification features require a separately qualified native policy. Use the
portable wrapper for independent embedding builds too; it checks the consumer's
resolved graph against the embedded dependency's source receipt.

Maintainers can check the complete baseline with
`python scripts/roll-chromium-dependencies.py --check`. To prepare a roll from a
clean workspace, use `--boring-source PATH` (or `--boring-revision FULL_SHA`),
`--chromium-version FOUR.PART.VERSION`, and/or `--root-store-revision FULL_SHA`,
plus `--output-dir .local/dependency-rolls/NAME`. Only selected inputs change.
The tool derives the BoringSSL revision from its Git submodule, synchronizes the
four version/revision pins and Cargo lockfile, regenerates PKI, rolls PSL and CT
policy data together, and can rebuild roots from five public source inputs.
It leaves the source workspace unchanged and creates an independent candidate
checkout, a binary-safe `changes.patch` and `report.json` with source/patch/data
receipts and required qualification. Review APIs, upstream patches, verifier
policy, schema and source-label changes, then run the qualification in the
candidate before applying its patch. Profiles are not automatically activated or
renamed by a data roll.

Vendored `h2`, `hyper` and `quiche` use checksum-pinned registry archives and
reproducible patches. Preflight validates both files and Cargo resolution; see
[the transport inventory](../third_party/rust-transport/README.md). Desktop credits
and generated notices include their licenses, and CycloneDX records archive and
patch identities.

Generated notices and CycloneDX artifacts identify Git packages by their full
resolved commit, even when the package version matches a registry release.
They record the native BoringSSL base revision, enabled binding features and
upstream patch hashes, distinguishing applied patches from inactive source
inventory. Patch selection is qualified against the reviewed upstream build
function; a change to that logic stops the roll for review before provenance can
claim which patches are applied.

## Content, WebSockets, and file formats

| Responsibility | Key dependencies | Transmog usage |
| --- | --- | --- |
| Content coding | `async-compression` | Bounded streaming gzip, Brotli, DEFLATE/zlib, and zstd decoding and re-encoding in `transmog-content`. |
| WebSocket compression and handshake primitives | `flate2`, `sha1`, `base64`, `getrandom` | Per-message DEFLATE plus RFC handshake and masking primitives in the bounded `transmog-websocket` implementation. |
| Structured data | `serde`, `serde_json` | Control models, persisted application state, diagnostics, capture metadata, and narrow process/FFI messages. |
| Native capture compression and integrity | `zstd`, `crc32fast`, `sha2` | Adaptive independent zstd chunks, block corruption checks and indexed body digests for lazy `.tmcap` reads. |
| Native capture encryption | `aes-gcm`, `argon2`, `hkdf`, `getrandom`, `zeroize` | Optional fixed-block AES-256-GCM, Argon2id password derivation, domain-separated epoch/locator keys, fresh randomness and secret cleanup in `transmog-capture`. |
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
| Browser-side code | TypeScript and `esbuild` | Typed UI components, browser interactions and projections. Generated bundles are reproducibly verified from the npm lockfile. |
| Windows integration | `windows-sys` | Current-user proxy settings, process attribution, AppContainer and Job Object sandboxing, console suppression, and related host APIs. |

The desktop updater uses Tauri's separate HTTPS client, scoped to signed update
retrieval. Its Rustls/Reqwest dependencies are allowed only on that desktop edge;
they never supply proxy or Composer TLS. See [ADR 0001](adr/0001-native-stack.md).

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

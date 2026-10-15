# ADR 0001: LLVM/Ninja and one BoringSSL family

Date: 2026-10-02
Status: accepted

## Decision

Use the human-facing profile name **Windows LLVM/Ninja**. Use the Rust toolchain
required by the current manifests, LLVM/Clang as the C/C++ compiler, Ninja as the
native build executor, CMake as BoringSSL's generator and NASM on Windows. The canonical Rust target
remains `x86_64-pc-windows-msvc` because `msvc` describes the ABI and Windows
SDK library ecosystem. `clang-cl` implements that ABI; Microsoft `cl.exe` is not
the selected compiler.

Pin `boring`, `boring-sys`, `tokio-boring` and the test-only `hyper-boring` to one
reviewed BoringSSL family. Select quiche's `boringssl-boring-crate` TLS backend; the
[dependency manifests](../dependencies.md) own current version pins. All origin TLS
contexts derive from one `UpstreamTlsContextFactory` and immutable trust snapshot. A CI graph check fails
on duplicate Boring families or a second proxy/replay TLS stack.

Embedders configure client authentication through the shared factory's
`with_client_authentication`, supplying the leaf, key and ordered public issuers.
The same identity configures Hyper and QUIC and supplies the public capture path.
Low-level SSL owners register known public issuers through `TlsCapture::client_issuers`;
completion retains the leaf first and deduplicates repeated issuers. This avoids
Boring's non-nullable extra-chain getter, which panics for an empty chain.
Certificate evidence never reads or serializes private keys.

The Windows desktop updater uses the supported Tauri updater SDK's HTTPS client
and platform trust to retrieve signed GitHub release assets. Its Rustls dependency
chain is confined to the desktop-owned updater and its existing Reqwest client,
which the desktop configures to use its active proxy and public interception root.
The crypto graph gate allows that exact client package on the desktop edge only.
This client never supplies a TLS context to proxy, replay, interception, or
HTTP/3 resources. Cargo-deny permits
only the named updater wrappers, and the crypto graph gate rejects every other
production path to those TLS crates. This exception keeps standard signed updates
available without maintaining a fork of the updater's transport and installer.

## Consequences

Windows builders need LLVM, Ninja, CMake, NASM, and compatible Windows SDK/link
libraries. quiche's HTTP/3 adapter can accept the same BoringSSL context policy.
If an async wrapper hides a required verification, cancellation, or streaming
hook, Transmog will maintain a thin Tokio driver over quiche rather than add
another QUIC/TLS implementation.

The reproducible Windows bootstrap pins compiler and build-tool distributions.
Local copies live under ignored `.tools`; hosted environments use reviewed,
hash-checked downloads. See [building](../building.md) for prerequisites and
bootstrap commands.

The Visual Studio C++ component in `.vsconfig` supplies only the current Windows
SDK, Universal CRT, STL headers, and ABI libraries needed by the MSVC Rust target.
Native C and C++ compilation is explicitly configured to `clang-cl`, archiving
to `llvm-lib`, and build execution to Ninja. Cargo's final Windows PE link uses
the current SDK `link.exe`: quiche 0.30 declares an auxiliary `cdylib`, and LLD
22.1.4 rejects that Rust-produced link despite an explicit subsystem. This
narrow linker choice avoids a quiche manifest fork and does not introduce a
second TLS or protocol implementation.

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

Pin `boring`, `tokio-boring` and `hyper-boring` to one reviewed BoringSSL family.
Build quiche with only `boringssl-boring-crate`; the
[dependency manifests](../dependencies.md) own current version pins. All origin TLS
contexts derive from one `UpstreamTlsContextFactory` and immutable trust snapshot. A CI graph check fails
on duplicate Boring families or a second production TLS stack.

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

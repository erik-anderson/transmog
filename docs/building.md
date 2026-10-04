# Build prerequisites

rustymiddle compiles BoringSSL, quiche, and the bundled zstd C sources from
source. A Rust-only installation is therefore insufficient even for
`cargo check --workspace`.

## Windows

The supported profile is named **Windows LLVM/Ninja**. It uses LLVM for all
native compilation and Rust's official Windows ABI target:

- Rust 1.97.1, target `x86_64-pc-windows-msvc`;
- LLVM/Clang 22.1.4 or newer (`clang-cl` and `llvm-lib`; `lld-link` is retained
  for diagnostics but is not Cargo's final PE linker);
- Ninja 1.13.2 or newer;
- CMake 4.4.4 or newer;
- NASM 3.02 or newer;
- Visual Studio 2022 or newer Build Tools/Community with:
  - **MSVC x64/x86 build tools** (`Microsoft.VisualStudio.Component.VC.Tools.x86.x64`);
  - **Windows 11 SDK 10.0.26100** (`Microsoft.VisualStudio.Component.Windows11SDK.26100`).

The technical `msvc` suffix names the ABI and system-library ecosystem; it does not select
the C/C++ compiler. The Visual Studio components are a build dependency even
though `cl.exe` is not
the selected compiler. They provide the current Windows SDK, Universal CRT,
C++ standard-library headers, and ABI-compatible import/static libraries that
`clang-cl` and Rust's Windows ABI target consume. Native C/C++ compilation is
configured to Clang, archiving to `llvm-lib`, and build execution to Ninja.
Cargo's final PE link uses the current Windows SDK `link.exe` because quiche
declares an auxiliary `cdylib` crate type for which LLD 22.1.4 reports a missing
subsystem despite an explicit subsystem argument. Using the SDK linker avoids
an otherwise unnecessary quiche manifest fork.

From Visual Studio Installer, choose **Modify**, then either import the checked-in
`.vsconfig` or select the two individual components above. Installation requires
administrator elevation and may require a restart. The library does not install
or modify build tools at runtime.

The repository-local bootstrap used during development pins official portable
distributions under ignored `.tools`. Those binaries are intentionally not
committed. A system installation of equivalent or newer compatible versions is
also supported.

After installing prerequisites, open a new PowerShell and run:

```powershell
pwsh ./scripts/dev-env.ps1 -Check
pwsh ./scripts/test.ps1
```

`dev-env.ps1` prepends repository-local tools when present, selects `clang-cl`,
`llvm-lib`, and Ninja, and discovers the Windows SDK linker and libraries from
either a normal Visual Studio C++ toolchain or the SDK-scoped fallback used by
managed development hosts. Cargo build scripts compile bundled zstd sources
directly with `clang-cl`; CMake-based native dependencies use the Ninja
generator. No separate zstd SDK or binary installation is required.

### Windows troubleshooting

`linker 'link.exe' not found` means the current Windows SDK/VC tools are not on
`PATH`. Run `dev-env.ps1` in the same PowerShell process before Cargo.

An error in BoringSSL similar to this:

```text
no viable constructor or deduction guide for std::array
```

means Clang found obsolete Visual C++ standard-library headers (the small
`ScopeCppSDK` bundled with some managed workloads is not sufficient). Install
the current **MSVC x64/x86 build tools** component, open a new shell, and confirm
that `INCLUDE` resolves to a current `VC/Tools/MSVC/.../include` directory before
the fallback SDK path.

If CMake selects a Visual Studio generator, ensure `CMAKE_GENERATOR=Ninja` is set.
If BoringSSL cannot assemble x86-64 files, ensure `nasm -v` succeeds in the same
shell.

## Linux

Install Rust 1.97.1+, Clang/LLVM, LLD, CMake, Ninja, NASM, a C/C++ standard
library development package, and normal libc development headers. Then run:

```sh
export CC=clang CXX=clang++ AR=llvm-ar CMAKE_GENERATOR=Ninja
cargo test --workspace --all-features --locked
```

On Windows with Docker Desktop's WSL2 backend, the complete Linux format,
strict-Clippy, all-target test, and release-build gate is reproducible without
installing a separate WSL distribution:

```powershell
./scripts/test-linux-docker.ps1
```

The runner uses the same digest-pinned tool image as content fuzzing, clears
the fuzz-only sanitizer flags, selects Clang plus `llvm-ar` and Ninja, mounts
the checkout read-only, and keeps Cargo downloads and build output in named
Docker volumes.

The standalone interoperability matrix additionally requires Docker Compose,
a standalone `curl` executable, Node.js/npm, and Playwright Chromium. The
runner builds with the same Windows LLVM/Ninja environment and starts
digest-pinned Nginx and Apache containers on ephemeral loopback ports:

```powershell
pwsh ./scripts/test-interop.ps1
```

The first run can download the container images, npm packages, and the pinned
browser. No CA installation or administrator access is required.

Content fuzzing is an optional maintainer workflow, not a build prerequisite.
It requires a nightly Rust toolchain and the pinned `cargo-fuzz` driver because
LLVM sanitizer-backed libFuzzer is Unix-only. See `fuzz/README.md`; the normal
deterministic bomb and cancellation tests still run on Windows and macOS.
Windows maintainers can run the Linux fuzz environment through Docker Desktop's
WSL2 backend with `./scripts/fuzz-content-docker.ps1` without installing Rust
inside the internal `docker-desktop` distribution.

## macOS

Install Rust 1.97.1+, Xcode Command Line Tools, CMake, Ninja, and NASM. Apple
Clang is supported; upstream LLVM may be selected explicitly when matching CI.

```sh
export CC=clang CXX=clang++ CMAKE_GENERATOR=Ninja
cargo test --workspace --all-features --locked
```

## Policy tools

Install `cargo-deny` and run the complete dependency policy plus the dedicated
normal-graph TLS check:

```powershell
cargo install cargo-deny --locked
cargo deny check
pwsh ./scripts/check-crypto-graph.ps1
```

The normal dependency graph must contain exactly one `boring` and one
`boring-sys` version and no second production TLS implementation. Platform
certificate enumeration packages are permitted only as sources of DER bytes;
BoringSSL performs all path and endpoint verification.

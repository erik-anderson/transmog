# Build prerequisites

Transmog compiles BoringSSL, quiche, and the bundled zstd C sources from
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

### Windows desktop shell

The product shell additionally requires the Microsoft Edge WebView2 Evergreen
Runtime and Node.js 24 or newer with npm. Current Windows 11 installations
normally receive WebView2 with Microsoft Edge, but it is a runtime prerequisite
rather than a bundled browser. A clean-machine release gate must verify the
Evergreen runtime is present and current before starting the app; Transmog does
not include a fixed runtime, bootstrapper, or offline WebView installer.

The reusable proxy libraries and CLI can run without changing Windows. The
desktop Start action always uses the current-user system-proxy adapter, which
requires Windows PowerShell 5.1 or newer and access to
`HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`. Explicit
CA installation/removal uses the current-user Root certificate store and can
display an operating-system consent dialog; it is never part of an unattended
build or test. Automated tests use an in-memory host backend. A development or
production CA must be created separately, and its private key must be protected
by a user-only ACL before use. The Windows desktop's **Set up HTTPS
interception** action applies that ACL automatically and then asks the user to
approve the public certificate trust dialog; embedders and the headless CLI
remain responsible for applying equivalent platform policy.

The desktop dependency toolchain is pinned in both lockfiles:

- Tauri Rust crates and npm API/CLI 2.12.1;
- Tauri single-instance plugin 2.4.5 (the last reviewed line compatible with
  the workspace's Rust 1.88 minimum; 2.5 requires Rust 1.90);
- Microsoft WebUI Rust and browser packages 0.0.30;
- `deno_core` 0.412.0 and V8 150.4.0 for the isolated script host;
- TypeScript 7.0.2 and esbuild 0.28.2.

The reviewed Windows V8 archive is pinned by both URL and SHA-256 in
`scripts/cache-v8.ps1`. Prime Cargo's documented `.rusty_v8` cache before an
offline build:

```powershell
pwsh ./scripts/cache-v8.ps1
# Or verify and cache an internally mirrored/downloaded copy:
pwsh ./scripts/cache-v8.ps1 -ArchivePath C:\staging\rusty_v8.lib.gz
```

The script refuses any archive other than SHA-256
`F231F82CBACB9AEFE6D9AF57E6DF2E8959A40E001F79306485133E3C075B98F0`.
Release packaging builds `transmog-script-host` and the isolated
`transmog-preview-worker` beside the desktop executable.
Production activation requires the host's Windows AppContainer plus Job Object;
there is no production fallback to an unsandboxed process.

Install build-time packages and generate the ESM bundle and WebUI
projection manifest:

```powershell
Push-Location ./apps/desktop/ui
npm ci
npm run build
npm run check
Pop-Location
```

`ui/dist` is generated and ignored by Git. `npm run check` type-checks the UI
and builds the client bundle, styles, local Monaco workers, icon, and WebUI
projection manifest, including on a clean checkout. Run it before invoking
Cargo directly. `transmog-app-webui` compiles `protocol.bin` and hashed CSS
into Cargo's output directory on every desktop build. The repository build
task generates the assets before Cargo consumes them:

```powershell
pwsh ./scripts/build-desktop.ps1 -Configuration Debug -LockedDependencies
```

Build, test, and run through the repository's LLVM/Ninja environment:

```powershell
. ./scripts/dev-env.ps1
cargo test --locked -p transmog-desktop
cargo build --locked -p transmog-script-host
cargo build --locked -p transmog-preview-worker
cargo run --locked -p transmog-desktop
```

The desktop locates `transmog-script-host.exe` beside its own executable. The
repository build script builds both binaries into the same Cargo profile
directory. Release packaging copies the exact release host to Tauri's
target-qualified `externalBin` inputs and removes those staging copies
afterward; the NSIS bundle therefore always installs both AppContainer helpers
beside Transmog.

Create an unsigned development installer from the repository root:

```powershell
pwsh ./scripts/package-windows.ps1 -UnsignedDevelopment
```

Signed release procedure, lifecycle policy, ownership boundaries, and the
clean-machine checklist are in [windows-release.md](windows-release.md). The
bundle relies on the host's Evergreen WebView2 runtime and does not include a
fixed runtime, bootstrapper, or offline installer. The first packaging run
downloads Tauri's hash-verified NSIS 3.11 toolchain and `nsis_tauri_utils` into
its user cache. After that cache and all Cargo/npm dependencies exist, the same
command succeeds with Cargo offline and network access blocked. Node and the
Tauri CLI are build tools only; neither the optimized executable nor the
NSIS-installed application starts a Node process or a development server.

For the local WebView2 delivery smoke test, launch the debug or release binary
with a loopback DevTools port and, while it is running, execute:

```powershell
Push-Location ./apps/desktop/ui
npm run smoke:webview -- --port 9333
Pop-Location
```

The script reloads the real WebView, activates the TypeScript island, and fails
on WebUI hydration, the typed status command, missing module/CSS assets, a CSP
violation, or a browser error. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` is
test-only and must not be set for production launches.

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
a standalone `curl` executable, Node.js 24 or newer with npm, and Playwright
Chromium. Node.js 24 is required because the fixture generator uses its built-in
zstd support in addition to gzip, Brotli, and deflate. The runner builds with
the same Windows LLVM/Ninja environment and starts digest-pinned Nginx, Apache,
and Caddy containers on ephemeral loopback ports:

```powershell
pwsh ./scripts/test-interop.ps1
```

The first run can download the container images, npm packages, and the pinned
browser. Docker Desktop must be running. No CA installation or administrator
access is required; the suite creates an ephemeral CA and exact-IP origin leaf
inside its per-run directory and deletes them during cleanup.

The runner also builds release copies of the proxy, script host, and preview
worker, then runs the UI-neutral product workflow with the production
AppContainer/Job Object requirement. The current user must be allowed to create
and launch an AppContainer profile, but no trust-store, system-proxy, registry,
or administrator mutation is performed. The test keeps all application state,
body-cache data, captures, exports, and temporary sandbox identities under its
per-run directory and removes them during cleanup.

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

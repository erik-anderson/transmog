# Building and prerequisites

## Build and launch the Windows desktop

After installing the [Windows toolchain](#windows) and
[desktop prerequisites](#windows-desktop-shell), run from the repository root:

```powershell
python scripts/dev-env.py --check
python scripts/dev-env.py --run cargo fetch --locked
npm ci --prefix apps/desktop/ui
python scripts/build-desktop.py --configuration Debug --locked-dependencies
python scripts/dev-env.py --run cargo run --locked -p transmog-desktop
```

This checks the native tools, prepares locked dependencies, builds the UI and both
sandbox helpers, and launches the desktop. Use `--configuration Release` and
`cargo run --release` for an optimized build. `CARGO_TARGET_DIR` is honored.
Initial dependency downloads require network access.

PowerShell 7 users can also use `./scripts/run-desktop.ps1` after fetching Cargo
dependencies. It configures tools, installs npm packages, builds and launches;
`-Configuration Release` selects the optimized build. `TRANSMOG_PYTHON` selects
an explicit interpreter for the wrapper.

This guide owns prerequisites and build commands. Use [testing](testing.md) to
select verification gates and [Windows release qualification](windows-release.md#local-release-qualification-before-dispatch)
before dispatching a signed release.

Transmog compiles BoringSSL, quiche, and the bundled zstd C sources from
source. A Rust-only installation is therefore insufficient even for
`cargo check --workspace`.

Python 3.11 or newer is mandatory on every development platform, including agent
environments. The portable build, test, Docker, and fuzz runners use only its
standard library. Verify that `python --version` launches an installed interpreter;
on systems where the command is named `python3`, use that name. PowerShell
convenience wrappers accept an explicit interpreter through `TRANSMOG_PYTHON`.
Windows certificate-store, installer, signing, and WebView2 integration checks
retain PowerShell where they exercise Windows behavior.

`python scripts/dev-env.py --check` checks the native toolchain. Python runners
configure their child processes without changing the parent shell. For a direct
command, use `python scripts/dev-env.py --run cargo check --workspace --locked`.
Dot-sourcing `scripts/dev-env.ps1` remains convenient for interactive PowerShell;
it obtains the same native environment from Python.

## Development cache maintenance

Configured Python Cargo commands and PowerShell environment setup check the
development cache once per 24 hours. Cleanup retains the three newest complete
variants for each Cargo target, removing older artifact sets with their matching
fingerprints and excess incremental caches. Variants built within the last day
are protected, and metadata-only checks do not displace retained rlib/PDB builds.
Below 20 GiB free space, an extra check can trim excess recent
variants while still retaining three. Active Cargo builds are skipped using
Cargo's profile locks. Release outputs, native build-script output, unknown
artifact layouts and target directories outside this repository are preserved.

Preview or run cleanup manually:

```text
python scripts/trim-build-cache.py
python scripts/trim-build-cache.py --apply
```

Use `--keep`, `--min-age-hours` and `--min-free-gib` to adjust manual retention.
`TRANSMOG_BUILD_CACHE_TRIM=0` disables automatic checks. CI and explicitly
redirected/custom Cargo layouts skip automatic cleanup. Deleting an older
variant may require recompiling that configuration next time it is used; removing
its fingerprint together with its artifacts lets Cargo rebuild coherently.

## Windows

The supported profile is named **Windows LLVM/Ninja**. It uses LLVM for all
native compilation and Rust's official Windows ABI target:

- Rust 1.97.1, target `x86_64-pc-windows-msvc` on x64 or
  `aarch64-pc-windows-msvc` on ARM64;
- LLVM/Clang 22.1.4 or newer (`clang-cl` and `llvm-lib`; `lld-link` is retained
  for diagnostics but is not Cargo's final PE linker);
- Ninja 1.13.2 or newer;
- CMake 4.4.4 or newer;
- NASM 3.02 or newer;
- Visual Studio 2022 or newer Build Tools/Community with:
  - **MSVC x64/x86 build tools** (`Microsoft.VisualStudio.Component.VC.Tools.x86.x64`);
    ARM64 hosts use **MSVC ARM64 build tools** (`Microsoft.VisualStudio.Component.VC.Tools.ARM64`);
  - **Windows 11 SDK 10.0.26100** (`Microsoft.VisualStudio.Component.Windows11SDK.26100`).

The technical `msvc` suffix names the ABI and system-library ecosystem; it does not select
the C/C++ compiler. The Visual Studio components are a build dependency even
though `cl.exe` is not
the selected compiler. They provide the current Windows SDK, Universal CRT,
C++ standard-library headers, and ABI-compatible import/static libraries that
`clang-cl` and Rust's Windows ABI target consume. Native C/C++ compilation is
configured to Clang, archiving to `llvm-lib`, and build execution to Ninja.
Cargo's final PE link uses the current Windows SDK `link.exe` because the
qualified quiche build includes an auxiliary `cdylib` that does not link with
the reviewed LLD configuration.

From Visual Studio Installer, choose **Modify**, then either import the checked-in
`.vsconfig` or select the two individual components above. Installation requires
administrator elevation and may require a restart. The library does not install
or modify build tools at runtime.

The repository-local tool setup pins official portable
distributions under ignored `.tools`. Those binaries are intentionally not
committed. A system installation of equivalent or newer compatible versions is
also supported.

On ARM64, `setup-windows-ci.ps1` installs the pinned native ARM64 LLVM distribution
so Rust build scripts can load its libclang DLL. CMake, Ninja and NASM use the
same verified portable tools through Windows emulation. `dev-env.py` selects the
ARM64 linker, CRT and Windows SDK libraries. `cache-v8.py` selects the pinned
archive for the native host, or accepts `--target` explicitly. To package ARM64,
run `package-windows.ps1 -Target aarch64-pc-windows-msvc` on an ARM64 build host;
optimized outputs are under `target/aarch64-pc-windows-msvc/release`. The signed
release workflow can bundle these prebuilt outputs on its protected x64 signer.

After installing prerequisites, open a new PowerShell and run:

```powershell
python scripts/dev-env.py --check
```

`dev-env.py` prepends repository-local tools when present, selects `clang-cl`,
`llvm-lib`, and Ninja, and discovers the Windows SDK linker and libraries from
either a normal Visual Studio C++ toolchain or the SDK-scoped fallback used by
managed development hosts. Cargo build scripts compile bundled zstd sources
directly with `clang-cl`; CMake-based native dependencies use the Ninja
generator. No separate zstd SDK or binary installation is required.

### Windows troubleshooting

`linker 'link.exe' not found` means the current Windows SDK/VC tools are not on
`PATH`. Run Cargo through `python scripts/dev-env.py --run cargo ...`.

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

### Build the standalone CLI

Release users can download the signed Windows binary and follow
[the CLI guide](cli.md); the compiler toolchain is only needed for development.
After installing the prerequisites above, build the CLI in the checkout:

```powershell
python scripts/dev-env.py --run cargo build --locked --release -p transmog --bin transmog-cli
.\target\release\transmog-cli.exe --help
```

The executable is `target/release/transmog-cli.exe`; the desktop and its WebView
toolchain are not required. On Linux or macOS, after configuring the platform
build environment below, use the same Cargo build command and run
`./target/release/transmog-cli --help`.

### Windows desktop shell

Use `./scripts/run-desktop.ps1` for the complete build-and-launch flow described
[above](#build-and-launch-the-windows-desktop). The commands below are useful
when running individual build or verification steps.

The product shell additionally requires the Microsoft Edge WebView2 Evergreen
Runtime and Node.js 24 or newer with npm. Current Windows 11 installations
normally receive WebView2 with Microsoft Edge, but it is a runtime prerequisite
rather than a bundled browser. A clean-machine release gate must verify the
Evergreen runtime is present and current before starting the app; Transmog does
not include a fixed runtime, bootstrapper, or offline WebView installer.

The reusable proxy libraries and the CLI's manual `serve` workflow can run
without changing Windows host settings. Guided CLI `record` configures and
restores the current-user proxy unless `--no-system-proxy` is specified, and
asks for root installation unless manual setup is selected. See the
[CLI guide](cli.md) for release-binary usage. The
desktop Start action uses the current-user system-proxy adapter unless manual
routing is configured. The adapter
requires Windows PowerShell 5.1 or newer and access to
`HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`. Explicit
CA installation/removal uses the current-user Root certificate store and can
display an operating-system consent dialog; it is never part of an unattended
build or test. Automated tests use an in-memory host backend. Desktop HTTPS
setup and guided CLI recording create their own interception roots. Durable
private keys use OS-user protection and restricted state directories; the CLI's
default ephemeral key stays in memory. The Windows desktop's **Set up HTTPS
interception** action asks the user to approve the public certificate trust
dialog. Manual `serve` users and embedders manage their clients' certificate
trust and routing themselves.

Desktop dependency versions are pinned in the [Cargo](../Cargo.lock) and
[npm](../apps/desktop/ui/package-lock.json) lockfiles. See the
[dependency map](dependencies.md) for their roles and ownership.

The reviewed Windows V8 archive URL and SHA-256 are pinned in
[`scripts/cache-v8.py`](../scripts/cache-v8.py). Prime Cargo's `.rusty_v8` cache
before an offline build:

```powershell
python scripts/cache-v8.py
# Verify and cache an internally mirrored/downloaded copy:
python scripts/cache-v8.py --archive-path C:\staging\rusty_v8.lib.gz
```

The script rejects bytes that do not match the pinned hash.
Release packaging builds `transmog-script-host` and the isolated
`transmog-preview-worker` beside the desktop executable.
Production activation requires the host's Windows AppContainer plus Job Object;
there is no production fallback to an unsandboxed process.

Install build-time packages and generate the ESM bundle and WebUI
projection manifest:

```powershell
python scripts/dev-env.py --run cargo fetch --locked
npm ci --prefix apps/desktop/ui
npm --prefix apps/desktop/ui run check
```

`ui/dist` is generated and ignored by Git. `npm run check` type-checks the UI
and builds the client bundle, styles, local Monaco workers, icon, and WebUI
projection manifest, including on a clean checkout. Run it before invoking
Cargo directly. Each UI build clears stale generated chunks. `transmog-app-webui`
compiles `protocol.bin` and hashed CSS into Cargo's output directory and embeds
the allowlisted resources with lossless Brotli compression. The custom protocol
decodes and caches resources on first use; Tauri does not embed another copy of
the UI build directory. The repository build
task generates the assets before Cargo consumes them:

```powershell
python ./scripts/build-desktop.py --configuration Debug --locked-dependencies
```

For build-and-launch commands, use the [entry point above](#build-and-launch-the-windows-desktop).
Choose validation from [desktop and capture tests](testing.md#desktop-and-capture-workflows).

The desktop locates `transmog-script-host.exe` beside its own executable. The
repository build script builds the desktop and both helpers into the same
Cargo profile directory. Release packaging copies the exact release host to Tauri's
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

## Container validation requirements

The Docker-backed gates require Python 3.11 or newer on the host, an accessible
Docker engine, and permission to build images, create named volumes and bind-mount
the checkout. On Windows, run Docker Desktop with its WSL2 backend in Linux
container mode. The PQC Chrome gate requires `linux/amd64` containers. A separate
host installation of Rust, Node, Chrome or Linux build tools is not needed for
that gate; its image supplies them.

| Gate | Container requirements |
| --- | --- |
| Linux workspace and fuzzing | The tool image built from [`fuzz/Dockerfile`](../fuzz/Dockerfile) supplies the repository Rust toolchains, Python, Clang/LLVM, CMake, Ninja, NASM and native development headers. Ordinary Linux validation clears the fuzz-only sanitizer flags. |
| PQC browser verification | [`e2e/pqc/Dockerfile`](../e2e/pqc/Dockerfile) extends the tool image with Node.js 24 and npm, the locked Playwright packages, available stable Chrome and its system libraries, and NSS `certutil` from `libnss3-tools`. Chrome's version is recorded rather than pinned or rejected. |
| Live PQC through the CLI | The browser image also supplies D-Bus and GNOME Keyring. The runner starts a private D-Bus session and unlocks a temporary Secret Service so the production CLI can protect its interception CA key. No desktop session or host keyring is required. |

Initial image builds need network access to the container registries, Debian
package repositories, npm, the browser download service and any uncached Cargo
dependencies. Subsequent runs reuse images and Cargo volumes. Only the explicit
`--live-url` option sends validation traffic to a public site; the ordinary
browser cases use loopback fixtures.

The runners mount source read-only and reuse
`transmog-linux-cargo-registry` and `transmog-linux-target` for Cargo downloads
and native build output. These volumes survive container cleanup and need space
for the Rust and C/C++ artifacts. The PQC runner writes public reports under
`.local/pqc-browser-verified`; temporary private keys, captures, NSS databases
and browser profiles stay inside the disposable container. It does not install
roots or configure proxies on the host.

Use [PQC verification](testing.md#post-quantum-tls) for trust/live gate commands,
browser refresh options, assertions and reports.

Custom containers must meet the repository's Node.js 24 or newer baseline.
A distribution's older Node package can install npm
dependencies yet fail when Playwright starts. **Interception CA key protection
is unavailable** means the CLI cannot reach an unlocked Secret Service in its
D-Bus session. Supply that service and keep the CLI in the same session; the
application deliberately refuses an unprotected-key fallback.

## Linux

Install Rust 1.97.1+, Clang/LLVM, LLD, CMake, Ninja, NASM, a C/C++ standard
library development package, and normal libc development headers. Then run:

```sh
python3 scripts/dev-env.py --check
python3 scripts/dev-env.py --run cargo build --workspace --all-features --locked
```

On Windows, the [Linux container gate](testing.md#repository-gate) qualifies the
workspace without a separate WSL distribution. See
[container requirements](#container-validation-requirements) for tools and volumes.

## Interoperability prerequisites

The standalone interoperability matrix requires the native build environment,
Docker Compose, a standalone `curl`, Node.js 24 or newer with npm, and Playwright
Chromium. Node.js 24 supplies the fixture generator's built-in zstd support.
The first run can download container images, npm packages and the pinned browser.
Docker must be running; on Windows use Docker Desktop's Linux container mode.

The product gate requires permission to create and launch an AppContainer profile.
It keeps temporary certificates, application state, body caches, captures, exports
and sandbox identities in its per-run directory and cleans them up. It does not
install host roots or change system proxy settings. Use the
[interoperability guide](testing.md#protocol-and-product-interoperability) for the
command, scope and proof requirements.

Content fuzzing is optional and has its own nightly Rust and `cargo-fuzz`
[prerequisites and runner](../fuzz/README.md).

## macOS

Install Rust 1.97.1+, Xcode Command Line Tools, CMake, Ninja, and NASM. Apple
Clang is supported; upstream LLVM may be selected explicitly when matching CI.

```sh
python3 scripts/dev-env.py --check
python3 scripts/dev-env.py --run cargo build --workspace --all-features --locked
```

## Policy tools

Install `cargo-deny` for dependency-policy checks:

```powershell
cargo install cargo-deny --locked
```

Release maintainers use the pinned `scripts/setup-release-validation-tools.ps1`
setup required by [release qualification](windows-release.md#local-release-qualification-before-dispatch).
The [repository gate](testing.md#repository-gate) runs policy, crypto-graph and
supply-chain checks; [dependency policy](dependencies.md) defines the allowed graph.

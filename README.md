# rustymiddle

`rustymiddle` is a library-first, explicit intercepting proxy for inspecting and
modifying HTTP/1.1, HTTP/2, and HTTP/3 origin exchanges. Browser traffic enters
through an HTTP proxy (including intercepted `CONNECT`); HTTP/3 is an origin
egress protocol, not browser-to-proxy QUIC.

The project is under active implementation. See [architecture](docs/architecture.md),
[certificate model](docs/certificate-model.md), [testing](docs/testing.md), and
[build prerequisites](docs/building.md), and [limitations](docs/limitations.md).

## Local verification

```powershell
pwsh ./scripts/dev-env.ps1 -Check
pwsh ./scripts/test.ps1
```

`test.ps1` configures the pinned native environment, then runs format, strict
clippy, the locked workspace tests, `cargo deny`, the single-TLS graph check,
and supply-chain artifact generation.

The primary build profile is **Windows Clang (MSVC ABI)**: LLVM/Clang and Ninja
compile native code while Rust uses its canonical `x86_64-pc-windows-msvc` ABI
target and the Windows SDK linker. BoringSSL additionally requires CMake and
NASM. Run `pwsh ./scripts/dev-env.ps1 -Check` to validate the toolchain.

Release preparation generates the locked third-party notice inventory and a
CycloneDX 1.5 SBOM:

```powershell
pwsh ./scripts/generate-supply-chain-artifacts.ps1
```

The live Chromium gate needs a one-time, interactive current-user CA setup;
subsequent runs only verify it read-only and are non-interactive:

```powershell
pwsh ./scripts/setup-live-test-ca.ps1
pwsh ./scripts/test-live.ps1
# Explicit teardown when live testing is no longer needed:
pwsh ./scripts/remove-live-test-ca.ps1
```

## Run the proxy

Create an operator-controlled CA and apply a user-only ACL to its private key:

```powershell
pwsh ./scripts/new-proxy-ca.ps1 -CertificatePath ./rustymiddle-ca.pem -PrivateKeyPath ./rustymiddle-ca.key
pwsh ./scripts/install-ca-user.ps1 -CertificatePath ./rustymiddle-ca.pem
```

Run the loopback proxy with a forced egress protocol or `auto`:

```powershell
cargo run --locked -p rustymiddle -- serve --ca-cert ./rustymiddle-ca.pem --ca-key ./rustymiddle-ca.key --listen 127.0.0.1:8080 --route h2
```

Configure the browser's HTTP proxy to the printed `LISTEN_ADDR`. HTTPS is
intercepted through `CONNECT`; HTTP/3 is origin egress only. When finished,
remove exactly the installed current-user root using the SHA-256 value printed
by the install command:

```powershell
pwsh ./scripts/uninstall-ca-user.ps1 -Sha256 <64-hex-digit-value>
```

## Security

The proxy binds only to loopback by default. Installing its CA grants the proxy
the ability to read TLS traffic; CA installation is always explicit and must be
reversed with the matching uninstall command. Private keys and traffic bodies
are never logged by default.

Licensed under the MIT License.

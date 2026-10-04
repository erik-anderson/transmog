# rustymiddle

`rustymiddle` is a library-first, explicit intercepting proxy for inspecting and
modifying HTTP/1.1, HTTP/2, and HTTP/3 origin exchanges. Browser traffic enters
through an HTTP proxy (including intercepted `CONNECT`); HTTP/3 is an origin
egress protocol, not browser-to-proxy QUIC.

The project is under active implementation. See [architecture](docs/architecture.md),
[Hooks lifecycle](docs/hooks-lifecycle.md), [embedding](docs/embedding.md),
[certificate model](docs/certificate-model.md), [testing](docs/testing.md),
[performance](docs/performance.md), [build prerequisites](docs/building.md), and
[limitations](docs/limitations.md). Forward work is organized by the
[layered monorepo roadmap](docs/specs/monorepo-roadmap.md), with a dedicated
[content-processing plan](docs/specs/content-processing-plan.md).
SAZ conversion behavior and its fidelity limits are documented in
[SAZ compatibility](docs/saz-compatibility.md). The optional declarative layer
is described in [automation](docs/automation.md); embedders may instead install
their own Hooks v2 interceptors directly. WebSocket upgrade, framing, hook, and
resource semantics are documented in
[WebSocket inspection](docs/websocket-inspection.md).
The UI-independent composition boundary is documented in the
[application/session service guide](docs/application-session-service.md).

## Local verification

```powershell
pwsh ./scripts/dev-env.ps1 -Check
pwsh ./scripts/test.ps1
# Real curl and Chromium through the proxy to pinned third-party servers:
pwsh ./scripts/test-interop.ps1
```

`test.ps1` configures the pinned native environment, then runs format, strict
clippy, the locked workspace tests, `cargo deny`, the single-TLS graph check,
and supply-chain artifact generation.

`test-interop.ps1` is the opt-in, local external-process compatibility gate.
It publishes digest-pinned Nginx, Apache, and Caddy only on ephemeral loopback
ports. Standalone curl proves HTTP/1.1 plus verified HTTP/2 and HTTP/3 origin
egress; Playwright-managed Chromium proves normal navigation, every supported
HTTP content coding (including a four-layer stack), and a WebSocket echo through
the browser's `CONNECT` behavior. Every case requires proxy modification or
terminal exchange evidence so client bypass cannot produce a false pass. The
runner uses an ephemeral CA in process and does not install a certificate or
modify an OS trust store.

The primary build profile is **Windows LLVM/Ninja**: Clang compiles native code,
`llvm-lib` archives it, Ninja executes native builds, and the Windows SDK
provides the final linker and platform libraries. Rust's technical target triple
still ends in `-msvc` because that names the Windows ABI, not the selected C/C++
compiler. BoringSSL additionally requires CMake and NASM. Run
`pwsh ./scripts/dev-env.ps1 -Check` to validate the complete toolchain.

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

Add `--capture ./session.rmcap` to stream redacted metadata to the native,
checksummed capture format. Body bytes remain excluded unless
`--capture-bodies` is also supplied. Existing output files are never
overwritten. Headless inspection and conversion use the same capture library:

```powershell
cargo run --locked -p rustymiddle -- capture inspect --input ./session.rmcap
cargo run --locked -p rustymiddle -- capture validate --input ./session.rmcap
cargo run --locked -p rustymiddle -- capture export --input ./session.rmcap --format jsonl --output ./session.jsonl
cargo run --locked -p rustymiddle -- capture export --input ./session.rmcap --format saz --output ./session.saz
```

SAZ is a finalized compatibility export, not the live storage format. Strict
mode emits conventional Fiddler archive members; `saz-extended` additionally
includes a namespaced fidelity manifest. Missing body bytes and observer loss
are marked as incomplete rather than silently presented as complete.

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

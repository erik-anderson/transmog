# Transmog

<p align="center">
  <img src="apps/desktop/icons/transmog-logo.svg" width="160" alt="Transmog logo">
</p>

**See what your apps send. Stop traffic in flight. Change it. Replay it.**

Transmog is a web debugging proxy designed to grow into a cross-platform
developer tool. It turns HTTP traffic into a live, inspectable workspace:
capture requests as they happen, select any exchange to explore its request and
response, pause traffic at breakpoints, override behavior with reusable rules,
and keep a durable record when the interesting bug finally appears.

The desktop experience is the product: a focused traffic list and split
request/response inspector, backed by a carefully layered Rust proxy engine.
Those layers also provide reusable crates for applications that need the same
interception, content-processing, automation, or capture capabilities without
the Transmog UI.

> Transmog is under active development. Initial product development and release
> qualification are focused on Windows, where the desktop application uses the
> system's Evergreen WebView2 runtime. The portable engine is already exercised
> on Linux; native Linux and macOS desktop products remain future work.

## What you can do

- **Watch traffic live.** Transmog follows new exchanges automatically and
  keeps capture running when you pin a request or scroll back to investigate.
- **Inspect the whole exchange.** Compare request and response metadata, view
  decoded text, inspect bounded binary data, and safely preview raster images
  and SVGs.
- **Break and modify.** Pause matching requests or responses, inspect their
  state, edit them, continue them, or fail them explicitly.
- **Build discoverable automation.** Apply conditional header changes, compose
  built-in actions, or use sandboxed scripts when a rule needs more control.
  Every traffic-changing hook leaves audit evidence.
- **Serve auto-responses.** Start from a captured response or create one from
  scratch, edit its matching criteria, and reorder rules using first-match-wins
  semantics. Traffic clearly identifies the rule that answered it.
- **Find the caller.** On supported local platforms, traffic records include
  the originating process name and PID; non-loopback clients are identified as
  remote.
- **Capture once, analyze later.** Save or stream compressed `.tmcap` traces,
  reopen bodies on demand, and import Fiddler SAZ archives into separate viewers.
  Password encryption is optional for native and SAZ files.
- **Find and reproduce issues.** Search headers and decoded text, compare timing
  waterfalls and shared transport evidence, copy request commands or edit and
  replay requests. A captured-page preview serves only already captured resources.
- **Debug modern web traffic.** Intercept HTTP/1.1 and HTTP/2 over an explicit
  proxy, inspect HTTP/1.1 WebSockets, and use HTTP/3 for supported origin
  egress. Content processing supports gzip, Brotli, DEFLATE, and zstd.

Transmog is intentionally conservative around privileged behavior. It binds to
loopback by default, guides the user through HTTPS interception setup, verifies
the selected root certificate before starting, and restores the previous
per-user Windows proxy settings when it stops. Desktop traffic is transient unless
you save a trace or explicitly record to a file. Desktop sensitive-header
redaction is off by default; Privacy can redact newly received headers, and
Save trace can redact just the exported copy. Abrupt-exit recovery is journaled
for the next launch.

## Try the Windows app

The repository currently builds the application from source. Install the
[Windows prerequisites](docs/building.md#windows), then validate and build the
checked-in application:

```powershell
pwsh ./scripts/dev-env.ps1 -Check
pwsh ./scripts/build-desktop.ps1 -Configuration Debug -LockedDependencies
. ./scripts/dev-env.ps1
cargo run --locked -p transmog-desktop
```

On first launch:

1. Choose **Set up HTTPS interception** and approve the Windows current-user
   root-certificate prompt.
2. Choose **Start proxy**. Transmog starts live capture and applies its
   loopback endpoint to the current user's Windows proxy settings.
3. Use a browser or application normally; its captured traffic appears in the
   Traffic workspace.
4. Choose **Stop proxy** when finished. It restores the previous proxy settings
   first and lets requests already in progress finish.

To create an unsigned installer for testing on another Windows machine:

```powershell
pwsh ./scripts/package-windows.ps1 -UnsignedDevelopment
```

The installer is for development evaluation and is not code-signed. See the
[desktop guide](apps/desktop/README.md) and
[Windows release guide](docs/windows-release.md) for packaging and qualification
details.

## Clean layers, reusable engine

Transmog keeps the desktop, application services, automation, protocol
adapters, and canonical exchange engine in separate layers with dependencies
pointing inward. UI and persistence types do not leak into the proxy core, and
transport-specific objects do not leak into interception hooks.

That design makes the lower layers useful on their own:

- embed the proxy runtime with application-provided hooks, routing, trust,
  upstream services, and bounded observers;
- reuse streaming content decoding and re-encoding independently of the UI;
- consume the session/application facade from another front end or headless
  workflow; and
- write native TMCap captures or finalized SAZ exports from an owned
  application.

Start with the [architecture overview](docs/architecture.md),
[key dependency map](docs/dependencies.md), and
[embedding guide](docs/embedding.md). The [documentation index](docs/README.md)
separates current behavior, architectural decisions, operational guidance, and
the [future roadmap](docs/roadmap.md).

For a foreign-language example, the [.NET 10 embedding sample](examples/dotnet-embedding/README.md)
uses a narrow native ABI to issue a bounded request through the canonical Rust
engine from a C# command-line application.

## Headless proxy and capture tools

Download the separate signed `transmog-cli.exe` from the
[release assets](https://github.com/erik-anderson/transmog/releases). It runs
without the desktop app or a Rust toolchain. Open PowerShell in its folder. For
a guided support capture, run the CLI, reproduce the issue, then press Ctrl+C
to stop and save a compressed trace:

```powershell
.\transmog-cli.exe record --output .\support-trace.tmcap
```

Save captures as `.tmcap`. TMCap compresses individual chunks as they arrive
and lets the desktop open body payloads on demand.

The CLI asks to install its public root without relaunching, configures the
Windows proxy, and removes the root afterward. Its default ephemeral private
key stays in memory. See [the support capture guide](docs/cli-support-capture.md)
for signature verification, review and sharing, persistent roots, redaction,
encryption, circular recording, and interrupted-run recovery.

For manual proxy setup, create an operator-controlled CA:

```powershell
.\transmog-cli.exe ca generate --cert .\transmog-ca.pem --key .\transmog-ca.key
```

Install only the public certificate in the client's trust store, then start the
listener and configure the client to use `127.0.0.1:8080` as its HTTP/HTTPS proxy:

```powershell
.\transmog-cli.exe serve --ca-cert .\transmog-ca.pem --ca-key .\transmog-ca.key --listen 127.0.0.1:8080 --capture .\session.tmcap --capture-bodies
```

`serve` leaves host proxy settings and certificate trust for you to manage. Its
capture redacts sensitive headers; `--capture-bodies` retains body bytes.
Stop with Ctrl+C, then inspect, validate, or convert the file:

```powershell
.\transmog-cli.exe capture inspect --input .\session.tmcap
.\transmog-cli.exe capture validate --input .\session.tmcap
.\transmog-cli.exe capture export --input .\session.tmcap --format jsonl --output .\session.jsonl
.\transmog-cli.exe capture export --input .\session.tmcap --format saz --output .\session.saz
```

Existing capture destinations are never overwritten. SAZ export requires both
request and response headers for every exchange; keep the TMCap source for full
evidence. When finished with manual setup, restore the client's proxy settings
and remove the exact public certificate you installed.

See [the CLI guide](docs/cli.md) for command workflows, proof IDs, password
options, export limits, and recovery. Help is available without starting a proxy:

```powershell
.\transmog-cli.exe help
.\transmog-cli.exe record --help
.\transmog-cli.exe help capture export
```

## Technology

Transmog uses Hyper for HTTP/1.1 and HTTP/2, quiche for QUIC and HTTP/3, and a
single BoringSSL family for TLS, certificate verification, interception
certificates, and cryptographic operations. Tokio provides the asynchronous
runtime, while `async-compression` supplies bounded streaming gzip, Brotli,
DEFLATE, and zstd processing.

The Windows application uses Tauri and Wry with the system's Evergreen
WebView2 runtime, Microsoft WebUI for the interface, Monaco for script editing,
and an isolated V8 process for sandboxed automation. See the
[key dependency map](docs/dependencies.md) for ownership boundaries,
supply-chain controls, and the authoritative manifest locations.

## Development and verification

The primary build profile is Windows with LLVM and Ninja: Clang compiles native
code, `llvm-lib` archives it, Ninja executes native builds, and the Windows SDK
provides platform libraries and the linker. BoringSSL additionally requires
CMake and NASM.

Run the complete local gate with:

```powershell
pwsh ./scripts/test.ps1
```

The opt-in interoperability suite drives the real proxy with standalone curl,
Chromium, and digest-pinned Nginx, Apache, and Caddy endpoints:

```powershell
pwsh ./scripts/test-interop.ps1
```

The Linux gate runs the complete workspace test matrix and a release build as
Linux binaries in a pinned Debian container. On Windows, Docker Desktop executes
that container on its WSL2 Linux kernel:

```powershell
pwsh ./scripts/test-linux-docker.ps1
```

This provides strong portability coverage for the Rust engine and headless
tools, but it is not Linux desktop qualification. The current Linux gate does
not exercise a native desktop WebView, system proxy and certificate-store
integration, production helper-process sandbox, installer, or native browser
end-to-end workflow.

See [build prerequisites](docs/building.md), [testing](docs/testing.md), and
[current limitations](docs/limitations.md) for the supported matrix and exact
boundaries.

## Security

Installing a local CA grants Transmog the ability to read TLS traffic from
clients that trust it. Protect its private key, use it only on systems and
traffic you are authorized to inspect, and remove the exact root when it is no
longer needed. Private keys and traffic bodies are never written to diagnostics
by default.

## License

Transmog is licensed under the [MIT License](LICENSE).

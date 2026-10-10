# Testing

Local deterministic fixtures are the primary correctness gate. Public sites and
hosted CI are not required to validate ordinary changes. Generated results are
per-run evidence, not maintained test-status reports.

## Repository gate

On Windows, enter the LLVM/Ninja environment and run:

```powershell
. ./scripts/dev-env.ps1
./scripts/test.ps1
```

The gate checks formatting, strict workspace Clippy, locked tests, dependency
policy, the single-TLS-family graph and generated supply-chain artifacts. It does
not install certificates or modify system proxy settings. Tests define individual
cases; this guide describes which gates to use instead of copying their inventory.

The Chromium HAR/NetLog import regression uses sanitized, checked-in archives
and runs in the ordinary Rust suite without a browser or network connection.
See [browser fixtures](../crates/app/tests/fixtures/browser/README.md) for regeneration.

For a narrower Rust change, select the affected Cargo packages. To qualify the
portable Rust workspace and headless binaries as Linux processes:

```powershell
./scripts/test-linux-docker.ps1
```

That runner uses a digest-pinned image, mounts the checkout read-only and keeps
Cargo/target data in named volumes. It does not qualify a native Linux desktop.
Hosted release gates and deliberately inactive broader workflows are described
in [CI](../ci/README.md).

## Desktop and capture workflows

Production browser checks exercise templates and fixture commands under CSP and
Trusted Types. Native WebView2 checks verify the actual application origin,
commands, accessibility, theme/forced colors, scaling, compact layouts and
single-instance routing. Run the relevant paths for UI or asynchronous changes:

```powershell
npm --prefix apps/desktop/ui run check
npm --prefix apps/desktop/ui run test:workspaces
./scripts/test-windows-desktop.ps1
./scripts/test-windows-desktop.ps1 -ViewerChecks
```

The viewer gate includes imported metadata, binary Composer replay, independent
window roles and captured-page isolation. The release soak adds
`-SoakMinutes 30` to the main-window gate. These probes isolate app data and close
only their owned windows. See [UI guidance](../apps/desktop/ui/AGENTS.md) for
populated, wide/compact and keyboard review.

For CLI changes, build the CLI and run its Windows console lifecycle probe:

```powershell
. ./scripts/dev-env.ps1
cargo build --locked -p transmog --bin transmog-cli
python ./scripts/test-cli-support.py --executable ./target/debug/transmog-cli.exe --artifacts ./artifacts/cli-lifecycle
```

It uses controlled loopback traffic, isolated profiles and owned hidden consoles.
It checks Ctrl+C, ephemeral/persistent roots, encrypted streaming, memory/disk
circular capture, crash recovery, command help, proof-ID validation, and
destination safety. It exercises circular save recovery and rejects lossy or
failed SAZ exports without leaving partial output, without installing roots or
changing the host proxy. User-facing workflows use the standalone binary; see
[the CLI guide](cli.md). For SAZ encryption/import/export changes, run the
independent reader:

```powershell
./scripts/test-saz-interop.ps1
```

That gate authenticates AES-256 exports with native 7-Zip, compares decrypted
members, rejects a wrong password and imports a 7-Zip ZipCrypto fixture. It runs
without signing credentials. See [SAZ compatibility](saz-compatibility.md).

## Protocol and product interoperability

With Docker Desktop available:

```powershell
./scripts/test-interop.ps1
```

The runner puts independently distributed curl/Chromium clients and digest-pinned
Nginx, Apache and Caddy origins on opposite sides of the release proxy. Unique
proof values appear in both the response and evidence so proxy bypass cannot
pass. Coverage spans supported HTTP/ingress-egress combinations, content codings,
WebSockets, edits, automation, retained bodies, preview and capture/export.

The headless product gate uses release proxy/script/preview helpers and requires
the production AppContainer/Job Object boundary. Containers use ephemeral
loopback ports; temporary certificates and fixtures are removed in cleanup.
It does not mutate OS trust, system proxy or durable user state.

## Local Chromium load gate

For repeatable browser scale qualification without public sites, run
`./scripts/test-load.ps1`. The [load-testing guide](load-testing.md) documents
CLI/desktop modes, the smoke profile, per-browser routing and generated evidence.

## Live Chromium gate

This optional network-enabled gate uses a fresh browser profile, QUIC disabled,
no certificate-error bypass and a durable current-user test CA. CA installation
and removal are explicit interactive setup/teardown; ordinary runs verify the
exact root read-only:

```powershell
./scripts/setup-live-test-ca.ps1
./scripts/test-live.ps1
./scripts/remove-live-test-ca.ps1
```

The run proves intercepted HTTPS and H1/H2/H3 origin egress, writing bounded,
redacted protocol evidence under `verification/`. See
[verification artifacts](../verification/README.md). Installer and clean-machine
qualification are in [the Windows release guide](windows-release.md).

## Fuzzing, performance and Miri

Content/WebSocket fuzz targets live in the independent `fuzz/` workspace so
libFuzzer stays out of shipping artifacts. Use [the fuzz guide](../fuzz/README.md)
for corpus policy and the pinned Linux/Docker runner. Performance commands and
measurement interpretation are in [the performance harness](performance.md).

On a host with the nightly component installed, the transport-neutral core can
also run under Miri:

```powershell
cargo +nightly miri test -p transmog-core --lib
```

Miri is an additional diagnostic; the repository gate does not install or silently
skip it. Documentation-only edits require link/content review, not app builds.

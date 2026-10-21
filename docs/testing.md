# Testing

Local deterministic fixtures are the primary correctness gate. Public sites and
hosted CI are not required to validate ordinary changes. Generated results are
per-run evidence, not maintained test-status reports. This guide owns routine
test selection and commands. Install [build prerequisites](building.md) first;
[Windows release](windows-release.md#local-release-qualification-before-dispatch)
owns the mandatory candidate qualification sequence.

## Repository gate

Python 3.11 or newer is mandatory. The runner configures native tools for its
child processes; on Windows this is the LLVM/Ninja environment. Run:

```powershell
python ./scripts/dev-env.py --check
python ./scripts/test.py
```

The gate checks formatting, strict workspace Clippy, locked tests, dependency
policy, the single-TLS-family graph and generated supply-chain artifacts. It does
not install certificates or modify system proxy settings. Tests define individual
cases; this guide describes which gates to use instead of copying their inventory.

Before a signed release dispatch, complete
[local release qualification](windows-release.md#local-release-qualification-before-dispatch).
It adds the hosted build's browser, Windows certificate, SAZ, optimized CLI,
PDB, and WebView checks to the portable repository gate.

The Chromium HAR/NetLog import regression uses sanitized, checked-in archives
and runs in the ordinary Rust suite without a browser or network connection.
See [browser fixtures](../crates/app/tests/fixtures/browser/README.md) for regeneration.

The [portable investigation corpus](../verification/fixtures/portable/README.md)
is checked by the ordinary Python gate and current native capture-reader tests.
Use `python scripts/portable_fixtures.py --check` for a mutation-free corpus check.
`cargo test -p transmog-portable` exercises the K1 production rule reader,
native matching/actions and verified object staging. `cargo test -p transmog-app
--lib rule_interchange` covers disabled import/export, conflict remaps, failure
atomicity, workspace/generation binding, linked snapshots and restart recovery.
K2's `cargo test -p transmog-portable --test working_cases` covers the case corpus,
directory history/recovery, stale writers, bounded large objects and failed
publication. `cargo test -p transmog-app --lib investigation_tests` covers inert
native evidence opening, source/occurrence identity, original context, streams,
capture-wide loss, encrypted inputs and changed originals. Unix no-follow/locking
checks run in the Linux container; Windows tests cover blocked replacement.
These K1/K2 checks alone do not qualify archive readers, package encryption or CLI support.
K3's `cargo test -p transmog-portable --test packages` qualifies production ZIP64
preflight and hostile payloads inside native packages, bare-archive rejection,
encryption, exact-ZIP forwarding, independent readers,
working copies and publication failures. Capture artifact tests cover authenticated
random reads, incomplete/tampered completion and failed-output poisoning. A sparse
ZIP64 metadata test exceeds four GiB without reading/allocating its declared body.
The current-format fuzz builder includes the mandatory document kind so its seeds
continue reaching the documented parser stages.

The complete `npm run test:workspaces` gate includes portable-case authoring,
password retry, inert context, note navigation, disabled import, cancellation,
retained progress and responsive/forced-color layouts. To isolate that flow after
`npm run check`, use `node scripts/check-workspaces.mjs --portable-cases-only`.

For the real WebView2 transfer flow, build a test-only desktop binary:

```powershell
python scripts/dev-env.py --run cargo build --locked -p transmog-desktop --features desktop-test-fixtures
./scripts/test-windows-desktop.ps1 -SkipReleaseBuild -ExecutablePath ./target/debug/transmog.exe -PortableCaseChecks
```

This creates a fresh profile and exercises production IPC, encrypted save/open,
same-filename replacement with pinned readers, wrong-password retry, explicit
plaintext working copy, inert notes, layout and
Escape focus. The feature-only file-picker adapter selects owned destinations inside
that test instance's export root; product builds have no such adapter. It does not
qualify a signed installer or the visual native file chooser. Source/evidence
streaming and actual disabled imports also have application integration coverage.

Use [the composition benchmark](performance.md#portable-archive-composition) for
compression policy measurements. The narrowed K4 checks only native context
listing/extraction, password modes and standalone binary operation; broader
case/rule CLI work is deferred.

For context CLI changes, run strict Clippy and all targets for `-p transmog`.
The executable tests exercise extraction, metadata-only reads, encrypted input,
no-prompt behavior and password spaces/quotes/backslashes/Unicode. A separate
opt-in standalone shell check copies the selected binary to an owned temporary
directory, restricts the product's PATH, and executes a quoted-password script:

```powershell
$env:TRANSMOG_CONTEXT_TEST_BINARY = (Resolve-Path ./target/release/transmog-cli.exe).Path
$env:TRANSMOG_CONTEXT_TEST_SHELL = (Get-Command pwsh).Source
python scripts/dev-env.py --run cargo test --locked -p transmog --test context standalone_binary_and_shell_quoted_passwords -- --ignored --nocapture
```

Use an absolute POSIX shell path such as `/bin/sh` for the shell variable on Linux.
Interactive masked entry also needs a real-terminal check; piped stdin deliberately
fails when an encrypted input has no supplied password. No desktop/browser checks
are implied by this CLI-only qualification.

For a narrower Rust change, select the affected Cargo packages. To qualify the
portable Rust workspace and headless binaries as Linux processes:

```powershell
python ./scripts/test-linux-docker.py
```

That runner uses a digest-pinned image, mounts the checkout read-only and keeps
Cargo/target data in named volumes. It does not qualify a native Linux desktop.
Hosted release gates and deliberately inactive broader workflows are described
in [CI](../ci/README.md).

## Desktop and capture workflows

Auth fixtures run with `cargo test -p transmog-auth` and the application auth
tests. The ordinary browser gate includes masking, reveal/copy, stale replies,
coverage, keyboard access and populated responsive layouts; isolate it with
`node apps/desktop/ui/scripts/check-workspaces.mjs --auth-only` after building UI.
The native imported-evidence gate uses a fresh profile and changes no proxy or
certificate settings:

```powershell
python scripts/dev-env.py --run cargo build --locked -p transmog-desktop
./scripts/test-windows-desktop.ps1 -SkipReleaseBuild -ExecutablePath ./target/debug/transmog.exe -AuthChecks
```

It retains screenshots and native command/accessibility evidence in its isolated
profile folder. This flow also verifies context-menu clipboard writes, masked and
secret-inclusive Auth subtrees, original headers and the bundled MAPI tree through
the real native commands. Copy-specific browser checks can be isolated with
`node apps/desktop/ui/scripts/check-workspaces.mjs --copy-only`; retained stream
message copying is included in `--streams-only` and the ordinary workspace gate.
The `auth_evidence` [fuzz target](../fuzz/README.md) supplements
deterministic truncation, hostile ASN.1/XML/JSON, duplicate and decompression tests.

Production browser checks exercise templates and fixture commands under CSP and
Trusted Types. Native WebView2 checks verify the actual application origin,
commands, accessibility, theme/forced colors, scaling, compact layouts and
single-instance routing. Run the relevant paths for UI or asynchronous changes:

```powershell
npm --prefix apps/desktop/ui run check
npm --prefix apps/desktop/ui run test:workspaces
./scripts/test-windows-desktop.ps1
./scripts/test-windows-desktop.ps1 -WindowGeometryChecks
./scripts/test-windows-desktop.ps1 -ViewerChecks
```

The viewer gate includes imported metadata, binary Composer replay, independent
window roles and captured-page isolation. The release soak adds
`-SoakMinutes 3` to the main-window gate. These probes isolate app data and close
only their owned windows. See [UI guidance](../apps/desktop/ui/AGENTS.md) for
populated, wide/compact and keyboard review.

The native memory gate warms status refreshes after the functional fixtures load,
then enforces the existing 256 MiB process working-set growth limit over 100
refreshes, or the requested soak duration. Viewer checks use the same bounded
phase against the populated viewer with the largest matching catalog, rather
than the empty reopened main window. Cold-to-loaded suite growth remains diagnostic because importing a large
capture changes the retained catalog. Before/after working set, private bytes,
process identity, window role, row count and iteration counts are retained in `status-memory.json` under
the isolated validation evidence folder; `memory-samples.jsonl` records fixture
phases. Sampling, workload and process-identity failures fail the gate.

The main-window WebView2 probe separately requires raw JavaScript heap below
96 MiB after the functional suite. On failure it records a post-collection heap
sample for diagnosis; the original raw sample still determines the result. This
test threshold does not change extension sandbox memory limits.

The window geometry gate repeats native close/reopen and maximize/restore cycles
on every connected display. It retains physical bounds and actual display DPI in
`artifacts/window-geometry`; use a scaled display to qualify the DPI regression.

For CLI changes, build the CLI and run its Windows console lifecycle probe:

```powershell
python scripts/dev-env.py --run cargo build --locked -p transmog --bin transmog-cli
python ./scripts/test-cli-support.py --executable ./target/debug/transmog-cli.exe --artifacts ./artifacts/cli-lifecycle
```

It uses controlled loopback traffic, isolated profiles and owned hidden consoles.
It checks Ctrl+C, ephemeral/persistent roots, encrypted streaming, memory/disk
circular capture, crash recovery, command help, proof-ID validation, and
destination safety. It exercises circular save recovery, labeled SAZ placeholders and rejection of
unsafe export fields without leaving partial output, without installing roots or
changing the host proxy. User-facing workflows use the standalone binary; see
[the CLI guide](cli.md). For SAZ encryption/import/export changes, run the
independent reader:

```powershell
python ./scripts/test-saz-interop.py
```

That gate authenticates AES-256 exports with native 7-Zip, compares decrypted
members, rejects a wrong password and imports a 7-Zip ZipCrypto fixture. It runs
without signing credentials. See [SAZ compatibility](saz-compatibility.md).

### Direct WebView2 smoke probe

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

## Protocol and product interoperability

With the [interoperability prerequisites](building.md#interoperability-prerequisites) installed:

```powershell
python scripts/test-interop.py
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

### Response editing acceptance

Qualify the connected [response editing workflows](automation.md#choose-an-editing-workflow)
across their actual boundaries:

| Gate | What it establishes |
| --- | --- |
| UI check and full `test:workspaces` suite above | Production-template authoring, preserved drafts, failed/stale saves, ordered tests, variant activation, comparison, keyboard and wide/compact layouts under CSP/Trusted Types. |
| `./scripts/test-windows-desktop.ps1 -ViewerChecks -CapturedPageChecks` | Real native commands and isolated previews: captured JavaScript → edit → compare → passive save → script-enabled HTML/CSS/image/API preview → supplied-response comparison → explicit activation/Baseline. Also qualifies linked-file update/frozen reload, script choice, network failures and independent browser state. |
| `python scripts/test-response-file-modes.py` | Copy/link through the real proxy, including explicit copies above the former size cutoff, restart repair, coherent historical snapshots and zero requests to a deny origin. |
| `python scripts/test-interop.py` | Live variant switching, header/script order, literal edits preserving fresh unrelated tokens, actual operation evidence, compressed/bounded failures, restart and native capture round trips against controlled origins. |

Run scoped Rust tests and strict Clippy for the app/session/renderer/desktop and
any changed automation, capture, runtime or core packages. File-snapshot tests
exercise atomic replacement, in-place changes, locking, truncation and bounded
backpressure independently of native editor tests. Browser fixtures cannot
establish wire fidelity or WebView2 isolation. Inspect the generated populated
screenshots; a passing check does not establish unexercised accessibility,
localization, soak or release qualification.

Current-format save/reopen tests establish persistence, not migration or backward
compatibility. Generated logs, hashes and screenshots remain per-run evidence in
`artifacts` and `target/ui-check`; do not turn this guide into a dated pass list.

## Post-quantum TLS

The ordinary Rust gate exercises real TCP and QUIC handshakes, wire-advertised
groups and signature schemes, classical fallback and TLS 1.2, ML-DSA client
authentication, malformed key shares, fragmented TLS I/O, HelloRetryRequest,
QUIC packet loss and resumed sessions. The full proxy matrix checks independently
negotiated client/upstream algorithms over H1, H2 and H3 origin routes, including
connection reuse and the selected certificate. Capture tests preserve the
negotiated facts through compressed and password-protected files.

To check the downstream algorithm set with an available Chromium browser:

```powershell
python ./scripts/test-pqc-browser.py
# Alternatively, use an installed Chrome or Edge without a version requirement.
python ./scripts/test-pqc-browser.py --channel chrome
python ./scripts/test-pqc-browser.py --channel msedge
```

The runner uses Playwright's bundled Chromium by default. It records the actual
browser version as run evidence; it does not pin or reject a Chromium release.
It verifies automatic selection, each ML-DSA variant, classical authentication
with hybrid exchange, and ML-DSA authentication with classical exchange.
The bounded loopback fixture keeps private keys in memory, writes only its
public root, and uses that root's SPKI allowance in an isolated test browser.
It does not install trust or change host proxy settings. Results are ignored
run evidence at `.local/pqc-browser/results.json`.

To exercise Chrome's ordinary certificate validation, use the isolated Linux
container gate. See [container requirements](building.md#container-validation-requirements)
for Docker, browser runtime and Secret Service prerequisites:

```powershell
python ./scripts/test-pqc-browser-docker.py
```

This gate installs the available stable Chrome without a version requirement
and trusts each ephemeral fixture root in a temporary NSS database. It uses no
certificate-error override, verifies the same six algorithm cases, and requires
Chrome to reject a wrong hostname and an untrusted root. Neither Windows trust
nor host proxy settings change. The public report is written to
`.local/pqc-browser-verified/results.json`. On Linux with Chrome and `certutil`
already installed, `test-pqc-browser.py --channel chrome --trust nss` runs the
same checks directly. Add `--refresh-browser` to rebuild a cached browser image
with the available Chrome release; the report records the version actually used.

An opt-in live site check validates real browser-to-proxy and proxy-to-origin
connections through the production CLI, with certificate verification enabled
on both legs:

```powershell
python ./scripts/test-pqc-browser-docker.py --live-url https://pq.cloudflareresearch.com/
```

The live check records a native capture, reads its connection evidence, and
requires X25519MLKEM768 (`0x11ec`) on both legs over an H2 origin route. It saves
only public negotiation facts to `.local/pqc-browser-verified/live-results.json`;
temporary private keys, browser profiles and captures are removed with the
container. A temporary Secret Service protects the CLI root key using the
production key-protection path. A live endpoint qualifies deployed hybrid key
exchange, while the
controlled fixtures qualify ML-DSA authentication, fallback and rejection cases.
Network or endpoint changes can fail this opt-in gate without implying a local
regression.

When Chromium changes its PQ algorithm set, update the
[shared policy](../crates/proxy-tls/src/algorithms.rs), credential variants and
wire/interoperability expectations together. Requalify the existing BoringSSL
family before considering a dependency change. See the
[negotiation policy](chromium-negotiation.md#algorithms-and-clienthello) for offers
and the [certificate model](certificate-model.md) for the trust boundary and [performance harness](performance.md#post-quantum-tls-handshakes)
for measurements.

## Local Chromium load gate

For repeatable origin-facing TLS, HTTP and TCP fingerprint comparisons across
H1.1, H2 and H3, run `python scripts/compare-chromium.py`. The
[comparison guide](chromium-comparison.md) documents signal coverage, protocol
proof, normalization and limitations. All measurements use isolated local
origins; public fingerprint-checking sites are not involved.

For repeatable browser scale qualification without public sites, run
`python scripts/test-load.py`. The [load-testing guide](load-testing.md) documents
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

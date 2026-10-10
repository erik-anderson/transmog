# Local Chromium load testing

The checked-in [load harness](../e2e/load/run.mjs) sends a real Chromium browser
through the release CLI and Windows desktop proxy, one target at a time. The
default workload completes 100 navigations with 100 verified exchanges each:
10,000 GET/POST pairs plus document, stylesheet, script, image, frame, redirect,
reload and history traffic. Response payloads alone exceed 2 GiB; POST bodies add
another 1.2 GiB. Every payload byte is verified in the browser and local origin.
Reduced smoke runs are explicitly reported as **not scale qualification**.

## Run

Prerequisites are the [native build tools](building.md), Node 24+, Python 3.10+
with its standard library, and WebView2 for desktop runs. Close Transmog first.
The driver builds locked release binaries and installs the locked Playwright
dependencies and Chromium. Dependency setup can download packages; the workload
itself uses only the loopback fixture:

```powershell
./scripts/test-load.ps1 -Smoke
./scripts/test-load.ps1
```

After setup, repeated runs can skip installation and building:

```powershell
./scripts/test-load.ps1 -SkipBuild -SkipDependencyInstall -SkipBrowserInstall
./scripts/test-load.ps1 -Target desktop -Storage memory -SkipBuild -SkipDependencyInstall -SkipBrowserInstall
```

Use `-Python <path>` if Python is not on PATH; a Windows Store alias is not a
Python installation. `PLAYWRIGHT_BROWSERS_PATH` can select an existing Chromium
cache. CLI-only runs also work on Linux. The portable entry point and harness
regression tests are:

```text
node --test e2e/load/harness.test.mjs
node e2e/load/run.mjs --target cli
node e2e/load/run.mjs --target both --smoke
node e2e/load/run.mjs --help
```

`-Navigations`, `-RequestsPerNavigation`, `-Concurrency`, `-TimeoutMinutes` and
`-Output` control workload and evidence. The Node entry point also accepts
explicit CLI/desktop binary paths. Full runs reject configurations below
10,000 exchanges or 2 GiB combined entity bodies. Concurrency is bounded at 32;
the default is eight browser fetch workers. `-Headed` shows the same single
Chromium instance. No browser is restarted between navigations within a target.

## Isolation and routing

The synthetic HTTP site binds an ephemeral IPv4 loopback port. A second
loopback server in front of Transmog accepts only that site's exact origin and
Host header; it rejects other HTTP destinations and all CONNECT requests before
forwarding. It adds a per-run gate marker that the origin requires, so Chromium's
implicit loopback proxy bypass cannot pass. Proxy evidence and retained traffic
provide independent confirmation that Transmog handled the exchanges.
Fixture sockets remain alive until the runner closes them, avoiding Node's
default short idle timeout during this capture/retention workload.

Only the harness's browser launch receives the proxy setting, including
Chromium's explicit loopback-bypass override. Background networking and QUIC are
disabled. The gateway remains part of the measurement path: its streaming and
the browser's byte verification add overhead, so this is a capture/retention
scale test rather than a transport microbenchmark. Local socket attribution
identifies the Node gateway; browser fetch metadata and User-Agent remain intact.

The desktop uses a fresh profile, the ordinary Tauri commands, and real WebView2
with loopback DevTools enabled only for its owned process. Its existing
`configureSystemProxy` preference is set to false before proxy startup. It
creates no system proxy recovery journal. A generated OS-user-protected CA is
used without root installation; traffic is plain local HTTP. The isolated
profile postpones startup update checks before launch. No public site, Docker,
certificate-error bypass, root-store change or user-profile modification is
required. Never attach this probe to a personal desktop profile.

## Gates and evidence

Each target must complete the planned browser workload with exact request and
response lengths and contents, and no browser or origin errors. CLI evidence
must match the origin's completed exchange count. The CLI exits through Ctrl+C
in its own hidden console on Windows, then its native capture must be sealed,
intact, loss-free, contain every exchange and retain exactly both original and
effective payload boundaries. `capture inspect` verifies payload checksums while streaming.

The desktop must retain every completed row without catalog eviction or sequence
gaps. Summed Traffic sizes must equal the origin's entity-byte counters, and the
body buffer must contain twice that amount for original/effective boundaries.
Selected body metadata must be complete. The populated Traffic list must remain
virtualized, select its offscreen final row with End, select every retained entry
with Ctrl+A, and render without JavaScript exceptions.
Exchange count and terminal state are independent gates: losing a start event
can hide an entire exchange without a per-entry gap marker, and losing a final
event can leave a completed request displayed as pending.
The harness saves a native trace, checks its body completeness and integrity,
then clears Traffic and verifies that both the catalog and retained-body count
return to zero. Small smoke captures retain the normal Clear all Undo until
shutdown. Shutdown uses the owned native window's CloseRequested path and must
release its live body-cache files in either profile.

Default desktop retention is unlimited disk storage so machines with modest RAM
can complete the test. `-Storage memory` exercises Automatic retention and rejects
hosts whose resolved budget cannot hold both observed boundaries of the entire
workload. Entry retention remains unlimited in both modes. Allow several GiB of
free disk space for live bodies and saved captures; keep more space for repeated
runs. Bodies are deterministic pseudo-random blocks, not zero-filled fixtures.

Retention and recording use bounded queues that wait for capacity. Under storage
pressure, requests slow down instead of dropping start events, body chunks or
completion events. Traffic-list projection releases the catalog writer lock
between snapshots and formats rows outside that lock. The same zero-loss gates
apply to both storage modes; reducing concurrency or increasing queue capacity
is not a fix for lost evidence. Small regression tests cover saturated event,
body and recording queues, slow list projection, and cancellation/panic handling.
Exercise both desktop storage modes when changing this pipeline. The memory
path can produce events faster than the disk path, exposing pressure that a
passing disk run does not rule out.

Each unique directory under ignored `artifacts/load/` contains `report.json`,
`report.md`, per-target results and logs, one-second process-tree memory samples,
per-navigation progress, native captures, and a populated desktop screenshot.
Failure results include diagnostics and available screenshots. Catalog loss
fails immediately with retained/pending counts. After a completed browser
workload, the desktop also tries to save its failing retained snapshot before
shutdown; that diagnostic trace does not turn a failed qualification into a pass.
Reports record
Git revision/dirty state, binary SHA-256, OS, Node/browser versions, workload,
throughput and fetch p50/p95/p99/max latency. Timings include browser validation;
memory includes the target's descendants (WebView2 for desktop) but excludes the
load-generating Chromium and Node fixture. Windows private bytes are recorded
alongside RSS; Linux records RSS. Shared pages can be counted in multiple
processes, and one-second samples can miss brief peaks. No hardware-independent
memory or latency ceiling is claimed.

Generated top-level CA files are removed at the end. Isolated profiles and
captures remain for failure diagnosis; profiles contain an OS-protected key.
Delete an individual run directory when its evidence is no longer needed.
An interrupted runner closes its control pipe so the process host stops its
owned target. Forced termination is a failed cleanup, never a passing shutdown.

This covers HTTP/1 browser capture, live retention, virtualized presentation,
save, clear and process lifecycle at scale. TLS, H2/H3, encoded content,
WebSockets, encrypted traces and interoperability remain covered by the
[other test gates](testing.md).

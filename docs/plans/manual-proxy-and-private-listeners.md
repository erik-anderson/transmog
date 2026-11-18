# Manual proxy and private MCP listener implementation handoff

Implement a desktop and CLI workflow for debugging an explicitly configured app through a Transmog listener with zero OS proxy-setting writes. Add independently owned MCP listeners so an authorized agent can capture, inspect and record its own traffic while other Transmog captures run concurrently.

The reviewed baseline is commit `9e5aec8aa26e`, inspected on November 17, 2026. This is an implementation plan. File and symbol references describe that baseline and should be checked against the implementing checkout.

## Confirmed product decisions

- The desktop remembers the last selected routing mode. Existing users start with **System proxy**, preserving an already-saved manual choice.
- **Manual proxy** starts the listener for clients that explicitly configure its address. Listener startup, operation and cleanup perform zero OS proxy-setting writes.
- CLI users can select manual routing and either a fixed or automatically allocated port.
- An MCP agent can start its own private capture sessions after receiving a specific permission. Session creation, listener start and recording completion run unattended. Opening a review window is optional.
- The first private-listener release covers capture, inspection and recording. Its capture state and live behavior are independent of the main workspace and other private listeners.
- Existing traffic scope and sensitive-data grants govern inspection and recording access.

## Recommended implementation decisions

The following choices make the plan executable. They are engineering recommendations beyond the user's explicitly confirmed preferences.

- Keep the persisted `configureSystemProxy` boolean and present it as **System proxy** or **Manual proxy**. Expose the effective running mode separately from the saved next-start preference.
- Reuse CLI `serve`, `record --no-system-proxy`, and `--listen`. Give manual recording the same machine-readable bound-address output as `serve`.
- Bind new MCP listeners to IPv4 loopback. An omitted port or port `0` requests an OS-assigned available port. A specified nonzero port must bind exactly.
- Add the permission `collect-private`, labeled **Start private capture sessions**. It authorizes creation, listener lifecycle and recording lifecycle for the key's own private listeners. Inspection and full-evidence recording retain their existing data-access requirements.
- Add a dedicated `transmog_session_listen` tool that atomically creates a private session and starts its listener. Add `transmog_session_listener_stop`. Extend the existing recording tools with an optional `sessionId`, defaulting to `main` for existing callers.
- Reuse the existing four-sessions-per-key, eight-total session limits and 30-minute idle renewal model. Ordinary authenticated session operations and explicit renewal keep the session alive. Incoming target-app traffic alone does not renew agent authority.
- Prepare usable CA material locally for unattended HTTPS decryption. Reuse valid protected desktop CA material when available, otherwise create session-local protected material through the existing CA facilities. Return the public certificate and its fingerprint for client configuration. Certificate trust installation remains an explicit user or client action.
- Route updater requests using normal network configuration when the desktop listener is in manual mode. Explicit selection of Transmog's listener for updater traffic belongs to system-proxy mode.

## Comparable tool conventions

- Fiddler Everywhere exposes **System Capturing** and **Explicit Capturing**, with explicit capture serving manually configured applications. This supports a visible routing choice near capture controls. [Fiddler capturing modes](https://www.telerik.com/fiddler/fiddler-everywhere/documentation/capture-traffic/capturing-modes)
- Charles exposes automatic OS proxy configuration independently of listener ports and supports automatic available-port selection. This supports reusing Transmog's existing fixed or automatic port preference. [Charles proxy settings](https://www.charlesproxy.com/documentation/configuration/proxy-settings/)
- mitmproxy's regular mode starts a listener, asks the operator to configure clients, and treats client certificate trust as a separate setup step. This supports concise endpoint and certificate instructions in manual mode. [mitmproxy proxy modes](https://docs.mitmproxy.org/stable/concepts/modes/)

## Current implementation and entry points

| Area | Baseline behavior and entry points |
| --- | --- |
| Saved routing choice | `Preferences.configure_system_proxy` already defaults to true. Start with [product_state.rs](../../crates/app/src/product_state.rs). |
| Application lifecycle | `Application::start_proxy` accepts optional `HostIntegration`. `ProxyCertificateSettings` records the effective host-integration choice. `Application::stop_proxy` starts a resumable drain. `Application::shutdown` provides terminal cleanup. Inspect [lib.rs](../../crates/app/src/lib.rs), [lifecycle.rs](../../crates/app/src/lifecycle.rs), and [service.rs](../../crates/proxy-session/src/service.rs). |
| Desktop host | `start_proxy_locked_with_mode` derives address and port from preferences and supplies the host adapter conditionally. `validate_proxy_start` blocks on host recovery for both modes. Startup and exit also call recovery. Inspect [windows.rs](../../apps/desktop/src/windows.rs) and [host-windows/lib.rs](../../crates/host-windows/src/lib.rs). |
| Desktop presentation | `startProxyNow` checks Windows CA trust and emits system-routing messages unconditionally. `stopProxyNow` assumes host restoration. The toolbar already copies the listener address. Inspect [settings-workspace](../../apps/desktop/ui/src/settings-workspace/settings-workspace.ts), [proxy-toggle](../../apps/desktop/ui/src/proxy-toggle/proxy-toggle.ts), and [app-shell](../../apps/desktop/ui/src/app-shell/app-shell.ts), including their HTML and CSS. |
| Preferences concurrency | General settings saves load a product snapshot, modify it, and save it. A new narrow routing save must integrate with this path to preserve newer routing selections and existing settings drafts. |
| CLI | `serve` already starts a manual listener and emits `LISTEN_ADDR`. `record --no-system-proxy` chooses host-free service startup, but `record` calls `recover_proxy` earlier. Inspect [main.rs](../../crates/proxy-cli/src/main.rs), [support.rs](../../crates/proxy-cli/src/support.rs), and [help.rs](../../crates/proxy-cli/src/help.rs). The Cargo package is named `transmog`. |
| Private application state | `create_inspection_session` creates a new application, copies product state, forks composer state, and links body storage and `live_parent` to the main application. A private listener needs a construction path with independent live resources. Inspect [inspection_sessions.rs](../../crates/app/src/inspection_sessions.rs). |
| MCP lifecycle | Private records, expiry, review handles and imports live in [application/sessions.rs](../../crates/mcp/src/application/sessions.rs). Removal and maintenance are currently synchronous. Main control and recording cleanup live in [application.rs](../../crates/mcp/src/application.rs). |
| MCP API routing | `catalog/sessions.rs` adds required `sessionId` to an explicit list of inspection tools. Other operations generally dispatch to the root adapter. Inspect [catalog.rs](../../crates/mcp/src/catalog.rs), [catalog/sessions.rs](../../crates/mcp/src/catalog/sessions.rs), and [catalog/models](../../crates/mcp/src/catalog/models.rs). |
| MCP authority | Permissions, scope and disclosure checks live in [access.rs](../../crates/mcp/src/access.rs). The create/edit-key UI owns another permission union, label map and form mapping in [mcp-settings](../../apps/desktop/ui/src/mcp-settings/mcp-settings.ts). |
| Desktop MCP bridge | `DesktopProxy::start` currently invokes the main desktop start path. Inspect [mcp.rs](../../apps/desktop/src/mcp.rs). Connector-driven desktop launch lives in [connector.rs](../../crates/mcp/src/connector.rs). |
| Background requests | `active_proxy` and `update_client` currently select any running main listener. Inspect [updates.rs](../../apps/desktop/src/updates.rs) and [update_traffic_tests.rs](../../apps/desktop/src/update_traffic_tests.rs). |

## Implementation sequence

### 1 Establish routing state and recovery ownership

Use the existing optional host integration as the runtime boundary. A manual listener starts through `service.start(server)` with host integration absent. Add an effective routing field to application status, derived from the admitted run. Carry it through serialization, desktop bootstrap, TypeScript models, initial render state and fixture models. Use the saved preference only when describing the next fresh start.

Audit every host-mutation entry point before adding the UI selector.

1. Desktop launch recovery in `windows.rs`.
2. `validate_proxy_start` and the UI's `hostRestorePending` gate.
3. Resume and stop paths, including pending restore tokens owned by the current service.
4. Normal exit, update handoff, failure cleanup and fallback `recover_for_exit` calls.
5. CLI preflight recovery, `start_owned_proxy`, Ctrl+C handling and root cleanup commands.
6. Connector `--start-app` launch, where the desktop may launch solely to serve private MCP operations.

Apply these ownership rules.

- A manual-only desktop or CLI run inspects recovery state read-only. Preserve an abandoned journal and expose **Restore settings** as an explicit action. Allow a fresh host-free listener to operate alongside that journal or another process's active system-proxy owner.
- Read the saved mode before desktop startup recovery. A launch using the saved manual choice defers abandoned-journal recovery. Normal system-mode recovery continues to use the existing ownership-aware implementation.
- Use the connector's existing `--mcp-start` launch argument to defer abandoned-journal recovery until an explicit recovery action or system-proxy start. Detect it before startup recovery and keep that intent independent of the persisted routing preference. Preserve the existing single-instance activation behavior and verify both first launch and attachment through the installed CLI.
- Manual-only exit performs listener and recording cleanup. Gate the abandoned-journal fallback so that this exit also performs zero proxy-setting writes.
- A real host transaction acquired earlier in the same process retains its exact restore obligation. Track that ownership independently of a subsequently saved manual preference. A failed restoration owned by the current service must finish before replacing that service's run.
- System-proxy startup continues to recover eligible interrupted owners before acquiring new ownership. Preserve the existing protection for subsequent VPN, bypass and external proxy edits.

Document the recovery change explicitly. Manual operation may surface a recoverable historical Windows routing problem while continuing to listen. Its status should distinguish that historical problem from the current listener's health.

Implement and test this boundary first. The important assertion is the host adapter's write/apply/restore call count, supplemented by Windows state snapshots. Equal before-and-after values alone cannot prove zero writes.

### 2 Expose manual routing in the desktop

Add one compact routing selector adjacent to the existing start/stop control, using the existing toolbar or proxy-status menu. Display **System proxy** or **Manual proxy** in the toolbar so the choice is discoverable before starting. Keep the main **Start proxy** action direct.

- Persist a routing selection immediately with a narrow backend update that merges into current product state. During save, keep the selection pending until persistence succeeds and retain a useful error on failure.
- Reconcile this update with general Settings saves so stale snapshots and delayed replies cannot overwrite the newer routing choice. Exercise both write orders. Preserve unrelated Settings drafts and layout updates.
- Saving a different mode during a run sets the next fresh start. Show the active mode and a concise next-start indication when they differ.
- **Start proxy** during **Finishing requests** resumes the admitted run with its original mode and port. A new routing choice takes effect after that run completes.
- Reuse the existing automatic/fixed port setting. Display and copy the actual bound address. For a wildcard listener, provide a locally usable loopback address and the existing remote-client guidance.
- Manual start progress should describe listener binding. Running feedback should give the endpoint and tell the user to configure the target application's HTTP and HTTPS proxy. Stop progress should describe finishing requests.
- Manual HTTPS decryption requires valid CA material. Gate the Windows trust requirement on effective system routing. Provide public-certificate export and the existing HTTPS tunnel choice so an app with its own trust store can be configured.
- Keep certificate creation and system trust installation independently callable. Check initial setup notices and the **Start with HTTPS tunnels** path, including their saved settings and focus behavior.
- Populate the initial renderer, hydrated state, keyboard path, tooltips, accessibility names and async status refreshes from the same effective routing state.

Update updater routing using the admitted run's mode. In manual mode, let the HTTP client use normal system configuration. In system mode, continue selecting the active Transmog endpoint and its interception trust as required. Verify checks, redirects and downloads after mode changes and proxy restarts.

### 3 Complete the CLI manual workflow

Keep the existing command forms and argument validation. These examples should work through the packaged binary.

```powershell
# HTTP inspection and HTTPS tunnels, with a fixed listener port.
.\transmog-cli.exe serve --listen 127.0.0.1:8888 --no-decrypt-https

# Decrypted manual proxy using explicitly prepared CA material.
.\transmog-cli.exe serve --listen 127.0.0.1:8888 --ca-cert .\transmog-ca.pem --ca-key .\transmog-ca.key

# Unattended manual recording with an automatically allocated port and HTTPS tunnels.
.\transmog-cli.exe record --output .\app.tmcap --no-system-proxy --no-decrypt-https --listen 127.0.0.1:0
```

- Gate `record` preflight proxy recovery on system-routing intent. A manual run must reach host-free startup even with an abandoned journal.
- Emit `LISTEN_ADDR=<actual address>` after successful bind for both commands. Preserve existing output consumed by scripts and keep diagnostics compatible with their current streams.
- Print manual client-configuration instructions on Windows as well as other platforms when manual mode is active.
- Document `--no-install-root` independently from `--no-system-proxy`. The latter governs proxy settings. The former governs certificate trust setup. Verify their supported combinations and error timing.
- Keep fixed-port bind failures actionable and release partial recording/listener resources. Port `0` uses the bind result directly.
- Ctrl+C must stop the owned listener and finalize the capture. Preserve recoverable evidence on writer failure and respect the command's existing shutdown contract.

### 4 Construct independent private capture applications

Introduce a focused application factory for a private live listener, reusing common initialization where useful. Give the resulting application its own session service, catalog, runtime IDs, body retention, recording manager and live-control state. Start with empty active automation and scripts.

Inspect `create_inspection_session` closely before reusing it. Its `live_parent`, body-store live-source link and composer fork support shared-traffic review. The listener factory must construct its own live resources and outbound pools. Read-only immutable configuration and public CA material may be reused deliberately.

Capture a validated network/trust configuration snapshot at session creation. Set manual routing, local loopback binding and session-owned storage explicitly. Initialize process inspection to All so the listener can capture the applications explicitly connected to its address. Apply the key's traffic scope independently at the MCP data-access boundary. Retention preferences should use existing product limits with aggregate accounting across private listeners, including response bodies, catalogs and in-flight requests. Reserve capacity atomically before exposing a session and release it after cleanup.

Use the proxy runtime directly through the application/service layers. The desktop bridge should provide validated CA and network inputs for the target session without invoking main-workspace start or mutating main preferences. Keep the permission type in the MCP layer.

For decrypted operation, prepare CA material under the existing certificate-operation synchronization. Return a public PEM path and fingerprint. Use protected key handling internally and keep public certificate metadata valid for the advertised lifetime. Retain the admitted CA identity/material for the private run so later desktop certificate replacement cannot change its signer or advertised certificate. For `decryptHttps: false`, start the existing HTTPS tunnel path with session-local preferences and return null certificate metadata. Setup failures return a structured error and release the session reservation.

Qualify caches, contexts, operation results and evidence lookups by session identity. Ensure private listener traffic receives updates from its own service. Check terminal events, body flushes and traffic-wait notifications. A root live-traffic fanout must visit only sessions subscribed to that shared source.

Validate session kind for shared live subscriptions and live-rule operations before dispatch. The initial listener kind supports its own capture and inspection pipeline. Return an unsupported-session-operation error for attempts to attach it to shared live traffic or execute shared live rules through it. Verify that generic tool routing cannot fall back to the main application for these private targets.

### 5 Add the permission and MCP contract

Add `Permission::CollectPrivate`, serialized as `collect-private`, throughout key creation, editing, storage, status, help, catalog descriptions and desktop labels. Older keys acquire it only through an explicit permission edit. **Allow all** includes it when the user selects or saves that choice.

Use the following proposed API contract, aligning names and schema helpers with repository conventions during implementation.

| Operation | Inputs and behavior |
| --- | --- |
| `transmog_session_listen` | Required `instanceId` and `operationId`. Optional `name`, integer `port` from 0 through 65535, and boolean `decryptHttps`, defaulting to the validated saved HTTPS preference. Requires `collect-private`. Reserve a session, prepare configuration, bind, publish, and return the actual endpoint. |
| `transmog_session_listener_stop` | Required `instanceId`, owned `sessionId`, and `operationId`. Stop the listener and finalize its active recording through bounded cleanup. Retain the session's evidence for subsequent inspection. |
| `transmog_session_list` and `transmog_session_renew` | Extend summaries with session kind, listener lifecycle, effective endpoint, HTTPS mode and recording state. Renewal follows existing expiry semantics. |
| `transmog_recording_start` and `transmog_recording_stop` | Optional `sessionId` defaults to `main`. Dispatch explicit private targets to their owning adapter and recording manager. Private recording lifecycle uses `collect-private`. Main recording lifecycle uses `collect`. |
| `transmog_session_close` | For a listener session, stop and finalize owned work before ending access and disposing eligible evidence. Retain user-authored dirty review state through the existing preservation mechanism. |
| Existing inspection tools | Use the returned private `sessionId` with existing traffic, exchange, header, body, stream and context operations. Recheck ownership, scope and disclosure on every read. |

An illustrative successful start result is below. Field names should be reflected consistently in output schemas, fixtures and help.

```json
{
  "sessionId": "opaque-session-id",
  "kind": "listener",
  "operationId": "app-debug-1",
  "lifecycle": "running",
  "listener": "127.0.0.1:54321",
  "proxyUrl": "http://127.0.0.1:54321",
  "routingMode": "manual",
  "decryptHttps": true,
  "certificate": {
    "publicPemPath": "<session-public-certificate-path>",
    "sha256": "<public-certificate-fingerprint>"
  },
  "expiresAt": 1794950000
}
```

Make the operation contract retry-safe.

- Scope operation identity by MCP instance and owning key. Reserve it before asynchronous work and retain the resolved defaults with the operation. Identical retries return the same session and bound port even if desktop settings changed meanwhile. Different inputs with the same ID return a conflict.
- Two concurrent start retries publish one listener. Two distinct starts requesting one fixed port produce one bind success and one actionable bind failure. Bind directly to port `0` for automatic allocation.
- Recheck current authority after CA preparation and bind. Revocation during startup cancels publication and cleans up the just-created resources.
- A delayed retry for an already-ended session returns its recorded terminal outcome. A new operation ID requests a new listener.
- Tie async stop results to the session and listener generation. If cleanup outlasts the tool response window, use the existing tracked-operation polling model. Ensure polling and cancellation authorize the listener operation through its owner and private-collection grant. Report success after recording finalization and port release have completed.
- Add schemas, argument validation, output models, annotations, discovery tests and help together. Handle optional recording `sessionId` explicitly in dispatch because the current inspection-tool decorator makes it required.

Enforce the new permission only where it grants authority. A key with `collect-private` can manage its own listener, while shared proxy controls continue checking `collect`. `Inspect` governs reads. Full native recording continues to require all-traffic scope and effective body/credential access, since the writer preserves complete session evidence. A missing data grant should return the existing authorization error directly. An agent with the requisite grants can complete the whole flow unattended.

Keep exact-host and selected-request read filtering effective for private evidence. The new lifecycle permission does not convert a selected-request scope into access to future requests. Recordings must pass the full-evidence scope check before opening their output file.

### 6 Implement asynchronous cleanup and durable recordings

Private session removal currently releases inspection state synchronously. Add owned asynchronous cleanup for listener sessions and integrate it into `maintain_sessions`, `reconcile`, `end_all`, session close, MCP disablement and desktop exit/update handoff.

- Mark closing state before awaiting so new operations cannot race a closing session. Snapshot registry entries under short locks, then perform shutdown outside registry locks.
- Serialize operations per private session. A slow private recording stop must allow unrelated sessions and shared collection to progress. Review root/private lock ordering in review publishing and management paths.
- Listener stop needs a terminal, bounded service cleanup path. The desktop's resumable `begin_drain` can wait for long-lived traffic indefinitely. Use the service's existing bounded shutdown facilities and preserve terminal evidence for canceled requests, tunnels and WebSockets.
- Key expiry, permission removal, MCP disablement and app exit stop every affected private listener, seal owned recordings, release ports and retain actionable cleanup failures. Reuse existing retry/diagnostic behavior for failures.
- Revalidate grants on every operation and in maintenance. Losing `collect-private` stops that key's private listeners. Losing recording disclosure authority seals the affected recording according to existing recording rules. A review window can retain evidence, while expired authority still terminates its listener.
- Keep connector disconnection consistent with the existing key-owned session model. A transient client disconnect can leave the session available until its existing idle deadline. Explicit renewals and `expiresAt` let an unattended agent manage long jobs.
- Distinguish stopping capture from discarding retained evidence. `listener_stop` keeps the session inspectable. `session_close` ends access and releases its ordinary retained evidence. Optional human edits can keep a read-only backend available after listener termination. Capturing traffic alone must allow unattended session closure.
- Store agent recordings in a durable export location outside the session's temporary directory. Include session identity in ownership metadata. Session disposal must preserve sealed output and any recoverable prefix after writer failure. Return artifact metadata through the existing ownership and disclosure conventions.
- Prevent partial failures from losing ownership. If bind, recording start, response serialization, cancellation or shutdown fails, retain enough operation state to report the outcome and complete cleanup exactly once.

### 7 Finish the desktop management view and documentation

Extend existing MCP session rows with the listener endpoint, lifecycle and recording state. Show **Copy proxy address** and **Stop listener** when applicable. Keep **Review** optional and use the existing review-window backend when explicitly opened. Include private listeners in meaningful MCP activity/status summaries without showing them as the main proxy's routing state.

Document the resulting workflows and intentional conventions in these files.

- [CLI guide](../cli.md) and [support capture guide](../cli-support-capture.md).
- [Desktop lifecycle](../product-shell.md), [network settings](../network-profiles.md), and [persisted product state](../product-state-and-support.md).
- [MCP guide](../mcp.md), [MCP reference](../mcp-reference.md), and built-in [MCP help](../../crates/mcp/src/help.rs).
- [Application session service](../application-session-service.md) where the new ownership model changes its explanation.
- [UX principles](../ux-principles.md) for remembered routing, active versus next-start state, and unattended private collection.
- [Testing guide](../testing.md) for new focused checks and native scenarios.

Include one agent example that creates a listener, configures a local test client, starts recording, inspects captured requests, stops and seals the recording, stops the listener, then closes the session. Include automatic-port output and public-certificate handling. Make that example an executable integration fixture as well as documentation.

## Acceptance matrix

| Scenario | Required observable outcome |
| --- | --- |
| Legacy preferences | Missing routing preference uses System proxy. Previously saved false uses Manual proxy. Mode survives restart. |
| Concurrent settings saves | Narrow mode selection and general Settings/layout saves preserve each other's newer values in either completion order. |
| Active and next-start mode | Saving another mode leaves the current run's mode visible. Resume uses the original run. The next fresh start uses the saved choice. |
| Manual host boundary | Start, failed start, operation, stop, resume and exit produce zero host proxy writes. Exercise existing proxy configuration, active external ownership and stale journals. |
| System host ownership | Start/stop restores the exact owned transaction. Existing recovery, external-edit preservation and pending-restore retry tests pass. |
| Manual HTTPS | Decrypt using a client that trusts the public CA while Windows lacks the managed root. Tunnel mode starts with absent CA material. |
| Port selection | Port 0 returns the bound address. Fixed ports bind exactly. Occupied/invalid ports fail before session publication and release reservations. |
| Private-only authority | A key with private collection permission and required data grants completes the entire workflow with no UI interaction. Shared start/stop calls require their existing permission. |
| Three concurrent captures | Main plus two private listeners capture distinct tagged requests sent to the same origin. Each catalog, body read, stream and recording contains only its own exchanges. |
| Rules and network state | An enabled main rule or script affects main traffic only. Private response bytes follow the private runtime configuration. Private outbound pools and updates remain independent. |
| Read boundaries | Other keys cannot address private session evidence or contexts. Host and selected-request scopes, redaction and expired disclosure still apply on every page. |
| Retries and races | Concurrent identical starts produce one port/session. Conflicting operation IDs return conflicts. Revocation during bind publishes no usable listener. A late stop cannot terminate a replacement run. |
| Stop and finalization | Stop with active HTTP, HTTPS, HTTP/2, streams, tunnels and WebSockets completes within the existing bounded shutdown policy, reports terminal evidence, releases the port and finalizes output. Other listeners continue. |
| Expiry and revocation | Idle expiry, key expiry/revocation, loss of permission, MCP disablement and app exit clean up owned listeners. Optional review/dirty state preserves evidence without extending revoked listener authority. |
| Artifact lifetime | A sealed recording remains readable after session closure. Writer failure leaves an identified recoverable prefix and a failed outcome. |
| Resource limits | Concurrent creation respects existing session limits. Private capture retention stays within aggregate configured budgets and releases its allocations after cleanup. |
| Updater traffic | Manual-mode capture stays free of automatically injected update requests. System-mode update checks/downloads retain their established behavior across restart. |
| Desktop interaction | Pointer and keyboard flows work at 1440×900, 960×720 and 800×600. Check long endpoints/names, zoom, forced colors, menu Escape/focus return and delayed status/save replies. |

## Verification instructions

Read the current root [AGENTS.md](../../AGENTS.md), [UI instructions](../../apps/desktop/ui/AGENTS.md), [UX routine](../ux-quality.md), and [UX principles](../ux-principles.md). Resolve and read the installed WebUI `ai.md` from `apps/desktop/ui` before changing components. Review the working tree before edits and preserve concurrent work. At plan creation, traffic navigation and refresh files plus `docs/traffic-inspection.md` and `docs/ux-principles.md` contained unrelated uncommitted changes.

Use Python 3.11 or newer and run the environment check before builds. In the planning environment, the first `python` on PATH was a non-executable Windows Store alias. `C:\Users\erik\AppData\Local\Python\bin\python.exe` executed Python 3.15.0 successfully. Resolve a working interpreter in the implementing environment, substitute its command for `python` below, and use `TRANSMOG_PYTHON` for wrappers where needed. From the repository root, prepare tooling and run the required root checks.

```powershell
python scripts/dev-env.py --check
npm ci --ignore-scripts
npm run check
```

From `apps/desktop/ui`, run the UI and browser gates after extending the relevant fixtures.

```powershell
npm run check
npm run test:workspaces
node scripts/check-workspaces.mjs --mcp-only
```

Use `check-proxy-recovery.mjs`, the proxy/settings fixtures and the MCP fixture helpers as starting points. Add regression cases for the ownership, unattended lifecycle and races in the matrix. Keep production templates and enforced Trusted Types in the browser path.

From the repository root, run affected Rust tests and strict Clippy. Include additional packages if implementation expands their scope.

```powershell
python scripts/dev-env.py --run cargo fmt --all -- --check
python scripts/dev-env.py --run cargo test --locked -p transmog-mcp -p transmog-app -p transmog-session --all-features
python scripts/dev-env.py --run cargo test --locked -p transmog -p transmog-host-windows -p transmog-app-webui -p transmog-desktop
python scripts/dev-env.py --run cargo clippy --locked -p transmog-mcp -p transmog -p transmog-host-windows -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop --all-targets -- -D warnings
```

Run and review the main workspace unused-dependency audit. Preserve and restore any preexisting `CARGO_LOG` value when using this diagnostic setting. Apply the documented SDK/fuzz audits if those workspaces are affected. Review feature-only and dev-dependency candidates before removing anything. [Unused-dependency audits](../building.md#unused-dependency-audits)

```powershell
$env:CARGO_LOG = 'cargo::diagnostics::rules::unused_dependencies=debug'
python scripts/dev-env.py --run cargo check --workspace --all-targets --all-features --locked
```

Build the desktop, CLI and required helpers together, then run the real WebView2 harness and connector checks in an isolated profile.

```powershell
python scripts/build-desktop.py --configuration Debug --locked-dependencies
./scripts/test-windows-desktop.ps1 -SkipReleaseBuild -ExecutablePath ./target/debug/transmog.exe
python scripts/test-mcp-startup.py --executable target/debug/transmog-cli.exe --artifacts artifacts/mcp-startup
python scripts/test-mcp-lifecycle.py --executable target/debug/transmog-cli.exe --artifacts artifacts/mcp-lifecycle
python scripts/dev-env.py --run cargo test --locked -p transmog-desktop --test proxy_recovery_task -- --ignored
python scripts/test-interop.py
```

Extend the native harness to exercise the new routing selector, installed connector launch intent, actual listener ports and unattended private recording. Use the existing harness prerequisites and owned-process cleanup. Keep screenshots, protocol transcripts, write-count assertions and capture-validation results in generated evidence. The zero-write checks should use instrumented host boundaries in addition to native state snapshots.

Before finishing, review the final diff, execute the complete workflows through their ordinary desktop/CLI/MCP entry points, and fix failures. Rerun affected checks after fixes. Report the tested commit or working-tree state, check results, evidence paths and any concrete verification limits. Link the updated user-facing guides and describe the new permission and existing-key migration behavior in the completion summary.

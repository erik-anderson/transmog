# Manual proxy UX and HTTPS trust

## Handoff

Make it possible to choose Manual proxy, configure one app, and tell whether its HTTPS traffic can be decrypted. Give CLI users accurate shutdown messages and MCP users accurate permissions, recording outcomes and session-lifetime guidance.

Start from the working tree containing the manual proxy and private listener implementation. At review time, HEAD was `c0751b32` and the feature included uncommitted and untracked files. A checkout of that commit alone is insufficient. Verify that `transmog_session_listen` and the desktop routing selector exist before editing. The [original feature plan](manual-proxy-and-private-listeners.md) describes the earlier work. This follow-up addresses the seven findings from the UX review.

## Product decisions

- **Routing** determines how traffic reaches the listener. In Manual proxy, the user configures the target app. Starting, stopping and closing a manual-only run perform zero OS proxy-setting writes. Preserve the remembered routing choice, the System proxy default for existing users without a saved choice, and the direct **Start proxy** action.
- **HTTPS decryption** requires a usable local interception CA and a client that accepts the certificates it signs. Many Windows apps use the Windows trust store. Other apps use their own CA store or bundle. Transmog can check its CA files and the exact root in the current user's Windows store. Neither check proves that a particular app accepts the root. Plain HTTP and opaque HTTPS tunnels need no interception root.
- Offer **Trust for my Windows account…** and **Copy public certificate path** from the same certificate setup surface in manual mode. These are proposed action labels. The Windows action uses the existing consent flow and grants trust for this account to apps that use the Windows store. Manual Start can proceed with a usable CA when the app manages its own trust. Keep trust setup accessible after Start and let the user retry a denied installation or use an app-specific store.
- **MCP** keeps its existing grants and unattended operation. A decrypted private listener returns its own public certificate path and SHA-256 fingerprint. An agent with the appropriate grants can use a disposable client configured for that certificate without a desktop review or Windows trust prompt.

## Checkpoint 1 — Desktop routing and certificate setup

**Entry points.** Trace `initializeShell`, `applyProxyRouting`, `updateCertificateState`, `setupCaNow` and `startProxyNow` in [Settings](../../apps/desktop/ui/src/settings-workspace/settings-workspace.ts). Follow notice ownership and status rendering in the [shell](../../apps/desktop/ui/src/app-shell/app-shell.ts) and [toolbar template](../../apps/desktop/ui/src/app-shell/app-shell.html). Use the existing bootstrap, CA identity checks, certificate-operation lock and current-user installation path in the [desktop host](../../apps/desktop/src/windows.rs).

1. Make the toolbar button's accessible name include the visible mode, for example **Manual proxy, routing and status**. Label the picker **Routing for next start**. Before a fresh manual start, show “Set the HTTP and HTTPS proxy in the app you want to inspect.” Keep the running mode visible when the saved choice differs. During a drain, resuming uses the existing mode and port. A saved choice takes effect on the next fresh bind.
2. Derive the certificate notice, Connection status and available actions from the same facts. Recompute after a successful routing save, startup, CA creation/reset/removal, trust installation or cancellation, and a saved decryption change. Use the active mode for a running listener and the saved mode for a fresh start. Scope updates to certificate-related notices. Preserve unrelated recovery and error messages, dismissal and keyboard focus. A stale response must not restore older copy or enable Start while the routing save is pending.
3. Put the two trust actions beside the public certificate in the existing [Connection panel](../../apps/desktop/ui/src/settings-workspace/settings-workspace.html). Copy the verified managed public PEM path. Keep the private key out of client setup instructions and copy actions. Use the existing exact-certificate installer and refresh its trust result even after cancellation or failure. Keep prepared material available for retry or app-specific use.

| Desktop state | What the user should see and be able to do |
| --- | --- |
| Manual, decrypting, CA missing or unusable | **Create interception certificate** or the existing explicit reset action. Explain that the target app must trust the new public root. Offer **Start with HTTPS tunnels**. Preserve the existing reset disclosure about the old certificate. |
| Manual, usable CA, Windows trust absent | Start the listener. Offer **Trust for my Windows account…** for apps that use Windows trust and **Copy public certificate path** for apps with their own store. “Certificate ready” describes the local material. |
| Manual, usable CA, Windows trust present | Show **Trusted for this Windows account**. Keep the public certificate path available to apps with separate trust settings. |
| System, decrypting | Preserve the existing managed-root and current-user trust requirements. A canceled trust prompt leaves a retry path. |
| Either routing mode, decryption off | Start without CA setup. Explain that HTTP remains inspectable and HTTPS contents stay encrypted. Choosing tunnels saves the decryption choice. |

**Review gate.** Reproduce the reported case in both directions with an existing CA trusted only by a test client. Switch System → Manual → System before Start. After each switch inspect the banner, actions and accessible name. Then start in Manual and save System as the next mode. Exercise delayed saves and a failed save. Use a local HTTPS origin already trusted upstream by Transmog and by the tunnel test client. Verify a decrypted request from a client that trusts the public PEM, certificate rejection from a client lacking that root, and successful tunnel forwarding without an interception CA. Verify manual startup makes zero implicit trust-install calls and zero OS proxy-setting writes. Use a disposable Windows account or a controlled trust-store fixture for explicit installation, denial and retry.

## Checkpoint 2 — Usable addresses and accurate CLI output

**Entry points.** Compare `applyStatus` and `copyListener` in the shell with both manual success messages in Settings. Trace `start_owned_proxy`, `wait_for_stop` and the final summary in the [recorder](../../crates/proxy-cli/src/support.rs), plus bound-address output in [serve](../../crates/proxy-cli/src/main.rs).

1. Format the address from the actual bound socket after port allocation. Use one desktop formatter for the toolbar, clipboard, popover, Connection panel and tunnel-start status. For a wildcard bind, show loopback and the actual port for a client on this computer. For remote clients, explain to use this computer's reachable LAN address and the same port. Preserve concrete interface addresses and bracket IPv6. Keep a raw bind address available wherever it is reported as a diagnostic. Give the CLI the same human-facing guidance and preserve `LISTEN_ADDR` as the actual bound address for scripts.
2. Carry the admitted run's host-integration ownership through shutdown reporting. Manual recording should say **Finishing active requests and saving the recording** for both the first Ctrl+C and duration expiry. Report the actual save outcome. Show restore progress, completion or pending recovery only for a run with a host transaction. Preserve second-interrupt behavior and failure reporting.
3. In CLI help and guides, distinguish `record --no-system-proxy`, `--no-install-root`, and `--no-decrypt-https`. A Windows user can choose manual routing and explicitly install the public root. A client with its own store can use the public root printed during manual certificate setup. Keep `serve` and its explicitly supplied CA workflow accurate.

**Review gate.** Exercise loopback, IPv4 and IPv6 wildcard, concrete interface and auto/fixed port cases in formatter or fixture tests. Verify exactly what gets copied. Run a one-second `record --no-system-proxy --no-decrypt-https --listen 127.0.0.1:0 --duration 1` into a fresh output path. Assert a usable local endpoint, a sealed recording and accurate stop text. Exercise Ctrl+C independently and test owned system restoration success and failure with the existing host adapter. Extend the [manual CLI regression](../../crates/proxy-cli/tests/manual_proxy.rs).

## Checkpoint 3 — MCP permission authoring and recording results

**Entry points.** Compare the [key editor](../../apps/desktop/ui/src/mcp-settings/mcp-settings.ts) with [listener authorization](../../crates/mcp/src/application/sessions/listener.rs) and [recording authorization](../../crates/mcp/src/application.rs). The [session summary](../../crates/mcp/src/application/sessions.rs) already provides `recordingState` and `recordingArtifact`. The [agent access panel](../../apps/desktop/ui/src/mcp-indicator/mcp-indicator.html) currently displays only the active-recording flag.

1. Enable **Start private listeners** for **All traffic** and **Specific hosts**. Disable it for **Selected requests** and explain “A new listener needs All traffic or Specific hosts.” Have **Allow all** operate on eligible controls. If the user narrows a draft, clear the ineligible selection visibly and require an explicit choice to re-enable it after widening scope. Handle existing keys when viewing or editing and keep unrelated permissions intact. Opening a dialog must not rewrite a grant. Retain server-side enforcement on every operation.
2. Under the permission, use “Start a separate proxy for a local app. Saving a recording requires All traffic and unredacted access to authentication headers and bodies.” Use the existing redaction control and effective body/credential checks. Keep **View request details** as the inspection grant and **Control collection** as shared-proxy authority. Authoring a private listener key must never silently widen scope or turn off redaction.
3. Read the recording state and artifact name into the panel model. Show **Recording** when active, **Recording saved** when sealed and **Recording failed** on failure. On failure with a retained file, identify it as a recoverable recording prefix. A failure without an artifact must still be visible. Keep idle rows quiet. Render a nonzero file count as **1 imported capture** or **N imported captures** independently of live traffic. Keep filenames readable at narrow widths.

**Review gate.** Exercise create, edit, **Allow all**, scope changes in both directions, default redaction and an existing incompatible key. Verify a host-scoped key can start a listener and an all-traffic key with the required data grants can finish recording unattended. Render saved and failed outcomes side by side, including failure without a filename, and stop a running listener from the panel. Confirm that main traffic and a second private listener continue.

## Checkpoint 4 — Lifecycle guidance and final review

1. Update the [MCP guide](../mcp.md), [reference](../mcp-reference.md) and relevant [tool descriptions](../../crates/mcp/src/catalog/sessions.rs). **Stop listener** releases the port, finalizes active recording and keeps session traffic inspectable. **End session** also ends agent access and releases ordinary session evidence. Durable recording files survive. Describe preserved human review work separately.
2. Explain the 30-minute idle deadline. Opening an offline inspection session for review keeps that session available. A private listener still expires during review. Incoming app traffic, session listing and operation polling do not renew its lease. Point long-running agents to `transmog_session_renew` and `expiresAt`. Link the example that configures the target client with the private listener's public CA.
3. Align the [desktop guide](../product-shell.md), [network settings](../network-profiles.md), [certificate lifecycle](../certificate-model.md), [CLI guide](../cli.md) and [support capture guide](../cli-support-capture.md) with the final labels and trust actions. Check the reset wording in the certificate guide. Put the enduring routing and trust convention in the [UX principles](../ux-principles.md).

At each checkpoint, reproduce the original issue, make the change, run the focused regression, then walk the normal entry point and fix any remaining finding. For the final walkthrough, have a reviewer use the visible labels to configure one app, complete trusted HTTPS capture, stop a recording and find its outcome. Verify keyboard operation, Escape and focus return at 1440×900, 960×720 and 800×600. Capture before/after evidence for the setup notice, address and recording outcomes. The original screenshots can be regenerated from the scenarios above if the local `target/ui-check/listener-ux-review-*.png` files are absent.

Follow the [repository instructions](../../AGENTS.md), [desktop UI instructions](../../apps/desktop/ui/AGENTS.md) and [UX routine](../ux-quality.md). Run the following checks from the repository root with a working Python 3.11+ interpreter.

```powershell
python scripts/dev-env.py --check
npm ci --ignore-scripts
npm run check
npm --prefix apps/desktop/ui run check
npm --prefix apps/desktop/ui run test:workspaces
python scripts/dev-env.py --run cargo fmt --all -- --check
python scripts/dev-env.py --run cargo test --locked -p transmog -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop -p transmog-host-windows -p transmog-mcp --features transmog-mcp/desktop
python scripts/dev-env.py --run cargo clippy --locked -p transmog -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop -p transmog-host-windows -p transmog-mcp --features transmog-mcp/desktop --all-targets -- -D warnings
```

Run the [unused-dependency audit](../building.md#unused-dependency-audits) for affected Rust workspaces. Use the production-template proxy recovery and MCP Playwright fixtures for focused regressions. Verify the desktop trust and routing flows in native WebView2 using the [Windows harness](../../scripts/test-windows-desktop.ps1) and an isolated profile. Report which states used real clients, which used fixtures, and any checks still open.

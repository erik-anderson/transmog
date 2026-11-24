# Local MCP agent access

Let agents inspect request/response context, control capture, send requests and
collaborate on traffic rules. MCP is off by default and accepts only local
loopback connections.

Agents can filter and search traffic, inspect WebSocket/SSE messages, extract JSON
fields and prepare replay drafts without sending them. Body search, extraction
and stream payload reads follow the same unredacted-access setting as body reads.

## Connect an agent

1. Open **Settings → Integrations** in the main desktop window and enable
   **Allow MCP connections**.
2. Choose **Create access key…**. Name the agent and choose its permissions,
   scope and expiry. Scope defaults to **All traffic in the main workspace**;
   exact hosts and selected requests are also available. Choose **Control
   collection** to let the agent start capture, or **Allow all** to select every
   operation without changing redaction.
3. Copy the generated TOML or JSON configuration into the agent's MCP settings,
   then reload its MCP connections. Preserve the CLI path and environment entries,
   including `LOCALAPPDATA` on Windows. Keep this configuration private.

Use a separate key for each agent. Anyone with a key can use its permissions;
agent names alone do not authorize connections. Enabling MCP does not start capture.

Bundled script guidance is available without connecting or starting the desktop:
use `transmog_help` with `topic: "scripts"` and a section such as `authoring`,
`languages`, `sandbox`, `debugging`, `console`, `examples` or `types`. It includes
the installed API revision and required settings.
MCP can inspect script effects, callback outcomes, logs and exceptions, and read
script activation and runtime health. Diagnostic text requires unredacted access;
the global registry and initializer output require all-traffic scope. It cannot
author, test or activate scripts. See [script diagnostics](mcp-reference.md#script-diagnostics).
Use **Automation → Scripts → Open API docs** for the same reference in the app.

The desktop must be running for traffic operations. Select **Start Transmog when
needed** in the configuration dialog to let an explicit agent connection launch it.
The CLI must be beside the desktop executable. This never enables MCP or starts capture.

## Redaction and unredacted access

**Redact authentication headers and bodies** is on by default. It hides
credential-like header values and all message bodies from agents, including
sensitive rule content, script-authored findings and free-text failure diagnostics.
Script attribution and counts remain visible; annotation text is excluded from
metadata matching while redacted. Human inspection is unchanged. Turning it off
allows unredacted reads within the existing scope.

An agent can request unredacted values with a reason. Open its request badge on
the top **MCP** indicator, review the agent and scope, then choose:

- **For next 1 minute**: allow access for 60 seconds. The **Unredacted · 0:59** badge
  shows the countdown; open it and choose **Revoke access now** to end access sooner.
- **Always**: turn off that key's saved redaction setting.
- **Deny**: keep values protected.

These choices grant no additional operations or traffic scope. Disclosed data may
be sent to the agent's provider. Temporary approval ends on expiry, key changes,
revocation, MCP disablement or main-window closure.

Header matching ignores case and separators, covering names such as `APIKey` and
`AuthMsaDeviceTicket`. It can hide benign headers and does not sanitize URLs.
See [matching details](mcp-reference.md#evidence-and-disclosure).

## Permissions and shared rules

| Permission | Allows |
| --- | --- |
| View request details | Search scoped traffic and inspect headers, timings, changes and stream metadata |
| Load capture files | Load and unload local captures in private inspection sessions |
| Control collection | Start/stop the shared proxy and the agent's recordings |
| Start private listeners | Start and stop app-specific loopback listeners owned by this key. Saving recordings also requires all-traffic scope and unredacted access. |
| Prepare changes | Create, edit and test the agent's disabled header/text rule drafts |
| Edit shared rules | Edit saved header/text rules and publish private drafts |
| Change live traffic | Activate temporary rules or targeted breakpoints; with Edit shared rules, enable saved rules or change/disable active ones |
| Send requests | Send authored or replayed requests within scope |

Agents can read complete retained context in pages, test a rule against captured
traffic, and replay through live rules to verify its effect. Body and credential
reads follow the redaction choice.

Every valid key can read relevant rule effects without a separate permission;
values follow the redaction setting. Review main-workspace drafts in
**Automation → Rules** and private drafts in their session's review window.
Agent refreshes preserve human drafts, and conflicting saved edits are rejected.

Temporary rules show the agent's name; their live effects stop when the control
session ends. Choose **Keep changes…** to retain reviewed rules. Shared saved
edits remain after revoking a key.

## Private inspection sessions

Enable **Start private listeners** for a key whose agent should capture traffic
from one app. The agent can start its own listener without opening a review
window. Configure only the intended app to use the returned loopback address.
The listener has its own traffic and leaves the Windows proxy settings alone.
For HTTPS decryption, configure the app to trust the listener’s public root.
The agent can use a client-specific CA bundle. See the
[local app example](mcp-reference.md#record-one-local-app).
Use **MCP → Inspection sessions** to copy the address, stop the listener, or
review its evidence. **Stop listener** releases the port, finalizes an active
recording and keeps traffic available for inspection. **End session** also ends
agent access and releases ordinary session evidence. Saved recordings remain on
disk. The panel reports saved and failed recording outcomes with filenames
when available.

Agents can load TMCap, SAZ, HAR and Chromium NetLog files into private sessions
without changing your workspace. Enable **Load capture files** for that key.
Encrypted captures wait for a password in the top **MCP** panel; the agent never
receives the password, and redaction stays unchanged.

Open **MCP → Inspection sessions → Review** to inspect the same evidence in a
separate capture window. Files opened there join the same session.
**Review private rules** opens the normal header and text editors. Agent updates
preserve your drafts; **Publish selected rule** creates a disabled shared rule.
Review and enable it in **Automation → Rules** in the main window.

Private sessions expire after 30 minutes of inactivity. An open review window
keeps an offline session available. A private listener still expires during
review. Incoming traffic, listing sessions and polling operations do not extend
its deadline. For a long capture, call `transmog_session_renew` and check
`expiresAt`.

Key expiry or revocation, disabling MCP, and restarting the app also end
sessions. Resolve unsaved edits before ending an offline session. If agent
access ends while the app is running, the reviewer can keep working on unsaved
edits.

A session can also subscribe to the main proxy. Subscribers see the same
exchanges and rule effects. Starting a private listener is a separate operation
for capturing one app’s traffic.

## Configuration, keys and restarts

Open **Configuration…** on an agent entry to copy its setup again. **Replace key…**
invalidates its current key; update the agent with the new configuration.
**Revoke…** permanently invalidates a key.

**Copy URL** and **Copy port** share the current private endpoint, which changes on
restart. Agents connect through the generated stdio configuration.

Keys, permissions, redaction settings and expiry survive desktop restarts.
Refresh selected-request scopes after restarting. Turning off **Allow MCP
connections** suspends the keys and ends temporary approvals and control
sessions. Still-valid keys work when you re-enable access.

Restart the agent’s MCP connection after restarting the desktop, re-enabling MCP
or installing an update. The CLI and desktop versions must match. You can leave
the agent application open during installation.

See [CLI commands](cli.md#mcp-agent-connections), the
[developer reference](mcp-reference.md) and [MCP verification](testing.md#local-mcp-access).

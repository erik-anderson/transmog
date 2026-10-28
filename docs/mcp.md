# Local MCP agent access

Transmog's CLI exposes a stdio MCP server for Codex, Copilot CLI and similar
agents. The desktop owns traffic, permissions and rule state. Each CLI invocation
supplies its own `TRANSMOG_ACCESS_KEY`; there is no shared signed-in connection
or credential cache.

## Authorize an agent

Open **Settings → Integrations**, enable **Allow MCP connections**, then choose
**Create access key…**. Name the agent, choose exact hosts, frozen selected requests
or all traffic, an expiry and the required permissions. Saving this dialog is the
explicit approval. Transmog shows the random key once, with configuration examples
for Codex and Copilot CLI. Store the key in that agent's private configuration.
Create a different key for each agent or investigation.

The key identifies a grant. A friendly name does not authenticate an executable;
any process that obtains the same secret can use that grant. This design separates
agents that keep their own keys private. It does not isolate mutually untrusted
processes running as the same operating-system user or protect a compromised user
profile. The desktop stores a SHA-256 verifier, never the plaintext key.

Run `transmog-cli mcp serve` with the key in the process environment. MCP discovery
and initialization work with the desktop offline. The agent calls
`transmog_connect` to obtain the current workspace instance, scope and permissions.
`transmog-cli mcp status` and `transmog-cli mcp call TOOL_NAME` (JSON on stdin) use
the same per-invocation authentication. Keys are never command-line arguments.
Use the generated configuration's `LOCALAPPDATA` entry as well: clients may
sanitize the CLI's environment, and this keeps discovery and optional desktop
launch attached to the same Windows user profile.

Configuration formats follow the official
[Codex MCP guide](https://developers.openai.com/codex/mcp) and
[Copilot CLI MCP guide](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers).

## Permissions and rule collaboration

Every valid key can list relevant saved rules and read their definitions. There is
no separate rule-read permission. Rules containing authored body content or known
credential-header values require the corresponding disclosure permission for their
full definition; the summary still describes their effects.

| Permission | Access |
| --- | --- |
| View request details | Search scoped traffic and inspect masked heads, timings, failures and effects |
| Read message bodies | Explicit bounded reads of retained body bytes and authored rule body content |
| Control collection | Start/stop the shared proxy and agent-owned recordings |
| Prepare changes | Create, edit and test the key's disabled header/text rule drafts |
| Edit shared rules | Edit existing saved header/text rules with revision checks |
| Change live traffic | Activate temporary rules or targeted breakpoints; combined with Edit shared rules, update or disable enabled saved rules |
| Send requests | Send authored or replayed requests within approved hosts |
| Read unmasked credential headers | Reveal known credential headers and credential-bearing rule configuration |

Humans review agent drafts in **Automation → Rules**, using their normal editors.
Agent names appear on temporary rules. Newer saved human revisions cause a conflict
instead of being overwritten. Desktop refresh hints preserve unsaved editor drafts;
reopen the latest saved version and reconcile before saving again.

Shared rule edits are durable. Revoking a key ends its temporary effects but does
not roll back shared configuration or a human's work. Temporary agent rules require
an owned control lease of 30–600 seconds. Expiry, revocation, permission changes or
master-off disable those temporary rules and detach their breakpoint controller.
Only the owning key can edit a temporary rule. A human can choose **Keep changes…**
to convert a reviewed temporary rule into ordinary saved configuration.

Whole-workspace native recording requires all-traffic scope, collection, body and
credential access. Editable breakpoint envelopes also require body and credential
access and are restricted to one approved exact host. Ending a controller aborts
its unresolved pauses. Agents cannot take over a human's existing controller.

## Restarts and connection lifecycle

Named grants, expiry, revocation and master enablement survive app restarts.
**Replace key…** invalidates the previous secret immediately and shows its
replacement once. **Revoke…** permanently invalidates it. Master-off suspends keys;
turning MCP back on allows still-valid keys to reconnect.
The desktop header shows enabled agent access and a count of temporary control
sessions; select it to open Integrations. Agent-owned recordings are sealed when
their grant ends or MCP is disabled. The shared proxy remains under normal
desktop collection controls.

Configure `--start-app` explicitly if `transmog_connect` should launch the sibling
desktop executable when it is absent. This never enables MCP, starts the proxy or
starts recording. Setup and certificate decisions remain in the desktop. Status
and ordinary tool calls do not launch the app.

Every desktop run has a fresh instance identifier. Selected-request grants from
earlier runs cannot silently expand or attach to reused IDs. Reconnect and edit
their selection in the desktop. Traffic/body references and temporary control
handles must be rediscovered after restarting; ordinary persistent host grants
continue working with their existing key.

## Evidence and transport boundaries

Agents inspect the same retained original/effective request and response
boundaries as a human. Body content is fetched explicitly, in bounded ranges with
retention/truncation metadata. Header comparisons report visible differences;
bounded equal-looking previews do not establish byte equality. Captured content
is untrusted evidence, including instructions embedded by a server.

The internal desktop endpoint binds only `127.0.0.1` on an ephemeral port. It has
no LAN/wildcard bind option. The CLI validates bounded local discovery and pins
the run's ephemeral TLS certificate before sending the key. It bypasses ambient
proxies, follows no redirects and rejects non-loopback endpoints. The server
checks Host, rejects browser Origin headers and authenticates every operation;
revocation is rechecked during work and before returning data. Discovery contains
no secret or TLS private key. It is protected by the user's local profile boundary.

Send operations require a caller-supplied operation ID. Reusing it with the same
request returns the stored outcome or reports uncertainty without resending.
Reusing it for different content fails. The CLI never retries mutations. After a
timeout or app restart, inspect the result before deciding to send a new request;
the in-memory send ledger is scoped to one desktop run.

Traffic, credentials, private keys and agent keys are not part of MCP activity
logs. Data explicitly disclosed to an agent may be sent to that agent's provider.
Host private keys, TLS session secrets, arbitrary local file reads and script
source/execution are not exposed by these tools.

## Development verification

The MCP crate tests authentication, revocation, scoping, shared edits, leases,
SDK discovery, and a real loopback proxy exchange. Workspace browser checks cover
the permission dialogs, key lifecycle and preservation of unsaved human drafts.
The normal Windows desktop harness also exercises the native permission dialog
and separate CLI invocations.

After building the desktop and CLI together, qualify offline discovery and opt-in
launch with `python scripts/test-mcp-startup.py --executable
target/debug/transmog-cli.exe --artifacts artifacts/mcp-startup`. This Windows
check requires no existing desktop process, uses a disposable profile and test
grant, closes only its own windows, and verifies reuse across two desktop runs.

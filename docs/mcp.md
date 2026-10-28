# Local MCP agent access

Connect Codex, Copilot CLI or another MCP client to inspect request/response
context, control collection, send requests and collaborate on traffic rules.
Access is off by default and listens only on this computer's loopback interface.

## Connect an agent

1. In the main desktop window, open **Settings → Integrations** and enable
   **Allow MCP connections**.
2. Choose **Create access key…**. Name it, select exact hosts, selected requests
   or all traffic, and choose permissions and expiry. Create a separate key for
   each agent.
3. Copy the generated Codex or Copilot configuration into that agent's settings.
   Keep the key private: Transmog shows it only once. Preserve the generated CLI
   path and environment entries, including `LOCALAPPDATA` on Windows.

The agent runs `transmog-cli mcp serve` and calls `transmog_connect` to connect.
Creating the key is the in-app approval; a name alone never authorizes a caller.
Anyone who obtains the same key can use its permissions.

The desktop must be running for traffic operations. Add `--start-app` to the
connector arguments to let an explicit connection launch it; the CLI must be
beside the desktop executable. This never enables MCP or starts collection.
MCP initialization and tool discovery still work while the desktop is closed.

## Review connections and unredacted access

The top **MCP** indicator shows connected access keys. It keeps recent calls
visible for three seconds to prevent flicker; an idle stdio connector alone does
not count as a desktop connection. Open it to see the key names and access controls.

**Omit authentication headers and bodies** is enabled for every key by default.
Known authentication-header values and all message bodies stay omitted even when
their read permissions are selected. This does not change the traffic retained
for human inspection.

An agent can request unredacted values with a reason. Open the request badge on
the MCP indicator, review the agent and scope, then choose:

- **For next 1 minute**: allow unredacted values for 60 seconds without changing
  the saved setting. An **Unredacted · 0:59** badge shows the countdown; open it
  and choose **Revoke access now** to end access sooner.
- **Always**: turn off that key's omission setting and grant body and credential
  reads. This change survives restart.
- **Deny**: keep values protected.

These choices do not expand traffic scope or grant editing, collection or sending.
Disclosed data may be sent to the agent's provider. Temporary approval also ends
when the key changes, expires or is revoked, MCP is disabled, or the main window
closes. The badge updates independently of the connection indicator's grace period.

## Permissions and shared rules

Every valid key can read relevant saved rules and their effects without a separate
rule-read permission. Body content and known credential values in rule definitions
remain subject to the read permissions and omission setting.

| Permission | Allows |
| --- | --- |
| View request details | Inspect scoped traffic, masked headers, timings and changes |
| Read message bodies | Read retained bodies and authored rule body content |
| Control collection | Start/stop the shared proxy and the agent's recordings |
| Prepare changes | Create, edit and test the key's own disabled header/text rule drafts |
| Edit shared rules | Edit existing saved header/text rules |
| Change live traffic | Activate temporary rules or targeted breakpoints; with Edit shared rules, change or disable enabled saved rules |
| Send requests | Send authored or replayed requests within scope |
| Read unmasked credential headers | Read known credential headers and credential-bearing rule configuration |

Review agent drafts in **Automation → Rules** using the normal editors. Newer
saved human edits cause a conflict instead of being overwritten, and agent
refreshes preserve unsaved human drafts.

Temporary rules show the agent's name and stop when their control session ends.
Choose **Keep changes…** to retain reviewed changes as ordinary saved rules.
Shared saved edits remain after a key is revoked.

## Keys and restarts

Manage keys in **Settings → Integrations**. **Replace key…** invalidates the old
secret and shows a new one once; **Revoke…** permanently disables it. Turning off
**Allow MCP connections** suspends all keys and ends temporary access and effects.
Turning it back on permits still-valid keys to reconnect.

Saved keys, permissions, omission settings and expiry survive app restarts.
Temporary approvals and control sessions do not. After restarting, agents must
reconnect and rediscover traffic references; selected-request scopes must be
refreshed in the app. Persistent host scopes continue using the same key.

See [CLI commands](cli.md#mcp-agent-connections) for direct calls,
[the developer reference](mcp-reference.md) for authentication and tool contracts,
and [MCP verification](testing.md#local-mcp-access) for test commands.

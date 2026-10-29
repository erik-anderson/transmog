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

The desktop must be running for traffic operations. Select **Start Transmog when
needed** in the configuration dialog to let an explicit agent connection launch it.
The CLI must be beside the desktop executable. This never enables MCP or starts capture.

## Redaction and unredacted access

**Redact authentication headers and bodies** is on by default. It hides
credential-like header values and all message bodies from agents, including
sensitive rule content. Human inspection is unchanged. Turning it off allows
unredacted reads within the existing scope.

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
| Control collection | Start/stop the shared proxy and the agent's recordings |
| Prepare changes | Create, edit and test the agent's disabled header/text rule drafts |
| Edit shared rules | Edit existing saved header/text rules |
| Change live traffic | Activate temporary rules or targeted breakpoints; with Edit shared rules, change or disable enabled saved rules |
| Send requests | Send authored or replayed requests within scope |

Every valid key can read relevant rule effects without a separate permission;
values follow the redaction setting. Review agent drafts in **Automation → Rules**.
Agent refreshes preserve human drafts, and conflicting saved edits are rejected.

Temporary rules show the agent's name and stop when its control session ends.
Choose **Keep changes…** to retain reviewed rules. Shared saved edits remain after
revoking a key.

## Configuration, keys and restarts

Open **Configuration…** on an agent entry to copy its setup again. **Replace key…**
invalidates its current key; update the agent with the new configuration.
**Revoke…** permanently invalidates a key.

**Copy URL** and **Copy port** share the current private endpoint, which changes on
restart. Agents connect through the generated stdio configuration.

Keys, permissions, redaction settings and expiry survive restarts. Temporary
approvals and control sessions do not. Reconnect after restarting and refresh
selected-request scopes in the app. Turning **Allow MCP connections** off suspends
all keys and ends temporary access and effects; still-valid keys work again when
you re-enable it.

See [CLI commands](cli.md#mcp-agent-connections), the
[developer reference](mcp-reference.md) and [MCP verification](testing.md#local-mcp-access).

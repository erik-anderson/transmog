# MCP connector reference

[Set up an agent](mcp.md) in the desktop before using these contracts. The
[`transmog-mcp` crate](../crates/mcp/src/lib.rs) owns authorization, the local
transport, the stdio connector and the application adapter. The
[tool catalog](../crates/mcp/src/catalog.rs) is authoritative for tool names,
arguments and discovery descriptions.

## Authentication and transport

The agent-facing MCP transport is stdio. Each CLI invocation presents its own
`TRANSMOG_ACCESS_KEY` to the desktop; connection state, names and previous calls
never authenticate a later operation. There is no shared credential cache or
OAuth sign-in. The desktop authenticates against a SHA-256 verifier. On Windows,
new and replaced keys also have a user-bound encrypted copy so the trusted settings
UI can reopen configuration. Plaintext keys never enter the persisted store. Legacy
entries without an encrypted copy require explicit key replacement for recovery. Keys identify grants, not verified executables. Processes that
can read the same user's private configuration are not isolated from each other.

The private desktop endpoint binds only `127.0.0.1` on an ephemeral port, with no
LAN or wildcard option. Discovery contains the endpoint, instance ID and ephemeral
TLS certificate pin, never the key or TLS private material. Before sending a key,
the CLI validates loopback discovery and pins the certificate. It bypasses
ambient proxies and follows no redirects. The server validates Host, rejects
Origin headers and rechecks authorization during work and before returning data.
See [transport implementation](../crates/mcp/src/transport.rs).

`mcp serve` initializes and lists tools without contacting the desktop. Only
`transmog_connect`, with `--start-app` configured, may launch the sibling desktop
executable. Status and ordinary calls never launch it. Every desktop run has a
fresh instance ID; retained references and control handles must be rediscovered.

Activity indicators group authenticated calls by key. Their three-second quiet
period is presentation state, excluded from persistence and authorization.

Older stored grants with partial sensitive-read permissions migrate to redaction-on,
without expanding their scope or operations. The UI controls both sensitive-read
capabilities through one redaction setting; their wire identifiers remain for
compatibility.

## Evidence and disclosure

Host scopes match exact hosts on any port and apply before paging. Selected-request
scope freezes entry IDs and their owning instance, so later selection changes or
restarts cannot broaden it.
Reads expose retained original/effective request and response boundaries, with
explicit bounded body reads, retention, truncation and representation metadata.
Equal bounded previews do not establish complete byte equality. Captured content,
including instructions embedded by a server, is untrusted evidence.

The default redaction policy suppresses known authentication-header values and all
message bodies, including sensitive authored rule content. It does not sanitize
arbitrary URLs or identify every possible custom secret. The
[application adapter](../crates/mcp/src/application.rs) defines header masking.

`transmog_sensitive_access_request` takes a reason and creates a pending request;
it cannot approve itself. The agent asks the human to respond in Transmog, then
polls `transmog_sensitive_access_status` before retrying a protected read. Polling
does not create prompts. Only trusted desktop commands decide or revoke consent.

One-minute consent uses a monotonic deadline and remains in memory. Always atomically turns off the saved redaction setting. Neither
choice expands scope or other capabilities. Revocation or expiry cancels access
and prevents in-flight results from disclosing values. Recording and breakpoint
work that requires those values ends too. See
[access grants](../crates/mcp/src/access.rs) and
[disclosure lifecycle](../crates/mcp/src/access/disclosure.rs).

## Changes, control and retries

Shared header/text edits use saved-rule revisions; stale writes conflict rather
than overwrite newer human work. Rule reads need no separate capability, while
their body and credential content follows the redaction setting. Temporary rules are
owned by one key and require a control lease of 30–600 seconds. Other keys can
read their effects but cannot edit them. Lease expiry, revocation, permission
changes or master-off disable temporary effects and detach their controller.
Human **Keep changes…** converts a reviewed temporary rule into durable content.
Revocation does not roll back shared saved edits.

When a grant ends or MCP is disabled, its owned recordings are sealed. The shared
proxy stays under the desktop's ordinary collection controls.

Whole-workspace native recording requires all-traffic scope, collection, body and
credential access. Editable breakpoint envelopes require body and credential
access and a scope of one exact host. Ending a controller aborts its unresolved
pauses; agents cannot take over a human's existing controller.

Send calls require an operation ID. Reusing it with identical input returns the
recorded outcome or uncertainty without resending; different input conflicts.
The CLI never retries mutations. After timeout, inspect the outcome before
sending again. The send ledger lasts only for the current desktop run.

MCP exposes no arbitrary file reads, script source/execution, host private keys
or TLS session secrets. Activity logs omit traffic bodies and credentials.
See [verification](testing.md#local-mcp-access) for regression and startup checks.

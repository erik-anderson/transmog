# MCP connector reference

[Set up an agent](mcp.md) in the desktop before using these contracts. The
[`transmog-mcp` crate](../crates/mcp/src/lib.rs) owns authorization, the local
transport, the stdio connector and the application adapter. The
[tool catalog](../crates/mcp/src/catalog.rs) and
[public models](../crates/mcp/src/catalog/models.rs) define tool names, nested input
schemas, output schemas, defaults and discovery descriptions. Validation uses
those same input contracts and identifies invalid fields without echoing values.
Tool annotations describe read-only, destructive, external and idempotent effects
individually. Tool errors set `isError` and return
`{ "error": { "code": "...", "message": "..." } }`.

## Authentication and transport

The agent-facing MCP transport is stdio. Each CLI invocation presents its own
`TRANSMOG_ACCESS_KEY` to the desktop; connection state, names and previous calls
never authenticate a later operation. There is no shared credential cache or
OAuth sign-in. The desktop authenticates against a SHA-256 verifier. On Windows,
keys have a user-bound encrypted copy so the trusted settings UI can reopen
configuration. Plaintext keys never enter the persisted store.
Keys identify grants, not verified executables. Processes that can read the same
user's private configuration are not isolated from each other.

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

## Evidence and disclosure

Host scopes match exact hosts on any port and apply before paging. Selected-request
scope freezes entry IDs and their owning instance, so later selection changes or
restarts cannot broaden it.
Reads expose retained original/effective request and response boundaries, with
explicit bounded body reads, retention, truncation and representation metadata.
Equal bounded previews do not establish complete byte equality. Captured content,
including instructions embedded by a server, is untrusted evidence.

### Find and inspect traffic

- `transmog_traffic_list` accepts `query` filters for metadata text, method, exact
  host, status range, duration range, content type, process, start-time range and
  terminal state. Times use Unix milliseconds. `sort` is `newest` (default) or
  `oldest` admission order. Filters always intersect the approved scope.
- Every scope returns the same compact summary shape. Pages use `nextCursor`;
  continue with `cursor` and `limit`, optionally pinning `instanceId`. Membership
  and order stay fixed for up to five minutes; summary values reflect current evidence.
  Evicted rows appear in `unavailableIds`. Cursors belong to one key and grant
  revision. Each key retains eight snapshots; shared limits bound total memory.
- `transmog_exchange_get` returns core identity and the requested `include`
  sections: `headers`, `bodies`, `timing`, `changes`, `transport`. The default is
  headers and body metadata. `transmog_exchanges_get` accepts up to 20 `ids` with
  the same projection and a 512 KiB total budget; unavailable items have individual
  errors. Explicit header/body reads retain their independent paging contracts.
- `transmog_traffic_search` searches selected `fields` using literal `text` or
  bounded `regex` matching. Metadata is the default. Header searches exclude
  credential values while redaction is on; body searches require unredacted
  access and include retained stream text in that direction. Scope applies before
  matching and counting. Results report examined entries and binary/unavailable
  bodies, then page through `transmog_traffic_list`. Search cursors retain the
  specific unredacted permissions used to find their matches. Search leaves the
  human's current search unchanged, and cancels after 30 seconds without partial results.
- `transmog_body_extract` takes up to 32 RFC 6901 JSON `pointers` at an explicit
  boundary. It requires a complete decoded JSON body of at most 64 KiB and
  unredacted access. Each result distinguishes a missing pointer from JSON null.
- `transmog_stream_list` returns content-free WebSocket/SSE message metadata;
  `transmog_stream_read` reads one payload by `sequence` with bounded base64 byte
  ranges, retention state and `nextOffset`. Payload reads require unredacted access.
- `transmog_traffic_wait` waits up to 30 seconds for scoped matching traffic after
  `afterRequestNumber`. It checks existing matches before waiting, leaves other
  calls available and returns `timedOut: true` for an empty timeout.

The default redaction policy suppresses known authentication-header values and all
message bodies, including sensitive authored rule content. It does not sanitize
arbitrary URLs or identify every possible custom secret. Header names are matched
case-insensitively after removing separators, using `auth`, `ticket`, `token`,
`secret`, `credential`, `cookie`, `session`, `password`, `passwd`, `passphrase`,
`csrf`, `xsrf`, `signature`, `apikey`, `accesskey`, `privatekey`, `subscriptionkey`,
`bearer`, `jwt`, `saml`, `dpop` and `assertion`. The
[application adapter](../crates/mcp/src/application.rs) is authoritative for matching.

`transmog_sensitive_access_request` takes a reason and creates a pending request;
it cannot approve itself. The agent asks the human to respond in Transmog, then
polls `transmog_sensitive_access_status` before retrying a protected read. Polling
does not create prompts. Only trusted desktop commands decide or revoke consent.

One-minute consent uses a monotonic deadline and remains in memory. Always
atomically turns off the saved redaction setting. Neither choice expands scope
or other capabilities. Revocation or expiry cancels access and prevents in-flight
results from disclosing values. Recording and breakpoint
work that requires those values ends too. See
[access grants](../crates/mcp/src/access.rs) and
[disclosure lifecycle](../crates/mcp/src/access/disclosure.rs).

## Changes, control and retries

Rule authoring uses a small `rule` definition with `kind`, `name`, exact `host`,
optional `method` and `pathPrefix`. `headers` definitions specify `phase` and
ordered `operations` (`set`, `append`, `override-if-present`, `remove`).
`response-text` definitions specify literal `find`, `replacement`, `mediaType`,
optional occurrence/count choices and a complete UTF-8 input/output bound.
Shared edits preserve other existing native match conditions. `transmog_rule_get`
returns the native definition for review plus the authoring `definition` when
disclosure allows it. Use its revision for updates. Revising a temporary draft
requires both `id` and `expectedRevision`.
Network-free rule tests return readable ordered headers with explicit UTF-8 or
base64 encoding, or the literal text operation's before/after sample and outcome.

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

`transmog_request_prepare` creates an editable draft from an authorized request
boundary without sending. It preserves duplicate headers, explains dropped
connection/framing and credential fields, and returns required acknowledgements.
`includeCredentials` and `includeBody` default to false and require unredacted
access. Captured-body references use complete `client-request` bytes; other
boundaries need an explicit replacement body. Review the draft, then pass its
`request` to `transmog_request_send` with a unique `operationId`. Preparation never
acknowledges credentials or non-idempotent sends on the caller's behalf.

MCP exposes no arbitrary file reads, script source/execution, host private keys
or TLS session secrets. Activity logs omit traffic bodies and credentials.
See [verification](testing.md#local-mcp-access) for regression and startup checks.

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

## Inspection sessions and capture operations

Inspection, search, comparison, context, replay and rule-draft tools require
`sessionId`. Use `main` for the main workspace, or create a private offline
session with `transmog_session_create`. List, renew and close it through
`transmog_session_list`, `transmog_session_renew` and `transmog_session_close`.
Private IDs, cursors, context references and operation IDs are owned by the key;
knowing another key's IDs grants no access. Instance identity, current scope and
disclosure are checked independently. Valid session activity renews a 30-minute
idle lifetime; incoming traffic does not. Human review keeps a session alive.

`transmog_capture_load` requires **Load capture files**, a private session,
`path` and `operationId`. Selected-request scopes cannot authorize new imports.
Supported sources are native TMCap (including recoverable and encrypted captures),
standard/extended/encrypted SAZ, HAR, and Chromium NetLog JSON/typed JSONL.
`transmog_capture_list` groups each imported file under one `captureId`, including
empty files and merged source captures. `includeContext` provides complete paged
source metadata with all-traffic scope and unredacted header/body access.

Load and unload return tracked operations. Poll `transmog_operation_status`;
`transmog_operation_cancel` requests cancellation; `cancelRequested` distinguishes
that request from completed cleanup. Identical operation-ID retries
return the recorded operation; different input conflicts. Publication is atomic.
`waiting-password` requires the user to enter the password in the desktop MCP
panel. No MCP tool accepts a password. A retry uses the same pinned source handle.
Password entry does not approve unredacted reads.

`transmog_capture_unload` removes the entire file group, its source indexes,
markers and readers, and invalidates dependent evidence references. It preserves
source files, separately saved copies and retained replay results. Unsaved human
edits return `needs_human_action`. Closing a review window preserves the session;
closing a session ends agent access and releases its evidence. Revocation and MCP
shutdown preserve dirty human review work while ending agent access.

`transmog_session_live_subscribe` admits references to the single live engine,
with optional request-side exact host/method filters and `includeExisting`.
Unsubscribe stops new memberships; admitted requests continue receiving updates.
No subscription starts another proxy, runs another hook pipeline or sends a
duplicate request. Direct sends retain evidence in their originating session;
`live-rules` sends traverse the shared proxy once and reach matching subscribers.
Shared controls and their leases are independent of inspection-session lifetime.

Private rules stay disabled. Prepare/test them in their session and publish an
exact revision through `transmog_rule_publish` with **Edit shared rules**. Publishing
creates a disabled shared saved rule. Shared updates and activation require the
existing shared-rule/live-traffic permissions. Rule lists include shared side
effects, and saved edits reject stale revisions without replacing human drafts.

Budgets are shared across sessions: eight sessions total/four per key, four pending
capture operations, 256 MiB of imported indexes, 200,000 live memberships, and
the existing context/snapshot and replay-retention budgets. A bounded main retention policy also reserves space for newer live traffic.
Files are bounded to
1 GiB; each session holds at most 16 captures (64 total). The operation ledger is
bounded for the app run and never evicts IDs in a way that permits duplicate work.
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
  sections: `headers`, `bodies`, `timing`, `changes`, `transport`, `context`, or
  `all`. `context` provides complete retained metadata, caller/source associations,
  hook effects and ordered header values through a paged reference. The default is
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
  boundary. It requires a complete decoded JSON body within the 256 MiB
  whole-document decoding budget and unredacted access. Each result distinguishes
  a missing pointer from JSON null;
  large results are available through their `context` reference.
- `transmog_stream_list` returns content-free WebSocket/SSE message metadata;
  `transmog_stream_read` reads one payload by `sequence` with bounded base64 byte
  ranges, retention state and `nextOffset`. Payload reads require unredacted access.
- `transmog_traffic_wait` waits up to 30 seconds for scoped matching traffic after
  `afterRequestNumber`. It checks existing matches before waiting, leaves other
  calls available and returns `timedOut: true` for an empty timeout.

`transmog_headers_read` pages complete ordered field values, preserving duplicates
and binary values with explicit UTF-8/base64 encoding. Its `context` reference
contains the entire sanitized block. `transmog_body_read` pages retained encoded
bytes as base64; `decodeContent: true` returns a decoded context page. Continue
with `transmog_context_read`, its `contextId`, and `nextOffset`. Offsets and
`totalBytes` are decimal byte-position strings; text offsets refer to UTF-8 bytes.
A tiny page that splits a character uses base64 to preserve its exact bytes.

Context references expire after five minutes and belong to one key, grant revision
and source scope. Each page rechecks its original disclosure requirements.
Up to 64 contexts per key and 256 globally share a 512 MiB memory budget;
older contexts for the same key can be replaced. Whole-document decoding is
bounded to 256 MiB and retains the native codec safeguards. Raw retained byte
ranges have no whole-document ceiling. Missing or incomplete sources produce
explicit errors rather than a purported complete document. Inline test/extraction
results set `inlineComplete: false` when the full result needs its context reader.

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
`transmog_rule_test` accepts exactly one of `sample` or `sampleExchangeId`.
Captured samples use complete retained headers and, for text rules, decoded body
content. Choose `requestBoundary` (default `client-request`) and
`responseBoundary` (default `client-response`) to test before or after live edits.
Tests return ordered headers or before/after text with the native outcome and a
complete result context; they do not send traffic or activate rules. Source scope
and unredacted access still apply. Native text transforms retain their 1 MiB
input/output limit.

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
Validation runs before admission, so rejected arguments do not consume an ID.
`transmog_request_status` reads this key's `pending`, `completed` or
`outcome-unknown` state without sending. Cancellation after admission records
uncertainty and never retries automatically. The CLI never retries mutations;
inspect traffic before choosing a new operation after uncertainty. The ledger
lasts only for the current desktop run.

`transmog_request_send` defaults to `mode: "direct"`, which sends to the origin
without applying live rules. Use `mode: "live-rules"` to verify changes through
the running loopback proxy, including its current rules, hooks and observers.
HTTPS uses CONNECT with certificate verification against the running proxy's
public interception CA and requires HTTPS decryption to be enabled. It requires
collection to be running and body retention configured; it never starts collection
automatically. Live-rules sends allow up to 4 MiB of request
body and consume up to 16 MiB of response body. Results identify the mode and an
exchange ID for inspecting observed boundaries and rule attribution.
Replay evidence remains available across later sends, within a history of
100 exchanges and a 256 MiB aggregate body budget (the latest exchange is kept).
Retention/source omissions remain explicit; replay history ends with the app run.

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

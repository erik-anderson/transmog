# Product state, retention and support

`transmog-app` owns preference validation and persistence above the proxy/session
layers. Desktop and headless callers share those failure and privacy semantics.

## Persisted state

Preferences contain theme, table/pane layout, window geometry, connection
configuration, live-entry/body-retention policy and explicit recent-path choices.
The desktop start command honors `configureSystemProxy`: false binds the proxy
for manually configured clients without changing Windows routing. This mode
does not require installing the managed CA in Windows; clients configure their
own HTTPS trust when needed. The ordinary Settings flow keeps system routing
enabled. With HTTPS decryption enabled, both modes validate and protect CA files.
Both always block pending host restoration.

Preferences starts with HTTPS decryption in its main Network group. Advanced
network settings contains remote access, an optional persistent proxy port,
connection reuse, caller-process isolation and allowed outbound protocols. Isolation falls
back to source IP and also separates downstream HTTP/2 sessions. Protocol changes
apply to new origin connections, including Composer in already-open capture
viewers; established connections continue serving requests until they close.
Saving preserves explicit manual routing and does not restart
the proxy. Embedders can supply their own network-isolation key computation.

Recorded CONNECT destinations also appear on decrypted connections. Traffic uses
the request method, connection leg and TLS evidence to distinguish opaque tunnels
from inspected messages, keeping decrypted bodies and TLS timings available.

HTTPS decryption changes apply at the next proxy start. When disabled, CONNECT
forwards opaque bytes without loading interception CA files or requiring CA trust.
Traffic and TMCap retain the destination, connection outcome, endpoints and byte
counts. The HTTP protocol recorded is the outer CONNECT protocol; the tunneled
protocol and contents remain unknown.
Live traffic, captured headers and bodies, credentials, breakpoint envelopes,
controller capabilities, replay drafts and CA private keys are not settings.
Recent artifact references are retained only when the user opts in.

Settings provides Save, Revert and Ctrl+S. Narrow layout saves merge with other
settings so a stale workspace snapshot cannot overwrite newer preferences.
Each save writes and flushes a temporary file, then atomically publishes a new
numbered generation. Startup validates the newest generation and quarantines
corrupt state, falling back to a retained valid generation or safe defaults.
Only the current schema is supported. Exact fields and validation belong to
[the product-state model](../crates/app/src/product_state.rs).

On Windows preferences live under `%LOCALAPPDATA%\Transmog` in the
`preferences.<generation>.json` family. Persistence errors remain visible and
cannot participate in proxy shutdown or host restoration.

## Retention and privacy

WebSocket/SSE history follows the request/client and response/server content
privacy choices. Its bounded message history is separate from the HTTP body buffer;
see [stream inspection](stream-inspection.md) for limits, lazy saved content and
the distinction between active streams and open-at-capture evidence.

Closing the main window releases its live traffic, removed-entry Undo evidence and
owned body-cache files. Saved traces and explicitly recorded files remain.
Traffic → Clear all acts across the whole list and filters. At 100 MB or more, Undo expires
after five minutes and releases the data; at 1 GB or more, data is released
immediately and Undo is unavailable.

New installations retain request and response bodies and include bodies in
explicit recordings. Sensitive header values are collected by default; the Privacy redaction checkbox is initially unchecked.
**Settings → Preferences** offers persistent redaction of Authorization,
Proxy-Authorization, Cookie and Set-Cookie values for newly received traffic and recordings. Header names,
ordering, duplicates and measured original lengths remain available. Changing
redaction does not rewrite retained traffic or saved files. Export-only redaction
applies just to the new copy; bodies, URLs and metadata can still contain private
data. Password encryption is available when saving or exporting traces.

Live-entry retention has no count cap by default. An optional maximum evicts
older completed live entries, preserves imported entries and allows active
requests to finish before eviction. Changing that maximum applies immediately.
It is separate from the body-byte budget.

The circular body buffer defaults to half installed physical RAM across observed
boundaries and stores those bytes in memory. Custom limits up to that threshold
also use memory; larger limits and **No max size (writes to disk)** use the app's
disk cache. Settings shows the resolved limit and storage mode. Body-policy
changes apply to new boundaries without spilling retained memory bodies to disk.
This quota is not a total application-memory ceiling: metadata, editors and
processing have separate resource bounds. Imported bodies use their pinned source
files and are independent of live eviction.

Request capture defaults to **25 MB** (25,000,000 bytes) per original/effective
request boundary. **Custom limit** accepts decimal MB values from 1 MB to 1 TB (1,000,000 MB).
**Unlimited** removes that per-request cap while keeping the
chosen circular buffer budget. Reaching a retention cap records an incomplete
prefix and full observed byte counts; it does not stop forwarding. Recordings
snapshot their policy when started. Saved traces have no fixed byte-size or
record-count ceiling, while metadata indexing and processing stay bounded.
Oversized or unknown-length requests in Auto routing stream over HTTP/1 or HTTP/2
rather than requiring a buffered HTTP/3 retry.

Starting the proxy does not create an automatic trace journal. **Save trace…** and
explicit recording write files. See [trace saving](trace-saving.md) and the
[CLI support guide](cli-support-capture.md); the CLI has separate preferences
and root ownership under `%LOCALAPPDATA%\Transmog-cli`.

Desktop automatic-check preferences and update reminders use a separate bounded `update-preferences.<generation>.json`
store under the same product-state root. Independent Settings saves cannot
overwrite the reminder deadline. Three generations are retained; updates preserve
them, and uninstall removes only their owned numeric filenames. Staged installer
bytes and installation consent are scoped to the running app session.

## Operational diagnostics

Operational diagnostics are bounded, redacted and separate from traffic.
The desktop writes `%LOCALAPPDATA%\Transmog\diagnostics.jsonl`, rotates the log
and reports lifecycle, host integration, certificate, command and frontend errors.
Messages containing credentials, private-key markers or local paths are replaced
before reaching memory or disk. Diagnostic write failures are non-fatal.

**Settings → Support → Technical details** shows the active log path and a report
with app/dependency versions, OS, native WebView runtime, state schema and recent
events. It contains no captured traffic. A support bundle is a create-new ZIP
with `diagnostics.json` and `product-state-summary.json`; it excludes bodies,
HTTP header values, credentials and private keys. Recent paths require both the
saved privacy opt-in and an opt-in for this export.

## Support procedure

1. Stop the proxy normally; preference or log failures cannot block restoration.
2. Use **Settings → Support → Refresh diagnostics** and inspect the readable
   summary before copying technical details.
3. Use **Create support bundle…** and choose a destination in the save dialog.
   Include remembered capture/export filenames and folders only if needed; file
   contents are excluded. This option requires the saved Privacy permission.
4. If preferences are corrupt, restart and review the recovery message.

A support bundle diagnoses the tool itself. To share captured traffic, review
and save a `.tmcap` trace instead.

## Third-party credits

**Settings → Credits** opens a scrollable modal inventory with dependency
versions, licenses, copyright notices and vendored native library notices. The
notices ship with the app and are available offline. Builds require version-pinned
license snapshots when published dependencies omit their license files.

Desktop proxy access defaults to loopback only. Remote computers require an explicit
persisted opt-in. The proxy port defaults to automatic selection; an authored
port from 1 through 65535 is retained and an occupied port produces an actionable
start failure. Address and port changes apply to the next fresh proxy start.
Windows routes through loopback even when the listener accepts remote clients.

Server connection reuse is enabled by default. Turning it off gives subsequent
requests dedicated HTTP/1, HTTP/2 and HTTP/3 connections, including Composer in
capture viewers. Active requests and streams finish normally. Keep HTTPS
decryption visible at the top of Preferences, above the advanced disclosure.

# Product state, retention and support

`transmog-app` owns preference validation and persistence above the proxy/session
layers. Desktop and headless callers share those failure and privacy semantics.

## Persisted state

Preferences contain theme, table/pane layout, window geometry, connection
configuration, live-entry/body-retention policy and explicit recent-path choices.
See [network profiles and connection settings](network-profiles.md) for choices,
defaults and activation. Profile definitions and the active recipe snapshot have
independent revisions; saving a definition does not activate it.

Live traffic, captured headers and bodies, credentials, breakpoint envelopes,
controller capabilities, replay drafts and CA private keys are not settings.
Recent artifact references are retained only when the user opts in.

Response experiments use the existing persisted automation rules and response
assets, including named variants and immutable revision history. Saving an
experiment revision does not change the explicitly activated live revision;
activation and experiment pause are separate actions. The editor draft remains in
the mounted workspace for the current session. See [response experiments](automation.md#response-experiments).

Saving a trace preserves retained network evidence; it does not package these
experiment definitions, script sources, linked files or temporary preview scenes.
Copying the application state directory is not a portable investigation format.
For sharing boundaries and unavailable bytes, see [trace saving](trace-saving.md#response-experiment-sharing).
The [portable investigation APIs](portable-investigations.md#implemented-rule-interchange-apis-k1)
provide selected rule/asset interchange for library callers. Imported source
definitions, identity mappings and asset metadata share one automation commit;
their immutable body blobs use the existing asset store. Imported named
experiments start paused with no active selection, and ordinary rules start
disabled. A saved state directory is still not a portable case.
[Working-case APIs](portable-investigations.md#implemented-working-case-apis-k2)
now keep selected evidence, source-associated notes/context, frozen experiments
and up to 32 manifest revisions in an explicit plaintext directory. Current
publication is transactional; interrupted staging remains separate recovery data.
Desktop Files actions now open/save reviewed ZIP64/TMCap transfers, with optional
whole-package encryption. Opening keeps a pinned read-only source and creates no
decrypted archive. Evidence uses the existing inert trace inspector; its lazy
readers retain source/key handles after package review closes. Plaintext working
copies and disabled automation import are separate explicit actions. Preparing a
new derived case from Traffic uses disclosed owned plaintext snapshots; forwarding
an opened package streams its exact ZIP without that intermediate copy.

Response assets persist the chosen copy/link behavior. Linked assets retain an
absolute path even when the file is unavailable; they do not silently become
copies or fall through to the origin. Copied assets may retain their source path
as informational provenance, but serving uses stored bytes and never reopens it.
These file paths belong to explicitly authored response assets, not preferences
or retained Traffic. The copy/link choice lives in response asset storage.

Linked responses use bounded temporary snapshots while serving. Identical active
encoded snapshots share storage; files are released when their response consumers
finish. Their hashes and selected rule/asset revisions are capture evidence,
without the external file path. Historical body bytes use the ordinary capture
store and its existing privacy, retention and eviction policy. Snapshot identity
alone does not retain a complete body. Explicit TMCap recording/saving preserves
the metadata and available body bytes; see [trace saving](trace-saving.md).
Temporary serving files, like decoded preview files, may survive an abrupt exit;
InPrivate is not a crash-erasure mechanism for those files.

Settings provides Save, Revert and Ctrl+S. Narrow layout saves merge with other
settings so a stale workspace snapshot cannot overwrite newer preferences.
Each save writes and flushes a temporary file, then atomically publishes a new
numbered generation. Startup validates the newest generation and quarantines
corrupt state, falling back to a retained valid generation or safe defaults.
Narrow profile writes merge with independent layout, window and certificate-policy
saves and reject stale catalog revisions. Exact fields, validation and supported
state migrations belong to [the product-state model](../crates/app/src/product_state.rs).
Network changes use the profile API rather than whole-state saves. Uncertain
preference recovery disables remote access and recovered client-certificate
rules until explicitly re-enabled, including after interrupted recovery.

On Windows preferences live under `%LOCALAPPDATA%\Transmog` in the
`preferences.<generation>.json` family. Persistence errors remain visible and
cannot participate in proxy shutdown or host restoration.

## Retention and privacy

Captured-page previews use fresh InPrivate browser sessions for each run. Reload
keeps that run's state and frozen responses; updated previews start fresh. Profile
cleanup and InPrivate do not guarantee removal of plaintext preview response files
after a crash. See the [preview privacy boundary](captured-page-preview.md#isolation-and-implementation).

WebSocket/SSE history follows the request/client and response/server content
privacy choices. Its message history is separate from the HTTP body buffer;
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

Starting the proxy does not create an automatic trace journal. **Save copy…** and
explicit recording write files. See [trace saving](trace-saving.md) and the
[CLI support guide](cli-support-capture.md); the CLI has separate preferences
and root ownership under `%LOCALAPPDATA%\Transmog-cli`. Its read-only
[`context` command](cli.md#read-recorded-context) lists or extracts original
configuration/support context from native captures. It supports a password
argument, masked prompt, password file and a no-prompt automation mode. It does
not collect current configuration or export traffic bodies.

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

**Settings → About → Credits** opens a scrollable modal inventory with dependency
versions, licenses, copyright notices and vendored native library notices. The
notices ship with the app and are available offline. Builds require version-pinned
license snapshots when published dependencies omit their license files.

## Customer website incidents

[Support capture profiles and incident intake](support-capture-profiles.md) guide
website evidence collection. They are separate from operational Transmog support
bundles and network profiles. Profile-only CLI choices, incident navigation and
export reviews are run/window state, not global preferences containing captured
notes. Incident marks belong to explicit recorded artifacts; normal proxy startup
does not create a journal. Review original context and HTTP/stream content before
sharing; header redaction alone does not anonymize them.


Literal response edit definitions persist with native automation generations.
Saving a disabled draft does not activate it. Operation outcomes, counts and
hashes follow the exchange's existing capture metadata lifecycle; they do not
create a permanent body store. Historical bodies remain subject to retention.
See [literal response edits](automation.md#literal-edits-to-fresh-responses).

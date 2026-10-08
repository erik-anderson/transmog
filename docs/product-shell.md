# Transmog product shell

The Windows desktop shell is a thin Tauri adapter over `transmog-app`. The same
application facade is suitable for a future CLI: it owns proxy lifecycle,
bounded session queries, safe inspectors, breakpoints, replay, capture, import,
export, product state, and privacy-safe diagnostics without depending on Tauri
or WebUI.

## Operating model

- **Set up HTTPS interception** creates a durable PEM CA and matching private
  key when needed, then explicitly asks the user to approve current-user trust.
  CA generation is create-new and never overwrites either file. Start verifies
  the files, recorded SHA-256 identity, and current trust state before binding.
- Desktop Start always journals the exact current-user Windows proxy registry
  values, starts an automatic bounded native capture, binds the listener, and
  then applies the loopback proxy. Stop and normal/OS-requested exit restore the
  exact prior values. The durable journal is recovered on next launch after a
  hard termination.
- Certificate trust installation/removal remains an explicit exact-SHA-256
  operation and can display an OS consent dialog. The setup action warns before
  triggering that dialog; unattended builds and tests never install trust.
- The traffic list queries bounded pages (100 by default, at most 200) over
  filtered and sorted retained metadata, and watches live automatically. It
  follows the latest row until the user scrolls away or selects a request;
  capture continues while that view is pinned.
- Traffic rows show the best-effort local caller process name and PID captured
  at connection accept time. Non-loopback peers show as remote and unresolved
  loopback callers remain explicitly unknown.
- Completed retained client responses can seed auto-responses directly from
  the Traffic inspector or multiselection review. The Automation
  workspace exposes a dense priority list and editable properties, with bulk
  actions, reversible deletion, duplicate warnings and a network-free matcher
  tester. The first enabled match wins; exact addresses, guided URL patterns
  and bounded regex are supported. A shared Traffic/Automation pause switch
  controls rule hooks without modifying rule states. Saved responses remain
  editable independently of the original Traffic entry. Winning traffic is
  marked `AUTO` and names the rule and immutable asset that served it. See
  [automation.md](automation.md) for matching and interaction details.
- The primary desktop canvas is a fixed-viewport traffic workspace: an
  internally scrolling request list remains visible above a persistent split
  request/response inspector. Tool views use the left rail and scroll only
  inside the application viewport; the document root never scrolls.
- The `system` theme follows the host light/dark preference live. Explicit
  light and dark choices override it and keep Monaco aligned with the shell.
- Response-body retention is enabled by default with a bounded one-GiB circular
  store and can be disabled. Text, binary, missing,
  truncated, redacted, and lossy evidence are distinct inspector states.
- Interactive breakpoints use one same-build controller. Closing the window,
  disabling breakpoints, timing out, or losing the controller fails unresolved
  decisions closed.
- Composer replay requires explicit acknowledgement for non-idempotent methods
  and credential-bearing fields and uses the canonical Rust HTTP/TLS stack.
- Native `.tmcap` files are the streaming source of truth. The desktop captures
  automatically while its proxy is running and can export a create-new, sealed
  TMCap snapshot without stopping the live source. JSONL export streams records
  sequentially. SAZ must be finalized and cannot preserve every native boundary
  or hook record, so each result includes a fidelity disclosure.
- Raster previews are decoded and normalized to PNG in an AppContainer helper,
  then displayed only through an opaque `<img>` URL. SVG stays SVG for fidelity
  but is also loaded only as an image from a no-store, `nosniff`, sandboxed CSP
  response that blocks scripts and external resources.
- Windows launches the script sandbox bootstrap, preview bootstrap, and
  PowerShell host helpers without creating visible console windows.
- Product state is schema-versioned, bounded, and committed as atomic
  generations. Corrupt newest state falls back to a prior valid generation or
  safe defaults. Persistence failure never blocks proxy shutdown.
- Structured diagnostics are bounded and redacted before memory or disk. A
  support bundle excludes traffic bodies, header values, credentials, keys,
  and paths by default, and reports the native WebView runtime version.

## Working in the desktop

Traffic keeps selection actions above the scrolling list. Table settings holds
column visibility, the inspector layout choice, and workspace reset. A selected
request exposes its full URL and copy action in a popover. The inspector shows
request and response together when space permits and switches between them in a
narrow pane. Detailed body retention information stays in a disclosure.

The traffic context menu and selection More menu offer **Copy as cURL**,
**Copy as PowerShell** on Windows, and **Copy all headers** for one entry.
These actions only put text on the clipboard; they never execute a request.
Windows cURL text is a PowerShell-compatible script that passes an encoded
configuration through standard input, avoiding native argument quoting differences
between PowerShell 5.1 and 7. PowerShell commands use the .NET HTTP client and
explain its normalization limits. Both preserve duplicate captured fields,
retain credentials unless capture redacted them, and regenerate body framing.
Commands that need a body file use a path placeholder; small PowerShell bodies
can embed even binary bytes as base64. Commands that can represent the request
directly go straight to the clipboard without opening a dialog. The file dialog's
Save as action saves complete original bytes with an eviction lease, then copies
a command referencing the selected path. Missing bytes and redacted values have
explicit placeholders. Copy all headers uses complete backend heads and puts
exactly two blank lines between the request and response blocks.

**Edit and replay** loads complete original request bytes as hex, preserving
Content-Encoding. It preserves an edited draft until the user chooses to replace
it. Missing bodies require a replacement or an explicit empty-body choice before
sending. The existing method and credential acknowledgements apply to replay.

Automation separates auto-responses, header overrides, and scripts. Rule
properties and batch review share the available space with the dense rule list;
narrow windows switch between the list and selected properties. Batch review
keeps skipped responses visible and preserves inclusion choices when refreshed.
Scripts distinguish draft, saved, validated, tested, and active revisions. Their
sandbox tester accepts a URL, method, headers, and body without sending traffic.

Breakpoints shows a live queue and one selected editor. The navigation badge
reports waiting requests even in other workspaces. Each request displays its
remaining decision time; expired requests cannot be continued. Replacement
drafts survive switching between waiting requests, and rejected edits remain
available for correction.

Composer opens from **Edit and replay** in Traffic. Request and response sit
beside each other in wide windows and use pane tabs in smaller ones. Loading
another request asks before replacing an edited draft. Sending leaves edits
available while the response arrives; failures keep the draft ready to retry.
Response headers, body, and execution details have separate views. Risk and
credential acknowledgements appear only when applicable.

Captures groups recording, inspection/recovery, and export into separate tasks
with native file pickers and readable result summaries. Recording controls
reflect the live state. Interrupted-file inspection reports the valid prefix
and can prepare a new recovered export; it leaves the source intact.

Settings separates preferences, connection, and support. Preferences have
explicit Save and Revert actions, with Ctrl+S to save. Connection presents the
current readiness and applicable recovery action; certificate paths and exact
certificate removal stay in Advanced. Support shows a readable diagnostic
summary before technical details, and including recent paths requires the saved
privacy preference. The header status menu links directly to Connection.
Routine workspace output stays near the action that produced it.

## File safety

Captures, generated CAs, JSONL, and SAZ use create-new semantics. Existing
destinations are not replaced. Imports are subject to explicit file, record,
and record-size bounds; interrupted native files recover only their checksummed
valid prefix.

Build and Windows prerequisites are documented in [building.md](building.md).
Product-state and support behavior is documented in
[product-state-and-support.md](product-state-and-support.md). Current gaps and
deliberately deferred platform work are maintained only in the
[roadmap](roadmap.md). Windows release qualification and eventual Linux/macOS
qualification remain separate platform gates.

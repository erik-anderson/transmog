# Transmog product shell

The Windows desktop shell is a thin Tauri adapter over `transmog-app`. The same
application facade is suitable for a future CLI: it owns proxy lifecycle,
bounded session queries, safe inspectors, breakpoints, replay, capture, import,
export, product state, and privacy-safe diagnostics without depending on Tauri
or WebUI.

## Operating model

Saved SAZ, TMCap and compressed `.tmcap.gz` files can be imported through **Traffic → Import…** or by
dropping them onto the traffic list. Each import keeps its source identity and
metadata; use **Trace metadata** on a selected imported request, or **Files →
Trace metadata…** to browse all sources. Network configuration is shown as
readable original console output, rather than being replaced with the viewer
machine's settings. Imported bodies are read on demand from the original file.
Compressed native files expand into a bounded, temporary reader owned by that
viewer; decoded body inspection remains lazy and the selected source is unchanged.

**Files → Open in a separate viewer…** creates an independent saved-capture
window with Traffic and Composer. It is labeled **Capture viewer** and has an
**Open main window** action. Proxy, certificate, recording, breakpoint and
automation controls belong to the main window, with backend permission checks.
Launching the app with a capture path opens a viewer if no main window exists.
If the main window is already open, file activation offers importing into that
session or opening a separate viewer. A drop always imports into the list where
it landed. Closing a viewer does not stop the main window's proxy.

Traffic search runs on **Search** or Enter. **Search options** selects literal
text or regular expressions, case sensitivity, accent handling for text,
metadata, headers and decoded text response bodies. It searches full retained
headers; compressed content and UTF text encodings are decoded for matching.
Binary formats are skipped without per-entry messages. Unavailable text and
text beyond the 16 MiB search limit are counted in the summary. Regular
expressions use a bounded engine without backreferences or look-around.

**Select all matches after searching**, **Select all matches**, and Ctrl+A in
search results work across every matching page. Searching again replaces old
selections. The selection menu offers **Remove selected entries** and **Remove
unselected entries** across the entire session, including hidden rows, with
Undo. Column filters remain applicable to search results and bulk selection.

- **Set up HTTPS interception** creates a durable PEM CA and matching private
  key when needed, then explicitly asks the user to approve current-user trust.
  CA generation is create-new and never overwrites either file. Start verifies
  the files, recorded SHA-256 identity, and current trust state before binding.
- Desktop Start always journals the exact current-user Windows proxy registry
  values, starts an automatic bounded native capture, binds the listener, and
  then applies the loopback proxy. Stop and normal/OS-requested exit restore the
  exact prior values. Stop restores routing first, then shows **Finishing
  requests** until admitted HTTP responses, TLS handshakes and upgraded relays
  finish. Idle clients do not delay shutdown. **Start proxy** during this state
  resumes the same listener and run; an old drain cannot stop the resumed run.
  The durable journal is recovered on next launch after a
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
cURL text starts with plain curl and uses POSIX shell quoting, suitable for Bash
on any supported platform; it has no PowerShell wrapper or executable suffix.
Windows PowerShell 5.1 aliases curl, so its users should choose Copy as PowerShell.
PowerShell uses Invoke-WebRequest with basic parsing for the portable 5.1/7 subset.
It uses .NET commands when duplicate fields, Cookie, restricted headers, a custom
method, an HTTP/1.0 version, or a body on GET/HEAD/TRACE need that fallback.
Both retain origin credentials unless capture redacted them and regenerate framing.
Generated notices explain normalization and fallback reasons.
cURL selects the recorded HTTP version and scopes Proxy-Authorization to its
proxy headers. PowerShell preserves HTTP/1.x, uses HTTP/1.1 for newer protocols,
and explains why proxy authentication must be configured separately.
Commands that need a body file use a path placeholder; PowerShell bodies
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

Recording captures, generated CAs, JSONL, and SAZ exports use create-new semantics.
Traffic **Save trace** and body **Save as…** can replace a destination after the
native file picker's overwrite confirmation; an unsuccessful save keeps the
existing file intact. Imports are subject to explicit file, record,
and record-size bounds; interrupted native files recover only their checksummed
valid prefix.

Build and Windows prerequisites are documented in [building.md](building.md).
Product-state and support behavior is documented in
[product-state-and-support.md](product-state-and-support.md). Current gaps and
deliberately deferred platform work are maintained only in the
[roadmap](roadmap.md). Windows release qualification and eventual Linux/macOS
qualification remain separate platform gates.

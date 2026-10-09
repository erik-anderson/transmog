# Transmog desktop

The Windows desktop is a Tauri/WebView2 adapter over `transmog-app`. WebUI
workspaces own presentation; the application facade owns traffic, proxy
lifecycle, capture, replay, persistence and diagnostics. Headless clients can
use the same facade without Tauri or WebUI. See [architecture](architecture.md)
for the dependency boundaries and [the desktop guide](../apps/desktop/README.md)
for build commands.

## Software updates

Startup checks run after the window is ready and keep the workspace usable. A
new stable release appears in Updates without taking keyboard focus. The user
can update now, download an update for normal app exit, or pause automatic prompts
for thirty days. Manual checking remains available in Updates and Support.
Offline startup stays quiet; a manual check reports a useful failure.

Downloads show progress and can be cancelled. Installation follows signature and
version verification, draft resolution, capture sealing, proxy shutdown, and
Windows host restoration. Close saved traffic viewer windows before installing
so their drafts can be resolved. A deferred update installs when the user quits Transmog
and does not reopen it. Crashes and operating-system shutdown do not launch an
installer. A failed safe handoff keeps the app open with recovery feedback.

## Start and stop

On first use, **Set up now** in the HTTPS setup banner, or **Settings → Connection
→ Set up HTTPS interception**, creates the desktop's CA and asks for approval to
trust its public certificate. Start verifies its recorded identity and trust
state before binding. Certificate installation and exact-root removal can
require Windows consent; private-key files belong to the desktop's user data.

**Start proxy** binds the listener and applies its endpoint to the current user's
Windows proxy settings. Traffic enters the configured body buffer; starting the
proxy does not implicitly create a trace file. Recording is a separate action.

**Stop proxy** restores the prior proxy settings first, then shows **Finishing
requests** while admitted requests, TLS handshakes and upgraded connections
finish. Idle clients do not delay shutdown. **Start proxy** during this state
resumes the same listener without disrupting active work. Closing the app uses
its shutdown flow; a journal restores interrupted host configuration on the next
launch after a hard termination. Setup and recovery actions are in Settings →
Connection. Certificate paths and exact removal are in Advanced.

## Live traffic and saved viewers

Traffic follows new exchanges until the user selects an entry or scrolls back.
Capture continues while the view is pinned. Filtering and sorting operate on all
retained metadata before pagination. Column visibility, inspector arrangement
and pane sizes are saved preferences.

Select a request to inspect its request and response together, or switch between
them in a narrow pane. The context menu and selection actions provide command
copying, timing reports, replay and reversible removal. See
[traffic inspection](traffic-inspection.md) for search, selection, header sizes
and request-copy behavior, and [request timings](request-timings.md) for how to
interpret latency and shared transport measurements.

**Import…** loads SAZ or TMCap files into the current window. Dropping files onto
the traffic list always imports them there. Each entry retains its source trace,
with **Trace metadata** leading to the original machine's network context and
capture metadata. Imported bodies are read on demand from pinned source files;
they are independent of live buffer eviction.

**Files → Open in a separate viewer…** opens saved captures in an independent
**Capture viewer** with Traffic and Composer. **Open main window** reaches the
proxy experience. Proxy, certificate, recording, breakpoint and automation
commands are restricted to the main window in both the UI and backend. Launching
with a capture path opens a viewer when no main window exists; otherwise the app
asks whether to import into the main session or open a separate viewer. Closing
a viewer does not stop the main window's proxy.

**Save trace…** saves retained traffic across pages and searches, with optional
password encryption, export-only header redaction and network context. Streaming
recording, recovery and native/JSONL/SAZ export are in Captures. See
[trace saving](trace-saving.md), [native capture encoding](native-capture-format.md)
and [SAZ compatibility](saz-compatibility.md) for file behavior. The separate
[CLI support recorder](cli-support-capture.md) is published outside the installer.

## Editing and replay

**Edit and replay** opens Composer from live or imported traffic. Edited drafts
survive navigation; replacing a draft asks for a decision. Sending requires
acknowledgement for non-idempotent methods and credential-bearing fields. Replay
uses the canonical verifying HTTP adapters even when the proxy is stopped.
Large captured or replacement-file bodies stream outside the WebView editor.
Response previews disclose display truncation; replay history keeps the source
association. See [traffic inspection](traffic-inspection.md#edit-and-replay).

Breakpoints presents one selected editor beside the waiting-request queue.
Remaining decision time stays visible, replacement drafts survive selection
changes and expired requests cannot be continued. Losing the controller,
disabling breakpoints or timing out fails unresolved decisions closed.

Automation separates auto-responses, header overrides and scripts. Captured
responses are the primary path to reusable auto-responses. Property edits remain
drafts until saved; active rule/script revisions and pause state are distinct.
See [automation](automation.md) for matching, ordering and sandbox behavior.

## Data and presentation boundaries

The shell uses a fixed viewport with scrolling inside each workspace. The system
theme follows Windows light/dark preferences; explicit themes also update Monaco.
Editors and secondary workspaces load on demand. Product state and diagnostics
are separate from captured traffic; failures saving either cannot block proxy
restoration. See [product state and support](product-state-and-support.md) for
retention preferences, diagnostics and support bundles.

Captured text stays inert in the workbench. Raster decoding uses a sandboxed
helper and images use an isolated resource origin. **Preview page…** is an
explicit Windows-only flow with its own browser profile, warning and script
choice; it serves captured resources while blocking uncaptured network access.
See [safe previews](safe-previews.md) and [captured-page preview](captured-page-preview.md).

Current support boundaries are in [limitations](limitations.md), future work in
[the roadmap](roadmap.md), and interaction conventions in
[UX design principles](ux-principles.md).

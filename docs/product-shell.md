# Transmog desktop

The Windows desktop is a Tauri/WebView2 adapter over `transmog-app`. WebUI
workspaces own presentation; the application facade owns traffic, proxy
lifecycle, capture, replay, persistence and diagnostics. Headless clients can
use the same facade without Tauri or WebUI. See [architecture](architecture.md)
for the dependency boundaries and [the desktop guide](../apps/desktop/README.md)
for build commands.

## Software updates

Windows x64 and ARM64 builds use the installer matching the installed app's
architecture. The existing update controls and channel preferences apply to both.

Startup checks run after the window is ready and keep the workspace usable. An
available release for the installed channel exposes an update action in the toolbar without taking
keyboard focus or navigating away from work. The user
can update now, download an update for normal app exit, or pause automatic prompts
for thirty days. Settings → About shows the installed version, update status, last successful
check, release notes and manual Check for updates. About also contains the
transmog.net website link and Credits with complete dependency license texts.
Offline startup stays quiet; a manual check reports a useful failure.

While the proxy is running, update checks and installer downloads use its active
listener and follow the usual capture and recording settings. With HTTPS
decryption enabled, their requests, redirects and responses appear in Traffic;
with decryption disabled, Traffic shows the HTTPS tunnels. Each operation uses
the current proxy state, including when it changes after a check. The updater
trusts the app's public interception root while routing through it and continues
to verify HTTPS certificates and installer signatures.

Downloads show progress and can be cancelled. Installation follows signature and
version verification, draft resolution, capture sealing, proxy shutdown, and
Windows host restoration. Close saved traffic viewer windows before installing
so their drafts can be resolved. A deferred update installs when the user quits Transmog
and does not reopen it. Crashes and operating-system shutdown do not launch an
installer. A failed safe handoff keeps the app open with recovery feedback.

Automatic update checks are enabled by default. Settings → About saves this
choice immediately, independently of ordinary settings and the thirty-day reminder.
Turning it off prevents startup network checks; manual checks remain available.
The installer offers the same choice and preserves it on updates and silent
installs. `/AUTOUPDATECHECKS=0` opts out explicitly; `=1` enables checking.

## Start and stop

The main window restores its saved size, position and maximized state across
launches, including on displays with Windows scaling.

On first use, **Set up now** in the HTTPS setup banner, or **Settings → Connection
→ Set up HTTPS interception**, creates the desktop's CA and asks for approval to
trust its public certificate. Start verifies its recorded identity and trust
state before binding. Certificate installation and exact-root removal can
require Windows consent; private-key files belong to the desktop's user data.

**Start proxy** binds the listener and applies its endpoint to the current user's
Windows proxy settings unless manual routing is configured. Traffic enters the configured body buffer; starting the
proxy does not implicitly create a trace file. Recording is a separate action.

**Stop proxy** restores the prior proxy settings first, then shows **Finishing
requests** while admitted requests, TLS handshakes and upgraded connections
finish. Idle clients do not delay shutdown. **Start proxy** during this state
resumes the same listener without disrupting active work. Closing the app uses
its shutdown flow; a journal restores interrupted host configuration on the next
launch after a hard termination. Setup and recovery actions are in Settings →
Connection. Certificate paths and exact removal are in Advanced.

Once active work finishes, physical connections have a bounded grace period to
close. An idle HTTP/2 client that does not acknowledge shutdown cannot leave the
proxy stuck in **Stopping** or prevent a new run.
**Start proxy** expedites that idle-connection cleanup before binding a new run;
it does not wait for the grace period to expire.

[Network profiles and connection settings](network-profiles.md) covers HTTPS
decryption, built-in and custom profiles, trust sources, listener access and
when changes take effect.

### AppContainer loopback access

Settings → Connection → **Manage loopback exemptions…** lists Windows
AppContainers with their display name, description, package, AppContainer name,
SID and users. Filter or sort the inventory, then use checkboxes, Ctrl-click,
Shift-click or keyboard range selection. **Exempt selected** allows those apps
to connect to a proxy on this computer; **Remove exemptions** reverses the
selection. Windows requests administrator approval. Canceling approval changes
nothing; failed or completed operations refresh the authoritative Windows state.
Saved exemptions with unavailable installed metadata remain visible for removal.
Selection is independent of exemption status; disclose selected rows hidden by a
filter and keep bulk actions outside the table scroller. Closing the modal during
an operation keeps monitoring its result; completion cannot reopen it or move focus.

Exemptions persist in Windows until explicitly removed. Transmog preserves
unselected exemptions and their attributes rather than replacing them with the
current selection. This follows the
[NetworkIsolation configuration contract](https://learn.microsoft.com/en-us/windows/win32/api/netfw/nf-netfw-networkisolationsetappcontainerconfig).
The verification script `scripts/test-loopback.ps1` exercises change planning
with fixture SIDs and only reads the real system inventory.

## Live traffic and saved viewers

Traffic follows new exchanges until the user selects an entry or scrolls back.
Capture continues while the view is pinned. Filtering and sorting operate on all
retained metadata before viewport windowing. Column visibility, inspector arrangement
and pane sizes are saved preferences.

Select a request to inspect its request and response together, or switch between
them in a narrow pane. The context menu and selection actions provide command
copying, timing reports, replay and reversible removal. See
[traffic inspection](traffic-inspection.md) for search, selection, header sizes
and request-copy behavior, and [request timings](request-timings.md) for how to
interpret latency and shared transport measurements.

WebSocket and SSE entries open a live **Messages / Events** inspector. Open streams
remain Active while quiet, separately from HTTP handshake completion. See
[stream inspection](stream-inspection.md) for message navigation, retention,
Stop/Start behavior and capture support.

**Files → Import traffic…** loads TMCap, SAZ, HAR or Chromium NetLog files into the current window. Dropping files onto
the traffic list always imports them there. The list's drop target highlights
accepted files, names the formats, and opens a picker with a click or keyboard.
The initial Traffic helper never returns after clearing or filtering.
Each entry retains its source trace, with **Details → Capture source** leading to the
original machine's network context and
capture metadata. Imported bodies are read on demand from pinned source files;
they are independent of live buffer eviction.

**Files → Import into separate viewer…** opens saved captures in an independent
**Capture viewer** with Traffic and Composer. **Open main window** reaches the
proxy experience. Proxy, certificate, recording, breakpoint and automation
commands are restricted to the main window in both the UI and backend. Launching
with a capture path opens a viewer when no main window exists; otherwise the app
asks whether to import into the main session or open a separate viewer. Closing
a viewer does not stop the main window's proxy.

**Ctrl+N** opens an empty Capture viewer from either window. The same action is
beside the **View** heading in Traffic's View flyout: **Open new viewer window**.
**Ctrl+W** closes the current viewer through the normal close flow, including
the unsaved-marker prompt; it does not close the main proxy window.
**Ctrl+O** opens the import picker for the current window and brings Traffic
forward. **Ctrl+,** opens Settings in the main window.

In Traffic, **Ctrl+F** focuses and selects the filter/search text without changing
the current mode. **Ctrl+Shift+F** selects Search contents and focuses that field.
**Ctrl+S** opens Save copy, or the progress dialog for a save already in flight.
Dialogs and editor-specific shortcuts retain priority. Unassigned modifier combinations and
held-key repeats do not start additional window or file operations.

**Files → Save copy…** saves retained traffic across the entire list and searches, with optional
password encryption, export-only header redaction and network context for TMCap
or SAZ, or HAR output for HTTP archive interchange. SAZ offers compatibility and
extended-fidelity choices. File-only recording and conversion are in Capture tools.
Inspect / recover offers a drop/picker target and Import trace action that bring
TMCap, SAZ or HAR files into Traffic. Inspect trace metadata reads the original
context and displays the full path without importing entries. Convert / export
converts native recordings, using `exported` in suggested filenames. Record to file
chooses a
destination before starting its proxy and writes requests directly to that file
without retaining them in Traffic. Switching from live capture waits for active
requests to finish and offers cancellation while waiting. Choose Headers only
or Headers and bodies. Stop recording seals the file and restores
Windows proxy settings. See
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

## Related captured-page previews

Baseline and modified [page previews](captured-page-preview.md) identify response
mode and script choice in their titles and trusted opening-window reports. The
main window alone can select workspace auto-responses. Capture viewers keep
baseline preview and the established main-window import route for using rules.
Reports and retained scene handles are checked against their opening window;
closing that window cancels preparation, closes its previews and releases retained
reports. Up to three recent runs retain bytes for baseline/updated iteration,
while older reports remain readable within the eight-report bound.

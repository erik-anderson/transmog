# Transmog desktop

Use the main window to capture traffic, then select a request to inspect, edit
or replay it. Capture viewers open saved traffic in separate windows.
For source code and build instructions, see [desktop development](../apps/desktop/README.md).

## Software updates

Updates use the installer for your configured channel and architecture (x64 or ARM64).

Startup checks run after the window is ready and keep the workspace usable. An
available release for the installed channel exposes an update action in the toolbar without taking
keyboard focus or navigating away from work. The user
can update now, download an update for normal app exit, or pause automatic prompts
for thirty days. Settings → About shows the installed version, update status, last successful
check, release notes and manual Check for updates. About also contains the
transmog.net website link and Credits with complete dependency license texts.
Offline startup stays quiet; a manual check reports a useful failure.

While a **System proxy** run is active, update checks and installer downloads
use its listener. With HTTPS decryption enabled, Traffic shows their requests,
redirects and responses. With decryption disabled, it shows the tunnels. The
updater uses the public interception root for that connection. In **Manual
proxy** mode it follows the computer’s normal network settings. Each check or
download uses the routing in effect when it starts. The updater verifies HTTPS
certificates and installer signatures.

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
launches, including on displays with Windows scaling. If its restored bounds are
entirely outside every connected display, startup moves it onto the primary
display (or another available display) before showing it, preserving its saved
size and maximized state. Partially visible windows and valid placements on
displays with negative coordinates keep their saved position.

The HTTPS setup banner offers certificate setup when a fresh start needs it.
**Interception certificate** under **Traffic inspection** in **Settings →
Connection** opens the same controls. Setup creates the desktop's CA. In
**System proxy** mode it also asks Windows to trust the public certificate.
For **Manual proxy**, apps that use Windows trust can use **Trust for my
Windows account…**. For an app with its own trust store, choose **Copy public
certificate path** and configure the app with that PEM. Starting with HTTPS
decryption verifies the CA identity before binding. System proxy also verifies
Windows trust. The target app determines whether it accepts the certificates
Transmog signs. Windows can ask for consent when adding or removing trust.

**Start with HTTPS tunnels** also works before certificate setup. It saves HTTPS
decryption as off and forwards encrypted connections without creating, loading,
or installing a CA. Selected plain HTTP remains inspectable. Certificate setup
later leaves that saved choice unchanged. Turn decryption on and save to inspect
new HTTPS connections without restarting capture. Turning it off lets active
requests finish and makes new requests reconnect without interception; existing
opaque tunnels and upgraded WebSockets continue. **Inspect traffic from** on
Connection selects all processes by default, with optional include or exclude
lists.

Open the routing button beside **Start proxy** and choose **Routing for next
start**. **System proxy** configures the current user’s Windows proxy settings.
With **Manual proxy**, start the listener, then copy its address into the target
app’s HTTP and HTTPS proxy settings. The toolbar shows the running mode.
Changing the saved mode takes effect on the next fresh start.

For an app on another device, first enable **Allow remote computers to connect**
under **Settings → Connection → Advanced proxy setup** and save the settings.
Start a new listener, then configure the device with this computer’s reachable
LAN address and the port shown in Transmog. [Listener settings](network-profiles.md#listener-and-outbound-settings)
covers port selection. Recording to a file is a separate action.

**Stop proxy** restores any Windows proxy settings owned by the run first, then
shows **Finishing requests** while admitted requests, TLS handshakes and upgraded connections
finish. Idle clients do not delay shutdown. **Start proxy** during this state
resumes the same listener without disrupting active work. The shared MCP Start
operation also resumes that listener using its existing routing and certificate.
Closing the app uses its shutdown flow. A journal supports recovery of interrupted
host configuration on the next launch in System proxy mode. Before changing proxy
settings, Transmog also attempts to register a current-user logon recovery task.
It restores the saved settings after an interrupted run even if Transmog is not
reopened. A live proxy run prevents background recovery from changing its settings.
Registration or task cleanup failures are diagnostic warnings and do not block
proxy use. Successful restoration removes the task. Failed restoration retains
the journal for retry.
Desktop and CLI captures share current-user ownership across their journals.
A new System proxy run first recovers an interrupted owner. Recovery and normal stop preserve
subsequent proxy, VPN, enable and bypass changes instead of replaying obsolete
settings over them.
An idle desktop can close or capture with manual routing while a separate CLI
run owns the Windows proxy; that live run is not reported as abandoned recovery.
Use **Recover host settings** in Settings → Connection for an interrupted
desktop or CLI run. Certificate paths and exact removal are under
**Interception certificate**.

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
Shift-click or keyboard range selection. **Add selected exemptions (N)** allows
selected apps that are not already exempt to connect to a proxy on this computer;
**Remove selected exemptions (N)** removes exemptions only from selected apps
that currently have one, regardless of which tool added it. Counts describe the
apps that would change; actions with nothing to change are disabled. For up to
three removal targets, the app names appear beside the actions. Selection stays
after applying changes so the same set can be changed again. Transmog does not
persist exemption ownership or change history.

Use **Exempt only** to find currently exempt apps. **Select all** selects the
whole inventory without a filter; **Select filtered** selects all matches when
search or Exempt only is active, including rows outside the scroll viewport.
Windows requests administrator approval. Canceling approval changes
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

The top bar's [Format converter](format-converter.md) opens an anchored scratch
utility for encodings and hashes. It can detach into independent native windows;
selected text and header-value context actions supply input without changing
the inspected traffic. Converter drafts are temporary and independent of proxy
state and capture-viewer lifetime.

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
Shift-clicking or middle-clicking the Windows taskbar icon also opens an empty
Capture viewer. Launching Transmog again without a file uses the same behavior;
the existing process remains the sole proxy owner.
**Ctrl+W** closes the current viewer through the normal close flow, including
the unsaved-marker prompt; it does not close the main proxy window.
**Ctrl+O** opens the import picker for the current window and brings Traffic
forward. **Ctrl+,** opens Settings in the main window.

In Traffic, **Ctrl+F** focuses and selects the filter/search text without changing
the current mode. **Ctrl+Shift+F** selects Search contents and focuses that field.
**Ctrl+Shift+Enter** expands the selected inspector or restores the Traffic list.
**Ctrl+S** opens Save copy, or the progress dialog for a save already in flight.
In Composer, **Ctrl+Enter** sends the current request after validation and the
applicable acknowledgements.
Dialogs and editor-specific shortcuts retain priority. Unassigned modifier combinations and
held-key repeats do not start additional window or file operations.

**Files → Save copy…** exports retained traffic with format-specific encryption,
redaction and context choices. [Trace saving](trace-saving.md) covers formats,
source metadata, import/recovery and conversion. **Save case…** shares selected
rules and evidence through [portable cases](portable-investigations.md#desktop-transfer-and-package-apis).

**Capture tools → Record to file** chooses a destination before starting its
proxy and writes directly to that file without retaining entries in Traffic.
Switching from live capture waits for active requests and offers cancellation
while waiting. Choose **Headers only** or **Headers and bodies**. Stopping seals
the file and restores any Windows proxy settings the recording changed. **Show in File Explorer** then
opens its folder with the saved recording selected. The [CLI
recorder](cli-support-capture.md) also runs without the desktop; it is included
in the installer and published separately.

## Editing and replay

**Edit and replay** opens Composer from live or imported traffic. Replay uses the
verifying HTTP adapters even when the proxy is stopped. See
[editing and replay](traffic-inspection.md#edit-and-replay) for draft preservation,
acknowledgements, body streaming and history.

Breakpoints presents one selected editor beside the waiting-request queue.
Remaining decision time stays visible, replacement drafts survive selection
changes and expired requests cannot be continued. Losing the controller,
disabling breakpoints or timing out fails unresolved decisions closed.

Automation separates auto-responses, header overrides and scripts. Captured
responses are the primary path to reusable auto-responses. Property edits remain
drafts until saved; active rule/script revisions and pause state are distinct.
See [automation](automation.md) for matching, ordering and sandbox behavior.

## MCP agents

Enable agent access and create scoped keys in **Settings → Integrations**.
The top MCP indicator shows connection activity; an **Unredacted** countdown
badge opens temporary-access revocation. Humans review agent rules in
**Automation → Rules**. See [MCP setup and permissions](mcp.md).

## Data and presentation boundaries

The shell uses a fixed viewport with scrolling inside each workspace. The system
theme follows Windows light/dark preferences; explicit themes also update Monaco.
Editors and secondary workspaces load on demand. Product state and diagnostics
are separate from captured traffic; failures saving either cannot block proxy
restoration. See [product state and support](product-state-and-support.md) for
retention preferences, diagnostics and support bundles.

Captured text stays inert in the workbench. Images use an isolated resource
origin. **Preview page…** opens captured resources in a separate browser profile
with an explicit script choice. The opening window owns the preview and its
reports. Closing that window cancels preparation and closes its previews.
See [safe previews](safe-previews.md), [page preview](captured-page-preview.md)
and [case previews](portable-investigations.md#desktop-previews-and-saved-comparisons).

Current support boundaries are in [limitations](limitations.md), future work in
[the roadmap](roadmap.md), and interaction conventions in
[UX design principles](ux-principles.md).

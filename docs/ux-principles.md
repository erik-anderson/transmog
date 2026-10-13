# UX design principles

Transmog is a traffic inspection and editing workbench. Its interface should
help users scan many entries, understand the selected item, and act without
losing their place or work. A feature is complete when its ordinary flow,
feedback, keyboard behavior, and constrained-window layout work together.

Use these principles when changing user-visible behavior, including backend
defaults and responses consumed by the desktop. The current implementation is
a starting point for consistency; a new component should still be reviewed for
its actual effect on the user's task. For a short per-change workflow, follow
[Maintaining UX quality](ux-quality.md).

## Organize around the task

- Upstream TLS and HTTP offers follow Chrome's behavior by default. Offer
  overrides belong in advanced settings; keep the ordinary capture path direct
  and retain the actual negotiated protocol and algorithms in connection evidence.

- Start from the user's next action. Keep the ordinary path direct: captured
  responses are the main source for auto-response rules; scratch authoring is
  secondary. Group distinct tasks into a few clearly named workspace tabs.
- Use a compact list and one selected-item properties pane for collections.
  Multiselection exposes useful batch actions. Selection, editing, and rule
  activation are separate operations and should have clear effects.
- Keep the primary action visible near the content it affects. Put occasional
  actions in the relevant action menu, advanced settings in disclosures, and
  technical evidence
  behind a readable summary. Avoid deep menu chains or hiding an essential
  action merely to save space.
- Show context when it becomes useful: selection actions after selection,
  recovery actions when recovery is needed, and credential or replay risk
  acknowledgements when the request requires them.

- Name counts for what they count. Traffic search counts matching entries;
  occurrence counts within an entry belong in the match view.

## Increase density without losing readability

- Remove repeated headings, duplicate status lines, oversized spacing, and
  unnecessary cards before reducing text size or control targets. Preserve a
  clear hierarchy through grouping, modest weight, and restrained color.
- Reuse shared control styles and spacing. Align text baselines across related
  labels, values, buttons, and table cells; centering their boxes is insufficient
  when font sizes or padding differ. Align checkbox text naturally with its
  control and keep labels close to their fields.
- Identify top-level navigations with a small neutral Page badge beside the
  traffic path and an accessible label. Use original client fetch metadata;
  do not infer navigation from HTML content, method or URL alone.
- Put fetch-destination filtering inside the existing Filters menu, using a
  checkbox dropdown. Combine selected destinations with OR, combine that group
  with other filters using AND, and show one removable chip for the group.
  No selections shows all traffic; an explicit empty destination is recorded
  fetch/XHR metadata, not missing metadata.
- Traffic headers and cells stay on one line and clip with ellipsis when columns
  are narrow. Keep complete values and stream state available on hover; column
  fitting includes the protocol’s stream badge.
- Give important labels room to wrap or reflow. Secondary URLs and metadata may
  use deliberate abbreviation or ellipsis when the complete value is readily
  available. A tooltip alone should not be necessary to discover an action's
  name, purpose, or state.
- Group editable response metadata under Response, with ten visible header lines.
  Original response context uses neutral styling; keep rule usage separate from
  source provenance.
- Let long-form editing use the available space. Response-body authoring starts
  at twelve lines. Preserve resize and expand affordances, with actions still
  reachable and drafts intact after restoring the editor. Body dialogs use
  Apply/Cancel; offer Save rule only when other properties have no unsaved changes.
  Formatting is explicit and undoable. Closing releases the editor model and
  returns focus to the opening action.
- Keep selection actions outside the scrolling list so populated tables cannot
  cover them. Empty-state presentation must not determine whether a populated
  workspace is usable.
- Reordering shows the insertion position before release, preserves selected
  groups and drafts, and supports edge scrolling. Cancellation restores the
  prior selection and focus; offer keyboard moves and Undo alongside dragging.

## Preserve the user's place and work

- Interception root keys use protection bound to the operating-system user.
  Protect new keys before writing files and migrate existing keys while preserving
  the certificate and its trust identity. Protection failures retain existing
  material and explain recovery; never silently replace a trusted root or save a
  plaintext key as a fallback.
  An inaccessible Windows DPAPI key is unlikely to recover. Explain this and
  offer the explicit certificate-reset flow: remove the exact recorded trusted
  root and old files before creating and asking Windows to trust a replacement.

- Startup update checks must keep the workspace usable and avoid taking focus.
  Keep version, manual checks, release details, the product website and Credits in
  Settings → About. A toolbar update action appears only for an available or
  pending update; background results never navigate away from the current task.
  Update consent specifies when installation happens. Resolve unsaved drafts and
  finish capture/proxy/host cleanup before installation can close the app. A
  thirty-day reminder pauses automatic prompts across subsequent releases.
  Update checks and downloads use the running proxy and its ordinary capture
  policy, so the app's own requests can be inspected alongside other traffic.
  Resolve the active route for each operation, including downloads after a check.

- Keep selection, scroll position, and drafts stable across live updates and
  workspace changes. Allocate one editor for the selected item rather than an
  editor for every row. Switching items must preserve or explicitly resolve an
  edited draft before replacing it.
- Distinguish draft, saved, validated, tested, enabled, and active state when
  those states differ. Saving must not silently activate a feature. A global
  auto-response pause changes hook participation while preserving each rule's
  enabled state.
- File response sources explain whether bytes are copied on save (up to 25 MB)
  or read directly per request. Keep linked paths visible, preserve missing-file
  configurations, and retain text drafts when clearing a pending file choice.
  Compression choices name the original type and apply consistently to edited
  text and replacement files, including linked files and binary bodies.
  Compression controls use canonical names: `x-gzip` is the `gzip` option.
  Captured headers, source context, and Traffic metadata show the server's
  actual coding token, including `x-gzip` when returned.
- Reject stale asynchronous results. A late preview, validation, or response
  must not overwrite newer edits, restore old selection, or switch the user
  away from work in progress. Prevent duplicate submissions while an action
  is pending, and retain usable input after errors.
- Favor Undo for reversible list actions. Use a focused confirmation when
  replacing edited work or taking a consequential irreversible action;
  routine selection, navigation, and reversible editing should remain fluid.

## Explain state and consequences plainly

- Use human-readable names, counts, times, units, and outcomes in ordinary UI.
  Keep opaque IDs, revisions, paths, and raw JSON available as technical detail
  when useful. Link to original Traffic only while the source is available;
  a durable saved response remains editable after its source disappears.
- Explain what changes and when it takes effect. Pattern placeholder names are
  optional annotations and do not affect matching. Saved and active revisions,
  global pause, disabled rules, and guaranteed shadowing must not look like the
  same state. Do not infer guaranteed duplicate rules from example URLs alone.
- Put progress, success, and actionable errors near the action that produced
  them. Keep shared status for conditions that matter across workspaces. A
  refresh must preserve an error instead of reporting success from stale state.
- Significant failures of explicit actions, including opening, importing, saving
  and exporting captures, open a modal error dialog with a clear title, failure
  details and a Close action. Escape dismisses it and returns focus to the task.
  Retained status and error messages have a dismiss control once work completes.
  Cancellation is ordinary feedback, and background refresh errors do not
  repeatedly interrupt the user.
- Make empty, unavailable, incomplete, truncated, redacted, and expired states
  distinguishable. Explain the next useful step. Disable an unavailable action
  with an accessible explanation rather than presenting a broken interaction.
- Mark active WebSocket and SSE streams with a compact protocol and Active
  badge beside the HTTP version in Traffic’s Protocol column, using the observed
  stream lifecycle. Quiet streams remain active; ended streams and saved captures have no live badge.
- Pending traffic means the exchange is still active. A terminal failure without
  an HTTP response shows Failed. Body completeness describes capture at its own
  message boundary; a later exchange failure does not make a fully retained body
  incomplete or establish whether the remote server processed it.
- Keep captured markup inert in the workbench. A captured-page preview is an
  explicit exception: open a separate isolated browser window after a warning,
  with scripts disabled by default and an Enable scripts choice for that window.
  Serve only captured responses; misses return an empty 404, and captured content
  cannot access application commands. Distinguish preview content from complete
  original data. Copy, save, edit, and export labels describe the data used.
- Auto-response matching uses authored conditions; captured Vary headers must not
  add implicit browser restrictions. Preview matching uses method and URL within
  the chosen source scope, with body, browser/process, Origin/Referer, Sec- headers
  and navigation timing as preferences. Prefer later captured responses for
  repeated URLs during a load, reset progression on reload, and relax hints to
  available method/URL matches instead of returning avoidable 404s.
- Copying a request command only copies text; it never executes traffic. File
  placeholders must explain what bytes are needed and offer Save as when complete
  bytes are available. Use complete backend data for copy/replay, preserve encoded
  bodies, and require an explicit replacement or empty-body choice for missing
  captured data before sending.

## Make input and layout predictable

- Use native buttons, form controls, dialogs, popovers, and disclosures with
  accessible names, visible focus, and logical focus order. Restore focus to a
  useful control after closing an editor or dialog. State needs a text or
  semantic indication as well as color; disabled text must remain readable.
- Managed lists share selection conventions: Ctrl+click adds an item, Shift
  selects a range, Shift+arrow and Ctrl+Shift+arrow extend selection, Ctrl+A
  selects, and Escape clears selection. Where supported, Ctrl+arrow moves
  focus independently, Del removes selected entries, and Ctrl+Z undoes a list
  action. Text fields retain their normal editing shortcuts. Ctrl+S saves
  drafts where there is an explicit Save action.
- Adapt to the space available to a pane, including user-resized splits.
  Preserve comparison in wide layouts; use list/detail or request/response
  switching when narrow. Keep navigation and important actions reachable.
  Scroll within the appropriate pane instead of letting content push the
  application shell beyond the viewport.
- Respect system theme, forced colors, reduced motion, zoom, and longer labels.
  Do not rely on hover alone to expose essential information or on drag alone
  to perform an action. A drag flow must have a reachable destination and a
  keyboard or menu alternative.

## Review the whole flow

Saved-capture windows are explicitly labeled **Capture viewer** and contain
Traffic and Composer. Keep proxy, certificate, capture-recording, breakpoint,
and automation controls in the main window; enforce these roles in the host as
well as in the UI. Opening a capture while the main window exists asks whether
to merge it into that session or open a separate viewer. Dropping a capture on
the traffic list always imports it there. Imported entries retain their source
trace association, with a direct action to view that trace's original metadata.

Traffic's drop helper appears only before any entries have been loaded in that
window. Clearing entries or filtering them out must not bring the helper back;
the list remains a drop target. **Files** groups importing into the current
window, saving a trace, importing into a separate viewer, and viewing current
imports. Disable current-import metadata when this window has no imports, and
return focus to **Files** after closing its metadata dialog.
Copy URL beside Copy headers uses the complete corresponding request target from
retained evidence, without display truncation.

Composer keeps its request editor and uses the shared message inspector for live
responses, including images, raw Hex, decoding, header actions and Save as. Keep
the latest complete received response separate from Traffic and retain it until
another response replaces it or the window closes. A pending send must preserve
later draft edits and avoid switching an edited request to the response pane.
Save as leases the original received body while the file dialog is open, so a
later response or window closure cannot change the bytes being saved. Preserve
the original captured-request association after editing and sending a draft.

Trace drop targets name the accepted formats (TMCap, SAZ, HAR, and NetLog), offer
a click/keyboard file picker, and highlight the actual accepting region during
a supported file drag. Unsupported files, inactive workspaces, and busy imports
do not show an accepting state. Captures' Inspect / recover workspace offers the
same import target and a separate Import trace action; both bring the file into
Traffic. Inspect trace metadata is read-only, shows the absolute source path and
original context, and does not add Traffic entries. Convert/Export uses
`exported` in suggested filenames, including recovered native prefixes.

Desktop traffic is transient. Closing a main window releases its retained traffic
and body cache; saved files remain. **Clear all** affects the whole window and
retains Undo for small removals. For at least 100 MB, Undo expires after five
minutes and releases the data; at least 1 GB is released immediately without Undo.

The Capture Record flow chooses a destination before starting a file-only proxy
run. It does not populate Traffic; Stop restores routing and seals that file.

**Save trace** saves every retained entry in the current window, across the list and
search results, excluding entries removed from Traffic. Save native traces as
`.tmcap`; TMCap compresses individual chunks and opens payloads on demand.
Offer SAZ compatibility and extended-fidelity choices alongside TMCap and HAR in
Save trace. Extended SAZ preserves original source associations. SAZ includes
retained WebSocket messages with explicitly disclosed framing normalization;
TMCap preserves full stream evidence, including parsed SSE events. SAZ includes
exchanges missing a request or response head using clearly labeled
compatibility placeholders, with an explanation in Fiddler session comments and
message headers. Missing responses use a labeled 599 status, or 502 for a recorded
DNS, TCP or TLS setup failure, and explanatory
text only when no response bytes were retained. Preserve captured heads and body
bytes, report placeholder counts, and restore missing-head state on Transmog
reimport. Clearly label generated connection summaries, report their count
separately, and distinguish interception TLS from opaque CONNECT context without
inventing handshake details. Export one summary per upstream physical connection,
including its recorded failure, and interleave summaries with requests by start
time. Distinguish the last observed snapshot from an unrecorded socket close.
Save progress stays visible while writing, converting and finalizing,
including after closing its dialog; closing does not cancel the save.
Preserve original source metadata and request associations when saving merged
captures. Network configuration from the saving computer is
an explicit, unchecked option and stays separate from imported source context;
show collection time, platform and failures in Trace metadata. Offer unchecked export-only sensitive-header redaction without changing retained
evidence. Explain that URLs, bodies and metadata may still contain private data.
Password encryption is an unchecked AES-256 option for saved traces, recordings
and native/SAZ exports. Ask for confirmation of a new password and explain that
it cannot be recovered. Encrypted imports ask only when required, allow retry
and Cancel, and leave Traffic unchanged on failure. Keep passwords out of
preferences, receipts, logs and initial UI state; clear dialog fields on close.
ZIP member names remain visible. JSONL has no encryption option.

Show collector, machine, purpose and time for newly collected network context;
older context stays explicitly original without invented provenance. Report
incomplete bodies and keep existing destination files intact when a save fails.

The installer offers independent TMCap, SAZ and HAR handler choices, with TMCap
checked on a first install. Preserve saved choices on updates and existing Windows
defaults, explain how Windows may ask for a default-app choice, and remove only
registrations owned by the uninstalling executable's exact path. Install upgrades
over the existing version after proxy and capture cleanup, preserving preferences
and saved captures; do not suggest uninstalling first.

Turning the proxy off restores the host proxy settings first. Show **Finishing
requests** while admitted requests, TLS handshakes or upgraded connections
finish; idle clients do not delay shutdown. Keep **Start proxy** available during
this state so turning it back on resumes the same run without disrupting work.
Once active work finishes and the run commits to shutdown, **Start proxy** must
expedite idle-connection cleanup and start a new run without waiting for peer
acknowledgements.
Open WebSockets and SSE responses remain active even between messages. Keep their
HTTP result separate from stream lifetime, and include them in Active filters.
Stop proxy preserves established streams; app exit interrupts them with bounded
cleanup. Report open streams separately from ordinary requests and setup work.
Preserve full stream history and complete messages for retained sessions. Large
valid captures may use proportionate memory; do not silently truncate payloads or
evict messages at fixed count or byte thresholds. Page the inspector and load
saved content on demand. Honor explicit privacy and whole-session retention choices.
Reject stale shutdown and status results after a new state transition.

Honor explicit manual client routing: when system proxy configuration is disabled,
starting the listener must not change host routing or require host-wide CA trust.
Managed CA validation and private-key protection still apply; HTTPS clients supply
their own trust. The ordinary desktop connection flow uses Windows routing.

Outbound connections are isolated by caller process, with source IP as fallback,
and by each HTTP/2 browser session. Keep the desktop control to one enabled-by-default
checkbox in Advanced network settings. Outbound protocol choices apply when
creating connections; established connections may continue serving new requests
until they close. Saving these choices must not restart the proxy or interrupt traffic.

Client certificate authorization lives behind Advanced network settings. Its
default continues without a certificate; ordered exact host/port rules can
continue without one, pause for selection, or select one compatible Windows
software identity automatically. Rule edits remain drafts until their explicit
Save. Store only public identity fingerprints and authored subject/issuer
attributes. Independent settings, layout and window saves must preserve this
authorization. Recovery from an uncertain latest preference generation disables
recovered certificate rules until the user reviews and re-enables them.
That disabled state survives subsequent launches and interrupted recovery.

Recovery from uncertain preference generations turns remote access off until
the user explicitly enables it again.

An actual verified server CertificateRequest can create a blocked-connection
indicator. Keep selection nonmodal and preserve work in other panes. Show the
host, owner, public certificate subject/issuer, expiration and fingerprint.
Offer a certificate, continuation without one, or cancellation for those live
connections only; answering never silently creates a saved host rule. Peer
closure, deadlines, policy changes and closing the owning window expire the
choice. Preserve an inspected expired context with disabled actions, reject
late answers, and avoid stealing focus during background refresh or light
dismissal. Capture viewers refer selection to the main window. Composer's
received-response certificate view uses its actual recorded connection evidence.
Closing the main window stops its own work while open viewers retain their
connections and can reopen the main window for certificate selection.

Disabling HTTPS decryption forwards opaque CONNECT bytes and records the destination,
connection outcome and byte counts without implying knowledge of the tunneled protocol
or contents. This choice applies on the next proxy start. Starting with decryption off
does not require interception certificate creation, key protection or host CA trust.

Live entries have no count limit by default. Offer an optional maximum that drops
older completed live entries, preserves imported entries and lets active requests
finish before eviction. Changing this limit applies immediately. Body bytes have
a separate budget; the body limit is not a total application memory ceiling.

Hex addresses every retained byte without a total display-size ceiling. Load
visible pages on demand, keep exact offsets, and preserve selection while paging.
The scrollbar spans the whole body; keyboard navigation and Go to offset reach
exact positions even when native scrollbar pixels cannot. A preview limit belongs
to bounded text and decoded previews, not the raw Hex view. Clipboard selections
have a separate bounded budget with Save as guidance for larger selections.
Compressed SAZ bodies may be validated and unpacked once into an owned temporary
byte cache for random access; release it with the imported body and its readers.
In a narrow Traffic inspector, put the request/response selector beside the URL
and status so switching messages does not consume an extra row above the body.

Live body retention uses a configurable circular buffer. Automatic chooses half
installed physical RAM, stored in memory. Custom limits up to that threshold
also stay in memory; larger limits and **No max size (writes to disk)** use an
application-owned disk cache. Starting the proxy never implicitly records a
trace. Save trace and explicit recording write files. Display the resolved limit,
storage location and installed RAM in Settings. Changing storage applies to new
body boundaries without spilling previously retained memory bodies to disk.

Retaining Traffic and recording captures apply flow control: a full event or
storage queue slows requests until the consumer catches up. Queue pressure must
not silently discard entries, body chunks or completion evidence. Keep queues
bounded and report actual storage failures; configured retention budgets and
explicit body limits still govern what data is kept.

Request-body retention defaults to **25 MB** (25,000,000 bytes). **Custom limit** accepts decimal MB values from 1 MB through 1 TB
(1,000,000 MB) and preserves the authored choice on reload. **Unlimited**
removes the per-request cap while preserving the selected circular buffer budget.
Saved trace files have no byte-size or record-count ceiling; storage errors remain
actionable failures. Keep individual encoded chunks bounded.
Keep forwarding independent from capture limits; report a retained prefix as
incomplete while preserving full measured byte counts. Explain that cache
changes apply to new requests and recording changes to the next recording.

Traffic defaults to a live metadata filter for URLs and processes, applied across
all retained entries before virtual windowing. **Search contents** is an explicit
mode that starts with **Search** or Enter, so typing does not repeatedly
read and decode captured bodies. Results describe a completed search snapshot;
new traffic remains capturable and a new search refreshes that snapshot. Search
can select every matching entry across the entire list, and rerunning it replaces the
selection with current matches. Remove selected and remove unselected act on
the whole workspace and support Undo. Skip inapplicable binary bodies quietly;
report unavailable text evidence once in the search summary.

Traffic is one continuous virtualized list. Its native scrollbar represents the
whole matching collection, with only a small window of rows rendered. Selection,
keyboard focus and range anchors use stable request IDs and the complete display
order; scrolling a row out of the DOM never deselects it. Ctrl+A selects the
current filtered/search collection, including offscreen entries, at that moment.
Later traffic is not automatically selected. Del and Undo act on those IDs.
Keep row geometry predictable in compact and ordinary modes. Wrapped cells use
measured heights with scroll anchoring to preserve the request being read.
Search match review reveals and marks the current request in Traffic while
preserving multiselection. Closing review returns focus to that request.

User-Agent and client-hint overrides use a top-to-bottom list: the first enabled
match wins, with drag handles and reachable Move up / Move down alternatives.
Editing a draft does not affect live requests until Save. Identical filters ask
before replacing the existing rule and retain its position. Deleted rules offer
Undo. Preserve hints without authored replacements; allow explicit stripping.
Preset replacements update existing headers only. Show enabled non-auto-response
automation in navigation and enabled header rules beside Header overrides.

The selected request’s **Details** dialog groups **Timings**, **Connection**,
**Certificates**, and (for imports) **Capture source**. Keep headers, body inspection,
and Copy URL directly accessible; occasional request operations belong in **Actions**.
**Files** groups import and save; **View** groups columns, density, and layout.
**Compare messages** is the default wide layout. Narrow inspectors show one message
with a request/response selector. Header byte sizes are an optional disclosure.

The Certificates tab opens the TLS context of that request's physical connections.
Separate the client-facing proxy certificate from the origin server path and
client authentication certificates. Show public certificate bytes, recorded
verification and negotiation failures together. Label fingerprints or imported
summaries whose certificate bytes were omitted. Recorded trust belongs to the
original connection; opening the viewer must not imply verification against the
current computer's trust or expose private keys or session secrets.
Prefer complete public bytes when another reference to the same fingerprint
contains them. Default to the latest successful upstream connection, then the
latest failed attempt when none succeeded. Include the role, outcome and identity
in the selector so attempts can be distinguished before selecting them.

Show negotiated exchange groups and handshake signatures for each physical
connection's own leg. Do not infer a missing algorithm from a certificate key,
cipher or configured preference; older captures show Unavailable. Distinguish
a resumed TLS 1.3 handshake's current group from its original authentication
signature and restored certificate chain. For TLS 1.2 resumption, label the
original session group and report that no fresh key exchange occurred.
Hybrid exchange or ML-DSA authentication alone
must not produce a claim that the whole trust chain is post-quantum secure.
Start the connection view with a compact summary of the recorded protocol,
current exchange group and cipher. Order negotiation details explicitly, placing
resumption before the original-session facts it qualifies. Keep configured policy,
trust provenance and additional imported metadata in secondary disclosures.
Avoid duplicate protocol and cipher rows; surface conflicting observations
without discarding their original text. Show invisible controls, including NUL,
as Unicode escapes, noting when display text differs from the original.
Preserve the original capture values in exports.

The Timings tab opens a snapshot of the selected exchange, with an explicit Refresh.
Present measured local milestones and overlapping latency phases. Distinguish
zero from unavailable, shared connection setup from per-request work, and bytes
queued by the proxy from delivery to the client. Response wait does not identify
server CPU time. Collapse local proxy work under 1 ms by default and explain
measured call time once, without repeating it on every row. Keep original imported timers available with their own labels
and clocks, without inventing missing measurements. Copied heads mark unrecorded versions
as HTTP/[unavailable]; replay commands explain their compatible fallback.


For a UI change, walk through the affected task with representative populated
data as well as the empty state. Inspect wide and narrow layouts, resized panes,
long labels and values, selection and multiselection, and relevant loading,
error, and unavailable states. Check actual pointer reachability, keyboard
navigation, focus restoration, baseline alignment, and editor/action visibility.
A successful build or an empty-state screenshot does not establish these.

Use the checks in [the UI instructions](../apps/desktop/ui/AGENTS.md), including
real WebView2 checks when native behavior, accessibility, or layout is affected.
Inspect screenshots for visual changes. Add behavior or layout regression
coverage for important interactions, asynchronous state, or a discovered
failure; avoid tests that merely repeat implementation details or lock in
incidental pixel values. Documentation-only edits do not require app builds.

Current flows and their semantics are described in
[the product shell](product-shell.md), [automation](automation.md), and
[product state and support](product-state-and-support.md). When a deliberate
design change replaces a convention, update the relevant guidance along with
the implementation so future changes have one current explanation to follow.

File recording chooses a destination before stopping live capture. Switching waits
for active requests to drain and offers cancellation while waiting. Label the
content choice **Headers only** or **Headers and bodies**; recording writes new
traffic to the file without adding it to Traffic.

Desktop proxy access defaults to loopback only. Remote computers require an explicit
persisted opt-in. The proxy port defaults to automatic selection; an authored
port from 1 through 65535 is retained and an occupied port produces an actionable
start failure. Address and port changes apply to the next fresh proxy start.
Windows routes through loopback even when the listener accepts remote clients.

Server connection reuse is enabled by default. Turning it off gives subsequent
requests dedicated HTTP/1, HTTP/2 and HTTP/3 connections, including Composer in
capture viewers. Active requests and streams finish normally. Keep HTTPS
decryption visible at the top of Preferences, above the advanced disclosure.

Automatic update checks are enabled by default. Settings → About saves this
choice immediately, independently of ordinary settings and the thirty-day reminder.
Turning it off prevents startup network checks; manual checks remain available.
The installer offers the same choice and preserves it on updates and silent
installs unless the user explicitly changes it.

AppContainer loopback exemptions are managed in a modal from Connection.
Keep installed metadata, users and existing Windows exemption status together;
group repeated per-user records by AppContainer SID. Selection is independent of
exemption status, supports the app's checkbox/range/keyboard conventions, and
discloses selected rows hidden by a filter. Keep bulk actions outside the table's
scroller. Preserve unselected Windows exemptions. Closing the dialog during an
operation keeps monitoring its result; completion cannot reopen it or move focus.

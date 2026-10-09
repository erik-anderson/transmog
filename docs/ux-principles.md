# UX design principles

Transmog is a traffic inspection and editing workbench. Its interface should
help users scan many entries, understand the selected item, and act without
losing their place or work. A feature is complete when its ordinary flow,
feedback, keyboard behavior, and constrained-window layout work together.

Use these principles when changing user-visible behavior, including backend
defaults and responses consumed by the desktop. The current implementation is
a starting point for consistency; a new component should still be reviewed for
its actual effect on the user's task.

## Organize around the task

- Start from the user's next action. Keep the ordinary path direct: captured
  responses are the main source for auto-response rules; scratch authoring is
  secondary. Group distinct tasks into a few clearly named workspace tabs.
- Use a compact list and one selected-item properties pane for collections.
  Multiselection exposes useful batch actions. Selection, editing, and rule
  activation are separate operations and should have clear effects.
- Keep the primary action visible near the content it affects. Put occasional
  actions in More, advanced settings in disclosures, and technical evidence
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
- Give important labels room to wrap or reflow. Secondary URLs and metadata may
  use deliberate abbreviation or ellipsis when the complete value is readily
  available. A tooltip alone should not be necessary to discover an action's
  name, purpose, or state.
- Let long-form editing use the available space. Response-body authoring starts
  at twelve lines. Preserve resize and expand affordances, with actions still
  reachable and drafts intact after restoring the editor.
- Keep selection actions outside the scrolling list so populated tables cannot
  cover them. Empty-state presentation must not determine whether a populated
  workspace is usable.

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
  Update consent specifies when installation happens. Resolve unsaved drafts and
  finish capture/proxy/host cleanup before installation can close the app. A
  thirty-day reminder pauses automatic prompts across subsequent releases.

- Keep selection, scroll position, and drafts stable across live updates and
  workspace changes. Allocate one editor for the selected item rather than an
  editor for every row. Switching items must preserve or explicitly resolve an
  edited draft before replacing it.
- Distinguish draft, saved, validated, tested, enabled, and active state when
  those states differ. Saving must not silently activate a feature. A global
  auto-response pause changes hook participation while preserving each rule's
  enabled state.
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

Desktop traffic is transient. Closing a main window releases its retained traffic
and body cache; saved files remain. **Clear all** affects the whole window and
retains Undo for small removals. For at least 100 MB, Undo expires after five
minutes and releases the data; at least 1 GB is released immediately without Undo.

The Capture Record flow chooses a destination before starting a file-only proxy
run. It does not populate Traffic; Stop restores routing and seals that file.

**Save trace** saves every retained entry in the current window, across pages and
search results, excluding entries removed from Traffic. Save native traces as
`.tmcap`; TMCap compresses individual chunks and opens payloads on demand.
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

The installer offers SAZ registration as an explicit choice. Preserve an existing
Windows default, explain how Windows may ask for a default-app choice, and remove
only registrations owned by the uninstalling executable's exact path.

Turning the proxy off restores the host proxy settings first. Show **Finishing
requests** while admitted requests, TLS handshakes or upgraded connections
finish; idle clients do not delay shutdown. Keep **Start proxy** available during
this state so turning it back on resumes the same run without disrupting work.
Reject stale shutdown and status results after a new state transition.

Live entries have no count limit by default. Offer an optional maximum that drops
older completed live entries, preserves imported entries and lets active requests
finish before eviction. Changing this limit applies immediately. Body bytes have
a separate budget; the body limit is not a total application memory ceiling.

Live body retention uses a configurable circular buffer. Automatic chooses half
installed physical RAM, stored in memory. Custom limits up to that threshold
also stay in memory; larger limits and **No max size (writes to disk)** use an
application-owned disk cache. Starting the proxy never implicitly records a
trace. Save trace and explicit recording write files. Display the resolved limit,
storage location and installed RAM in Settings. Changing storage applies to new
body boundaries without spilling previously retained memory bodies to disk.

Request-body retention defaults to **25 MB** (25,000,000 bytes). **Unlimited**
removes the per-request cap while preserving the selected circular buffer budget.
Saved trace files have no byte-size or record-count ceiling; storage errors remain
actionable failures. Keep individual encoded chunks bounded.
Keep forwarding independent from capture limits; report a retained prefix as
incomplete while preserving full measured byte counts. Explain that cache
changes apply to new requests and recording changes to the next recording.

Full traffic search starts with **Search** or Enter, so typing does not repeatedly
read and decode captured bodies. Results describe a completed search snapshot;
new traffic remains capturable and a new search refreshes that snapshot. Search
can select every matching entry across pages, and rerunning it replaces the
selection with current matches. Remove selected and remove unselected act on
the whole workspace and support Undo. Skip inapplicable binary bodies quietly;
report unavailable text evidence once in the search summary.

Timings opens a snapshot of the selected exchange, with an explicit Refresh.
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

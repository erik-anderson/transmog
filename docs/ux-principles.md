# UX design principles

Transmog is a traffic inspection and editing workbench. Help users scan entries,
understand evidence and act without losing their place or work. These principles
apply to UI, commands, defaults, validation and asynchronous behavior. Follow the
[UX quality routine](ux-quality.md) and [UI instructions](../apps/desktop/ui/AGENTS.md)
when changing the experience.

## Organize around the task

- Keep collecting, filtering, selecting and inspecting traffic direct. Use a few
  clearly named workspaces, compact collections and one selected-item properties
  pane. Multiselection should reveal useful batch actions.
- Keep ordinary traffic import and save ahead of case sharing. Cases are an
  occasional sharing workflow, grouped secondarily; opening a case leads toward
  inspecting its traffic. Recognize supported file kinds through normal opening
  paths so users need not choose a specialized command first.
  Case sharing uses one native package format with optional password protection;
  keep packaging internals out of the save flow. Traffic interchange formats,
  including SAZ, remain in the ordinary traffic workflow.
- Review markers alongside traffic, with concise notes and actions sharing
  horizontal space. Marker navigation reveals time or linked evidence; an explicit
  highlight action selects the closest request. Group time-filter inputs with
  their Apply action. Preserve edits in a new copy and distinguish clearing
  note text from deleting its marker.
- Keep primary actions beside their content. Put occasional actions in menus,
  advanced settings in disclosures and technical evidence behind readable
  summaries. Avoid deep navigation and repeated toolbars.
- Preview preparation and diagnostics stay on the originating saved-item view.
  Keep its selection, scroll, drafts and visible return focus through launch or
  cancellation; navigate to source evidence only through an explicit source action.
  Place activation Undo with its generation-valid outcome and exact restore target,
  independently of feedback from unrelated saves or organizational changes.
- Optional introductory help never opens or moves focus automatically. Offer it
  briefly, keep a permanent replay path, and teach through the current saved
  view without requiring content changes. Preserve per-user dismissal history
  independently of layout resets; leaving the view closes its guide safely.
- Present built-in traffic changes as rules named by their outcome, with request
  and response direction visible. Share discovery and properties while preserving
  each operation’s execution and activation semantics. Group secondary actions
  into at most one level of submenus; retain direct actions for the current task.
- Keep selection, editing, saving and activation understandable. A combined
  save-and-activate action is appropriate when its label explains the effect and
  validation and persistence succeed before activation. Keep saving a draft
  available separately; saved, enabled, active and paused are different states.
- Saved debugging workspaces keep one editable current collection and explicit
  named milestones. Save the whole draft without navigation or per-item save
  prerequisites. Keep the applied live snapshot independent of current saves,
  named versions and durable demos; activation is always explicit.
- Collect existing work through visible source and destination choices. Default
  to copies; moving removes originals atomically only after destination storage
  succeeds, with active-effect consequences explained before submission.
- Durable demonstrations retain the exact reviewed available evidence and
  modifications. Later edits, external files, retention and restart cannot change
  playback; replacement and deletion are explicit. A page association alone
  remembers identity and does not retain or silently replace evidence.
- Saved-item activation controls act immediately on saved configuration and
  preserve property drafts. Give editors a direct Discard changes action and
  persistent save state; unchanged saved content disables Save. Keep activation,
  draft persistence and unapplied revisions separate, including in expanded editors.
- Navigation counts share a definition across levels and identify their units.
  Count owned content once, expose pause reasons and distinguish configured
  participation from observed matches.
- Show recovery and risk acknowledgements when relevant. Name counts for what
  they count, with human-readable units and outcomes. Keep opaque identifiers
  available as technical detail.
- Distinguish exporting current Traffic from converting a saved file. Source
  inputs start empty; examples and hints must never act as selected file paths.
- Explain copy and export consequences beside the action. A clear action label
  and concise disclosure are sufficient for routine saves; avoid redundant
  acknowledgement checkboxes. Describe capture limitations without implying
  that normal protocol or retention behavior is a failed export.
- Runtime UI and help describe the current platform directly. Omit qualifiers
  such as “On Windows”; select the applicable copy when behavior varies by
  platform. Keep OS product names that identify a setting or feature.
  Documentation may compare platforms.

## Make layout and input predictable

- Remove repeated labels, excess spacing and unnecessary containers before
  reducing text size or control targets. Reuse styles and align text baselines.
- Keep pinned action rows outside scrolling fields so inputs never pass beneath
  them. Reuse surrounding context instead of repeating inspector headings, and
  show technical guidance beside the cases that need it.
- Keep actions outside list scrollers. Preserve comparison in wide panes and
  provide list/detail or request/response switching in narrow ones. Scroll within
  workspaces so content cannot push navigation beyond the viewport. Temporary
  inspector expansion preserves selection and reading position without changing
  saved splits; keep the return to the list visible and available by keyboard.
- Important labels wrap or reflow. Abbreviated metadata must have an accessible
  complete form; essential actions and state must not depend on tooltips.
- Use native controls with accessible names, visible focus and logical tab order.
  Restore focus after menus, dialogs and editors close. State needs text or
  semantics as well as color. Respect theme, forced colors, reduced motion,
  zoom and long labels.
- Debugger source selections can become watches or explicit Console commands.
  Watch expressions remain editable and survive closing the debugger during the
  app session; runtime values keep their shorter worker lifetime.
- Lists share Ctrl+click, Shift ranges, Shift+arrow, Ctrl+Shift+arrow, Ctrl+A and
  Escape. Where supported, Ctrl+arrow moves focus, Del removes and Ctrl+Z undoes.
  Text fields retain ordinary editing shortcuts; Ctrl+S saves an explicit draft.
- Prefer familiar shortcuts for existing actions and show them beside the action
  or in its tooltip. Scope window and workspace commands to their context; let
  dialogs and editors handle their own keys first. Holding a shortcut must not
  repeatedly open windows or file dialogs.
  Reserve workspace layout shortcuts before ordinary list navigation or button
  activation so their behavior does not depend on which control has focus.
- Application chrome and selectable lists do not select text during pointer or
  range selection. Editors and inspection evidence, including headers, bodies,
  full URLs and reports, retain ordinary text selection and copying.
- Read-only inspection copy menus target the item under the pointer independently
  of selection. Use complete underlying evidence for field values; name previews
  accurately. Offer only actions supported by the context and preserve masking in
  ordinary Auth subtree copies. See [copying inspection values](copying-inspection-values.md).
  Anchor pointer menus at the cursor, adjusting for their actual size at window
  edges. Omit actions that have no meaning for the target; explain disabled
  actions only when a relevant value is unavailable. Text-block labels do not
  turn their contents into name/value fields. Keep captured body text separate
  from explanatory prose so copying evidence never includes a status message.
- Removing Traffic keeps open live WebSockets inspectable unless the user has
  explicitly enabled termination on clear. Cancellation stops selected live work
  while preserving its evidence; Undo never reconnects a terminated connection.
- Dragging shows its destination, preserves selected groups, supports edge
  scrolling and restores selection on cancellation. Provide keyboard/menu
  alternatives and Undo.
- Long-form editors keep resize/expand controls and reachable actions. Formatting
  is explicit and undoable; closing releases editor resources and restores focus.
  Live validation feedback appears after the editing surface so temporary errors
  cannot move the input while the user types, including in expanded editors.
- Header authoring uses compact Name/Value rows with a bulk text alternative.
  Preserve duplicate occurrences, order, empty values and invalid drafts; changing
  views must not silently repair or reinterpret them. Temporary exclusions retain
  their editable values across views. Query editors preserve untouched URL spelling
  and encoding; opening a structured view must not normalize the source.
  Body representation changes preserve bytes, and responses distinguish their
  retained request from later edits. Reopening history uses that attempt's evidence,
  never a changed source or file. Label prepared data separately from observed data.
- Scratch utilities preserve their draft when dismissed. Detaching transfers
  work only after its destination is ready; independent utility windows retain
  their own drafts and do not depend on the originating workspace's lifetime.
  Clear obsolete derived output when its input changes. Chaining a structured
  decoding result should leave it ready for an explicitly chosen next operation.
  Lead scratch decoding with readable content and its structural outcome. Keep
  protocol wrappers and technical evidence afterward, preserving each layer's
  interpretation and source rather than presenting an opaque wrapper as the
  decoded content's outcome.

- Protocol extensions offer visualizers through explicit selection or opt-in Auto
  parsing of the visible selected message, and declare
  their request/response header and body inputs. The host owns presentation;
  hidden visualizers release their workers and data. Manage installation and
  declarations together in [Extensions settings](extensions.md), without access histories.
  Present corresponding request/response visualizers together, preserving exact
  direction-specific declarations and actions behind a compact access summary.
  Combine matching conditions only when all predicates other than the selected
  message direction are equivalent.
  Keep development reload actions in those settings and apply them only to active
  instances across windows, preserving each last good result on replacement failure.
  Optional column cells read bounded header metadata after primary row rendering;
  their results must not delay unfiltered traffic queries, rebuild ordinary cells
  or mount installed-extension controls while those settings are closed. Explicit
  extension column filters match retained headers natively before paging, with
  bounded work and no body reads or extension scripts.

## Preserve work across changes

- Preserve selection, scroll and drafts across live updates and workspace changes.
  Resolve unsaved work before replacing it. Allocate one editor per visible task.
  Check a requested destination before offering to discard a draft. If navigation
  aborts after Discard, still-visible typed fields remain dirty; do not overwrite
  newer work. A completed Save stays saved even when navigation is superseded.
- Traffic appends arrivals in request admission order and follows the bottom by
  default. Selecting a request or scrolling away pauses automatic scrolling;
  arrivals keep loading in place. Scrolling back to the bottom does not resume
  following. Choosing a column sort pauses following and retains that order during
  updates. Follow newest and Clear all restore request order and resume following.
- Reject stale asynchronous results. Late validation, previews or responses cannot
  overwrite newer edits, restore obsolete selection or steal focus. Prevent
  duplicate submissions and retain useful input after failure.
- Prefer Undo for reversible list actions. Confirm replacement of edited work and
  consequential irreversible actions; keep routine selection fluid.
- Closing a progress view is not cancellation. Keep monitoring ongoing work and
  make its result available without reopening dialogs or moving focus.
- Debugging keeps controls reachable, reveals the paused statement and loads Local
  variables without extra selection. Preserve breakpoint and exception choices
  through reruns, and distinguish hiding the controller, stopping execution and
  detaching a healthy worker. Compact action icons retain accessible names and
  readable tooltips for pointer and keyboard use.
  The dockable workspace stays usable beside Traffic. Moving its surface preserves
  execution and drafts; closing it continues and detaches. Browsing unloaded
  sources never activates them. Saved breakpoints retain intent across worker
  lifetimes, and unresolved statements are cleaned up after verified source changes.
  Keep debugger discovery beside Scripts and Extensions; reveal the top shortcut
  after its first open in the current session. F12 toggles it, including from editors.
  Preserve debugger placement and pane choices through close/reopen. Keep source
  navigation searchable without activating workers, and let compact docks switch
  panes instead of squeezing the editor between fixed sidebars.
  Pane menus mark actual visibility, including the active pane in compact docks,
  and expose the same state to assistive technology.
  Keep the call stack visible and scopes independently expandable. Browse properties
  in scrollable trees, with expandable ranges for large collections. Automatic
  previews and watches reject side effects; property inspection and completion
  never run getters or proxy traps. Explicit Console commands may change the worker.
  Edit runtime values directly at the inspected binding or own writable property;
  replacement expressions reject side effects. Keep failed edit drafts and explain
  read-only values. Refresh inspection after explicit commands and edits, preserving
  expanded paths, reading position and useful focus through stepping. Presentation
  memory never reuses expired runtime handles. Separate editing a Watch expression,
  changing a runtime value and opening the current script draft.
  Use 1024×768 as the practical docked debugger baseline, with resizing and undocking
  available when the user needs more room for code and variables.
  Expire values when their execution scope changes, and keep ordinary Console
  output readable with technical identifiers behind details.
  Show ordinary Console logs, warnings and errors by default. Let users combine
  levels and opt into verbose output. Keep prompt logs individually filterable,
  with commands and results visible. Logged object snapshots preserve bounded
  data from log time, and contextual copying uses displayed values without
  executing code. Distinguish severity with icons and accessible labels as well
  as color.
  Keep Console's execution destination separate from output filters and callback
  pause targeting. A live pause preserves an explicitly chosen Scratch destination.
  Each owning window retains its Scratch heap through debugger dismissal and moves;
  Reset scratch appears only for Scratch and preserves the transcript, history and
  draft. Clear output changes presentation rather than runtime state. Any-worker
  callback targeting stays available before and after attachments; each arm has one
  callback winner, with cancellation and the remembered target kept distinct.
- Keep inspection and repair available when a saved configuration becomes invalid.
  Preserve authored choices and explain recovery instead of silently substituting
  a configuration with different trust, privacy or authorization consequences.
- Keep caller selection separate from HTTPS decryption. Default to all callers;
  distinguish uninspected passthrough from selected callers' opaque connection
  evidence. Offer certificate-free HTTPS tunnels explicitly, preserve that
  choice after certificate setup, and keep interception certificate setup beside
  the decryption control. Saving HTTPS changes updates new CONNECT requests
  without restarting capture. Disabling retires intercepted connections for new
  requests while admitted work finishes; enabling leaves existing opaque tunnels
  alone. Keep this timing concise and visible. See
  [process inspection](network-profiles.md#choose-processes-to-inspect).
- Startup discards obsolete script workspace schemas rather than migrating them.
  Never treat malformed data or a newer schema as obsolete.
- Preserve explicit evidence associations independently of saved content revisions
  and live activation. Show unavailable evidence with its remembered identity;
  replacing it requires an explicit choice rather than matching a later URL.

- Incremental script authoring starts with an empty draft and a task-named handler
  menu. Adding a handler includes executable sample code behind a descriptive
  false-by-default local boolean. Selecting an existing handler reveals its source.
  Insertion is one undoable source edit, with permissions and activation explicit.
  Keep save and activation controls direct. Group sandbox runs and live debugging
  in the Debug menu beside the language and handler menus. Keep script and draft
  status alongside the actions that change them.
  Show activity from other automation alongside the workspace tabs.
- Script authoring defaults to TypeScript while execution uses immutable saved
  JavaScript. Preserve the applied build through authoring failures and language
  changes. Retain both language drafts; returning to TypeScript requires an
  explicit review of generated JavaScript against current JavaScript. Source/build
  alignment, current type-check results and applied state remain distinct. Show
  actual runtime availability independently of configured activation. Reveal
  comparisons in a reachable surface, and preserve recoverable language drafts
  through later switches. Shared script saves compare with the loaded authoring
  baseline and require an explicit reviewed choice on conflict. Keep the draft's
  native traffic scope visible beside its source. Single-callback sandbox results
  explain bypassed filters; exchange scenarios show native selection, carried state,
  final fixture messages and per-phase outcomes. Both distinguish simulation from
  network verification. Bundled API help is available
  without leaving the editor and is shared with agent help.

## Present evidence and failures honestly

- Agent inspection offers complete retained sources through bounded pages, with
  source scope and disclosure checked on each read. Display previews never stand
  in for complete evidence. Keep replay context available across later sends.

- Auth inspection separates structural decoding, encrypted/opaque coverage,
  capture-time observations and cryptographic verification. Mask credentials
  and raw representations by default; explicit reveal/copy resets with its
  owning evidence. Disclose that original retained sources can contain secrets.
  Surface Auth when selected-message discovery finds evidence. Put explicit
  whole-body format choices with body viewers and show their result clearly.
  Keep value reveal controls inline and copying in the inspection context menu.
  Keep field-specific interpretation beside its value; group technical evidence
  into one disclosure per finding after readable decoded content. Do not repeat
  a child's summary on its enclosing artifact. Present a single-source envelope
  chain as one readable artifact while preserving its decoder layers and warning
  ownership. Explain shared verification and privacy limits once in a disclosure
  near the top of Auth, without repeating generic caveats under individual fields.
  Keep the source-coverage summary visible; promote retries for failed evidence
  and group routine refresh with the inspection disclosure.
  See [Auth inspection](auth-inspection.md).

- Local certificate trust controls add trust grants. Disabling them cannot turn
  off user or enterprise distrust. Public distrust follows the selected policy
  source rather than silently borrowing another source's rules.

- Distinguish observed behavior from configured intent, and original evidence
  from generated summaries or previews. Opening a capture does not reverify its
  historical connections. Preserve original values when display requires escaping.
  Schema-free protocol viewers distinguish wire evidence from possible type or
  version interpretations and preserve access to the corresponding source bytes.
  Native structured documents and extension results share host-owned navigation,
  field details and copying. Their producers retain separate snapshot and worker
  lifecycles; rendering a native document does not create an extension instance.
  Tree navigation preserves the user's choice to close details; opening them is
  explicit. Show result counts and coverage beside structured comparisons, with
  changed values ahead of expandable technical evidence.
  Large structured views may load explicit field ranges on demand. Keep source
  bytes accessible, identify flattened nesting, and report when copying requires
  a smaller range instead of silently omitting unloaded content.
- Separate reusable Favorites, applied filters and window-local drafts. Keep
  Favorite persistence independent of filter activation. Closing or switching
  fields preserves the draft. Apply validates and commits all edited fields
  together; Discard restores their applied state. Removing a field also clears
  its pending draft. Keep unapplied state visible outside the picker.
- Lead filtering with the frequent task: ranges first where available, observed
  choices easy to reach, and idle Favorite libraries collapsed with a count.
  Offer value authoring on demand and search for larger libraries without hiding
  checked terms. Distinguish saved/custom and observed groups, revealing OR/AND
  controls when relevant; additional requirements always combine with AND.
- Keep checked choices first on opening and rows stable during editing. Preserve
  explicit disclosure choices, typed values and useful keyboard focus across
  asynchronous refreshes. Outlined and filled stars show Favorite state
  independently of checkbox selection and retain a visible focus ring.
- Validate complete batches before saving, save successfully before applying, and
  preserve newer edits when work finishes late. Shared-library changes must not
  rewrite draft roles or applied predicates. Preview the whole draft across the
  retained scope, count overlapping matches once, and show a correction prompt
  instead of a result count while input is incomplete or invalid. See
  [Traffic filtering](traffic-inspection.md#search-and-select) for the control
  behavior, matching rules and limits.
- Count and match filter choices with the same definitions. Host wildcards include
  the base domain and its descendants at whole DNS label boundaries. Common
  choices must not hide unfamiliar observed values; expose a counted Other choice
  when useful and distinguish it from missing evidence. Use canonical names for app-owned
  values, normalizing external spellings without changing captured evidence.
- Keep comparison findings focused on changed content. Reordering identical
  headers does not create a change finding; retain original positions in source
  evidence and in findings for actual header changes.
- Distinguish measured zero, empty, unavailable, incomplete, truncated, redacted
  and expired data. Name the relevant boundary; a later failure does not establish
  that an earlier complete body was truncated or that a server did not process it.
- Explain disabled actions. Put progress and actionable errors near their action.
  Significant explicit open/import/save/export failures use a focused modal with
  details and Close; Escape returns focus. Completed status messages are dismissible.
- Treat cancellation as ordinary feedback. Background refresh failures must not
  repeatedly interrupt work, and stale state must not turn a failure into success.
- Copying commands never sends traffic. Captured markup stays inert in the
  workbench; explicit preview flows must retain their documented isolation.
  Host private keys and TLS session secrets never become inspection evidence.
  Captured HTTP credentials follow the Auth view's masking and explicit-value rules.

## Keep product contracts in their topic guides

Human and agent rule edits share saved revisions and conflict checks.
Every authorized agent may read relevant rule effects; shared edits require an
explicit permission. Preserve unsaved human drafts during agent refreshes and
keep active agent access visible with a direct route to its controls.

Agent inspection sessions open quietly and remain independent of the user's
workspace. Review attaches the existing capture viewer to the same evidence;
closing the window keeps the session. Keep ending a session explicit, preserve
unsaved human work when access ends, and distinguish private drafts from shared
live rules. Passwords for agent-requested capture loads are entered only in the
app and never imply approval to disclose unredacted values.
Agent requests for unredacted values appear without stealing focus.
Default to omission, make temporary consent visibly time-bound with an immediate
revoke action, and distinguish it from a durable change to the saved setting.
Agent setup defaults to all main-workspace traffic, with explicit operations and
one redaction choice. Allow all selects operations without disabling redaction.
Keep configuration reusable through trusted settings with OS-protected key storage.
Keep connection activity stable across brief calls. Show temporary sensitive-value
access as a badge on the connection indicator; authorization and revocation
feedback must stay immediate even when connection status is smoothed.

Update the owning guide when a deliberate product decision changes a convention.
Keep exact labels, defaults, limits and activation rules there, with links from
other guides rather than repeated inventories.

| Contract | Owning guide |
| --- | --- |
| Traffic layout, selection, body viewers and Composer | [Traffic inspection](traffic-inspection.md) |
| Contextual inspection copying and serialization | [Copying inspection values](copying-inspection-values.md) |
| Captured auth formats, masking and interpretation | [Auth inspection](auth-inspection.md) |
| Stream lifetime and retained messages | [Stream inspection](stream-inspection.md) |
| Timing interpretation | [Request timings](request-timings.md) |
| Profile choices and activation | [Network profiles](network-profiles.md) |
| Certificate evidence and CA lifecycle | [Certificate model](certificate-model.md) |
| Client-certificate authorization and prompts | [Client certificates](client-certificates.md) |
| Proxy/window lifecycle and update interaction | [Desktop](product-shell.md) |
| Scratch conversions and converter-window lifetime | [Format converter](format-converter.md) |
| Persistence, retention defaults and privacy | [Product state](product-state-and-support.md) |
| Rule editing, activation and local script debugging | [Automation](automation.md) and [extension debugging](extensions.md#debugging-a-visualizer) |
| Agent permissions, key lifecycle and shared rule collaboration | [Local MCP access](mcp.md) |
| Preview isolation | [Safe previews](safe-previews.md) and [page preview](captured-page-preview.md) |
| Save/import behavior and fidelity | [Trace saving](trace-saving.md) |
| Portable case sharing, encryption and working copies | [Portable investigations](portable-investigations.md#desktop-transfer-and-package-apis-k3) |
| Support profiles, incident navigation and evidence review | [Support capture profiles](support-capture-profiles.md) |
| Installer ownership and release gates | [Windows release](windows-release.md) |

The [quality routine](ux-quality.md) owns interaction, layout and verification
checks. Documentation-only changes need content and link review, without app builds.

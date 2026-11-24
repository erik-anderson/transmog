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
- Keep **Start proxy** direct. Show the active routing mode beside the listener
  and let users choose and remember routing for the next new listener there.
  Manual routing gives the user a copyable address and keeps recovery from a
  prior Windows system-proxy run available as a separate action.
- Explain interception trust alongside manual routing. Distinguish a usable
  public CA, Windows account trust and the target app's trust configuration.
  Offer explicit Windows installation and the public certificate path in the
  same setup surface. Keep HTTPS tunnels available without an interception CA.
- Group listener setup, inspection policy and outbound transport under
  Connection. Keep appearance, retention and privacy in Preferences. Save and
  revert each tab's draft independently, with certificate setup and retry in the
  same Connection view.
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

- Let users inspect extension inputs and choose which visualizers run.
  Automatic parsing requires opt-in and applies to the visible message.
  The host renders the result and releases hidden workers. Keep a working
  result visible if a development reload fails. Optional columns must leave
  the ordinary traffic list responsive. See [extension settings, matching and
  reload](extensions.md).

## Preserve work across changes

- Preserve selection, scroll and drafts across live updates and workspace changes.
  Keep loaded Traffic summaries visible while their replacements load. Background
  refreshes must not replace known evidence with loading or unknown placeholders.
  Publish arrivals with their available core metadata together. Coalesce busy
  streams with bounded refresh work and a final update after activity settles.
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
- When a worker pauses, reveal the statement, stack and locals. Keep source
  browsing independent of execution. Watches, previews and property expansion
  must be safe to inspect. Explicit Console commands may change worker state.
- Moving a debugger preserves execution, drafts, expanded rows and reading
  position. Closing it continues and detaches paused workers. Keep expired
  values identifiable while fresh ones load. Runtime edits belong to the exact
  paused frame and property the user chose.
- Keep the Console command destination, output filters and pause target
  independent. Clearing output preserves variables. Show ordinary logs by
  default, distinguish warnings and errors accessibly, and let users copy the
  exact message or field under the pointer. See the [debugger controls](scripting.md#debug-a-script)
  and [workspace lifetime decision](adr/0012-debugger-workspace-lifetime.md).

- Keep inspection and repair available when a saved configuration becomes invalid.
  Preserve authored choices and explain recovery instead of silently substituting
  a configuration with different trust, privacy or authorization consequences.
- Keep caller selection separate from HTTPS decryption. Default to all callers;
  show relayed CONNECT tunnels as Traffic entries even for excluded callers,
  retaining connection context without their headers or contents. Explain the
  inspection bypass separately from the tunnel's observed outcome. Offer
  certificate-free HTTPS tunnels explicitly, preserve that
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

- Start script authoring with an empty TypeScript draft. Inserting a handler
  is one undoable edit. Sample behavior starts off, and the user chooses scope,
  permissions and activation explicitly. Keep those choices visible beside
  the source.
- Keep the editing draft, compiled JavaScript and applied version distinct.
  Preserve the applied version across build failures. Review language changes
  and conflicting saves before replacing work. Sandbox results must identify
  which filters and callbacks ran and which network behavior still needs testing.
  Keep the [script reference](scripting.md) available in the editor and agent help.

## Present evidence and failures honestly

- Agent script diagnostics follow the authorized exchange and unredacted-access
  setting. Keep callback outcomes separate from network outcomes and authored
  findings; distinguish skipped callbacks, silent success, withheld text and lost
  details. Global script health and initializer diagnostics require all-traffic
  scope. Source, locals and debugger control remain local authoring capabilities.

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
- Saving a Favorite makes it reusable across windows. Applying a filter changes
  only that window’s query. Preserve the draft, typed values and focus while
  the user switches fields or async results arrive.
- Keep ranges and observed values easy to reach. Reveal how saved and observed
  choices combine, keep rows stable during edits, and keep selected choices
  reachable during search. Apply validates all fields together. Preview the
  whole draft using the same matching rules as the list and count each request
  once. See [Traffic filtering](traffic-inspection.md#search-and-select).

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

Agent access must stay visible and easy to revoke. Keep temporary consent
time-limited. Grant operations and disclosure separately. Private reviews
share the session’s evidence, preserve unsaved human edits and require an
explicit choice to publish rules. Enter capture passwords in the app.
See [MCP permissions and session behavior](mcp.md).

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
| Portable case sharing, encryption and working copies | [Portable investigations](portable-investigations.md#desktop-transfer-and-package-apis) |
| Support profiles, incident navigation and evidence review | [Support capture profiles](support-capture-profiles.md) |
| Installer ownership and release gates | [Windows release](windows-release.md) |

The [quality routine](ux-quality.md) owns interaction, layout and verification
checks. Documentation-only changes need content and link review, without app builds.

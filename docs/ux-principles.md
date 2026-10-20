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
  horizontal space. Marker navigation reveals time or linked evidence; applying
  a time filter is explicit. Preserve edits in a new copy and distinguish clearing
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
- Explain copy and export consequences beside the action. A clear action label
  and concise disclosure are sufficient for routine saves; avoid redundant
  acknowledgement checkboxes. Describe capture limitations without implying
  that normal protocol or retention behavior is a failed export.

## Make layout and input predictable

- Remove repeated labels, excess spacing and unnecessary containers before
  reducing text size or control targets. Reuse styles and align text baselines.
- Keep actions outside list scrollers. Preserve comparison in wide panes and
  provide list/detail or request/response switching in narrow ones. Scroll within
  workspaces so content cannot push navigation beyond the viewport.
- Important labels wrap or reflow. Abbreviated metadata must have an accessible
  complete form; essential actions and state must not depend on tooltips.
- Use native controls with accessible names, visible focus and logical tab order.
  Restore focus after menus, dialogs and editors close. State needs text or
  semantics as well as color. Respect theme, forced colors, reduced motion,
  zoom and long labels.
- Lists share Ctrl+click, Shift ranges, Shift+arrow, Ctrl+Shift+arrow, Ctrl+A and
  Escape. Where supported, Ctrl+arrow moves focus, Del removes and Ctrl+Z undoes.
  Text fields retain ordinary editing shortcuts; Ctrl+S saves an explicit draft.
- Prefer familiar shortcuts for existing actions and show them beside the action
  or in its tooltip. Scope window and workspace commands to their context; let
  dialogs and editors handle their own keys first. Holding a shortcut must not
  repeatedly open windows or file dialogs.
- Application chrome and selectable lists do not select text during pointer or
  range selection. Editors and inspection evidence, including headers, bodies,
  full URLs and reports, retain ordinary text selection and copying.
- Removing Traffic keeps open live WebSockets inspectable unless the user has
  explicitly enabled termination on clear. Cancellation stops selected live work
  while preserving its evidence; Undo never reconnects a terminated connection.
- Dragging shows its destination, preserves selected groups, supports edge
  scrolling and restores selection on cancellation. Provide keyboard/menu
  alternatives and Undo.
- Long-form editors keep resize/expand controls and reachable actions. Formatting
  is explicit and undoable; closing releases editor resources and restores focus.
- Scratch utilities preserve their draft when dismissed. Detaching transfers
  work only after its destination is ready; independent utility windows retain
  their own drafts and do not depend on the originating workspace's lifetime.

- Protocol extensions offer visualizers through explicit selection or opt-in Auto
  parsing of the visible selected message, and declare
  their request/response header and body inputs. The host owns presentation;
  hidden visualizers release their workers and data. Manage installation and
  declarations together in [Extensions settings](extensions.md), without access histories.
  Keep development reload actions in those settings and apply them only to active
  instances across windows, preserving each last good result on replacement failure.
  Optional classification columns read bounded header metadata after primary row
  rendering; their results must not delay traffic queries, rebuild ordinary cells
  or mount installed-extension controls while those settings are closed.

## Preserve work across changes

- Preserve selection, scroll and drafts across live updates and workspace changes.
  Resolve unsaved work before replacing it. Allocate one editor per visible task.
  Check a requested destination before offering to discard a draft. If navigation
  aborts after Discard, still-visible typed fields remain dirty; do not overwrite
  newer work. A completed Save stays saved even when navigation is superseded.
- Reject stale asynchronous results. Late validation, previews or responses cannot
  overwrite newer edits, restore obsolete selection or steal focus. Prevent
  duplicate submissions and retain useful input after failure.
- Prefer Undo for reversible list actions. Confirm replacement of edited work and
  consequential irreversible actions; keep routine selection fluid.
- Closing a progress view is not cancellation. Keep monitoring ongoing work and
  make its result available without reopening dialogs or moving focus.
- Keep inspection and repair available when a saved configuration becomes invalid.
  Preserve authored choices and explain recovery instead of silently substituting
  a configuration with different trust, privacy or authorization consequences.
- Preserve explicit evidence associations independently of saved content revisions
  and live activation. Show unavailable evidence with its remembered identity;
  replacing it requires an explicit choice rather than matching a later URL.

## Present evidence and failures honestly

- Distinguish observed behavior from configured intent, and original evidence
  from generated summaries or previews. Opening a capture does not reverify its
  historical connections. Preserve original values when display requires escaping.
- Count and match filter choices with the same definitions. Common choices must
  not hide unfamiliar observed values; expose a counted Other choice when useful
  and distinguish it from missing evidence. Use canonical names for app-owned
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
  Private keys, passwords and session secrets never become inspection evidence.

## Keep product contracts in their topic guides

Update the owning guide when a deliberate product decision changes a convention.
Keep exact labels, defaults, limits and activation rules there, with links from
other guides rather than repeated inventories.

| Contract | Owning guide |
| --- | --- |
| Traffic layout, selection, body viewers and Composer | [Traffic inspection](traffic-inspection.md) |
| Stream lifetime and retained messages | [Stream inspection](stream-inspection.md) |
| Timing interpretation | [Request timings](request-timings.md) |
| Profile choices and activation | [Network profiles](network-profiles.md) |
| Certificate evidence and CA lifecycle | [Certificate model](certificate-model.md) |
| Client-certificate authorization and prompts | [Client certificates](client-certificates.md) |
| Proxy/window lifecycle and update interaction | [Desktop](product-shell.md) |
| Scratch conversions and converter-window lifetime | [Format converter](format-converter.md) |
| Persistence, retention defaults and privacy | [Product state](product-state-and-support.md) |
| Rule editing and activation | [Automation](automation.md) |
| Preview isolation | [Safe previews](safe-previews.md) and [page preview](captured-page-preview.md) |
| Save/import behavior and fidelity | [Trace saving](trace-saving.md) |
| Portable case sharing, encryption and working copies | [Portable investigations](portable-investigations.md#desktop-transfer-and-package-apis-k3) |
| Support profiles, incident navigation and evidence review | [Support capture profiles](support-capture-profiles.md) |
| Installer ownership and release gates | [Windows release](windows-release.md) |

The [quality routine](ux-quality.md) owns interaction, layout and verification
checks. Documentation-only changes need content and link review, without app builds.

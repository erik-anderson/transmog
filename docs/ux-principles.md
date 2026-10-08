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

## Increase density without losing readability

- Remove repeated headings, duplicate status lines, oversized spacing, and
  unnecessary cards before reducing text size or control targets. Preserve a
  clear hierarchy through grouping, modest weight, and restrained color.
- Reuse shared control styles and spacing. Align text baselines across related
  labels, values, buttons, and table cells; centering their boxes is insufficient
  when font sizes or padding differ. Align checkbox text naturally with its
  control and keep labels close to their fields.
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
- Make empty, unavailable, incomplete, truncated, redacted, and expired states
  distinguishable. Explain the next useful step. Disable an unavailable action
  with an accessible explanation rather than presenting a broken interaction.
- Keep captured markup inert and distinguish preview content from complete
  original data. Copy, save, edit, and export labels should describe the data
  the operation actually uses.
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

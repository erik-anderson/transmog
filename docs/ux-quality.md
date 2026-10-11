# Maintaining UX quality

Transmog's primary task is to collect or import traffic, find the relevant
request, and inspect its headers and body. Preserve that path as features grow.
Use this routine for changes to controls, copy, defaults, status, or asynchronous
behavior. The [UX principles](ux-principles.md) define product conventions;
the [UI instructions](../apps/desktop/ui/AGENTS.md) define implementation and
required checks.

## 1. Start with the user's task

State what the user is trying to accomplish and walk through the current flow.
Read the relevant topic guide and inspect the actual UI. Identify the frequent
action, the occasional actions, and what the user needs to know before acting.
For a small change, a sentence and an existing screen are enough.

## 2. Keep the screen easy to scan and use

- Keep collecting, filtering, selecting, and inspecting traffic direct. Count
  the steps added to that path and remove unnecessary navigation or confirmation.
- Group related secondary actions in existing menus, tabs, or disclosures.
  Keep the primary action and essential state visible. Avoid duplicate controls
  and a new toolbar for every feature.
- Improve density by removing repeated labels, excess spacing, and unnecessary
  containers. Keep text readable and controls easy to reach. Align text
  baselines, including search fields beside buttons.
- Use short, familiar labels that describe the action or outcome. Reuse the
  app's vocabulary. Empty states suggest an action available in that window;
  progress and errors explain what happened and the next useful step.
- Reuse established controls, spacing, selection behavior, and keyboard patterns.
  Preserve comparison when space permits and keep both messages accessible when
  panes become narrow.

## 3. Exercise the complete interaction

Use representative populated traffic, long URLs and values, and relevant empty,
loading, failed, or unavailable states. Check single selection and multiselection
when affected. Complete the task with both pointer and keyboard.

Open and close menus and dialogs with their buttons and Escape. Focus should
return to a visible, useful control; closing a menu must not leave it on a hidden
item. Preserve the inspected request, scroll position, and drafts during updates.

For asynchronous changes, delay a result, change selection or move focus, then
let it complete. Try cancellation and retry. Verify that late work cannot steal
focus or restore obsolete state, and that canceling an operation still leaves
the app monitoring any underlying work. Open a workspace for the first time
after proxy state has changed, then leave and return.

## 4. Verify the rendered experience

Prefer the repository's Playwright checks against production templates for
repeatable interaction checks and screenshots. Use realistic fixtures and inspect
the screenshots yourself. Check a wide window, a compact window, and a resized
inspector; useful starting sizes are 1440×900, 960×720, and 800×600. Include long
labels, zoom, and theme or forced-color checks where affected. Check clipping,
scrolling, baseline alignment, and whether actions remain reachable.

Run the checks required by the [UI instructions](../apps/desktop/ui/AGENTS.md).
Use native WebView2 checks for native behavior, accessibility, or layout; browser
fixtures alone do not establish native correctness. Add regression coverage for
important behavior and reproduced failures. Prefer observable outcomes over
incidental pixel values or tests that repeat the implementation.

## 5. Review, fix, and leave clear evidence

After implementation, review the diff and repeat the affected flow as a user.
Fix actionable findings, then rerun the checks affected by those fixes. A passing
build and an empty-state screenshot are only part of verification.

Report what changed, which flows and layouts were checked, and any remaining
limitations. Keep screenshots and test results in generated evidence. Update the
relevant product guide when behavior changes, removing superseded instructions;
keep lasting conventions in the UX principles and this routine here.
Documentation-only changes need content and link review, without app builds.

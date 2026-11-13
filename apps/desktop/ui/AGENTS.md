# Desktop UI development

## UX design

Before changing user-visible behavior, read and apply
[the UX design principles](../../../docs/ux-principles.md). Use them to review
the complete interaction, density, text alignment, state feedback, keyboard
behavior, and constrained-window layout. Preserve established flows unless the
task intentionally changes them, and verify populated states as well as empty
ones. The principles apply to new features and small edits alike.
Use [Maintaining UX quality](../../../docs/ux-quality.md) as the per-change
walkthrough, including a review and fix pass before reporting completion.

## Framework and composition

- Read the consuming application's installed `@microsoft/webui/ai.md` before
  changing WebUI code. Resolve it from this directory; do not substitute a newer
  online reference. The repository's `webui-reference` skill explains resolution.
  Follow its bundled task-specific guides, and reread the reference after upgrades.
  Keep the Rust renderer, compiler, and browser framework on matching versions.
- Use `@attr` and `@observable` properties with typed bubbling events between
  workspaces. Templates own DOM structure and visual state; CSS owns styling.
  Keep reactive data in observable models; component references are not reactive
  view models. Pass parent-owned values into children and emit changes upward.
  Avoid imperative DOM construction and inline style mutations for application
  UI. Native APIs remain appropriate for input values, dialogs/popovers, focus,
  and measurements.
- Name components and files by their role. Avoid change-history names such as
  `refactor-*` and redundant application prefixes.
- Start the browser runtime explicitly after component imports. Replace observable
  object/array roots to notify bindings; await `$whenRendered()` only when subsequent
  focus, measurements, or native controls need the updated DOM. Render completion
  includes affected descendants: load and mount them first, and retain stale-work
  guards across the await. Cancel native events synchronously before awaiting.
- Apply a complete draft, its native control values, and its baseline before
  yielding; await only the DOM-dependent follow-up. Claim submission and request
  ownership before the first await, and let only that owner release busy state.
  Recheck context, cancellation, focus, and reading position after each wait.
  Closing a native surface must invalidate a pending open even before it appears.
  Track presentation cancellation through lazy loading as well as rendering.
  Close the originating menu and restore its focus before capturing that intent.
  A child's stale-work checks do not protect its caller's continuation; preserve
  caller ownership across child operations too. Discard must invalidate retained
  editor drafts before a same-item cache can reuse them.
  Consume a confirmed discard when navigation replaces or leaves that editor;
  a canceled destination must preserve retained fields as dirty.
  When fixing an asynchronous race, review the same completion pattern in sibling
  components and cover representative siblings with delayed-render regressions.
- If URL routing is added, use `@microsoft/webui-delivery` after resolving its
  Trusted Types compatibility with the embedded origin; see ADR 0007.

## State and lifecycle rationale

- Rust rendering and client hydration share `src/initial-state.json` to avoid
  first-paint/hydration mismatches. Keep persisted defaults aligned with it; the
  renderer tests enforce this contract.
- Save layout through its narrow persistence command. Independent layout and
  settings/window writes must merge so stale snapshots cannot overwrite each
  other's changes. Keep captured traffic and credentials out of UI preferences.
- Sort/filter retained metadata before virtual windowing so results describe the full
  retained capture. Preserve row keys and inspection position during updates;
  coalesce refresh hints and reject stale asynchronous results.
- Secondary workspaces mount after their definitions load and stay mounted after
  first use to preserve drafts. Monaco
  loads after its Trusted Types environment. Dispose owned editors, models, and
  listeners when their component disconnects.
- Reserve layout space before lazy editor hydration, including client-created
  rows. Otherwise empty hosts appear onscreen together and eagerly allocate
  editors. Allocate Monaco only for visible editor surfaces.
- Keep editor actions available in the expanded surface and preserve drafts,
  focus, and editor layout when restoring. Provide scrolling within panes when
  the window cannot accommodate their content.
- Captured markup remains inert text in the workbench. The explicit captured-page
  preview uses a separate native browser profile and warning/script-choice flow;
  never render it in a trusted workspace. Use the isolated preview origin for
  workbench images and preserve its CSP and Trusted Types enforcement.
- Save response files from the complete original body through the backend.
  Preview data may be shortened, formatted, or normalized to another image
  format. Keep an eviction lease while the native file dialog is open, and
  derive suggested names from original headers rather than display-safe copies.

## Verification

From this directory:

```powershell
npm run check
npm run test:workspaces
```

Browser checks use the locked Playwright installation in `e2e/playwright`;
install its dependencies and Chromium once. They render production templates
under enforced Trusted Types with fixture commands. Extend interaction checks
when changing async state, persisted layouts, or editor lifecycle.

From the repository root:

```powershell
python scripts/dev-env.py --check
python scripts/dev-env.py --run cargo clippy --locked -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop --all-targets -- -D warnings
python scripts/dev-env.py --run cargo test --locked -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop
```

Use `scripts/test-windows-desktop.ps1` for real WebView2 checks. DevTools flags are
test-only: isolate test app data and close only the process started for the check.
Temporary UI mutations in probes must be restored immediately after measurement.
The Docker-backed headless product test is ignored by ordinary Cargo runs; use
`python scripts/test-interop.py` when changing end-to-end proxy/interoperability behavior.

Keep README content about the tool's benefits and high-level architecture. Prefer
rationale over documenting tunable constants or implementation details that are
already clear in the code.

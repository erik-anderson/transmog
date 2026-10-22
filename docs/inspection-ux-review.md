# Inspection UX review and improvement plan

Reviewed 22 October 2026. Scope: the Traffic request/response inspectors,
inspection copy menus, Body, Auth, and their shared keyboard/layout behavior.
This is an implementation review using production templates and synthetic
populated traffic, not a usability study of the whole application.

The user's task is to select a request, understand its evidence, and copy the
relevant content without losing selection or navigating through repeated chrome.
The frequent path is **Traffic → selected message → value**. Decoding mechanics,
raw credentials and diagnostic retries are secondary.

## Corrections implemented

| Observed problem | Correction |
| --- | --- |
| Short copy menus jump upward because placement reserves 290 pixels regardless of content. | Measure the rendered menu and clamp only at window edges. Reposition when its content or the viewport changes, accounting for zoom. |
| Body and Raw data treat a display label as a field name. | Offer **Copy body preview** or **Copy raw data** only. Text blocks retain line breaks. Converter output follows the same rule. |
| Object and Auth finding menus offer disabled scalar actions despite having no scalar. | Omit value/pair actions for grouping nodes. Retain explained disabled actions for real fields whose evidence is unavailable. |
| Each Auth field adds an identical **Meaning and limits** disclosure, including empty notes. | Show nonempty interpretation notes beside their values. |
| Auth offers separate provenance and raw-value disclosures before decoded children, and repeats child summaries on wrappers. | Put decoded children first; group provenance and raw evidence into **Technical details** per finding. Remove inherited summaries. Masking and explicit reveal/copy semantics remain intact. |

These changes remove misleading actions and repetitive controls. The remaining
opportunity is to show more useful evidence in the initial viewport.

## Implementation scope

### 1. Present each auth artifact as one readable unit — high priority

**Evidence:** The Bearer → JWT fixture still repeats a heading, recognition/status
line, source label and source button at both levels. At 1440×900, this uses a
substantial part of the response pane before the first claim; 800×600 is worse.
Raw decoder nesting is driving the visual hierarchy.

**Plan:** Present a single-child envelope chain as one artifact heading, for
example **JWT / JWS · Bearer credential**. Show its source once, followed by the
readable claims. Preserve the exact chain and distinct source mappings under
Technical details. Keep sibling artifacts distinct, and keep warnings attached
to their owning layer. Do not flatten unrelated sources or claim cryptographic
verification. Remove expanded-state summaries that merely repeat visible fields.

**Acceptance:** A nested Bearer/JWT requires no disclosure to read issuer,
audience and capture-time expiry interpretation; each source action appears once
per distinct source. Multiple challenges, malformed siblings, JWE, nested
Negotiate and redacted evidence retain their separate meanings. Ordinary subtree
copy remains masked, with exact source navigation and stable focus.

**Implementation area:** Auth presentation model and template; extend native
JWT/SAML/NTLM/Kerberos fixtures before changing the view hierarchy.

### 2. Make room for inspection in compact windows — high priority

**Evidence:** At 800×600, the default traffic split leaves a short inspector.
Message chrome, Auth actions and wrapper metadata occupy most of that space.
The first decoded values require scrolling even with only four traffic rows.
The existing draggable dividers help, but users must discover and adjust them.

**Plan:** Add one **Expand inspector / Restore** control beside the existing
inspector actions, with a keyboard equivalent. Preserve the previous divider
positions, selected request, request/response mode and scroll on restoration.
Keep a compact selected-request identity and a direct way back to the list.
Retain side-by-side request/response comparison when width permits. Do not
automatically change the user's saved split or collapse the list on selection.

**Acceptance:** At 800×600 and 125% zoom, expanding takes one action and exposes
the first readable auth fields or body lines without scrolling past chrome.
Restore returns to the same selected row and layout. Keyboard users can expand,
inspect, and return without traversing the entire traffic list.

**Implementation area:** Traffic layout state and pane dividers. This follows
artifact simplification so additional space serves evidence, not repeated labels.

### 3. Give Body a clear representation and scope — medium priority

**Evidence:** Body has a viewer select, Body options, nested Preview limit,
representation facts, and Save as. The context menu correctly says “preview,”
but a user still has to reconcile which text is displayed and what Save retains.
Status/error prose also shares the body text surface with actual evidence.

**Plan:** Use one compact status line for representation, decoded/original state,
and preview coverage. Keep a significant truncation, decoding failure or missing
body reason visible without opening details. Keep Decode and preview limits in
one settings disclosure. Model evidence text separately from status prose so a
body-copy action cannot masquerade as copying a body when it only copies an
error. Keep explicit whole-body auth formats clearly identified and preserve
the existing return to Body.

**Acceptance:** Users can tell whether they are copying displayed text or saving
the full retained body before acting. Test formatted JSON, original text,
compressed data, invalid encoding, empty/304, truncated, evicted and unavailable
content. Copying an explanation is labeled as an explanation; an absent body
does not offer an apparently valid body copy. Hex byte actions remain distinct.

**Implementation area:** Message inspector's body presentation state, toolbar
and copy provider. Avoid changing retention or serialization semantics.

### 4. Apply one context-menu contract across inspection — medium priority

**Evidence:** Inspection copy, selected-text conversion and Traffic actions have
separate placement and keyboard implementations. Selected-text conversion still
uses an estimated menu height; Traffic uses another estimate. This fix addresses
the newly added inspection menus, but the surrounding menus can still feel
different at window edges.

**Plan:** Reuse measured placement and lifecycle behavior while preserving each
menu's action set and selection semantics. Define a small action matrix for text
selection, whole text blocks, fields, tree groups, table cells and unavailable
values. Prefer the existing Copy action first when text is selected. Provide
consistent Escape, arrows, Home/End and visible focus on dismissal, including
menus inside dialogs. Add unobtrusive visible copy success feedback; the current
success announcement is available to assistive technology but visually hidden.

**Acceptance:** Pointer and keyboard menus remain beside their target at every
edge, zoom level and dialog boundary. A late read after dismissal or a selection
change never writes to the clipboard. Clicking elsewhere leaves focus on the
clicked control. No unsupported or meaningless action appears for a target.

**Implementation area:** Shared menu utility and context providers, then Traffic
and converter adoption. Do not merge their distinct row-selection behavior.

### 5. Lead with relevant auth state, then optional detail — medium priority

**Evidence:** Retry is always prominent, three status terms recur per finding,
and source coverage is an open list at the end. Users have to interpret parser
status and verification status while searching for the useful fields.

**Plan:** Show a concise persistent verification statement and a coverage summary
near the artifact count. Keep malformed, unsupported, encrypted and incomplete
conditions visible where they apply; put routine complete-source detail behind
one coverage disclosure. Promote Retry on failure or stale evidence and retain
a secondary refresh path for successful results. Keep capture-time observations
distinct from cryptographic verification.

**Acceptance:** A complete scan and a limited scan are visibly different before
scrolling. Failure offers an immediate retry; successful parsing never looks like
verified authentication. Related-traffic results keep their searched scope and
provide an explicit return path without losing the source inspection position.

## Implementation and three-round review

All five stages above have been implemented. Expansion is temporary UI state;
no preference schema or retained-evidence format changed. Reviews were performed
in sequence against code, production-template interactions and rendered screens.
The table records actionable fixes and checks, including failures found by the
broader regression suite after the focused passes.

| Stage | Round 1 — behavior and structure | Round 2 — boundaries and lifecycle | Round 3 — rendered UX and regression |
| --- | --- | --- | --- |
| Auth artifacts | Added a separate presentation model for same-source single-child envelopes. Fixed template grouping; evidence and subtree serialization stay intact. | Fixed loss of the nested Kerberos role. Added boundary tests for sibling challenges, different source ranges, encrypted layers, warning ownership and redaction. | Inspected wide/compact screenshots and checked one source action, readable claims and technical layers. Aligned Copy name with the presented artifact while preserving the original subtree. Native 125% zoom exposed JOSE metadata ahead of claims; readable JWT claims now lead without reordering copied evidence. |
| Expanded inspector | Added Expand/Restore and Ctrl+Shift+Enter without remounting messages. | Kept expanded resizing temporary and restored list/inspection scroll, selection, modes and keyboard focus. Corrected the focus assertion to use the owning shadow root. | Verified 800×600 at 125% zoom, wide comparison and reading-position restoration. The full Hex check found an extra header row at 640×720/150%; the narrow button label now shortens to Expand and the action spacing keeps the controls on two rows, retaining accessible names. |
| Body representation | Separated evidence, metadata and explanation copy surfaces; removed the nested Preview limit disclosure. | Prevented stale status after a delayed Hex read; retained truncation warnings and distinguished content decoding from JSON formatting. | Inspected the visible warning/status line and verified that warning text never enters body copies. Checked absent 304 evidence, read errors, eviction, live updates, preview settings and delayed results. |
| Context menus | Shared measured placement, viewport/resize handling and arrow/Home/End navigation; placed Copy first for selected text. | Preserved Traffic row targeting and modal boundaries, checked copy cancellation and delayed clipboard edits, and added visible success feedback. | Edge/zoom screenshots exposed a keyboard anchor measured after its parent popover disappeared. Capture now precedes opening; regression assertions cover the actual source position. Checked modal selected-text copying and Escape focus. |
| Auth status and return | Added persistent unverified/coverage text, collapsed routine coverage and moved successful refresh into Inspection options. | Added searched scope, Back to evidence and Back to auth source, restoring the original stage, interpretation and pre-search reading position. | Checked limited coverage, immediate failure retry and compact rendering. Fixed focus on dismissal of related results and prevented delayed navigation from stealing newer focus. |

Focused browser evidence is in `target/ui-check`, including
`auth-expanded-800.png`, `auth-expanded-zoom.png`, `body-warning.png`,
`menu-traffic-800.png` and `menu-text-800.png`. The Auth test covers returning
from related traffic and a superseded navigation. The menu tests retain checks
for late clipboard reads, native dialog boundaries and complete values.

Final verification:

- Python 3.15 development-environment check passed.
- UI type checking, all 43 unit tests and the production build passed.
- The complete browser workspace suite passed with zero CSP violations; this
  includes Auth, copy, converter, Body refresh, Hex and the wider Traffic flows.
  Auth and copy checks also passed after the native-review claim-order fix.
- Required Rust checks passed: 563 tests, zero failures, 10 intentionally ignored
  native-fixture or separate-environment checks. Strict Clippy passed with warnings
  denied for all required packages and targets.
- Isolated native WebView2 verification passed against the final debug executable,
  including actual clipboard writes, nested artifacts, source navigation,
  accessibility, MAPI subtree/Traffic menus and expanded inspection at
  800×600/125% zoom with the setup banner present. Evidence is in
  `artifacts/windows-desktop-validation/b62d2aa2aa0e4a61b6dd020841713cb7`, including
  `auth-native-expanded-zoom.png` and the machine-readable `auth-native.json`.

Logs: `target/inspection-final-ui-check.log`,
`target/inspection-final-browser.log`, `target/inspection-final-rust-tests.log`,
`target/inspection-final-clippy.log`, and
`target/inspection-final-native-build.log`,
`target/inspection-final-native-tests.log`, `target/inspection-final-auth.log`,
and `target/inspection-final-copy.log`.

## Full-change review — maximum three rounds

A separate review of the complete implementation followed the stage reviews.
Each actionable finding was reproduced in a browser regression before its fix.

| Round | Finding and correction | Regression coverage |
| --- | --- | --- |
| 1 — complete diff and interaction state | Restore recorded only nonzero scroll positions, so reading in the expanded inspector lost an original position at the top. It now records scrollable panes at zero as well. A selected read-only report in a modal could copy its obsolete selection after its value changed; copying now checks the source value. | Compact/zoomed expansion, modal selection replacement, and native top-position restoration. |
| 2 — navigation across workspaces and evidence types | Related Auth navigation into a streaming exchange left Messages / Events covering the requested evidence. Forward and return navigation now select the HTTP handshake inspector. | Related navigation between WebSocket exchanges, visible Auth tabs, return position and focus. |
| 3 — rendered walkthrough and complete regression | A navigation superseded by newer focus reported that retained evidence was unavailable and replaced the newer status. Superseded navigation now leaves that status alone. | Delayed Auth navigation with a newer focus/status, followed by the complete browser and native verification. |

The capped loop ended after three rounds with all four findings fixed. Round 3
contained a finding, so this sequence did not end on a clean review round.
Verification passed:

- UI type checking, production build and all 43 unit tests.
- The full browser workspace suite, with zero CSP violations. The final
  cancellation-status correction then passed the focused Auth suite, including
  delayed forward and return navigation, stream evidence and scroll restoration.
- Strict Clippy and 563 required Rust tests, with zero failures and 10 intentional
  skips. These ran before the final UI-only cancellation-status correction.
- The rebuilt final executable passed isolated WebView2 Auth and clipboard
  checks, including restoration from scroll position zero at 800×600/125% zoom.
  The final native screenshot was visually inspected. Evidence:
  `artifacts/windows-desktop-validation/777b7aa09ddb4bcf80c37a0b52e89992`.
- `git diff --check` passed.

Logs: `target/full-review-ui-check.log`, `target/full-review-browser.log`,
`target/full-review-round1-auth.log`, `target/full-review-round1-copy.log`,
`target/full-review-round2-auth.log`, `target/full-review-round3-auth.log`,
`target/full-review-clippy.log`, `target/full-review-rust-tests.log`,
`target/full-review-native-build.log` and `target/full-review-native-tests.log`.
The `full-review-*-before.log` files retain reproduced regression failures.

## Additional review — stopped after a clean second round

The user requested up to three further rounds because the earlier final round
was not clean. Two additional rounds were completed:

| Round | Outcome | Evidence |
| --- | --- | --- |
| 1 | Found and reproduced Ctrl+F targeting the hidden toolbar while the inspector was expanded. Ctrl+F and Ctrl+Shift+F now restore Traffic before focusing search, retaining the inspected request. | Added main/viewer window shortcut regressions and a native compact-window check. The pre-fix failure is in `target/additional-review-round1-before.log`; the corrected shortcuts pass. |
| 2 | **Clean: no new actionable findings and no further implementation fixes.** Rechecked the full inspection change, copy snapshot guards, Auth navigation/cancellation, expansion, and nearby keyboard/dialog flows. | Verified Save copy dismissal keeps expansion and returns focus; search shortcuts and Auth checks pass. The full browser suite and native verification also pass. |

Stopped after the clean second round. Final verification passed: 43 UI unit
tests, type checking, production build, full browser suite with zero CSP
violations, strict Clippy, 563 Rust tests (10 intentional skips), native WebView2,
and `git diff --check`. Native evidence is in
`artifacts/windows-desktop-validation/9ba1a63d3a544b0dbd88805d6821b22b`;
`auth-native-search-restored.png` was visually inspected at 800×600/125% zoom.
Logs are retained as `target/additional-review-ui-check.log`,
`target/additional-review-round1-shortcuts.log`,
`target/additional-review-round2-shortcuts.log`,
`target/additional-review-round2-auth.log`, `target/additional-review-browser.log`,
`target/additional-review-clippy.log`, `target/additional-review-rust-tests.log`,
`target/additional-review-native-build.log` and `target/additional-review-native-tests.log`.

## Delivery sequence and review gates

1. Implement artifact presentation and compact inspection as separate changes.
   Compare the same fixtures before and after; require fewer repeated source
   controls and more readable evidence in the first viewport.
2. Implement Body representation/scope, then consolidate menu behavior. Check
   selected text versus whole-value copy and preserve complete underlying values.
3. Refine auth status/coverage once the information hierarchy is stable. Run short
   task walkthroughs: identify issuer/expiry, copy one masked credential, compare
   two challenges, diagnose an unavailable body, and return to the same request.

For each change, exercise pointer and keyboard at 1440×900, 960×720, 800×600,
125% zoom, a resized inspector, dark mode and forced colors. Include long URLs,
values and notes, delayed reads, errors, and request/stage changes. Use production
template browser checks and real WebView2 verification; retain generated
screenshots and results. Keep product contracts in
[Auth inspection](auth-inspection.md), [copying inspection values](copying-inspection-values.md)
and [Traffic inspection](traffic-inspection.md).

## Evidence for the initial correction

Browser evidence is generated under `target/ui-check`: `copy-body-*-*.png`,
`copy-details-800.png`, `copy-auth.png`, `auth-1440.png`, `auth-960.png`,
`auth-800.png`, `auth-dark.png`, `auth-forced-colors.png`, `auth-zoom.png` and
`auth-resized.png`.
The copy checks assert cursor proximity, viewport bounds, action relevance,
line-break preservation, and keyboard return focus. Auth checks cover readable
notes, technical grouping, masking, explicit copy, stale replies and source
navigation. Native evidence is produced by
`scripts/test-windows-desktop.ps1 -AuthChecks` under
`artifacts/windows-desktop-validation`.

Verified for this change:

- Python 3.15 development-environment check passed (using the installed
  interpreter directly because the Windows Store alias cannot execute here).
- `npm run check`: 39 tests passed and the UI build succeeded.
- `npm run test:workspaces`: the complete browser suite passed, including the
  new copy-action/placement checks; the resized Auth check also passed separately.
- Required Rust app/session/WebUI/desktop tests: 563 passed, 10 intentionally
  ignored native-fixture or separate-environment checks, no failures.
- Strict Clippy passed for the required app/session/WebUI/desktop packages and
  all targets with warnings denied.
- Isolated WebView2 Auth checks passed against the rebuilt debug executable,
  including actual clipboard writes, source authorization, stale-result rejection,
  accessibility, nested auth formats, and MAPI subtree/Traffic context menus.
  Native evidence: `artifacts/windows-desktop-validation/4e5c68a570c54e0495ac4c48eef5c3e4`.

Logs are retained in `target/inspection-ui-check.log`,
`target/inspection-browser-tests.log`, `target/inspection-rust-tests.log`,
`target/inspection-clippy.log`,
`target/inspection-native-build.log` and `target/inspection-native-tests.log`.

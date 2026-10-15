# UX design principles

Transmog is a traffic inspection and editing workbench. Help users scan entries,
understand the selected item and act without losing their place or work. These
principles apply to UI, commands, defaults, validation and asynchronous behavior.
Use the [UX quality routine](ux-quality.md) for each change and the
[UI instructions](../apps/desktop/ui/AGENTS.md) for implementation and verification.
Topic guides define current flows and limits; keep them aligned when a deliberate
product decision changes a convention.

## Organize around the task

- Keep collecting, filtering, selecting and inspecting traffic direct. Captured
  responses are the primary source for auto-response rules; scratch authoring is
  secondary. Use a few clearly named workspaces.
- Use a compact list and one selected-item properties pane. Multiselection reveals
  useful batch actions. Selection, editing, saving and activation are distinct.
- Keep primary actions beside their content. Put occasional actions in menus,
  advanced settings in disclosures and technical evidence behind readable summaries.
  Avoid deep navigation and repeated toolbars.
- Show recovery and risk acknowledgements when relevant. Name counts for what they
  count: matching entries and occurrences within an entry are different measures.
- In Traffic, **Details** groups Timings, Connection, Certificates and imported
  Capture source; **Actions** groups request operations; **Files** groups import
  and save; **View** groups columns, density and layout. Keep headers, bodies,
  Copy URL and Copy headers directly accessible.

## Density, layout and input

- Remove repeated labels, excess spacing and unnecessary containers before reducing
  text size or control targets. Use shared styles and align text baselines.
- Keep Traffic cells on one line with ellipsis and complete values on hover.
  Include stream badges when fitting columns. Important labels wrap or reflow;
  essential actions and state must not depend on tooltips.
- Keep actions outside list scrollers. Support wide comparison and narrow
  list/detail or request/response switching within the available pane. The narrow
  message selector belongs beside the URL/status. Scroll within workspaces.
- Use native controls with accessible names, visible focus and logical tab order.
  Restore focus after dialogs and editors close. State needs text or semantics as
  well as color. Respect theme, forced colors, reduced motion, zoom and long labels.
- Lists share Ctrl+click, Shift ranges, Shift+arrow, Ctrl+Shift+arrow, Ctrl+A and
  Escape. Where supported, Ctrl+arrow moves focus, Del removes and Ctrl+Z undoes.
  Text fields retain ordinary editing shortcuts; Ctrl+S saves an explicit draft.
- Dragging shows the destination, preserves selected groups, supports edge scrolling
  and restores selection on cancellation. Provide keyboard/menu moves and Undo.
- Long-form editors keep resize/expand controls and reachable actions. Response
  authoring starts with twelve body lines and ten header lines. Body dialogs use
  Apply/Cancel, offer Save rule only when other properties are clean, and make
  formatting explicit and undoable. Closing releases the editor and restores focus.

## Preserve work and explain state

- Preserve selection, scroll and drafts across live updates and workspace changes.
  Resolve unsaved work before replacing it. Allocate one editor per visible task.
- Distinguish draft, saved, validated, tested, enabled, active and paused states.
  Saving a definition must not silently activate it. Pausing auto-responses preserves
  individual rule enablement. Saved enabled switches may apply immediately.
- Reject stale asynchronous results. Late validation, previews or responses cannot
  overwrite newer edits, restore obsolete selection or steal focus. Prevent duplicate
  submissions and retain useful input after failure.
- Prefer Undo for reversible list actions. Confirm replacement of edited work and
  consequential irreversible actions; keep routine selection fluid.
- Put progress and actionable errors near their action. Significant explicit
  open/import/save/export failures use a focused modal with details and Close;
  Escape returns focus. Completed status messages can be dismissed. Cancellation
  is ordinary feedback; background refresh errors must not repeatedly interrupt.
- Distinguish empty, unavailable, incomplete, truncated, redacted and expired
  evidence. Explain disabled actions. A terminal failure without a response is
  Failed, not Pending. Body completeness is independent of later exchange failure.

## Traffic, streams and evidence

The [traffic guide](traffic-inspection.md), [stream guide](stream-inspection.md)
and [timing guide](request-timings.md) define the detailed interactions.

- Use one continuous virtualized Traffic list. Filter and sort all retained
  metadata before windowing. Selection, focus and range anchors use stable IDs and
  the complete display order; leaving the DOM never deselects an entry. Ctrl+A
  selects the current collection, including offscreen entries, without selecting
  later arrivals. Search review reveals its request while preserving multiselection.
- Live filtering covers URLs and processes. **Search contents** starts only with
  Search or Enter and produces a snapshot. Skip binary bodies quietly and summarize
  unavailable text once. Remove selected/unselected operates across the workspace.
- Mark top-level pages only from original fetch metadata. Fetch-destination filters
  use OR within the group and AND with other filters; recorded `empty` differs
  from missing evidence. Show one removable filter chip.
- WebSocket/SSE Active badges follow stream lifetime, including quiet intervals;
  saved captures never appear live. Preserve complete messages and history for
  retained sessions. Do not add silent per-message/count eviction. Page metadata
  and load saved payloads on demand; honor explicit privacy and session retention.
- Hex reaches every retained byte with exact offsets, paging, keyboard navigation
  and Go to offset. Text-preview limits do not cap Hex. Bound clipboard selections
  separately and offer Save as for larger data. Release owned temporary byte caches
  with their readers.
- Copy actions never execute requests. Use complete retained headers and encoded
  bytes, explain command normalization and missing-data placeholders, and offer
  body-file saving. Missing replay bodies require replacement or explicit empty use.
- Composer preserves later draft edits during sends and keeps the last complete
  response until replaced or closed. It uses the shared response inspector. Save as
  leases the original bytes; previews and formatting never replace the source.
- Show measured zero separately from unavailable, shared connection setup separately
  from request work, and local queueing separately from remote delivery. Collapse
  local proxy work under 1 ms. Keep imported timers with their original labels and
  clocks; response wait is not server CPU time.

## Network profiles and certificate evidence

Follow [root selection](trust-anchor-ids.md), [negotiation](chromium-negotiation.md),
[ALPS](http2-alps.md) and [client certificates](client-certificates.md).

- Match the qualified Chromium version by default. Built-ins declare fixed settings;
  overrides belong in custom profiles. Selection previews, **Use profile** and
  **Save and use** activate, and **Save only** preserves the active revision.
  Activation gives proxy and Composer fresh pools across windows; admitted work
  keeps its original recipe. Certificate-data updates do not edit profile choices.
- Default to **WebView2 (Chromium 154)**. Its root-source distinction selects
  WebView2 components; Chromium selects Chrome components. Custom profiles expose
  OS, bundled Chromium, installed Chrome and installed WebView2 sources, with the
  documented bundled fallback. Keep one local-Windows-trust switch beside that
  selector; disable it in OS mode, which already includes local roots.
- Use one Chromium verifier for every source. Keep Chrome/WebView2 datasets
  separate while applying the same semantics and freshness rules. Record selected
  path, source/version/hash, local-trust provenance and actual CT/CRLSet/OCSP results.
  Distinguish checked, stale, disabled, private-trust and restored-session states.
- Trust Anchor IDs are independent of PQC choices. If the selected store has no
  supported IDs, omit the offer and explain why; never borrow another store's IDs.
  ID controls and an explicit `crs.pb` path belong in More handshake options.
- Real ECH and ECH GREASE are separate controls. Record accepted ECH and authenticated
  public-name rejection distinctly. Never send origin identities or requests on the
  rejection connection.
- H2 and H3 ALPS are separate controls. Apply server transport settings, but leave
  Client Hint values and permissions with the caller. Do not fabricate hints or
  retry requests to satisfy them. Show negotiated empty, declined and unavailable
  ALPS distinctly, with readable ordered contents and complete original Hex.
- Certificate views separate client-facing interception, origin paths and client
  authentication. Use recorded trust, never imply a fresh verification on opening.
  Prefer complete public bytes for matching fingerprints; expose no private keys
  or session secrets. Default to the latest successful upstream connection, then
  the latest failed attempt, with role/outcome/identity in selection labels.
- Show each leg's actual algorithms, never infer them from keys, ciphers or settings.
  Label restored authentication and TLS 1.2's original group. Hybrid exchange or
  ML-DSA authentication alone does not establish a fully PQ-secure trust chain.
  Keep policy in secondary disclosures, retain conflicting observations, escape
  invisible controls for display and preserve original values in exports.
- Client certificate rules are explicit saved authorization, defaulting to no
  certificate. Prompt only for a real verified CertificateRequest. Use a nonmodal
  connection indicator with public identity details; answers apply to live
  connections only and never create rules. Expire late choices without losing
  inspected context. Viewers refer selection to the main window; closing the main
  window must not revoke unrelated viewer work.

## Proxy lifecycle, privacy and persistence

The [product-state guide](product-state-and-support.md) owns retention defaults;
[product shell](product-shell.md) owns lifecycle and settings flows.

- Keep Decrypt HTTPS and Profile above advanced settings. Decryption changes apply
  at the next start; opaque CONNECT needs no interception CA and reports only known
  destination, outcome and byte counts. Manual routing must not change OS settings
  or require host trust; decrypted operation still validates and protects CA files.
- Default to loopback, automatic port selection, caller isolation and server reuse.
  Remote access requires explicit opt-in. Preserve authored ports and explain
  conflicts. Disabling reuse affects subsequent requests, including viewer Composer.
- Stop restores host routing first, then shows **Finishing requests** while admitted
  work and streams finish. Start can resume the run. Once shutdown commits, expedite
  idle cleanup rather than waiting for peer acknowledgements. Exit uses bounded
  cleanup. Reject stale shutdown results after a new transition.
- Protect CA keys before writing them; migration preserves public identity. Never
  replace a trusted root silently or fall back to plaintext. An inaccessible DPAPI
  key offers explicit exact-root reset and renewed Windows consent.
- Live traffic is transient. Starting does not create a trace. Closing its owning
  window releases live data. Clear covers the whole window: retain Undo for small
  removals, expire it after five minutes at 100 MB, and release immediately at 1 GB.
- Keep live-entry, HTTP-body and stream retention separate. Live entries default to
  no count cap; optional eviction preserves active and imported entries. Display
  resolved body limits and storage mode. New policies affect new boundaries or the
  next recording, without spilling already retained memory bodies to disk.
- Full queues backpressure capture rather than discard evidence. Capture limits
  never stop forwarding; retain measured counts and mark prefixes incomplete.
  Saved files have no fixed size/count ceiling, but indexing and processing stay bounded.
- Narrow settings writes merge; stale layout saves cannot overwrite authorization.
  Uncertain preference recovery disables remote access and recovered client-certificate
  rules until explicitly re-enabled, including after interrupted recovery.
- AppContainer exemptions preserve unselected OS entries. Separate selection from
  enabled state, disclose hidden selections and keep bulk actions outside the scroller.
  Closing the modal keeps result monitoring active without reopening it on completion.

## Automation and safe previews

Follow [automation](automation.md), [safe previews](safe-previews.md) and
[captured-page preview](captured-page-preview.md).

- Ordered auto-responses and User-Agent overrides use first enabled match. Do not
  infer restrictions from captured Vary or duplicate behavior from example URLs.
  Header presets replace existing fields only; preserve hints without authored
  replacements and allow explicit stripping. Reordering and deletion preserve drafts
  and offer Undo. Duplicate header-rule filters ask before replacement.
- Explain copied response files versus linked per-request files, keep missing paths
  repairable and preserve text drafts. Compression choices apply consistently to
  text and binary sources; UI choices use canonical names while evidence retains
  original coding tokens.
- Captured markup stays inert in the workbench. Page preview requires an isolated
  window, warning and per-preview script choice, initially off. Serve captured
  resources only; misses return empty 404s and app commands remain inaccessible.
- Preview matching uses method/URL in the chosen trace scope, with body, browser,
  process, headers and timing as preferences. Advance repeated URLs through captured
  responses, reset on reload and relax hints to available matches. Do not imply the
  result reproduces the original browser state.

## Files, viewers and updates

Follow [trace saving](trace-saving.md) and [Windows release policy](windows-release.md).

- Capture viewers contain Traffic and Composer; enforce main-window-only proxy,
  trust, recording, breakpoint and automation commands in the host as well as UI.
  Opening from outside asks merge/separate when the main window exists. Dropping
  files imports into that window. Preserve original trace associations.
- Drop targets name formats, offer a keyboard/click picker and highlight only when
  accepting. The initial Traffic helper never returns after clearing/filtering.
  Inspect metadata is read-only; Convert/Export suggests `exported` filenames.
- File recording chooses a destination before switching from live capture, waits
  for active work and offers cancellation. Use **Headers only** / **Headers and
  bodies**. File-only recording does not populate Traffic; Stop seals and restores.
- Save trace includes every retained entry except removed ones. Keep progress visible
  after its dialog closes; closing is not cancellation. Preserve existing destinations
  on failure, original source context on merged saves, and explicit incomplete evidence.
- Offer TMCap, HAR and both SAZ profiles with honest fidelity limits. Label missing-head
  placeholders and generated connection summaries, distinguish them from captured
  server responses, and report their counts. Preserve full stream evidence in TMCap.
- Export redaction, computer network context and password encryption are opt-in.
  Explain residual private data and visible ZIP member names. New passwords require
  confirmation; imports allow retry/cancel and publish nothing on failure. Never save
  passwords in preferences or diagnostics. Context retains collector, time and source.
- Updates stay nonmodal and leave the workspace usable. About owns version, checks,
  release details, website and Credits. Automatic checks default on and save independently;
  reminders pause prompts for thirty days. Checks/downloads use the active proxy route.
  Resolve drafts, capture, proxy and host cleanup before installation.
- Install updates in place. Preserve preferences, captures, handler choices and Windows
  defaults; remove only registrations owned by the exact installation. Offer independent
  TMCap/SAZ/HAR handlers, with TMCap selected initially. Uninstall-first is not the update flow.

## Review the whole flow

Exercise populated and empty states, wide and narrow layouts, resized panes,
long values, multiselection, loading, errors and unavailable evidence. Check pointer
reachability, keyboard navigation, focus restoration, alignment and editor actions.
Use real WebView2 for native behavior, accessibility or layout, and inspect screenshots
for visual changes. Add regressions for important behavior or reproduced failures;
avoid tests tied to incidental pixels. A build alone does not establish UX quality.
Documentation-only edits require content and link review, not app builds.

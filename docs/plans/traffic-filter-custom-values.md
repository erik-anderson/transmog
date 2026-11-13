# Traffic filter implementation reference

Status: implemented. The [Traffic guide](../traffic-inspection.md#search-and-select)
owns the current user workflow, labels and limits. The [UX principles](../ux-principles.md)
own the lasting conventions. This reference records the implementation boundaries
and regression fixture.

## Implementation boundaries

- Queries copy selected values, not Favorite identities. Library changes cannot
  rewrite an applied filter. `values` holds observed picks, `savedValues` holds
  custom terms, and `selectionMode` defaults to `either` for older payloads.
  `both` intersects populated groups; `ignore-saved` pauses the saved side.
  Additional requirements and different fields combine with AND.
- Facets receive the complete draft. Option counts lift the edited field's
  restrictions while retaining other fields' edits and the surrounding search,
  incident and extension scope. The preview counts unique requests matching the
  full draft before paging. Queries, views, selection/export IDs and explanations
  use the same normalization and matching rules, without body reads.
- Validate every edited field before saving or applying. Keep incomplete and
  over-limit drafts out of count requests. Late completion must not commit older
  edits, including a time bound whose displayed text is unchanged across a
  daylight-saving transition.
- The Favorite library is independent of layouts, captures and query limits.
  Canonicalize and validate a complete batch before one atomic write. Canonical
  keys make repeated adds idempotent; revisions and clear receipts keep Undo from
  overwriting intervening edits.
  A failed write retains the last good library and the editable draft.
- Desktop commands share one durable Favorite owner across the main window and
  saved-file viewers; each window keeps its own Traffic query and draft. Library
  refreshes preserve selections, roles, input and focus. Apply changes Traffic
  only after pending Favorite saves succeed.

| Area | Source |
| --- | --- |
| Queries, normalization and counts | [Session queries](../../crates/app/src/sessions.rs), [typed facets](../../crates/app/src/sessions/facets.rs), [choice catalog](../../crates/app/src/sessions/filter-catalog.json) |
| Favorite persistence | [Library and atomic operations](../../crates/app/src/traffic_favorites.rs) |
| Window ownership and commands | [Desktop commands](../../apps/desktop/src/windows.rs), [viewer command policy](../../apps/desktop/src/window_workspaces.rs) |
| Picker workflow | [State and handlers](../../apps/desktop/ui/src/traffic-workspace/traffic-workspace.ts), [template](../../apps/desktop/ui/src/traffic-workspace/traffic-workspace.html), [value parsing and chips](../../apps/desktop/ui/src/traffic-filters.ts) |
| Extension filters | [Retained-header classifications](../../crates/app/src/classifications.rs) |
| Wire and initial-render contracts | [Models](../../apps/desktop/ui/src/models.ts), [shared defaults](../../apps/desktop/ui/src/initial-state.json) |

## Acceptance fixture

Seed GET and POST to `api.example.test`, GET to `cdn.domain.example`, GET to
`eu.cdn.domain.example`, POST to `domain.example`, and GET to `otherdomain.example`.
Before each case, check only `api.example.test` and `*.domain.example` in the saved
selection. Counts are unique requests; reset other fields unless the row says
otherwise.

| Change to the draft | Expected preview and applied result |
| --- | --- |
| `either`, no observed picks | 5 of 6. The apex and both CDN hosts match the wildcard; the lookalike suffix does not. |
| `either`, pick the apex | 5 of 6; overlap counts once. Favoriting the apex leaves its new saved checkbox unchecked. |
| `both`, pick the apex | 1 of 6, including when an exact apex term is also checked in the saved group. |
| `both`, pick `otherdomain.example` | 0 of 6. |
| `both`, pick `cdn.domain.example` | 1 of 6; changing to `either` gives 5 of 6. |
| `ignore-saved`, pick the apex / clear that pick | 1 of 6 / 6 of 6. Saved checkmarks remain available. |
| `either`, no picks, Method restricted to GET | 3 of 4. Option search and paging do not change the result count. |
| `either`, no picks, add Host Contains `cdn` | 2 of 6. The added condition is saved and checked; a fresh window can reuse it unchecked. |

## Regression coverage

Keep coverage for:

- Empty Traffic and later arrivals; wildcard apex, deep descendants and lookalike
  suffixes; IDNs, IPs, canonical duplicates, malformed batches and literal commas.
  Preserve typed aliases, missing/Other choices and older query payloads.
- Multi-field Apply, combined previews, explicit Discard and chip removal without
  draft resurrection. Check keyboard focus, stable row positions, retained input
  and selected Favorites outside search results.
- Shared add/clear/Undo across windows and restart; atomic failed writes, retries,
  delayed saves and newer edits, including ambiguous local time bounds.
- Preview/list/timeline/selection/export agreement beyond the first page, with
  search, incident and extension scopes, paused views and zero/unavailable counts.
- Long values, compact layouts, zoom and forced colors, with reachable footer
  actions and native keyboard/accessibility checks where affected.

## Verification

Follow the [repository](../../AGENTS.md), [UI instructions](../../apps/desktop/ui/AGENTS.md)
and [UX quality routine](../ux-quality.md) for required checks. From
`apps/desktop/ui`, use `node scripts/check-workspaces.mjs --traffic-filters-only`
for fast iteration.

The [browser filter suite](../../apps/desktop/ui/scripts/check-traffic-filters.mjs)
includes [draft/race regressions](../../apps/desktop/ui/scripts/check-traffic-filter-review.mjs).
Run the [Windows acceptance runner](../../scripts/test-windows-desktop.ps1) with
`-ViewerChecks -TrafficFavoritesChecks` for real WebView2 main/viewer and restart
checks. Keep tested source hashes, command results, screenshots and qualification
limits in generated evidence; browser fixtures alone do not establish native
correctness.

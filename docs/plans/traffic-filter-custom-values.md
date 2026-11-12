# Traffic filter Favorites and reusable matches

## Implementation assignment

Implement reusable match lists in the existing Traffic field picker. A user must be able to paste `api.example.test, *.domain.example, future.example.test` before any requests arrive, apply it, then discover an outside host in Traffic and add it to the set. Deliver working behavior in the main window and saved-file viewers, regression coverage, and updated Traffic documentation. This plan is self-contained. The source paths and commands below were checked at `786011152ec69247ba0647c8426aaa338d032ccb`.

Today **Use exact value** accepts one unseen value at a time. **Custom match** replaces the previous condition with the same comparison. The existing picker already provides contextual counts, draft/apply, chips, range editors and several entry points. Extend those paths. [Current Traffic guide](../traffic-inspection.md#search-and-select)

## Interaction contract

**Saved and custom hosts** and **Observed hosts** are the two membership groups; other fields use values instead of hosts. Favorites and other custom terms belong to the first group. **Favorite** is the term for the star and the persisted entry. Use an outline star with the tooltip **Add to favorites** and a filled star with **Remove from favorites**. Give the button a stable accessible name such as **Favorite api.example.test** and expose its pressed state.

| Control | Immediate effect | Effect on Traffic |
| --- | --- | --- |
| Favorite star in either section | Add or remove the Favorite for this field across windows. | The current draft and applied selections keep their meaning. |
| Checkbox under **Favorites** | Select a reusable term such as `*.domain.example` in the draft. | Participates in the saved selection on Apply. |
| Checkbox under **Observed hosts** | Pick that exact hostname in the draft. | Participates in the picked selection on Apply. |
| **Save to Favorites** | Validate the entire paste and save its distinct values as Favorites, preserving checkbox selections. | Traffic remains unchanged. |
| **Save and apply** | With pending text, validate and save first, then apply the draft in one action. | On success, update Traffic and close the picker. |
| **Clear this field** | Clear both draft selections, checked additional requirements and input for this field. | Apply commits this change with the whole draft. Favorites remain available. |
| **Clear all Host Favorites** | Remove this field’s Favorites, including saved comparison terms. Report the count and offer **Undo**. | Existing selections and requirements retain their current values and roles. |

1. **Enter a list.** Put **Is in** under **Add values** in the saved/custom group. Start this authoring disclosure open when no reusable values exist; otherwise keep idle authoring compact. Accept comma- or newline-separated values even in empty Traffic or while counts load. Accept exact literals and Host wildcards together. Blank input adds nothing. A pending valid paste participates in the complete **After Apply** preview. For an invalid paste, keep the text, identify the offending item, and allow correction before committing any part of the batch.
2. **Work with an outside host.** Keep **Saved and custom hosts** above the searchable **Observed hosts** group, with **Combine groups** between them. Clicking an observed star adds a saved row, initially unchecked. The observed row stays in place with its checkbox and focus intact. Its two accessible checkbox names distinguish **Use saved term api.example.test** from **Pick observed host api.example.test**. The same host may participate independently on both sides.
3. **Remove a Favorite.** An active saved term keeps its value under **Other filter values** after the star is cleared. Identify it as a saved-selection term even when the Favorite is gone. Keep picked hosts accessible when option search or paging hides their rows. Clearing Favorites never transfers a selection from one side to the other. Undo restores removed Favorites and respects subsequent edits.
4. **Apply or dismiss.** Checkbox changes, combination mode and enabled requirements form one per-window draft across fields. Apply validates every edited field, saves pending pasted values atomically, and commits the whole draft. The preview includes all pending fields. Close, Escape, outside dismissal and field switching preserve drafts without prompting. Discard changes resets every field to its applied state; clearing all filters clears all drafts. The fixed footer, Filters button and field chooser expose unapplied state separately from applied filters. Explicit Favorite changes persist independently.
5. **Enter additional requirements.** Put **Additional requirements** after both membership groups and state that all checked requirements must also match. A fresh field starts empty. The user chooses a comparison, enters a value and explicitly adds the condition. For example, **Host contains “cdn”** exists only after someone enters `cdn`. Save authored conditions by field, operator and canonical value, present them with individual enable controls, and keep them separate from **Is in** alternatives. All enabled Contains conditions must pass. Keep a single active Equals and one bound per endpoint. Selecting a different value preserves the prior authored value for reuse.

### One combination control per field

Each nonempty selection matches any of its own checked terms. **Combine groups** applies to membership in this field. A Favorite star alone contributes no predicate.

| Saved selection | Picked selection | **Match either group (OR)** (default) | **Match both groups (AND)** | **Use observed hosts only** |
| --- | --- | --- | --- | --- |
| Populated | Populated | Matches saved OR picked | Matches saved AND picked | Matches picked |
| Populated | Empty | Matches saved | Matches saved | All pass membership |
| Empty | Populated | Matches picked | Matches picked | Matches picked |
| Empty | Empty | All pass membership | All pass membership | All pass membership |

Apply enabled **Additional requirements** conditions after the membership test, with AND. Other fields and the surrounding query scope also apply. Switching to Ignore retains the saved checkmarks. Adding Favorites or pasting while Ignore is selected preserves the mode. In Both mode, two nonempty disjoint sets legitimately return zero.

### Matching, inventory and feedback

- Match the parsed request host. Canonicalize case, IDNs and a trailing DNS dot for exact and wildcard comparisons. Preserve captured display evidence. A leading `*.` matches the base DNS name and zero or more whole descendant labels. `*.domain.example` matches `domain.example`, `cdn.domain.example` and `eu.cdn.domain.example`. `otherdomain.example` falls outside the suffix boundary. Accept exact IPv4 and IPv6 literals. Reject ports, schemes, paths, credentials, interior or bare asterisks, wildcard IP literals and malformed hosts. Parse both sides consistently. Trim each token, ignore empty separators and deduplicate canonical equivalents before checking limits.
- Offer **Is in** for built-in fields that support exact choices. Use their existing types and canonicalizers, including protocol aliases, normalized Content type, status codes/families, missing values, and Other. Synthetic choices retain their typed identity. Provide literal single-value entry for text fields where commas can be data. Preserve existing range editors, fixed-vocabulary restrictions, and the separate fixed extension-column picker. Reserve wildcard syntax for Host.
- Populate the observed inventory from **all retained requests in the surrounding scope with the edited field’s restrictions lifted**. Other fields, metadata search, incident scope, completed content-search results and applicable extension filters still constrain it. Option search and pagination change which choices are visible. Compute contextual row counts and the unique-request draft total before option or table paging. Overlapping matches count a request once in the preview.
- Keep outside rows actionable and in stable order. An unchecked exact host can read **Included by `*.domain.example`**. A picked `otherdomain.example` in Both mode can read **Outside Host filter · No saved term matches**. If a separate requirement fails, name that requirement. Explain a missing pick when only the saved side matches. Put **After Apply · 5 of 6 requests** beside Apply. Counts describe requests, including when several rows overlap. Show loading and unavailable states explicitly. Preserve typed text, focus and selection if an old response arrives late.

## Work sequence

### 1. Define the query contract and native matcher

Extend the existing per-dimension filter group. Keep legacy `values` as picked alternatives and add defaulted saved alternatives and combination mode, or use a wire-equivalent backward-compatible representation. Add a typed Host subdomain alternative and reject it on other fields and in exact observed choices. Carry values and selection roles in each query snapshot so editing a Favorite can never change an applied query. Check both Rust struct construction sites and older serialized inputs. Keep existing scalar and legacy Host/Method/Fetch destination restrictions valid.

Prepare normalized exact lookups and DNS suffix matching once per query. Preserve the fast path for unrestricted queries. Keep the existing bounds of 32 dimensions, 128 alternatives per field, 1,024 across fields, 14 scalar constraints and 255 characters per scalar value. Bound both role lists together, deduplicate within each role, and validate supplied input even when a mode pauses it. A large Favorite library is separate from the active query budget. Bound stored Favorites and paged reads independently. Explain capacity errors and keep the full paste editable. Check the existing host normalizer for reusable parsing. The support-profile matcher also includes the apex. Use one whole-label domain predicate for Traffic matching, counts and match explanations. [Session queries](../../crates/app/src/sessions.rs) · [Typed facets and limits](../../crates/app/src/sessions/facets.rs) · [Host normalization](../../crates/proxy-capture/src/support.rs)

### 2. Return contextual inventory and draft results from the same predicate

Extend the facet request/response or add a narrow draft-preview request. Supply the edited field’s draft separately from the applied surrounding scope. Validate the draft, then use the existing `unrestricted_query` behavior to build the candidate inventory. Compute the preview numerator, denominator, per-option counts, and match reasons from a consistent snapshot. Exact count-map lookups alone cannot count wildcard terms. Bound the work to retained metadata and requested options, respect the existing explicit-option and page limits, and avoid one full query per Favorite. For aggregate options such as 2xx, represent partial coverage accurately if showing match state.

Use the same predicate for query sessions, list and timeline views, selection of matching IDs, and filtered save/export scopes. Check query routing and any cache or view identity that depends on the filter. Preserve Follow newest, paused views, imported viewers and fixed content-search snapshots. Preserve the extension metadata budget and keep Traffic queries independent of body reads. [Application query boundary](../../crates/app/src/lib.rs) · [Extension classifications](../../crates/app/src/classifications.rs)

### 3. Persist Favorites through narrow shared operations

Store only values the user explicitly favorites or authors, keyed by stable field ID, typed term or comparison, and canonical value. A first run or older installation gets an empty library. Share it between the main window and saved-file viewers, with window-local selections. Store it in local user data independently of layout snapshots, capture files and case exports. Opening or searching traffic does not populate it.

Implement list, add-batch, remove and clear-by-field operations plus a reversible clear. Serialize mutations through a shared owner and atomically persist them. Derive notices and Undo from the entries actually removed. Undo must preserve intervening additions and later explicit removals. Use revisions or equivalent conflict handling for overlapping edits. Refresh other windows’ libraries without rewriting drafts. A retry must be idempotent. After a failed library write, keep the last good state and offer retry with the draft intact. Apply the new filter only after **Save and apply** has persisted its batch. If saving succeeds and a later query refresh fails, report each outcome accurately.

Viewer windows construct independent `Application` instances with temporary storage. Route the narrow Favorite operations to one durable owner and retain the viewer’s own Traffic query scope. Reuse the existing shared-owner pattern where suitable. Register only the commands the relevant windows need and extend their allowlist tests. [Desktop commands](../../apps/desktop/src/windows.rs) · [Viewer construction and command policy](../../apps/desktop/src/window_workspaces.rs) · [Shared preference owner example](../../crates/app/src/debugger_workspace.rs)

### 4. Integrate the picker, entry points and documentation

Extend the TypeScript models and query builder with the two selection roles. Represent durable Favorites, the window’s applied query and the open picker’s draft separately. Update field switching, row/context shortcuts, column menus, chip Edit/Remove, Clear filters, range controls and filtered actions to use the same contract. Existing value and comparison inputs reopen with their original meaning. Reuse native checkboxes, star buttons, select and disclosure controls in production WebUI templates. Keep the Apply/footer actions reachable on a compact screen. Preserve keyboard focus across Favorite changes and asynchronous updates.

Update the Traffic guide with one empty-capture paste and one outside-host example. Update the lasting filter conventions in the UX principles. Follow the root and scoped UI instructions, including the installed WebUI reference and the UX quality routine. [Picker logic](../../apps/desktop/ui/src/traffic-workspace/traffic-workspace.ts) · [Picker template](../../apps/desktop/ui/src/traffic-workspace/traffic-workspace.html) · [Filter helpers and chips](../../apps/desktop/ui/src/traffic-filters.ts) · [Wire models](../../apps/desktop/ui/src/models.ts) · [Initial render state](../../apps/desktop/ui/src/initial-state.json) · [UX principles](../ux-principles.md) · [UX routine](../ux-quality.md)

## Acceptance fixture

Seed six requests across five hosts. Use GET and POST to `api.example.test`, GET to `cdn.domain.example`, GET to `eu.cdn.domain.example`, POST to `domain.example`, and GET to `otherdomain.example`. Check `api.example.test` and `*.domain.example` in the saved selection. Reset to those two saved terms before each row. The preview counts requests.

| Draft operation | Expected preview and applied result |
| --- | --- |
| Either, no picks | 5 of 6. The unchecked apex and CDN rows show wildcard coverage. The misleading suffix remains outside the filter, searchable and actionable. |
| Either, pick the apex | 5 of 6. The apex overlaps the wildcard and is counted once. Favoriting it leaves its new saved checkbox unchecked. |
| Both, pick the apex | 1 of 6. Also checking an exact apex term under Saved values keeps the count at 1. |
| Both, pick `otherdomain.example` | 0 of 6 because the lookalike suffix is outside the saved wildcard. |
| Both, pick only `cdn.domain.example` | 1 of 6. Either on the same inputs gives 5 of 6, including the overlapping CDN request once. |
| Ignore, pick the apex / clear that pick | 1 of 6 / 6 of 6. Switching back restores the two saved checkmarks. |
| Either, no picks, also filter Method to GET | 3 of 4. Picker search and option paging leave that total intact. |
| Either, no picks, explicitly add Host Contains `cdn` | 2 of 6. A fresh library contains zero example conditions. After restart the saved condition can be available unchecked. |

### Regression scenarios

- Paste into empty Traffic, apply a zero-count term, and then introduce a matching request. Test deep wildcard matches, the apex, lookalike suffixes, canonical duplicates, IDNs, IPs, malformed and over-limit batches, and a literal comma on a supported text field. Verify legacy query payloads, range endpoints, and preserved typed choices.
- Favorite an unchecked outside host with the keyboard and press Escape. Reopen to find it favorited and unchecked. Remove a Favorite from an applied wildcard, clear Host Favorites with Process Favorites present, then Undo after an intervening edit. Verify applied results, surviving edits, and a fresh window’s unrestricted filter.
- In two windows, interleave add, clear, Undo and a stale Apply. Restart after a successful write. Inject a failed write, an invalid paste, a delayed facet page and a newer draft. Check persisted values, the Traffic result, focus and draft ownership in each case.
- Compare preview, list, timeline and filtered selection/export IDs beyond the first table page. Test search narrowing, late arrivals, a paused view, a saved viewer, zero versus unavailable counts, and compact/zoomed/forced-color layouts. Inspect populated wide and compact layouts with long values and the footer visible.

## Verification and handoff

Use Python 3.11+ and the repository’s portable dev wrapper. Run these checks from the repository root.

```powershell
python scripts/dev-env.py --check
npm ci --ignore-scripts
npm run check
python scripts/dev-env.py --run cargo fmt --all -- --check
python scripts/dev-env.py --run cargo clippy --locked -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop --all-targets -- -D warnings
python scripts/dev-env.py --run cargo test --locked -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop
```

Run tests for any additional changed crate. From `apps/desktop/ui`, run these UI checks and use the focused picker command for fast iteration.

```powershell
npm run check
node scripts/check-workspaces.mjs --traffic-filters-only
npm run test:workspaces
```

Extend the existing Traffic filter and coverage checks. Verify the real WebView2 picker and shared main/viewer Favorites with isolated app data through `scripts/test-windows-desktop.ps1` and its viewer check. Inspect the rendered UI and actual persistence across restart. Record the tested commit, commands, results, screenshot locations and any outstanding limitations in the implementation handoff. [Repository instructions](../../AGENTS.md) · [Scoped UI instructions](../../apps/desktop/ui/AGENTS.md) · [Browser interaction checks](../../apps/desktop/ui/scripts/check-traffic-filters.mjs) · [Filter coverage](../../apps/desktop/ui/scripts/check-filter-coverage.mjs) · [Windows acceptance runner](../../scripts/test-windows-desktop.ps1)

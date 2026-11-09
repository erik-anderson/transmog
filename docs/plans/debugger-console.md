# Debugger Console implementation handoff

Implement a Console where ordinary logs are visible immediately, warnings and errors are easy to spot, and a user can inspect or copy the value they clicked. The design applies to Scratch, script workers and extension workers in each owning window. The reviewed baseline is `6471698e` on `main`.

## 1. Separate messages from command results and add level filtering

Today the level selector accepts one severity or all of them. Callback logs already carry a level. During prompt evaluation, the debugger receives individual output events but the UI concatenates them into the command result. A command that calls both `console.warn()` and `console.info()` therefore cannot be filtered correctly.

- Render each log call as a distinct row with a stable identity, including calls made at the Scratch or worker prompt. Associate the rows with their originating command and context. Preserve call order across refreshes, show the command once, and keep its result or thrown exception visible when changing levels.
- Replace the selector with one **Levels** button and a menu of independent checkboxes. Apply each toggle immediately and keep the menu open for further choices. Show **Default levels**, **All levels**, or a short selection summary such as **Warnings + Errors**. If all levels are off, make that state clear and keep the prompt usable.

| Visible level | Methods | Default |
| --- | --- | --- |
| Info | `console.log`, `console.info`, `console.trace` | On |
| Warnings | `console.warn` | On |
| Errors | `console.error` | On |
| Verbose | `console.debug` | Off |

Keep trace frames available under Info. Apply the same mapping to callback logs, prompt logs and existing logpoint severities. Preserve notices for omitted or incomplete output and distinguish those from messages hidden by a filter. Filtering changes visibility only. **Clear output** still clears the transcript, and **Reset scratch** still affects only Scratch.

Keep **Run in**, the output context filter and **Pause in** independent. Preserve level choices across docking, detaching and reopening the debugger in its owning window. For migration, map an old single level to its new group. Use the new defaults for a missing setting or legacy `all`, since the previous format stored the same value for an intentional choice and the default. [Chrome Console API levels](https://developer.chrome.com/docs/devtools/console/api)

## 2. Make mixed output easy to scan

Keep Info neutral and readable. Give warnings an amber triangle and a subtle tint, and errors an error icon and a red tint. Use theme colors, accessible level names and distinct icon shapes so the difference survives forced colors. Distinguish command input from returned values at a glance. Use restrained color for strings, numbers and booleans in object previews.

Use one compact row per message. Keep context secondary, align source links consistently when a location is available, wrap long messages, and leave technical identifiers in **Details**. Render an expandable result preview once. In a narrow dock, keep the prompt, message text and disclosure controls reachable while new messages arrive.

## 3. Expand logged objects as snapshots

The current **Result** inspector already expands nested live values and handles large collections. Ordinary logs retain flat labels such as `[Object]`. Extend the logging path so `console.warn("request", obj)` shows the string followed by an expandable object in argument order.

- While a debugger is attached, capture a bounded snapshot at the instant of the log call. Carry it with both ordinary callback records and debugger evaluation output events. Keep readable text as a fallback for records without snapshots. Continue using the existing live inspector for expression results.
- Capture safe own data properties. Show accessors and proxies as labeled leaves without invoking getters, proxy traps, custom string conversion or `toJSON`. Handle arrays, repeated references and cycles. Bound depth, children, total nodes, bytes and capture work. Show where data was shortened.
- Account for snapshot payloads in worker record, invocation, transport and UI retention limits. Validate incoming snapshots before retaining them. Check protocol compatibility across script and extension hosts. Keep snapshots local to debugger output. Preserve the existing lightweight logging path when detached.
- Reuse the existing disclosure controls for independent branch expansion and collapse. Keep a retained snapshot readable after mutation, worker completion or Scratch reset. Live expression results still expire with their execution context. Filtering or appending messages should preserve expansion and reading position for retained rows.

## 4. Add menus that act on the clicked item

Reuse the app's copy-menu placement, selected-text handling and keyboard conventions. Offer actions according to the target.

| Target | Actions |
| --- | --- |
| Selected text | Preserve the existing selection actions and the exact selection. |
| Message | **Copy message**. Offer **Reveal in source** when a valid location exists. |
| Object property | **Copy name** and, for a scalar, **Copy value**. For a branch, **Copy displayed subtree** copies the displayed names and values with indentation and visible omission markers. |
| Transcript background | **Copy visible output** for the current context and level filters, and **Clear output**. |

Copy from the displayed record without evaluating JavaScript or fetching hidden descendants. Right-click should preserve selection, focus context and expansion. Support Shift+F10 and the Context Menu key, arrow-key navigation, Escape and focus return. Bind actions to stable row and property identities so a refresh cannot redirect a copy to a different item. [App copy conventions](../copying-inspection-values.md) · [Chrome Console actions](https://developer.chrome.com/docs/devtools/console/reference#clear-copy-expand-collapse-and-save-messages)

## Implementation entry points

- **Rows, filtering, view state and menus:** `apps/desktop/ui/src/debugger-workspace/debugger-workspace.{ts,html,css}`. Start with `updateConsoleFeed`, the `output` and `evaluated` event handlers, and view save/restore. `console-output.ts` tracks cleared records and loss notices.
- **Property rendering and copying:** `debug-value.{ts,html,css}`, `properties.ts` and `inspection.ts` in that same directory. Shared menu support lives in `apps/desktop/ui/src/inspection-copy-surface.ts`, `inspection-copy-menu/` and `context-menu.ts`.
- **Runtime and wire data:** `crates/js-runtime/src/console.rs` captures arguments. `crates/js-runtime/src/debugger/mod.rs` forwards prompt output. Start with `logging.rs`, `debugger.rs` and `supervision.rs` in `crates/script-diagnostics/src/` for record bounds, event shape and validation.
- **Help:** update `crates/proxy-script/help.json` and the Console guidance in `docs/automation.md`. Regenerate `docs/scripting.md` with `node scripts/script-help.mjs`.

## Acceptance and verification

1. From a fresh Scratch console, emit an info message, a warning, an error and a debug message. The first three appear. Enable Verbose to reveal the retained debug message. Select Warnings + Errors, then restore Info. Repeat with mixed calls from one command and from a script callback. Check commands, results, logpoints, traces and source links.
2. Log an object with nested headers, mutate a header, then finish the callback. Expand and collapse the logged header and verify its original value. Exercise multiple arguments, arrays, cycles, accessors, proxies and oversized inputs. Verify hostile hooks never run and a flood stays within the existing budgets.
3. Right-click a scalar, a branch, selected text and empty transcript space. Inspect the actual clipboard contents. Repeat with the keyboard. Filter and clear while new rows arrive, then dock, detach and reopen. Check target identity, focus return, scroll position, filter choices and isolation between owning windows.
4. Inspect populated wide, compact and forced-color layouts. Extend the existing debugger browser checks and native Windows debugger probe for the new behavior. Capture evidence showing a mixed transcript, a nested warning and the contextual menu.

Follow the root and desktop UI `AGENTS.md`, including the UX review routine. Use Python 3.11+ and run `python scripts/dev-env.py --check` before building. Run the prescribed root and UI checks, the affected Rust tests and strict Clippy through the dev wrapper, and the Windows `-ScriptDebuggerChecks` probe. The focused browser entry point is `node scripts/check-workspaces.mjs --debugger-workspace-only` from `apps/desktop/ui`. Review the final diff and rerun affected checks after fixes. Report the checks, evidence and any remaining limitations.

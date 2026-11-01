# Script debugging completed implementation record

Completed (2026-11-01): phases 0–8 are implemented, reviewed and qualified.
Workspace, strict static checks, browser and real WebView2 journeys passed,
along with the final 10,000-case protocol fuzz campaign and all 17 Docker-origin
interoperability tests. Both isolated test projects have no remaining resources.
Acceptance results, source identity and phase review evidence are under
`artifacts/script-debugging/initial/`. The maintained behavior is documented in
[Automation](automation.md#debug-script-execution), [Script API](scripting.md),
[Extensions](extensions.md#debugging-a-visualizer) and [ADR 0011](adr/0011-local-script-diagnostics-and-debugging.md).
The phased plan below is retained as a completed implementation record. Future
changes should follow the maintained guides; this is no longer an active handoff.

Implement consistent exception diagnostics, bounded debug logging, and an
interactive JavaScript/TypeScript debugger for traffic scripts and packaged
visualizers. Deliver useful diagnostics first, then debugging of isolated test
runs, then explicitly armed debugging of live execution.

Original execution target: **Sol 6.1 with Extra High reasoning**. Complete the
review gates in every phase before advancing. Keep production runtime isolation,
traffic semantics, and the applied script revision intact.

## Scope and completion

The completed work must provide:

- Exception type, message, bounded causes, and navigable call frames with correct
  source attribution for initialization and every supported callback.
- Debug, info, warning, error, and trace output that can survive a callback
  throwing, failing validation, timing out, or terminating its worker.
- Source breakpoints, pause on exceptions, continue, step into, step over, step
  out, call stack selection, and read-only inspection of locals and closures.
- Traffic-script debugging in both single-callback tests and exchange scenarios.
- Visualizer debugging across initialization, input/context/decoded continuations,
  expansion, and explicitly selected live updates.
- Explicitly armed live traffic-script debugging, with bounded waiting and
  truthful behavior when other exchanges share the same worker.
- Tests, product documentation, generated declarations/help, and review evidence
  covering successful, failed, canceled, and stale operations.

Remote debugger access, a public debugging MCP API, arbitrary expression
evaluation, variable mutation, conditional breakpoint expressions, live source
editing, and a VS Code adapter are outside this delivery. The internal design
should permit future adapters without exposing unrestricted Inspector commands.
Do not replace V8 or run extension code inside the desktop WebView.

## Repository requirements

Read the root [AGENTS.md](../AGENTS.md), [UX quality routine](ux-quality.md),
[UX principles](ux-principles.md), [desktop UI instructions](../apps/desktop/ui/AGENTS.md),
[building guide](building.md), and [testing guide](testing.md). Before WebUI work,
use the repository's `webui-reference` skill to resolve and read the consuming
application's installed `@microsoft/webui/ai.md`.

Use a working Python 3.11 or newer and run `python scripts/dev-env.py --check`
before building. Use `python scripts/dev-env.py --run cargo ...` for Rust commands.
On this workstation the initial review used
`C:/Users/erik/AppData/Local/Python/bin/python.exe` because the Windows Store
`python` alias could not execute. Resolve the interpreter again when starting.

For TypeScript or `.mjs` edits, install root tooling with
`npm ci --ignore-scripts` and run root `npm run check`. The root check and
`npm --prefix apps/desktop/ui run check` cover different requirements; both are
needed. Prepare UI assets before Rust builds that embed them.

Inspect the actual desktop flows before modifying them. Preserve user work,
existing selections, and unrelated changes. Native verification must use an
owned disposable profile; do not close a user's running app or alter their
system proxy or trust settings to make a test pass. Release dispatch is outside
this task.

## Starting evidence and code map

The reviewed source baseline is commit
`1f2f17fa9e0029baccb0b0de1b624b984453a613`. Reconfirm the behavior against the
checkout used for implementation; preserve any intervening user changes.

| Area | Starting behavior and primary files |
| --- | --- |
| Shared V8 runtime | [js-runtime](../crates/js-runtime/src/lib.rs) extracts a bounded stack string. Inspector bindings exist in the pinned `v8` dependency, but the runtime does not create an Inspector session. |
| Traffic failure contract | [proxy-script](../crates/proxy-script/src/lib.rs) carries category, message, line, and column. [script-host](../crates/script-host/src/lib.rs) truncates text to 512 characters and parses the first `main.js` position. |
| Traffic mapping and supervision | [script-supervisor](../crates/script-supervisor/src/lib.rs) maps one position and enforces callback deadlines. [app scripts](../crates/app/src/scripts.rs) flattens some failures into application messages. |
| Terminal callbacks | [adapter](../crates/proxy-script/src/adapter.rs) retains only the category for live terminal callback exceptions. |
| Visualizer failures | [extension runtime](../crates/extension-host/src/runtime.rs) replaces exception details and parse locations with generic explanations. [extension protocol](../crates/extensions/src/protocol.rs) has no structured exception payload. |
| Diagnostic output | [traffic result contract](../crates/proxy-script/src/contract.rs) and [visualizer documents](../crates/extensions/src/document.rs) carry diagnostics with returned results. No console API or independent log stream is installed. |
| Outer deadlines | [hook chain](../crates/proxy-core/src/intercept/chain.rs), body pipelines, and [scenario runner](../crates/proxy-script/src/scenario.rs) impose limits in addition to worker supervisors. |
| Desktop | [script editor](../apps/desktop/ui/src/script-editor/script-editor.ts), [traffic workspace](../apps/desktop/ui/src/traffic-workspace/traffic-workspace.html), [visualizer](../apps/desktop/ui/src/extension-visualizer/extension-visualizer.ts), and extension settings own the relevant flows. Trace native commands through [windows.rs](../apps/desktop/src/windows.rs) and application/session boundaries. |
| Disclosure | [MCP projections](../crates/mcp/src/application.rs), inspection paging/search, traces, portable cases, and support diagnostics must not acquire new disclosure paths. |

The initial review passed nine runtime/host unit tests and two Windows sandbox
tests for TypeScript exception positions. Those results establish the starting
behavior only; they do not qualify this implementation.

## Review process required in every phase

Each phase follows **design, implement, verify, review, fix, verify again**.
Use at least three substantive review passes with the phase-specific lenses
below. A review is a new examination of the resulting behavior and diff, not a
restatement of the implementation summary.

1. **Contract and correctness review.** Trace inputs through every affected layer
   to visible output. Check invariants, compatibility, state transitions, and
   negative cases. Reproduce suspected defects before dismissing them.
2. **Adversarial and lifecycle review.** Exercise malformed inputs, sensitive
   data, resource limits, races, cancellation, ownership changes, and worker loss.
   Verify that fixes from the first pass did not create another defect.
3. **Product and regression review.** Repeat the complete author task with
   representative data, then inspect code maintainability and unaffected flows.
   For UI phases, inspect rendered screenshots and complete keyboard workflows.

Fix every actionable in-scope finding and rerun the checks affected by the fix.
Close a phase only after **two consecutive review passes on the final revision
find no actionable in-scope defects**, with all required tests passing. The
three mandatory passes can count toward those two clean passes. A change after
a clean pass resets the clean-pass count. Additional reviews do not require
rerunning unchanged passing test suites without a reason.

Keep records under `artifacts/script-debugging/<run-id>/phase-N/`: source commit
and diff identity, review lens, findings with severity and locations, fixes,
commands and outcomes, screenshots where applicable, and remaining limitations.
Keep a concise phase checklist there for resuming work. Do not check generated
review transcripts into product guides. The implementation agent can perform
the review passes itself; separate reviewer agents are not a prerequisite.

A failed test, unverified acceptance criterion, or environment blocker is not a
clean pass. Report it accurately and continue independent work. Do not mark a
phase complete by silently removing its requirements or disabling its tests.

## Phase 0 Design contracts and prove the Inspector integration

**Deliverables**

- Record the exact baseline commit and relevant runtime, package, and protocol
  versions. Inventory all error, log, source, deadline, worker-ownership, and
  disclosure paths. Include module initialization and terminal callbacks.
- Define runtime-neutral diagnostic, log, and debug-session contracts. Keep V8
  handles and Inspector wire details inside runtime/worker implementation layers.
  Add a small shared contract module or crate only where it avoids duplication
  without creating dependency cycles.
- Prototype Inspector creation, context registration, a source breakpoint,
  pause/resume, one step operation, and local-variable retrieval in the pinned
  Rust V8 embedding. Run the essential path in the real Windows AppContainer.
  Use a native control channel; do not open a debugger network listener.
  Consult the [V8 embedding guide](https://v8.dev/docs/inspector) and
  [Inspector protocol definitions](https://github.com/ChromeDevTools/devtools-protocol/blob/master/json/js_protocol.json),
  then verify the exact API supported by the pinned dependency. Do not assume
  the current upstream protocol and our embedded version are interchangeable.
- Define authenticated session, worker generation, invocation, and pause
  identities. Enumerate legal state transitions: starting, ready/armed, running,
  paused, resuming, stopping, completed, failed, and expired. Distinguish script
  activation pause from a debugger pause in both API and UI vocabulary.
- Set concrete, tested limits for exception depth/frames/bytes, log records/bytes,
  command/event queues, retained sessions, property pages, pause leases, arm
  duration, and session lifetime. Establish baseline execution cost before
  choosing performance acceptance thresholds; record those thresholds before
  implementation. Debugging must remain finite even if a client stays connected.
- Design bounded outbound event delivery and the input pump used while V8 is
  paused. Define priority for stop/result/state messages over lossy log events,
  ordering, dropped-event counters, and request/reply correlation.
- Define read-only inspection of objects without executing getters, proxy traps,
  custom formatters, `toJSON`, or user `toString`. Explicitly represent unavailable
  or unsafe values rather than pretending every local is materializable.
- Update or add an ADR only for durable cross-cutting decisions. Keep phase
  progress and test transcripts outside ADRs.

**Review lenses**

| Pass | Required challenge |
| --- | --- |
| 1 | Can both extension surfaces use the contracts without V8 leaking into core or separate incompatible debugger implementations? |
| 2 | Can paused execution still receive stop/cancel, and can a compromised or stalled worker extend a pause indefinitely? |
| 3 | Can an author distinguish draft, executing source, applied revision, activation pause, debugger pause, and fresh fixture state? |

**Exit:** the pinned-engine spike demonstrates breakpoint, stepping, locals,
resume, and cancellation under the sandbox; the identity, resource, compatibility,
and state-transition decisions are concrete enough to implement. Do not advance
to debugger UI based only on an in-process or unsandboxed demonstration.

## Phase 1 Preserve structured exceptions and source locations

**Implementation**

- Add bounded exception details alongside safe host status: classification,
  exception name/message where safely available, frames, generated location,
  mapped source location, cause chain, and explicit truncation/unavailable flags.
  Keep script-authored text distinguishable from host-authored explanations.
- Capture engine-owned stack information where available. Avoid making a user
  `stack` getter or string conversion the primary extraction mechanism. Handle
  primitive throws, non-Error objects, overridden stacks, cycles, and failing
  diagnostic access without losing the original failure classification.
- Preserve details through worker IPC, supervisors, application errors, sandbox
  results, live traffic, initialization failures, and terminal callbacks.
  Retain existing exchange failure behavior; terminal errors remain evidence
  after completion rather than retroactively changing the exchange outcome.
- Map every available traffic-script frame through the exact executing build's
  source map. Keep generated locations as a fallback. Use structured origins
  instead of searching arbitrary exception text for `main.js`.
- Define an optional, versioned package declaration for local visualizer source
  maps and original sources. Bind it to the immutable package inventory/digest.
  Map all imported modules. Missing or invalid optional maps degrade to generated
  JavaScript locations with an explanation; they do not prevent valid JavaScript
  from executing. Never fetch source-map URLs or read arbitrary local paths.
- Preserve script sources and source maps according to existing artifact
  persistence rules. Debug-session values and exception details remain ephemeral
  unless an existing, explicitly chosen export contract permits them.
- Bump affected private host protocols together and test rejection of incompatible
  helpers. Preserve old saved script builds and extension packages when the new
  diagnostic metadata is absent. Review public API/version implications explicitly.
  Changes to declarations and compiler provenance must not force re-transpilation
  of an already applied JavaScript artifact on restart, resume, or retry.

**Verification and review**

Pass 1 traces nested throws through initialization, all six traffic handlers, and
every visualizer callback; tests cover multiple mapped frames, imported modules,
helper frames, Unicode columns, and generated-source fallback. Pass 2 attacks
cause cycles, huge messages, hostile getters, malformed maps, path escapes,
forged origins, and redacted MCP reads. Pass 3 verifies that the actual executed
revision is displayed after draft edits, retries, failed activation, and reload.

**Exit:** both surfaces retain actionable exceptions end to end; no terminal or
initialization path reduces an available detailed failure to its category alone;
mapping never points into an unrelated current draft.

## Phase 2 Add bounded logging independent of returned results

**Implementation**

- Install a host-owned `console` with `log`, `debug`, `info`, `warn`, `error`, and
  `trace`. Define the supported argument rendering and formatting explicitly;
  do not imply full browser/Node console compatibility. Add matching TypeScript
  declarations and examples for both authoring surfaces.
- Bind each log to the exact revision/package generation, callback or initializer,
  invocation, exchange or visualizer instance, sequence, level, and time. Record
  trace frames using the same source mapping as exceptions.
- Stream bounded log events through framed IPC while code executes. Preserve
  records already accepted by the supervisor when the callback throws, result
  validation fails, the deadline expires, or the process dies. Mark an incomplete
  stream; do not promise to recover messages still inside a killed process.
- Use bounded serialization that cannot invoke user code or inspect inaccessible
  inputs. Render cycles, proxies, functions, BigInt, symbols, typed arrays, and
  oversized values predictably. Avoid retaining live V8 objects for ordinary logs.
- Make logger overflow truncate/drop with visible counts, without blocking the
  worker indefinitely or invalidating an otherwise valid traffic decision.
  Logging CPU work still counts toward execution limits. Preserve the existing
  validation contract for explicitly returned `diagnostics` unless intentionally
  versioned; integrate their display without duplicate records.
- Retain bounded per-run/instance buffers plus a global memory budget. Define
  clearing, eviction, level filters, and startup-failure retention. Keep all new
  log content out of ordinary support logs, traces, and agent responses unless
  the relevant explicit disclosure policy allows it.

**Verification and review**

Pass 1 checks ordering and attribution across successful, throwing, invalid-result,
and terminal callbacks. Pass 2 floods output, stalls the reader, cancels during
logging, kills a worker after an acknowledged record, and attempts hostile value
formatting. Pass 3 checks that logging does not change traffic decisions, poison
later invocations, leak across runs, or require enabling a script to inspect a
failed initialization.

**Exit:** authors can see the accepted messages preceding failures, understand
truncation, and use logging without changing successful result semantics.

## Phase 3 Present diagnostics and logs in the desktop

**Implementation**

- Add a compact shared diagnostic/log presentation to script test results, live
  script findings, and visualizer developer details. Use readable summaries with
  expandable exception causes and call frames; preserve ordinary traffic density.
  Keep the visualizer's ordinary status host-controlled. Put detailed authored
  exception/log text in explicitly opened developer details with an explicit copy
  action, rather than spreading it into generic application status or support logs.
- Make frame navigation select the executing source snapshot and exact position.
  Prefer the matching draft only when its identity is verified. Otherwise open
  a labeled read-only source view, with generated JavaScript fallback. Keep
  source maps and code as inert data under existing CSP/Trusted Types rules.
- Provide accessible level filters, clear, copy, truncation counts, and visible
  attribution. Copy the underlying bounded evidence, preserving its disclosure
  state. Label any shortened view and avoid copying explanatory UI prose as logs.
- Preserve TypeScript issue navigation and activation/recovery flows. Do not
  flatten structured failures into `describeError` before rendering them.
- Show logs from initialization and visualizer continuation failures even when
  no final document exists. Keep last good visualizer output separate from the
  failure of its replacement. Clear or label stale results when identity changes.
- Recheck MCP projections, search, paging, saved traces, portable cases, and
  support reports. New nested diagnostic fields must follow existing disclosure
  grants; source code and locals must not become indirectly readable by MCP.

**Verification and review**

Pass 1 follows an author from failure to source to repair to rerun on both
surfaces. Pass 2 delays results while editing, switching requests, reloading an
extension, closing a pane, revoking disclosure, or changing window ownership.
Pass 3 inspects populated layouts at 1440x900, 960x720, and 800x600, plus zoom,
forced colors, keyboard use, focus restoration, long paths, and large log lists.

**Exit:** real native failures and logs are inspectable and navigable, with
readable rendering and no regressions in ordinary script or traffic workflows.

## Phase 4 Implement the debugger runtime and session lifecycle

**Implementation**

- Turn the Phase 0 spike into a production-quality optional Inspector integration
  in `transmog-js-runtime`, shared by the two hosts. Explicitly register/destroy
  contexts and release sessions and object handles before their isolates.
- Expose a narrow native command set: breakpoint management, pause-on-exception
  policy, continue, step into/over/out, stack retrieval, and paged scope/property
  reads. Include caught and uncaught exception modes. Handle `debugger` statements
  only in an authorized session. Reject unsupported protocol commands.
- Bind handles and commands to session, worker generation, invocation, and pause
  generation. Invalidate stack/scope/object handles on resume, reload, restart,
  stop, or ownership loss. Preserve lexical scope and shadowing; explain values
  unavailable because of optimization or missing source information.
- Map source breakpoints to executable generated positions. Return resolved,
  pending, or unbound status and the actual location; do not show a breakpoint
  as active merely because an editor decoration exists. Support breakpoints set
  before module initialization and in package-relative imported modules.
- Implement the pause-time input pump without V8 access from an unsafe thread.
  Ensure queued application callbacks cannot run during nested Inspector message
  processing. Stop must work while paused and while running an infinite loop.
- Introduce explicit bounded debugger pause leases. Keep startup, active execution,
  waiting-for-admission, and debugger-pause time separately accounted for. Scope
  deadline adjustments to authorized sessions in the supervisor and outer hook
  chain/body/terminal paths; ordinary hooks keep their existing protections.
- Use independent native enforcement for cancellation, client loss, session and
  pause expiry, and memory limits. Stop must remain effective if the V8 thread or
  Inspector transport becomes unresponsive. Do not rely on UI heartbeats as the
  only resource bound.
- Keep debugger transport local and authenticated inside the existing sandbox
  boundary. No shipping `--inspect` listener or WebView DevTools dependency.
- Restrict source retrieval to verified user modules and constant bundled helpers.
  Internal invocation scripts contain serialized event values; exclude their
  source text from debugger source views, source caches, and source copying.
  Reject breakpoint `condition` parameters as well as standalone evaluation
  methods; a method allowlist alone does not make Inspector commands read-only.

**Verification and review**

Pass 1 uses a deterministic engine fixture to assert actual pause positions,
step order, stack frames, locals/closures, and resolved breakpoints. Pass 2 tests
stale handles, forged commands, flood limits, repeated attach/detach, pause during
initialization, infinite loops, disconnect, pause expiry, and cancellation races.
Pass 3 measures debugger-disabled overhead and confirms that enabling inspection
does not grant new filesystem, network, dynamic-import, or string-code access.

**Exit:** both real sandboxed hosts pass the same debugging conformance suite;
sessions always release handles, workers, queue slots, and retained inputs.

## Phase 5 Deliver traffic script debugging in sandbox tests

**Implementation**

- Add **Debug test** alongside existing callback/scenario testing. It starts a
  fresh isolated worker using the exact tested draft build and parameter values,
  without saving, enabling, or modifying the applied script.
- Build the shared debugger panel: source breakpoint gutter, resolved/unbound
  indicators, current statement, continue/step/stop controls, call stack, scopes,
  paged values, exceptions, and logs. Show mode, revision, selected phase, paused
  state, and remaining bounded pause/session time.
- Use conventional scoped keyboard shortcuts with visible labels; do not steal
  editor, dialog, or text-entry shortcuts. Breakpoints are local user debugging
  preferences, never part of executable source or an automatically activated rule.
- Run scenarios through production selection and body pipelines, preserving
  request/response state, native filters, phase skip reasons, and terminal-error
  verdicts. Debug pauses do not alter controlled fixture time. Single-callback
  mode still explains bypassed filters and empty initial exchange state.
- Keep source immutable for the running session. Editing the authoring draft
  marks it newer than the executing snapshot; restart debug explicitly compiles
  that draft. Do not silently apply edited code or move old breakpoints to a new
  revision without re-resolution.
- Stop cancels the debug run and retains bounded diagnostic evidence. A running
  test does not become successful just because execution was stopped. Resuming a
  completed run is invalid. Closing the owning debug session stops it; ordinary
  workspace navigation preserves reachable ownership and controls.

**Verification and review**

Pass 1 debugs a nested TypeScript helper, changes a header/body through normal
code, and steps across a complete request/response scenario with exchange state.
Pass 2 tests edits while paused, new test input, double clicks, old command
responses, terminal exceptions, timeout expiry, and window close. Pass 3 repeats
the task in real WebView2 using keyboard and pointer at all required layouts,
then verifies existing Save draft, Save and enable/apply, Pause, Retry, and
language-recovery flows.

**Exit:** an author can diagnose a failing draft using breakpoints, stepping,
call stack, and locals, repair it, and rerun without disturbing live automation.

## Phase 6 Deliver visualizer debugging

**Implementation**

- Expose an explicit developer debug action from extension management and the
  selected visualizer's details, reusing the debugger panel. Installed and
  development packages keep identical sandbox and declared-input restrictions.
- Provide a fresh debug instance using the exact package snapshot and an explicit
  selected-message fixture. Bind granted retained sources/context and leases to
  that run. Never add undeclared headers/bodies or fetch uncaptured data merely
  because a debugger is attached.
- Support pauses in initialization and every callback: `visualize`, `resume`,
  `resumeContext`, `resumeDecoded`, `expand`, and declared `onTraffic`. Preserve
  continuation tokens and native validation while paused. Expose operation and
  continuation identity without presenting protocol machinery as primary UX.
- Make fixture versus live-instance debugging explicit. A new fixture instance
  has fresh module globals. For existing-instance debugging, attach at a safe
  operation boundary and preserve its current globals and parser state.
- Queue/coalesce live hints under existing bounds; account for missed hints and
  do not run another callback while paused. A stopped/hidden instance releases
  its worker. Package reload invalidates the old debug session and handles rather
  than retargeting them to new code; show the last good document appropriately.
- Render TypeScript sources when verified maps are packaged; otherwise provide
  full JavaScript debugging. Keep authored document diagnostics separate from
  runtime exceptions and debug logs.

**Verification and review**

Pass 1 steps through a real bundled parser with nested imports and input,
context, decoded-input, and expansion continuations. Pass 2 exercises body
eviction, canceled retention, revoked inputs, bad tokens, package reload,
worker replacement, hidden views, and concurrent windows. Pass 3 verifies that
debugging is discoverable but ordinary Auto visualization, last-good-result
behavior, live updates, and extension management remain direct.

**Exit:** authors can debug a packaged visualizer through its whole lifecycle
without widening its inputs or losing the identity of the executing package.

## Phase 7 Add explicitly armed live traffic script debugging

**Implementation**

- Add **Debug next matching exchange** for an exact applied revision, with a
  visible native request filter, a bounded arming lifetime, and one selected
  exchange per debug session. Keep this distinct from existing HTTP request
  editing breakpoints and from pausing script activation.
- Attach to the actual worker at a safe invocation boundary. Preserve module
  globals and exchange state. A fresh worker is available through sandbox testing;
  do not substitute it silently when debugging an existing live revision.
- Establish an exclusive debug reservation on the script worker. Drain existing
  work under ordinary deadlines before arming, or return a bounded busy outcome.
  While armed/owned, admit the selected matching exchange and reject conflicting
  admissions to that revision with a clear native busy failure. Never silently
  skip the script, route to a copied runtime, or enqueue unbounded waits.
  Recognize every callback of the selected exchange as the reservation owner.
  A competing admission must not enter the ordinary queued-invocation cancellation
  path and accidentally kill the reserved worker. If older pinned exchanges can
  still request later callbacks, treat them explicitly as conflicts; an empty
  current callback queue alone does not establish exclusive exchange ownership.
- Explain the impact before arming: other requests requiring that revision may
  fail while it is reserved. Traffic using unrelated workers keeps its normal
  behavior. The reservation is temporary and never persists across restart.
- Apply pause-aware timing only to the reserved exchange's relevant callbacks and
  pipelines. Cancellation, native transport limits, shutdown, and independent
  memory/session bounds still apply. An upstream/client disconnect can end the
  run while paused; make the resulting handles and Continue action invalid.
- Continue preserves normal script decisions and validation. On normal completion,
  release the reservation and detach breakpoints while retaining the reusable
  worker. Stop, owner loss, or expiry aborts the affected work and terminates the
  worker when safe resumability cannot be established; show unavailable state and
  require the existing explicit Retry applied script recovery.
- Define activation/revision replacement and multiwindow ownership atomically.
  Ending a debug session must not enable a paused script, restore an obsolete
  revision, or undo a newer user activation choice. Allow one controller; other
  windows can see ownership and state without competing commands.

**Verification and review**

Pass 1 uses controlled proxy traffic to prove preserved module globals, exact
revision execution, correct state across callbacks, and normal edited output
after Continue. Pass 2 stresses concurrent admissions, an already busy worker,
unmatched requests, cancellation while paused, client disconnect, stop/continue
races, owner loss, expiry, and replacement of the applied revision. Pass 3 checks
that the reservation's impact and recovery are understandable and that ordinary
proxy throughput, HTTP breakpoints, timeouts, and unrelated scripts are unchanged.

**Exit:** live debugging cannot silently bypass automation, confuse code
revisions, hold unbounded traffic, or leave a worker without a visible owner and
an effective termination path. This phase is part of completion; if it cannot
meet these criteria, report the implementation as partial rather than claiming
live debugging support.

## Phase 8 Qualify the whole feature and complete documentation

**Implementation and verification**

- Run the full repository gate and the relevant browser/native qualification
  against the final source state. Execute real Windows AppContainer tests for
  both hosts and real WebView2 tests for user workflows. Browser fixtures alone
  do not establish worker IPC, native lifecycle, or sandbox correctness.
- Extend the protocol fuzz corpus for interleaved logs, debug commands/events,
  malformed identities, unexpected ordering, oversized data, and old versions.
  Run the relevant bounded fuzz target through the documented container runner.
- Run a repeated lifecycle/soak test covering attach, pause, inspect, resume,
  stop, reload, and window close, recording worker/thread/handle counts and memory
  after quiescence. Measure ordinary execution, logging enabled, saturated logging,
  and debugger attached against the Phase 0 baseline and acceptance thresholds.
- Audit every route that could persist or expose authored text, source, or locals:
  traces, cases, support exports, native errors, MCP detail/search/paging, clipboard,
  and diagnostic copies. Add canary-secret tests at the actual output boundaries.
- Update `docs/scripting.md` through its help generator, API declarations,
  `docs/automation.md`, `docs/extensions.md`, relevant UX conventions, testing
  instructions, and durable architecture decisions. Document console semantics,
  limits, source fallback, unavailable locals, pause expiry, and live reservation
  consequences. Remove superseded limitations only after qualifying their fixes.
- Ensure all required helpers ship together and incompatible versions fail with
  actionable messages. Debugging off must preserve old saved workspaces and
  packages, and must not create a listening port or enable desktop DevTools.
- Provide a final phase-by-phase acceptance report with exact source identity,
  commands, results, evidence paths, and honest limitations. Retire this active
  plan once maintained guides describe the delivered behavior; Git history can
  retain the implementation plan.

**Final review iterations**

Pass 1 is a fresh end-to-end review from the author's task through the native
engine and back to rendered output. Pass 2 reviews security, compatibility,
resource use, cancellation, and disclosures across the combined feature rather
than individual phase diffs. Pass 3 repeats complete user journeys and compares
them with the original acceptance list. Apply the same two-clean-pass exit rule.

## Verification commands and evidence

Select affected packages during a phase; expand after integration or when a
failure or shared-contract change justifies it. Do not repeatedly run the full
workspace for unchanged local fixes. These are existing entry points; add focused
debugger tests to the appropriate suites rather than inventing unsupported flags.

```powershell
python scripts/dev-env.py --check
npm ci --ignore-scripts
npm run check
npm --prefix apps/desktop/ui run check
python scripts/dev-env.py --run cargo fmt --all -- --check
python scripts/dev-env.py --run cargo test --locked -p transmog-js-runtime -p transmog-script -p transmog-script-host -p transmog-script-supervisor -p transmog-extensions -p transmog-extension-host -p transmog-extension-supervisor -p transmog-extension-registry
python scripts/dev-env.py --run cargo test --locked -p transmog-core -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop -p transmog-mcp
python scripts/dev-env.py --run cargo test --locked -p transmog-mcp -p transmog-app -p transmog-session --all-features
python scripts/dev-env.py --run cargo clippy --locked -p transmog-app -p transmog-session -p transmog-app-webui -p transmog-desktop --all-targets -- -D warnings
npm --prefix apps/desktop/ui run test:workspaces -- --scripts-only
npm --prefix apps/desktop/ui run test:workspaces
python scripts/test.py
```

Also run strict Clippy for other changed crates. Use the current
[desktop build instructions](building.md#build-and-launch-the-windows-desktop)
to prepare the app and packaged helpers, then the owned-profile
`scripts/test-windows-desktop.ps1` harness with documented arguments. Extend its
native smoke coverage for debugging. Follow [fuzzing guidance](../fuzz/README.md)
for `script_protocol` and any newly separated target. Platform-only tests that
are skipped elsewhere require a Windows run; a green Linux result is insufficient.

The final handoff must demonstrate these user journeys with retained evidence:

| Journey | Required observable result |
| --- | --- |
| Nested TypeScript exception | Original source, meaningful exception, causes when available, all retained frames, explicit truncation. |
| Terminal and initializer exception | Detailed evidence remains available even without a successful callback result. |
| Log immediately before throw/timeout | Supervisor-accepted messages remain, correctly attributed, with incomplete/drop status where needed. |
| Log flood | Bounded memory/queues; control remains responsive; valid results are not rejected solely by new logger overflow. |
| Sandbox scenario debugging | Real breakpoint pauses, deterministic stepping, locals/closures, state across callbacks, unchanged applied revision. |
| Visualizer continuation debugging | Correct package/module, declared retained inputs, valid continuation behavior, cancellation and reload cleanup. |
| Live script debugging | Actual worker globals preserved, exact applied revision, explicit reservation impact, correct Continue and Stop outcomes. |
| Source changes during pause | Running snapshot remains stable; stale handles/locations are rejected; newer draft is preserved. |
| Sensitive data | No new automatic trace/support/MCP disclosure; explicit local inspection follows current permissions. |
| Client or window disappears | Bounded cleanup, no orphan worker or indefinite paused exchange, truthful recovery state. |
| Debugging disabled | Existing builds/packages work; execution limits and sandbox restrictions hold; measured overhead meets the recorded budget. |

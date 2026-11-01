# ADR 0011: Local script diagnostics and debugging

Status: Accepted

## Context

Traffic scripts and visualizers execute in separate V8 worker processes. Authors
need exceptions, logs, breakpoints, stepping and locals without weakening the
sandbox or confusing an executing revision with an edited draft. Traffic workers
are persistent and shared by multiple exchanges; visualizers retain parser and
continuation state. A paused worker cannot serve ordinary queued work safely.

## Decision

Share runtime-neutral diagnostic/debugger contracts in `script-diagnostics`.
Keep all Inspector and V8 value access on the worker's engine thread. Inspector
is created only for an explicitly owned session and communicates over inherited,
authenticated native pipes. No listening port or unrestricted protocol bridge
exists. Public controls are typed: breakpoints, exception policy, continue/step,
stack/scopes, own-property pages, handle release, idle detach and Stop. Evaluation,
mutation, conditions, custom formatters and live replacement are excluded.

Register actual compiled module IDs. Authored sourceURL labels never determine
identity, and internal invocation strings containing input JSON are never cached
or shown. Original positions use bounded maps tied to immutable executable or
package digests, with generated fallback and no filesystem/URL fetching. Source
excerpts come only from a shared bounded ephemeral cache.

Capture native exception descriptors and console arguments without calling
authored getters, proxies, serializers or conversions. Keep these streams separate
from callback results, so accepted logs can survive failure. Finite frame, text,
queue, history, handle and source budgets apply before exposing evidence. Preserve
explicit loss, incomplete, unavailable and shortened indicators. A single bounded
writer reserves control capacity while console traffic is lossy. The writer queue
and thread start only with the first console/debug event; silent execution keeps
the original synchronous result path. A validated result completes a completely
silent stream. Any observed console prefix still requires its ordered end marker.

Native supervisors own independent pause/session leases and inspection
deadlines. Only authenticated pause transitions update cumulative clocks;
execution budgets exclude that time while cancellation, process limits and
client timeouts remain effective. Termination is bound as soon as an attachment
is accepted, including before command transport initialization. Attach and
detach confirmations are ordering barriers before the next callback. Hiding the
debugger leaves ownership intact. Explicit stop-and-close, window destruction
and document reload release ownership independently of UI polling.

Live traffic debugging reserves the actual idle worker for one native-filtered
exchange. Track outstanding exchange factory leases, including older pinned
exchanges; an empty callback queue alone is insufficient. Reject competing work
before queue submission and its cancellation guard. The selected exchange retains
ownership through body and terminal callbacks. Keep admission reserved until the
idle detach confirmation. Normal completion or idle arming expiry can retain the
heap. Stop, owner loss or unsafe cancellation terminates the worker and uses the
existing explicit Retry recovery. Registry generation/revision/hash checks and
mutation locking prevent stale attachment. Replacement ends old ownership and
never restores activation.

Fresh visualizer fixtures preserve ordinary retained-input authorization and
document validation. Existing attachment uses the actual idle parser. The same
native controller covers continuations, expansion and explicitly selected live
updates; its pause clock also scopes retention timing. Keep the last valid tree
distinct from current failures and mark stopped instances as frozen.

## Consequences

Locals, source, console output and rich exceptions are sensitive ephemeral local
evidence. They are excluded from normal trace/case persistence, support errors and
MCP projections. Desktop commands are scoped to the calling window and are not
registered as MCP tools. An explicit copy action can copy local developer details.

Each process permits up to four active controllers and tracks up to 16 session
records across application instances. Each session retains 64 events/128 KiB
plus separately bounded source and breakpoint inventory; property replies are at
most 32 KiB. Sources share a 32 MiB cache, log captures a 12 MiB pool, and
catalog snapshots an 8 MiB pool. Existing inputs/packages and worker memory
retain their own independent budgets. These caps intentionally trade older
evidence and unavailable locals for bounded resource use. UI state explains
expiry, loss and explicit recovery.

Native AppContainer tests, production-template browser checks, WebView2 journeys,
protocol fuzzing and lifecycle/resource measurements qualify the combined feature.
An engine unit test alone cannot establish native ownership or UI correctness.

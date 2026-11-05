# ADR 0011: Local script diagnostics and debugging

Status: Accepted

Workspace ownership, conditional breakpoints and explicit local evaluation are
defined in [ADR 0012](0012-debugger-workspace-lifetime.md).

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
authenticated native pipes. Expose only typed native commands, with no listening
port or unrestricted Inspector bridge.

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

Native supervisors own attachment, inspection and execution deadlines.
Only authenticated pause transitions update cumulative clocks;
execution budgets exclude that time while cancellation, process limits and
client timeouts remain effective. Termination is bound as soon as an attachment
is accepted, including before command transport initialization. Attach and
detach confirmations are ordering barriers before the next callback. Window
destruction and document reload release ownership independently of UI polling.
Registry generation/revision/hash checks and mutation locking prevent stale
attachment. Replacement ends old ownership and never restores activation.

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

Session records, events, property replies, sources and logs have independent
bounds. These caps trade older evidence and unavailable locals for bounded
resource use; the UI exposes loss and recovery. Exact limits belong in the
[native supervision contracts](../../crates/script-diagnostics/src/supervision.rs)
and [application controller](../../crates/app/src/debugging.rs).

Qualification covers the sandbox, worker protocol and desktop UI. See
[Testing](../testing.md#script-diagnostics-and-debugging).

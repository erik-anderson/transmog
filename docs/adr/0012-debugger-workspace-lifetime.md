# ADR 0012: Debugger workspace lifetime

Status: Accepted. Extends ADR 0011 for ordinary worker attachments.

The app owns one debugger workspace per application/capture authority. Live proxy
scripts belong to the proxy application; visualizers belong to their capture
window. A detached surface receives the existing authority, never a new proxy or
visualizer instance. Native code commits a move only after the destination has
loaded its editor and acknowledged readiness. Commands from the previous surface
are then rejected. A failed handoff restores the previous placement.

Opening subscribes to current and future worker lifetimes. It does not reserve an
exchange or activate unloaded extensions. Closing continues and detaches healthy
workers, including pending attachments. Document/window loss ends ownership.
Persistent workspace pauses have no fixed human response deadline. Independent
command and execution limits still bound unresponsive code; cancellation and
shutdown remain available. Dependent callback, body and upstream-attempt budgets
exclude debugger blocking, including work queued behind the paused worker.

Breakpoints are durable logical file positions with statement anchors. Verified
catalog changes retain exact matches, move unique statements, and remove genuinely
unresolvable intent. Missing package bytes and unloaded workers do not prove a
breakpoint stale. Native sources and maps remain tied to immutable artifacts.

Immediate commands are explicit local execution with the worker's existing
capabilities. Idle evaluation uses its native context ID; paused evaluation uses
the selected native call-frame ID. Conditions use V8's side-effect check. Object
inspection continues to use inert own descriptors. The app exposes typed commands
over authenticated inherited pipes, without an Inspector endpoint or MCP command.

Temporary pause settings apply to current and future workers without editing
saved breakpoints. Pause-next and Continue to here are execution controls, not
durable breakpoint records. Logpoints use side-effect-checked expressions and
bounded output; their inspection time suspends dependent execution budgets.
Queued inspection shares a progress budget so a valid running evaluation cannot
make waiting reads expire. Stalled evaluations and automatic inspection retain
independent native deadlines.

Script tests and visualizers share this workspace. Fresh fixtures pause before
their first user statement, allowing breakpoint setup before execution.

# ADR 0012: Debugger workspace lifetime

Status: Accepted. Extends ADR 0011 for ordinary worker attachments.

The app owns one debugger workspace per application/capture authority. Live proxy
scripts belong to the proxy application; visualizers belong to their capture
window. A detached surface receives the existing authority, never a new proxy or
visualizer instance. Native code commits a move only after the destination has
loaded its editor and acknowledged readiness. Commands from the previous surface
are then rejected. Editing pauses while the snapshot moves and resumes after
restoration. A failed or canceled handoff restores the previous placement and
rejects late readiness acknowledgments.

Each surface transfers bounded value leases with its Console state. The destination
keeps live result roots and releases old scope and property handles. Native pause
epochs qualify idle values as well as paused values; replayed events cannot renew
an expired handle or duplicate output.
Paging and collapsing release hidden child values before another read, preserving
inspection capacity without retaining every visited page. Pending releases remain
part of a surface's handoff until native command submission succeeds.
The snapshot also retains Console history navigation, filters, scroll position,
and per-run clear positions; later output and loss reports remain visible.

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

The current callback comes from supervisor-owned attribution, including silent
callbacks. Its request is resolved through the same application's retained catalog.
Show request validates the current pause and routes only to that debugger's owning
window; it does not accept an arbitrary request or window identity from the UI.

Temporary pause settings apply to current and future workers without editing
saved breakpoints. Pause-next and Continue to here are execution controls, not
durable breakpoint records. Logpoints use side-effect-checked expressions and
bounded output; their inspection time suspends dependent execution budgets.
Queued inspection shares a progress budget so a valid running evaluation cannot
make waiting reads expire. Stalled evaluations and automatic inspection retain
independent native deadlines.

Script tests and visualizers share this workspace. With pauses enabled, fresh fixtures pause before
their first user statement, allowing breakpoint setup before execution.

# Extension contracts and packages

`transmog-extensions` provides the UI-independent revision-1 visualizer metadata,
native matching, input projection, output validation and `.tmext` container
primitives. `transmog-extension-host` and `transmog-extension-supervisor` provide
the isolated JavaScript worker and its bounded lifecycle. `transmog-extension-registry`
provides shared installation and active-instance ownership. `transmog-app`
connects explicit activation to retained headers and body ranges. Desktop Extensions
settings provides installation and declaration management. The desktop inspector
offers explicit selection, scoped retained context and host-owned structured rendering. Desktop live controls,
transactional hot reload and the bundled MAPI extension remain in the
[implementation roadmap](roadmap.md#request-and-response-visualizer-extensions).

An extension is a versioned package containing independently identified visualizer
contributions. Each contribution declares its entry module, host surfaces, native
matching rules and exact request/response input access. Both sides require an
explicit header-name list and a body mode: `none`, `optional` or `required`.
Omission is invalid. Header wildcards, case-colliding names and matching/context
predicates which depend on undeclared headers are rejected. A declaration never
overrides capture redaction or grants network access.

`project_headers` copies only declared occurrences, preserving original order,
names and indices; redacted values remain unavailable. `check_body_access` denies
body access for a `none` declaration. These are enforcement primitives for the
host broker, not an auditing system. Declaration details are inert metadata for
the Extensions settings experience.

Native matching handles exact media types separately from parameters, rejects
conflicting Content-Type evidence and supports bounded method, endpoint, status
and header predicates. `ActiveSubscriptions` indexes explicitly registered live
instances by capture/conversation, direction and selective native predicates.
Installed packages are not per-event subscribers; an empty active set exits
immediately. Removing one instance does not remove another contribution from
the same extension.

Output is a `VisualizationDocument`: typed tree/table/text/hex data, diagnostics,
format metadata, exact source spans and bounded derived buffers with provenance.
The host validates unique IDs, depth/count/encoded-byte budgets, exact decimal
offsets and accessible source bounds. A derived source may reference granted
sources and earlier derived buffers; it cannot invent a readable source or form
a provenance cycle. Extensions supply no executable UI or network locations.

`.tmext` uses a restricted Stored ZIP64 profile. `manifest.json` is the first
member and bounded to 256 KiB; the manifest inventories every other member with
exact length and SHA-256. The initial ceilings are 2,048 payload files, 16 MiB
per file and 64 MiB total payload. Names are portable relative ASCII paths;
aliases, traversal, Windows device names, case collisions, links, encryption,
compression and contradictory ZIP metadata are rejected. The reader does not
extract archive paths.

`Package::open` validates structure and metadata. Reading a payload checks its
CRC and manifest digest; creating an execution `Snapshot` verifies all payloads
and binds them to an immutable manifest digest. Development source collectors
can create snapshots from already-collected bytes, filling omitted source
digests before ordinary validation. The package layer does not read developer
directories itself. `write_package` always emits ZIP64, even for small packages.

The shared `transmog-package-zip` crate owns structural ZIP validation and writing.
Portable investigation packages retain their narrower manifest/object namespace
through their own adapter. The two formats do not accept each other's manifests.

## Installation and instance ownership

The shared registry persists validated declaration summaries, provenance, enabled
state and developer hot-reload preference. Listing or enabling reads no code and
starts no worker. Installation verifies a local package, stages its immutable
revision and atomically publishes the catalog. Failed publication removes a new
candidate; successful replacement unloads active instances and removes the old
managed revision. A different provenance cannot silently take over an existing ID.
Corrupt catalog metadata leaves capture available and reports a recovery warning.

Registering an unpacked developer directory reads only its manifest. Activation
uses pinned local directory handles, rejects reparse points and links, bounds
every file read and collects an immutable snapshot. Source lengths and digests
are calculated for that snapshot, so authors do not maintain them after each
edit. The exact executing declaration is published only after worker initialization
succeeds. Unregistering a developer extension never deletes its sources.

Each instance belongs to one application/window owner. Foreign instance IDs
cannot retrieve its worker or retained-data resources. Disabling or uninstalling
is global: it stops all package instances across registry clones. Instance resource
owners receive an immediate, idempotent stop callback before worker termination
to cancel subscriptions and clear retained buffers, even when a pending operation
still holds a temporary reference. Enabling does not resurrect those instances.
Hot-reload preference is persisted here; file watching and transactional result
replacement belong to the pending desktop broker integration.

## Extensions settings

Open **Settings → Extensions** to install a local `.tmext`, load an unpacked
folder, enable/disable an entry or uninstall it. Each entry exposes its visualizer
surfaces, native matching declarations, exact request/response header names, body
access and live/context capabilities under **Visualizers and declared inputs**.
Names and all extension-provided metadata remain inert text. This is declaration
management, without access histories or an auditing subsystem.

Changes use the shared registry across main and capture-viewer windows. Disabling
stops ready and starting workers; enabling does not resume them. Bundled entries
can be disabled, while their files remain owned by the app. Removing an unpacked
entry leaves its source folder intact. Its hot-reload preference is saved here;
the active-view watcher remains pending with visualization controls below.

Inventory reads and enable changes never evaluate source modules. Failed writes
restore the saved checkbox state. A cleanup failure after a catalog change reads
back the authoritative inventory, preserving the actual installation result.
Leaving and returning to a pending inventory refresh requests fresh metadata and
rejects the earlier response. The management view releases its event listener
when hidden.

Production-template browser checks cover keyboard changes, inert markup,
failed writes, removal cleanup, stale refreshes, compact layouts and forced
colors. Native WebView checks exercise the real command boundary and persisted
enable/hot-reload settings, verify unpacked sources survive removal, and retain
screenshots and memory evidence in the ordinary desktop validation artifacts.

## Retained-evidence broker

The application facade offers matching enabled contributions for one selected
header/body surface and exact message boundary. Discovery uses cached declarations
and borrowed retained headers without loading modules or reading body bytes.
Discovery does not copy header values per contribution or package file inventories;
with no enabled contributions it skips the retained-evidence lookup. Actual input
projection uses complete retained field values, independently of display previews;
it preserves duplicates, ordinals and redaction. Native admission is checked again
against the executing declaration before body capabilities are created.

Inputs pair the request and response on the same transport leg: client request
with client response, or upstream request with upstream response. The broker does
not silently substitute another observation boundary. Missing required bodies
produce an unavailable prerequisite; optional bodies preserve explicit retention
states. Header-only declarations acquire no body source or read lease.

Granted body capabilities identify an exact retained prefix/revision. Reads are
lazy and bounded to 64 KiB per page, use the existing retention range reader and
hold an eviction lease only during the read. An idle viewer cannot pin an
arbitrarily large retained body. The supervisor enforces the aggregate byte budget
across continuations; the application bounds the entire operation by the declared
duration as well as the worker's individual invocation deadline. Source loss is
reported rather than replaced with different evidence.

Each owning pane supplies a stable slot and monotonically increasing decimal
generation. Close events retain only small generation tombstones, so a delayed
start cannot resurrect a closed generation and a delayed close cannot stop a
newer one. Cancellation reaches pending initialization as well as ready workers.
Global disable/uninstall also cancels pending initialization. Closing an owning
application prevents any subsequent activation. Source bytes for host highlighting
require the still-active owner, slot, generation, exact worker, host-issued page
epoch and original source capability. Navigation invalidates the prior page,
including starts still queued in IPC. Native minimize/hide handling cancels
instances and rejects new starts while hidden; losing focus alone keeps a visible
visualizer active. Original source names and exact extents are supplied by the
host without reading their bytes.

Related reads use `needContext`/`resumeContext`. The host returns at most 32 exchanges
per page and the declared maximum (up to 256) over the worker's lifetime. Earlier
evidence is permitted by a `context` declaration; later retained evidence also
requires `lookAhead: true`. Cursors are single-use and bound to the active instance
and traversal direction. Returned exchanges include exact decimal admission order,
the same header/body grant as the selected input, and explicit complete, more,
unavailable or limited status. Missing historical bodies remain absent even when
the selected-message declaration requires a body.

The host enforces the same origin using scheme, canonical hostname and effective
port, both for the original client target and the selected transport boundary.
It also requires the same capture/import source, caller, exact endpoint and query,
and matching values for every declared correlation header. Unknown process identity
restricts context to the same client connection. Missing, redacted, duplicate or
oversized correlation values make context unavailable. Queries participate in
native correlation without being exposed to scripts. These constraints cannot be
widened by a manifest, selector or cursor. Saved evidence without source provenance
cannot fall back to the live capture. An unknown original target cannot stand in
for a verified origin.

Native scope and input predicates run against borrowed retained heads before
projecting values or creating body capabilities. Context reads page through a
bounded metadata scan; inactive extensions perform no scans. Related source reads
recheck the anchor, capture and origin before and after reading. Removing the
anchor or unloading the visualizer revokes those capabilities and clears cursors.
The same byte, source-count, deadline and cancellation limits apply throughout.

These APIs parse selected retained snapshots, related context and deferred expansions.
They do not yet decode HTTP
Content-Encoding for an extension. Existing built-in inspection remains available.

## Native live-feed broker

The application API accepts `live: true` only for a fresh explicit activation of
a contribution declaring `liveUpdates`, against live retained traffic. Ordinary
activation remains snapshot-only. Installed and inactive contributions have no
subscription. Active contributions share one native catalog subscription per
owning application; removing its last feed drops that subscription. Disable,
uninstall, navigation and hidden-pane cancellation unregister admission, clear
queued metadata and stop the worker together.

Native change hints distinguish headers, body growth, body completion and HTTP
termination. Body growth requires `bodyProgress` and body access on that exact
side; header-only interests skip body hints before looking up retained heads.
The native index uses capture, origin, boundary, direction and selective matching
before checking the full conversation guard. Without correlation declarations,
live scope is restricted to the same connection and endpoint, including query.
Declared correlation can relate connections from the same known caller. Neither
case can widen capture or origin scope.

Only admitted exchange IDs and revisions enter a 32-entry queue. Repeated changes
to one message coalesce into its latest full snapshot. Overflow and subscription
lag increment a transient lost-hint counter, delivered to `onTraffic` as
`info.missedHints`; the matching set of lost hints can be uncertain. Host results
remain partial after such loss until the instance is reopened. Inputs include a
stable opaque `exchangeId`, exact decimal `requestNumber`, delivery `sequence` and evidence
`revision`, and a native lifecycle `phase`. Byte capabilities still identify their
exact frozen extent; capture and body-retention dispatchers can advance separately.
`sequence` orders delivered snapshots; `requestNumber` permits reconstruction in
request admission order even when coalesced responses arrive out of order.

`poll_visualizer_live` projects a bounded batch only when admitted work exists.
Empty polls read no bodies, start no blocking task and invoke no script. Final
body updates wait for admitted retention metadata within a bounded deadline,
without flushing unrelated body work. Current scope and input rules are checked
again before creating capabilities. Script and byte continuations use the same
sandbox, deadlines and aggregate budgets. `stop_visualizer_live` removes admission
while preserving the active worker for the visible snapshot and expansions;
an operation already started may finish. The desktop controls and their polling
lifecycle remain the next integration step.

## Desktop visualizers

Matching contributions appear in the existing Body viewer selector and in a
Headers viewer selector when one applies. Raw headers and Auto remain the initial
choices. Selection is explicit and resets when changing exchange or observation
boundary. Request, response, header and body contributions own independent slots;
one package may run several at once, including both surfaces in Split mode.

The host renders a virtualized keyboard-navigable structure tree, complete selected
values, format metadata, paginated tables, text and Hex. Table previews shorten
cells and bound columns; the structure preserves access to every field. Nested
presentation hints choose the selected structure's initial view. Captured names,
values, diagnostics and transform descriptions remain inert text.

Source-range buttons select exact decimal offsets in the existing paged Hex
viewer. Original sources use the active worker's capability; derived buffers stay
document-local and show their transform and parent regions. Enclosing provenance
is labeled separately from exact mapping. Displaying a tree does not read source
bytes just to populate its source selector.

Hidden/inactive surfaces clear documents and byte caches and cancel pending
starts as well as ready workers. Returning to a still-selected visible surface
shows Open visualizer; that explicit action opens a fresh instance. Selection
changes and late replies cannot restore an old document. Native suspension uses
the same explicit reopening after the window is restored.
Losing focus while visible does not suspend it. Extension-management notifications
check exact activity so an unrelated installation change does not restart a view.

Browser fixtures cover large trees, concurrent surfaces, deferred expansion,
wide offsets, derived provenance, late activation and narrow/forced-color layouts.
The isolated native desktop fixture exercises the same renderer through actual
AppContainer initialization, retained byte continuations, expansion, denied source
reads and unloading. These are synthetic host acceptance tests, not MAPI parity
or validation against real Office captures; that end-to-end pass is deferred.

## Isolated worker

The supervisor starts an exact helper executable only on explicit activation.
Production execution always uses the native sandbox; there is no development
bypass. The helper verifies its AppContainer and Job Object boundary before
reading code or traffic. Before any extension module initializes, V8 string-code
generation is disabled and network, filesystem, subprocess, environment, timer,
WebAssembly and Deno/Node globals are unavailable.

The manifest requires `runtime: "javascript-es2022"`, independently of the package
and visualizer API versions. Executable entries and dependencies are `.js` or
`.mjs` ES modules. ES2022 is the supported authoring baseline; newer syntax is not
a compatibility promise. Transmog executes JavaScript without transpiling it.
Authors may use TypeScript, JavaScript with JSDoc, or another language that emits
compatible JavaScript. Pin that compiler in the extension's own build, target
ES2022 and emit ES modules before packaging. Compiler upgrades do not change
Transmog's executable contract. The SDK declarations are optional authoring aids,
not runtime imports. Optional TypeScript sources and source maps are inert payloads;
source-map diagnostics are not yet implemented.

Source transfer is authenticated and chunked, then checked against the immutable
manifest inventory. The worker validates and loads only the selected contribution's
closed graph of static package-relative JavaScript modules. External URLs, filesystem
imports, package imports, traversal and dynamic imports are rejected. Helpers can
share local parser modules without initializing other contribution entry points.

`visualize`, `resume`, `resumeContext`, `expand` and declared `onTraffic` calls return synchronously
through an independently versioned private protocol. Byte access uses bounded
`needInput` continuations. The supervisor binds replies to exact request IDs and
continuation tokens, projects headers before IPC, withholds undeclared body
capabilities, checks source extents, and validates output documents before returning
them to the host. Header source IDs identify the exact evidence revision.

Each operation has a deadline; initialization has a separate 15-second limit.
Input budgets apply across header delivery and continuation pages. Each instance
has a bounded queue, and admission caps are 16 workers globally and 8 per package.
Stopping removes queued work, kills/reaps the isolated process and joins the I/O
supervisor, releasing the worker slot even if a stopped UI handle remains alive.
These runtime primitives do not themselves subscribe to captured traffic.

Windows integration tests exercise real AppContainer launches, undeclared reads,
external imports, infinite-loop termination, independent instances, continuation
forgery and repeated unload. Native probes use owned TCP/UDP receivers because
Windows may silently discard denied UDP sends after accepting them locally.
They verify no loopback TCP/UDP, DNS-format packet or LAN UDP reaches those peers,
and that outbound TCP returns a network-policy denial. The script tests also
verify missing runtime network APIs before top-level extension code executes.

The authoring declaration source is
[`transmog-extensions.d.ts`](../crates/extensions/sdk/transmog-extensions.d.ts).
Contract revisions are explicit; private worker IPC is independently
versioned. Runtime budgets and SDK details remain subject to qualification with
the MAPI parity corpus before any compatibility promise.

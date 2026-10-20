# Extension contracts and packages

`transmog-extensions` provides the UI-independent revision-1 visualizer metadata,
native matching, input projection, output validation and `.tmext` container
primitives. `transmog-extension-host` and `transmog-extension-supervisor` provide
the isolated JavaScript worker and its bounded lifecycle. `transmog-extension-registry`
provides shared installation and active-instance ownership. `transmog-app`
connects explicit activation to retained headers and body ranges. Desktop Extensions
settings provides installation and declaration management. The desktop inspector
offers explicit selection, scoped retained context, opt-in live controls and host-owned structured rendering.
The bundled MAPI extension remains in the
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

Each matching rule is a conjunction; alternative rules combine with OR. A rule
for a request viewer may require both request and response headers. Missing
paired headers defer availability; a later response can make the viewer appear.
Presence tests use the retained name, including redacted fields. Exact and
not-equal value tests never inspect redacted values, and an absent field cannot
satisfy not-equal. Declare every matching name on its exact input side.

## Static traffic columns

Optional `trafficColumns` declarations classify retained requests using literal
labels selected by the same native header matcher. Each has a package-local
`id`, heading `name`, exact `inputs` and `values` with alternative `matches`.
Request and response conditions may be combined. Both body declarations must
be `none`; HTTP decoding, method/path/status predicates, executable callbacks
and interpolation of captured header values are rejected. A package may contain
only column metadata, with empty `visualizers` and `files` inventories.

For example, this abbreviated column requires a specific request SOAPAction
and a SOAP response before adding its fixed label. Authors should use the
exact protocol action values applicable to their classifier:

```json
{
  "id": "protocol",
  "name": "Protocol",
  "inputs": {
    "request": {"headers": ["soapaction"], "body": "none"},
    "response": {"headers": ["content-type"], "body": "none"}
  },
  "values": [{
    "value": "MS-FSSHTTP",
    "matches": [{
      "direction": "response",
      "contentTypes": ["application/soap+xml"],
      "headers": [{"direction": "request", "name": "soapaction",
        "test": {"op": "equals", "value": "CellStorageService"}}]
    }]
  }]
}
```

**Traffic → View → List columns → Extension columns** offers enabled declarations.
All start hidden; choose up to eight. Labels appear after ordinary columns in
declaration order, once per matching literal. These initial classification
columns display labels without adding sort/filter actions. Their package-qualified
identities persist as layout preferences, independently of traffic values.
Disabled or removed entries disappear from the table; unavailable selected
identities can still be unchecked in View. Extensions settings shows their
header names, literal values and rules.

Classification receives at most 512 existing retained identities per native
call; the desktop requests at most 100 rows near the viewport and caches at
most 1,024 results. Only selected declarations and matching header names are
projected, with borrowed values and finite metadata, comparison and output
budgets. Exhaustion shows **Limited**, discarding partial labels. Missing or
redacted values cannot produce a value match. Changed row evidence, declarations,
selection or visibility invalidate pending results; hidden workspaces release
the cache and metadata listener.

Column labels load on idle time, separately from traffic queries and row loading.
Only one bounded label request may be in flight per workspace; scrolling coalesces
to its newest viewport. Installed headings are indexed once, and optional column
controls are created only while their settings are open. With no columns selected,
row refresh and rendering skip classifier calls and use no per-row classifier
allocations. Label replies update only extension cells, retaining ordinary cells,
focus and scroll. Unchanged labels keep their prior cells through idle refreshes;
changing declarations or selection and hiding the workspace clear those caches.

This path uses the already available retained header index for live or imported
traffic. It does not import another capture, walk all TMCap entries, acquire a
body capability, open a body blob, read extension modules or start a worker.
There is no per-installed-extension capture callback or full-capture label cache.

The production-template performance probe is
`node scripts/check-workspaces.mjs --traffic-perf-only --perf-output <path>`
from `apps/desktop/ui`. It exercises 100,000 entries with no columns, 2,048
installed declarations, and one/eight selected columns. It checks bounded DOM and
label caches, no classification calls when off, no full query while scrolling,
and continued row loading while label replies are delayed. Native Viewer checks
also record viewport rendering and 20,000-entry refresh/query IPC timings. Compare
warm samples on the same machine; absolute timings vary with system load.

## Documents and packages

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

App-owned bundles provide separately generated manifest metadata and a lazy
ordinary `.tmext` resource. Startup/discovery and enable changes do not open that
archive or copy it into the user installation store. Explicit activation verifies
the immutable archive and its payloads. App upgrades refresh trusted declarations
while preserving the saved enabled choice. Corrupt catalog storage leaves bundles
disabled; an ID conflict preserves user-owned files and permits uninstall recovery.

Registering an unpacked developer directory reads only its manifest. Activation
uses pinned local directory handles, rejects reparse points and links, bounds
every file read and collects an immutable snapshot. Source lengths and digests
are calculated for that snapshot, so authors do not maintain them after each
edit. The exact executing declaration is published only after the worker returns
a host-validated visualization document. Initialization alone, parser failures and
invalid output preserve the prior catalog. Publication rechecks the active owner,
worker and catalog revision so a stopped or superseded candidate cannot publish.
Unregistering a developer extension never deletes its sources.

Each instance belongs to one application/window owner. Foreign instance IDs
cannot retrieve its worker or retained-data resources. Disabling or uninstalling
is global: it stops all package instances across registry clones. Instance resource
owners receive an immediate, idempotent stop callback before worker termination
to cancel subscriptions and clear retained buffers, even when a pending operation
still holds a temporary reference. Enabling does not resurrect those instances.
The native reload command starts a fresh immutable revision alongside the current
development instance. It rechecks exact owner, pane generation, page and visibility
before publishing its valid document and replacing the active ticket. Failed
source, sandbox, parser, capacity or publication attempts retain the original
worker and source handles. Successful replacement revokes those old handles and
stops its feed/worker. Close, hide, navigation, a newer activation or another
candidate cancel pending work. Catalog serialization and syncing happen before
the pane's final swap lock. Reload requires spare worker capacity; it never evicts
the current worker to make space.

Live parsing starts from fresh retained evidence and a new bounded queue; no
JavaScript heap state transfers. A changed contribution declaration stops live
delivery in the candidate and requires another explicit opt-in.

Development reload actions live in **Settings → Extensions**. Each enabled
unpacked entry offers **Reload active visualizers**, which requests replacement
of all its currently active instances in every window, and **Hot reload active
visualizers**, which applies source changes automatically. Neither action opens
an inactive visualizer. With none active, the settings action reports that fact;
the next explicit visualization collects fresh source bytes. Installed and bundled
entries have no development reload controls.

Visible development viewers check native metadata on activation and relevant
management events. Only hot reload schedules ongoing checks, one every 500 ms,
serialized with itself and deferred while parsing is busy. Packaged and inactive
viewers have no source-status polling. Replacement preserves the old result while
working, retains matching field selection, expansion and view mode on success,
and reports failures beside the retained result. Retry source replacement in
Extensions settings; the visualizer adds no development reload button. Hide,
navigation or deselection cancels queued and running replacements, clears the
result and stops observation. Metadata publication during a swap cannot invalidate
the new view through an old activity-check reply.

### Active development source notifications

The main-window `reload_extension` settings command requests a fresh revision for
all active instances of one enabled unpacked extension, including other window
owners. It only sets a transient package signal and announces it; it reads no
source files and starts no inactive contribution. Each instance acknowledges its
own request. A request arriving during candidate initialization remains pending
when that candidate did not snapshot it. Candidate replacements share one
cancelable admission gate across windows; ordinary live parsing remains independent.
Replacement still requires spare worker capacity and retains each last good result
on failure. Manual requests work with automatic hot reload turned off.

Native `visualizer_reload_status` reports metadata changes only for the exact
owned active development instance with hot reload enabled. Installed packages
and inactive developer entries have no watcher. Observation starts before the
immutable snapshot, and filters to `manifest.json`, declared JavaScript modules
and their parent directory renames. Optional TypeScript sources and maps remain
inert. Status polling opens no scripts and evaluates no modules, including while
an editor holds a changed script exclusively.

Windows watches share one sleeping I/O completion thread, created on demand and
joined after its last watch closes. All asynchronous issuance runs on that thread,
so a retiring blocking-pool caller cannot cancel observation. A fixed aligned
16 KiB buffer and its `OVERLAPPED` remain alive until cancellation completes and
the exact completion packet is consumed. At most 32 watches, including initializing
candidates, exist in one observer. The observer reads directory metadata only.
The isolated `transmog-directory-watch` crate owns this native FFI boundary.

The design was reviewed against [Chromium's Windows watcher](https://chromium.googlesource.com/chromium/src/+/main/base/files/file_path_watcher_win.cc)
(source blob `a1dbf2a6d017be4dedc6fdbfe2dc6df4f1167231`). Like that implementation,
it uses completion ownership, prompt rearming and explicit overflow handling.
Unlike a general-purpose path watcher, developer roots stay pinned against
replacement, and observation never widens to a parent outside the registered
root. Child file replacement and nested-directory renames remain observable;
moving or replacing the root requires turning observation off first. Native
failures retain the last good visualization and offer manual reload.

Each watch retains one bounded batch of up to 1,024 unique names and 32 KiB of
name text. The first completion rearms promptly; a second unconsumed completion
merges names and pauses only that watch until its consumer polls. Coalescing
duplicate names does not itself report loss or reload unlisted files. Filesystem
storms cannot accumulate callbacks or repeatedly
read source contents. Zero-byte overflow, malformed names and lost batches become
a conservative reload hint for the declared inventory. Relevant changes settle
for 250 ms, with at least one second between attempts. A reload acknowledges its
hint when it starts, so invalid edits do not loop. Enabling observation on an
already active result requests one rebuild for edits that may have been missed.

Turning hot reload off closes all package watches without stopping the visible
parser. Hide, navigation, disable, uninstall and unloading cancel observation as
well as the worker and feed. There is no filesystem polling fallback on platforms
without this notification boundary, and no watcher work on the traffic-list path.

## Extensions settings

Open **Settings → Extensions** to install a local `.tmext`, load an unpacked
folder, enable/disable an entry or uninstall it. Each entry exposes its visualizer
surfaces, native matching declarations, exact request/response header names, body
access and live/context capabilities under **Visualizers and declared inputs**.
Names and all extension-provided metadata remain inert text. This is declaration
management, without access histories or an auditing subsystem.

For an unpacked entry, **Package extension…** opens a native save dialog for a
`.tmext` archive. Packaging works even when the extension is disabled. It reads
only the manifest and its declared files, computes current lengths and hashes,
and saves a Stored ZIP64 archive without evaluating scripts. Unlisted files,
including a previous output in the source folder, are excluded. Validation or
write failure preserves the previous destination; a declared source file cannot
be chosen as the output. Packaging does not install the archive, publish changed
development declarations or reload active visualizers.

Changes use the shared registry across main and capture-viewer windows. Disabling
stops ready and starting workers; enabling does not resume them. Bundled entries
can be disabled, while their files remain owned by the app. Removing an unpacked
entry leaves its source folder intact. Its hot-reload preference controls native
observation only while development visualizers are active, across every window.

Inventory reads and enable changes never evaluate source modules. Failed writes
restore the saved checkbox state. A cleanup failure after a catalog change reads
back the authoritative inventory, preserving the actual installation result.
Leaving and returning to a pending inventory refresh requests fresh metadata and
rejects the earlier response. The management view releases its event listener
when hidden.
Catalog corruption blocks management until recovery. Other declaration notices
leave recovery controls available; a warning alone does not disable uninstalling
an installation that conflicts with an app-owned ID.

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
HTTP content decoding is separately declared on each input side with
`decodeHttpContent: true`; that side must also grant a body and the exact
`content-encoding` header. A script requests `needDecoded` for an original body
capability and receives `resumeDecoded`. Discovery and ordinary body grants do
not decode anything. Incomplete, unsupported, malformed, unavailable-metadata
and over-budget results remain explicit, without substituting encoded bytes.

Decoding uses the host's bounded gzip, deflate, Brotli and Zstandard codecs.
Encoded work and subsequent decoded byte pages share the operation's input
budget. Retained decoded buffers also share the instance's input-memory ceiling;
source count, expansion, decoder window, layer, deadline and cancellation limits
still apply. The retained content-coding metadata must agree with the declared,
visible headers. An identity representation reuses its original capability.

A decoded representation has its own host-issued source capability and keeps
the original bytes accessible. Its parent is the full encoded extent, labeled
as enclosing provenance rather than an exact byte mapping. Repeated requests
reuse the immutable representation. Decoded reads recheck the original source,
including related-evidence capture and origin constraints; retention loss and
unloading revoke access. The host source selector shows the coding and parent
region without reading bytes merely to populate the list.

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
an operation already started may finish.

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

Contributions that declare live updates show a Live updates checkbox beside the
result title. It starts off on every activation. Imported traffic or a missing
retained conversation leaves it unavailable. Turning it on opens a fresh worker
from the selected exchange; turning it off stops admission and polling while
keeping the last visible result and its expansion capabilities. Polls are
serialized with expansion and stopping. Updates retain selected fields, expanded
rows, tree scroll, view mode and unchanged byte selections. A replaced deferred
branch without materialized children collapses for explicit expansion, keeping
live delivery from automatically parsing a large set of previously opened
branches. If delivery falls
behind, a warning offers an explicit fresh opening to rebuild context. Parser
failure stops the feed and freezes the last result, revoking byte access until
an explicit retry succeeds. Hiding the pane clears that result and reaps its
worker; opening it again returns to snapshot mode.

Browser fixtures cover large trees, concurrent surfaces, deferred expansion,
wide offsets, derived provenance, late activation and narrow/forced-color layouts.
The isolated native desktop fixture exercises the same renderer through actual
AppContainer initialization, retained byte continuations, expansion, denied source
reads and unloading. These are synthetic host acceptance tests, not MAPI parity
or validation against real Office captures; that end-to-end pass is deferred.
The native live fixture also sends controlled HTTP exchanges through the proxy
to the production sandbox, checking response-body continuation, origin and
content-type filtering, explicit opt-in, stopping and inactive unloading.

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

`visualize`, `resume`, `resumeContext`, `resumeDecoded`, `expand` and declared `onTraffic` calls return synchronously
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

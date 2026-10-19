# Roadmap

This roadmap lists remaining work. Current behavior is documented in the
[topic guides](README.md).

Priorities are intentionally not tied to dates. A feature moves out of this
file when its durable contract is documented and its implementation and tests
are checked in.

## Desktop and traffic workspace

- Add a searchable capture library and project-level organization across saved
  captures, persisted automation, scripts, response assets and preferences.
- Add desktop file actions for the implemented bounded rule/asset interchange,
  conflict review and disabled-import APIs in [portable investigations](portable-investigations.md).
- Add request-body auto-response conditions only with explicit retention,
  privacy, resource, and replay semantics.
- Extend headless tooling for persisted automation and script-workspace
  management. Guided support recording, circular capture, native streaming
  output and password-protected export already exist in the CLI.

## Request and response visualizer extensions

Implementation in progress. The [contract/package foundation](extensions.md) is
implemented, along with the isolated TypeScript worker/supervisor. Desktop
retention/context integration and MAPI parity remain below.
The user wants to select a retained request or response, choose a matching
visualizer, and explore its headers/body alongside the source evidence. Ordinary
capture, selection, and built-in inspection must not load extension code.

### Requirements agreed for this plan

- Author extensions in TypeScript and execute them in an OS-sandboxed process.
- Distribute ZIP64-based `.tmext` packages, with inexpensive metadata discovery.
- Support explicitly registered unpacked development directories and optional
  hot reload under the same execution restrictions as packaged extensions.
- Extensions return structured data; the host owns every UI component.
- Request and response body visualizers are first-class registrations. A single
  extension may register several visualizers/content types/directions, with
  multiple registrations and active instances usable concurrently.
- Request and response headers are first-class inputs and matching criteria.
  Support header-only, body-only, and combined visualizations, including when
  there is no body or the counterpart message is not yet available.
- Every contribution must explicitly list the exact request and response header
  names it may access and declare body access for both sides. The host enforces
  this data-access contract for performance and predictable read-only access;
  no implicit grants. Show declarations in Extensions settings without building
  an access-auditing system.
- Bundle a MAPI extension with the application. Broad Fiddler MAPI inspector
  parity is a shipping gate, even though implementation proceeds in stages.
- After explicit visualization, allow bounded access to related retained
  exchanges in the same conversation and capture. Discovery grants no reads.
- Extensions may declare support for live request/response updates. The host
  offers an off-by-default Live updates toggle for those visualizers; enabling
  it explicitly authorizes a bounded feed within the active scope.
- Native metadata filters admit only relevant traffic before body reads or IPC.
  Installed inactive extensions have no feed, worker, or per-event callback.
  Deactivating a visualizer stops delivery and unloads its worker/parser state.
- Extension scripts cannot access the network. This includes development mode;
  there is no extension network permission or host network proxy API.

### Architecture and existing seams

Reuse the approach in [ADR 0008](adr/0008-traffic-workspace-and-script-isolation.md):
`deno_ast` for TypeScript and `deno_core`/V8 in a separate process, authenticated
bounded IPC, supervisor deadlines, and the Windows AppContainer/Job Object
boundary in `transmog-process-sandbox`.

Introduce a UI-independent extension contract/package layer and a visualization
supervisor above capture storage. A dedicated `transmog-extension-host` reuses
the sandbox and narrowly factored runtime primitives. Give visualization its
own protocol and read-only capabilities; `ScriptAction`, interception hooks,
and live traffic-script processes are not its execution API. A parser failure
ends the visualization job without affecting capture or live exchanges.

The current script compiler accepts one module, rejects runtime imports, and
has traffic-specific source, invocation, and output limits. Reuse its compiler
and isolation machinery, not those assumptions. For extensions, support a closed
graph of package-relative static TypeScript imports, declared in the manifest.
Reject external URLs, bare runtime packages, dynamic imports, native modules,
and code generation. Load modules from verified bytes supplied by the host;
the worker does not open the package or developer directory itself. Compile
after activation inside the worker, with source maps and bounded diagnostics.
Authoring and CI run semantic TypeScript checks; transpilation alone is not a
type check. Dependencies must be included in the package, with licenses.

`crates/app/src/inspector.rs`, body retention/range readers, and the existing
`message-inspector` and `hex-viewer` are the integration points. Keep catalog
discovery and result validation in Rust. Render only validated presentation
models through the host's production WebUI templates. Composer and imported
capture windows should use the same service and viewer contract.

### Package, metadata, and compatibility

Proposed initial layout:

```text
org.transmog.mapi.tmext
  manifest.json
  src/main.ts
  src/...
  data/...
  LICENSE
  THIRD-PARTY-NOTICES
```

The manifest declares package format version, extension ID/version, visualizer
API major/minor requirements, display name, individual visualizer IDs, entry
modules, display surfaces, directions, declarative media-type/method/endpoint/header
conditions, header/body input requirements, context needs, optional live-update
support and event interests, requested budgets,
and payload file lengths/SHA-256 digests. Package, visualizer API, and private
worker IPC versions evolve independently. Reject unsupported required features
before offering an unusable visualizer. Live support is declared per visualizer
and is discoverable without loading its code.

The extension package is the installation/versioning unit; each visualizer is
an independently discoverable contribution with its own stable ID, entry module,
display matches, declared context/live input selectors, and optional live-update
support. Use `(extensionId, visualizerId)` for registration identity, and a separate
opaque instance ID for each active pane/session. Never key workers, subscriptions,
cancellation or UI selection by extension ID alone.

One package can expose separate request and response visualizers, multiple
protocol viewers, or a single contribution accepting both directions and several
content types. Register all contributions atomically from metadata; none is an
exclusive mode of the extension. For example, the display registrations may be:

```json
{
  "id": "org.transmog.mapi",
  "visualizers": [
    {
      "id": "mapi-request",
      "entry": "src/request.ts",
      "inputs": {
        "request": {
          "headers": ["content-type", "x-requesttype", "x-clientinfo"],
          "body": "required"
        },
        "response": {
          "headers": ["content-type", "x-responsecode"],
          "body": "optional"
        }
      },
      "matches": [
        { "direction": "request", "contentTypes": ["application/mapi-http"] }
      ]
    },
    {
      "id": "mapi-response",
      "entry": "src/response.ts",
      "inputs": {
        "request": {
          "headers": ["content-type", "x-requesttype", "x-clientinfo"],
          "body": "optional"
        },
        "response": {
          "headers": ["content-type", "x-responsecode"],
          "body": "required"
        }
      },
      "matches": [
        { "direction": "response", "contentTypes": ["application/mapi-http"] }
      ]
    }
  ]
}
```

This is a partial manifest example, not the complete package schema. The entry
modules can share package-local parser modules and data. A request contribution
may declare response events as protocol context, and vice versa; distinguish
which message a viewer displays from which narrowly matched paired/context
inputs it needs. Additional input directions/content types require explicit
declarative selectors, within the same authorized capture/conversation. They
are not implied by installing another contribution in the same package.

Each contribution also declares its host surface (`headers`, `body`, or both).
Both `inputs.request` and `inputs.response` are mandatory, each with an explicit
header-name array and a body mode of `none`, `optional`, or `required`. Empty
header arrays and `body: "none"` explicitly request no access; omission is an
invalid manifest, not a permissive default. A header-only contribution can be
registered alongside the MAPI body contributions and run concurrently.
Only the selected contribution and its module dependencies initialize; registering
several contributions must not execute all of their entry modules.

Recommend a restricted Stored ZIP64 profile initially: the packer emits ZIP64
records even for small archives, with `manifest.json` first and uncompressed.
Stored entries keep inspection cheap and avoid package inflation. Package size
does not determine body-processing limits. Reuse the hardened ZIP validation
approach in `crates/portable/src/package_zip.rs`, factoring neutral pieces as
needed; its current case-specific member-name rules are not an extension reader.
Compression can be a later explicit package-format capability.

Bound archive/member counts, directory and manifest bytes, entry lengths, and
aggregate content before allocation. Reject traversal, absolute/drive/UNC paths,
alternate data streams, symlinks/reparse entries, duplicate or case-colliding
names, overlapping entries, encryption, and contradictory local/central records.
Verify checksums and digests before supplying executable/data bytes. Avoid bulk
extraction; copy a validated metadata record into the catalog. A sidecar is a
cache of the embedded manifest, never an independently authoritative manifest.

Install packages atomically into immutable revision storage; bind metadata and
execution to the same package identity. Verify a changed archive before use,
including when a path was replaced after discovery. Bundled metadata and package
digests come from the app build. Hashes identify content, not publisher trust.
Show bundled, installed, and development provenance in Extensions settings,
with install, disable/enable, and uninstall management as described below.
Installing or listing an extension never runs code or hooks.
Updates swap revisions atomically, invalidate results, and preserve an in-use
revision until its job closes. Bundled packages update with the app.

Recommend local package installation without a marketplace or automatic update
service in this scope. Unpacked development overrides of a bundled ID require
an explicit override selection; other collisions cannot silently replace it.
Third-party publisher signing can be added separately without weakening the
sandbox or claiming that package hashes authenticate a publisher.

### Matching and explicit activation

Match each registration in native code using manifest metadata and the selected
message boundary's headers. Media-type rules use the request's Content-Type for
a request and the returned Content-Type for a response. A request viewer can be
used before any response exists. Parse MIME type/subtype case-insensitively;
separate parameters such as charset and preserve the original header. Exact media types
are sufficient for the first API. Do not use substring matches, body sniffing,
extension callbacks, or unrestricted regular expressions for discovery. Invalid
or conflicting Content-Type evidence should not silently select a parser.

Media-type matching is one supported selector, not a prerequisite for every
contribution. Header-oriented viewers may instead declare bounded header-presence
or exact-value predicates, qualified by direction, method, status or endpoint.
Require a selective native predicate per registration; do not make a missing
Content-Type a wildcard for all extensions. Evaluate request and response header
conditions against the named side, with missing/redacted/invalid values explicit.
The host must not test hidden sensitive values to infer a match that would reveal
them. Header-only matches work for HEAD/204 responses and captures without bodies.

MAPI's main registration is `application/mapi-http`. Request-side registration
uses the corresponding request media type. Add a narrowly qualified `text/html`
error-response registration using MAPI request metadata and nonzero
`X-ResponseCode`; do not offer MAPI for ordinary HTML. Permit small declarative
header predicates so these exceptions do not need executable detection code.
When multiple viewers match, offer their names and provenance without executing
them to choose a winner. Keep built-in Auto behavior unchanged.

At startup, read only the small installed/bundled catalog. Explicit installation
may copy and validate a package without executing it; registering a development
folder reads bounded directory/manifest metadata. During ordinary startup and
viewer discovery, no extension source read, TypeScript compilation, V8 startup,
package payload extraction, body read, or conversation analysis occurs. Those
activation costs are deferred until the user selects that visualizer.

Activation snapshots the chosen extension revision and evidence, acquires bounded
body leases only for declared body inputs, starts the isolated worker, and renders
its validated result. Header-only jobs perform no body reads or decoding. Reuse
the current result while unchanged evidence is viewed. In snapshot mode, changing
the selected exchange or message boundary requires a new explicit visualizer
selection. A growing body marks its view out of date and offers Refresh rather
than continuously reparsing it. Enabling Live updates explicitly permits continued
delivery for the active scope, as specified below; unrelated traffic never triggers
extension work.

Use one worker per active visualizer instance, with small global and per-extension
caps and bounded queues. Request and response viewers from the same package can
both run when their panes are visible; selecting one does not replace the other.
Their immutable module/compilation artifacts may be shared, but their mutable
parser state, subscriptions, source handles, cancellation and outputs are isolated.
Use fair scheduling/admission so one package's many registrations cannot consume
all capacity; exhausted capacity yields a clear local unavailable/paused state.
Keep parser state and capture-derived result caches only while the
visualizer is active. Key results by extension digest/API, exact source and
boundary revision, header revisions, context dependencies, decoding settings, and
options. Never persist capture-derived caches by default. Metadata and code-only
compilation caches may remain after unloading; compiled code is keyed by compiler/runtime
build and contains no captured data. Turning Live updates off stops the feed;
an active snapshot viewer may still need its worker for deferred expansion.

### Native filtering and active-view lifetime

Compile declarative match rules into bounded native predicates. Filter by capture
and conversation identity, direction/boundary, and declared media-type, request
method, response status, endpoint and protocol-header predicates. A live subscription
can narrow the contribution's declared input selectors, never broaden them; input
selectors default to its display matches unless paired/context selectors are
explicitly declared. Apply declared context selectors to historical broker reads
too; a parser cannot use context enumeration to bypass its subscription filters.
Preserve narrow paired
request/response lifecycle information when required by the declared contract,
without forwarding unrelated response bodies.

Maintain separate indexes for catalog discovery and currently active live
subscriptions. A shared dispatcher uses capture/scope, direction/media-type and
header-name indexes to find only candidate active subscriptions, then evaluates
their bounded metadata predicates. Parse common metadata once per event. Do not scan every
installed extension for every request, broadcast first and filter in TypeScript,
or create a capture observer/queue for each installed package. With no active
subscriptions, the extension event path exits before extension-specific work.
Native predicates run before leases, source reads, decoding, serialization, IPC,
or parser invocation. Bound predicate counts and header/path lengths as well as
the active subscriber count. Subscribing must not initiate a whole-capture body
scan; initialize only from bounded, filtered retained context.
Registration validation checks that every header referenced by matching or
correlation rules is in that contribution's declared input list for the correct
side. A `contentTypes` selector therefore requires `content-type` on its matched
side. Native matching cannot become an undeclared header-access side channel.

For MAPI, use its media type and qualified error-response rules together with
declared request direction/method and protocol headers such as `X-RequestType`,
where applicable. Derive endpoint/conversation identity from the activated
exchange; do not assume all servers use a fixed `/mapi/` path or suppress an
unknown operation that the parity contract requires reporting. Header matching
and pairing need no body parsing. Headers not yet available defer admission;
later revisions can be admitted when their metadata is sufficient.

Active means the selected visualizer is displayed in a visible inspector surface
of a non-minimized window. Mounted but hidden workspaces, collapsing/hiding its
owning header/body pane, switching its viewer, closing the inspector, and
minimizing or hiding its window deactivate it. Merely moving keyboard focus or using another
application while the view remains visible does not deactivate it. Each visible
viewer owns its own subscription, state and worker; one window cannot keep a
hidden viewer in another window subscribed.

On deactivation, remove the subscription from dispatch first, revoke its generation
and source handles, cancel queued/in-flight work, and terminate/reap its worker
within a bounded shutdown deadline. Release queues, leases, context, derived
buffers and capture-derived result caches. Do not wait for an extension's `dispose`
handler to cooperate, keep a warm-process grace period, or replay accumulated
events in the background. A hot-reload candidate must also be canceled if its
owning visualizer becomes inactive. Only small UI preferences/selection IDs and
metadata/code caches may remain; they confer no active subscription.

Reopening the visualizer is an explicit activation and starts a fresh worker
from available retained evidence; Live updates must be enabled again. Inactive
intervals are not silently represented as continuously observed traffic. Restored
layout may show a suspended placeholder but cannot start processing by itself.

### Optional live request/response updates

The host owns a Live updates toggle beside the active visualizer's controls.
Show it only when that visualizer's manifest declares support and the capture
can receive new traffic. Default it off for each newly opened visualization;
restoring layout or restarting the app must not subscribe or load extension code.
Bundled MAPI should declare support so it can incrementally maintain decoding
context as related requests and responses arrive. Hot reload and Live updates
are independent options: one changes code, the other delivers traffic evidence.

For example, a visualizer can declare this metadata (names remain provisional):

```json
{
  "liveUpdates": {
    "events": ["request", "response"],
    "bodyProgress": false
  }
}
```

The declaration requires an optional typed `onTraffic(batch)` export. It receives
bounded, ordered batches of new or revised request/response descriptors through
the supervisor, and returns the same validated visualization/continuation model
as snapshot parsing. The worker can retain bounded protocol state between calls.
No extension subscribes itself, starts a timer, accesses the capture event bus,
or installs an interception hook. Native manifest matching and scope checks run
before events or readable source handles reach the worker.

Enablement anchors the subscription to the current capture and conversation,
extension revision, and declared message directions/boundaries. Show that scope
and Live/Paused/Behind status in host UI. New matching requests and responses
are fed automatically within it, including responses to previously observed
requests after the native filters above pass. A paired response that fails the
declared input predicates is identified as unavailable to that parser, rather
than passed as a matching payload. Related context reads use the existing broker
and its same scope and privacy rules.
Selecting another matching exchange within this scope can reuse the live viewer
without another activation; it does not silently expand the subscription. Leaving
the visualizer or selecting a different capture/conversation ends the subscription
and requires explicit enablement in the new scope. New events never automatically
change the inspected exchange or steal focus, scroll, selection or expanded nodes.

Each event carries the subscription generation, monotonic delivery sequence,
exchange/message identity, boundary, evidence revision, lifecycle/completeness,
and opaque header/body source descriptors. Header availability, revisions, and
terminal body state are the basic lifecycle updates; final state can be complete, failed, canceled or
truncated. Header-only subscribers receive relevant header/lifecycle revisions
without body handles or body-progress notifications. Deliver body-growth revisions
only when `bodyProgress` is declared, with host-controlled coalescing and rate limits.
Extensions read bounded retained
bytes on demand; the feed is not a copy of every transport chunk. Treat changes
as idempotent upserts by message identity/revision, never as duplicate requests.
Record host observation order without implying causal ordering across exchanges.

Establish the initial snapshot and subscription at an explicit sequence boundary:
buffer later events while initializing, then deliver them in order. Serialize
`onTraffic`, expansion and continuation calls within a worker. Source revisions
must be stable for each operation; report eviction/unavailability rather than
substituting newer bytes. Generation checks reject replies from a disabled feed,
superseded snapshot or replaced worker. Output remains a host-rendered document;
large documents use bounded pages and stable node IDs, not arbitrary UI patches.

Bound pending event count/bytes, batch size, processing time, retained context,
source leases and output memory. Apply per-batch budgets plus rolling session
work/read budgets; a long-running subscription must neither bypass limits by
opening new batches nor inevitably exhaust a one-shot lifetime work allowance.
Never block traffic capture on extension delivery. Coalesce only superseded
body-growth updates for the same message; do not silently drop requests,
responses or terminal transitions needed by a stateful protocol parser. On an
unrecoverable queue overflow or sequence gap, pause the feed, show Behind with
the missing range, and require bounded resynchronization from retained evidence.
If the evidence is gone, show incomplete context instead of reporting continuity.

Turning Live updates off unsubscribes immediately, cancels pending feed work,
rejects late output, and freezes the last valid view. Re-enabling it reconciles
the bounded retained changes since the last applied sequence or rebuilds from
a fresh snapshot; skipped evidence cannot be silently ignored. Worker failure,
memory pressure or capture closure pauses/stops the feed with a useful status.
Clear/removal/retention events invalidate affected handles and dependent views;
they do not keep invisible capture history alive in extension state. No worker
or subscription survives the owning viewer's deactivation, even if its UI component
remains mounted. Live mode retains all sandbox restrictions, including the
prohibition on direct or brokered networking.

### Required data access declarations

Treat the input manifest as an enforced maximum data grant per contribution,
bound to its immutable revision and active instance. Header declarations must be
finite lists of exact valid HTTP field names, normalized to lowercase for
case-insensitive lookup. Reject wildcards, prefixes, regex names, invalid tokens
and duplicate normalized names. No implicit access to Content-Type, the complete
header block, paired-message headers, or another contribution's allowed fields.
The host's own ordinary inspection remains independent of these extension grants.

Enforce the same lists on initial input, live events, historical context,
continuation/range reads, derived metadata and source references. Filter header
fields before copying/serializing them into worker IPC. Paging returns only
declared fields, with duplicate occurrences and order preserved, never an
unfiltered block that the script is expected to ignore. Header-derived metadata,
such as content type or charset, must use declared dependencies; the host may
still inspect its own metadata internally to validate evidence safely. Requested
HTTP content decoding must declare `content-encoding`; interpreted charset
metadata must declare `content-type`. Shared
caches cannot expose another instance's broader input grant. Reject undeclared
reads with a typed access-denied result, even if a caller guesses a valid handle.

Body access is explicit and independent for request and response:

| Mode | Enforced behavior |
| --- | --- |
| `none` | No body handle, lease, read, decode, bytes or body-progress delivery for that side. Continuations cannot acquire access later. |
| `optional` | Permit bounded on-demand body reads when retained; parsing can proceed without them. Do not eagerly fetch bytes just because access is permitted. |
| `required` | The body is a prerequisite for visualization of that input; report unavailable/incomplete evidence honestly. Reads remain bounded and on demand. Header/lifecycle events may arrive first and do not imply the body is ready. |

These grants never turn on retention, recover discarded evidence, widen capture
or conversation scope, override sensitive-header redaction, or grant networking.
Selecting Live updates authorizes repeated use of the existing grant, not new
header/body permissions. An update or hot reload validates the candidate revision's
declarations and updates the displayed summary. Changed declarations require a
fresh worker and scoped handles; never mutate the grant of an existing worker
or retain data that the replacement can no longer access. This follows the normal
update/hot-reload lifecycle without a separate permission-approval workflow.

Show declared access per contribution in Extensions settings: request and response
header names, body modes, context scope, and live-event interests. Do not add
access histories, delivered/read-field tracking, audit records, audit retention,
or per-read prompts. Keep only transient counters needed to enforce resource
budgets and ordinary bounded error diagnostics, without captured header values
or bodies. Read-only enforcement belongs in the host contract, independently of
the settings display.

### Input, context, and resource contract

An invocation contains an opaque job/generation ID; capture, exchange, direction,
and message-boundary identity; permitted immutable header/body-source descriptors;
authorized content-type/coding metadata and completeness; declared safe protocol
headers; and effective budgets. Input describes original/effective request and response boundaries
explicitly so a parser cannot accidentally combine different traffic stages.
Absent, redacted, truncated, evicted, and still-capturing inputs remain distinct.

Supply declared headers from both the selected message and its paired request or
response, with each side's availability and exact boundary/revision identified.
Preserve header order, duplicate occurrences, original names and retained value
bytes, plus a normalized name for lookup. Provide safe text decoding metadata
without replacing the bytes; do not flatten repeated headers into a dictionary
or concatenate them indiscriminately. These are captured HTTP fields, not a
reconstruction of physical HPACK/QPACK bytes. Include method/target and observed
status where declared and available; a missing response is not an empty header
block. Honor existing capture redaction and sensitive-header policy, retaining
explicit redacted/omitted states and original lengths only where recorded.

Bound header count/value bytes and total header input, using paged reads for
large retained blocks. Header-only jobs receive no body source capability and
cannot request one through a continuation. Optional-body jobs remain useful
with unavailable bodies; required-body jobs explain the missing prerequisite.
Headers stay available independently of body eviction. Header-driven viewers
return the same tree/table/text model and can link nodes to a specific header
occurrence and value-byte span. Header references never masquerade as body offsets.

Pass bytes through bounded binary/chunk frames, not giant JSON number arrays.
Keep metadata framing and binary framing authenticated and versioned, validating
lengths before allocation. Expose a typed byte-reader over supplied buffers to
TypeScript. Use decimal strings for 64-bit offsets/lengths on the JSON boundary
and `bigint` internally; JSON numeric rounding must never change an offset or
MAPI integer. Parsing must not use the truncated text preview as its input.

Host decoding of HTTP Content-Encoding happens only when requested under declared
input dependencies, on demand under existing codec budgets, and produces a
separately identified source. HTTP transfer framing has
already been removed by capture. MAPI framing, compression, and obfuscation are
the extension's responsibility. Keep both representations inspectable; failed
decoding must not make encoded bytes look like decoded bytes.

Provide a narrow context broker for paired messages and related earlier exchanges.
It exposes only opaque handles scoped to the activated capture and conversation,
ordered metadata pages, and bounded reads of explicitly requested retained
sources. The manifest declares the required non-secret correlation fields; the
host computes scope from capture provenance, endpoint, client/session identity,
and those fields. If a secret is needed only for identity, the host can supply a
scoped opaque correlation token instead of revealing its value. Missing or
ambiguous identity produces a diagnostic, not a wider search. Never merge
independent imports or infer a conversation solely from a hostname or timestamp.

Related reads are authorized by the user's explicit visualization selection,
as agreed for this plan. The host validates every handle, boundary, field, and
read; extensions cannot enumerate arbitrary captures, read paths, change traffic,
or obtain credentials. Authorization, proxy authorization, and cookie values
are not part of the default input. Captured body content remains sensitive even
when headers are withheld; results receive the same retention/privacy treatment.

Keep synchronous, bounded parser calls. A parser may return a typed `need-input`
continuation requesting allowed byte ranges or context pages; the supervisor
services it and resumes the job. Bound continuation count, cumulative bytes,
visited exchanges, CPU/wall time, heap, process memory, output, strings, tree
nodes/depth, derived bytes, and expansion pages across the whole job, not just
each call. This supports incremental parsing without exposing general async
I/O, unrestricted host calls, or resetting the budget through pagination.
Cumulative work time excludes time the user spends reading; per-operation hard
deadlines and separate session/lease idle expiry still prevent stranded workers.
For live sessions, each update batch and its continuations are a bounded job;
rolling session budgets and state/queue caps additionally constrain the feed.

Requested manifest limits can only lower host ceilings. Establish final defaults
from the MAPI corpus before freezing v1; the existing traffic-script 100 ms
default and 4 MiB JSON frame limit are not an adequate visualization policy.
Measure cold startup, first useful result, context reconstruction, expansion,
memory and cancellation. Exhaustion yields an explicit partial/limited result
with a useful retry action; never silently drop context or claim complete output.

### Host-owned output and sub-visualizations

Define a versioned `VisualizationDocument` with a title/summary, parse status,
diagnostics, source descriptors, root node references, and optional deferred
child pages. Every field is untrusted data and validated by the host.

| Output element | Contract |
| --- | --- |
| Node | Stable ID within a document, field name, semantic type, typed value, display hints, and child references or an opaque expansion token. |
| Value | Explicit string, boolean, integer, float, enum, flags, timestamp, GUID, bytes, object, or array; preserve exact wide integers and raw values. |
| Format metadata | Protocol/schema name, charset, byte order, numeric radix, compression/encoding, units, and interpretation warnings. |
| Source span | Source ID/kind, exact offset/length, and optional bit range; references only validated body/derived buffers or a header occurrence's retained value bytes. |
| Derived source | Bounded generated bytes, parent source spans, transform description, and mapping precision: exact or enclosing source region. |
| Sub-visualization | A typed tree/table/text/hex presentation of a node or source, selected and rendered by host components. |
| Diagnostic | Unsupported, malformed, incomplete, missing-context, resource-limited, or internal-error status, scoped to a source/node when possible. |

Nodes can refer to multiple byte spans. Decompressed ROP fields refer to the
decompressed source, with provenance back to the compressed envelope. Do not
pretend a decompressed field has an exact original-byte range when no such
mapping exists. Selecting a node highlights its bytes in the appropriate source;
the user can switch to the enclosing encoded region. Unknown structures preserve
their bytes and show what could not be interpreted.

Expose operations equivalent to `visualize`, `resume`, `expand` and `dispose`,
plus `onTraffic` for visualizers declaring live support, over this data model.
Expansion tokens belong to a document/revision and expire when it is replaced
or closed. Validate IDs, references, acyclicity, bounds and total output cost
before rendering, with pagination/virtualization for large trees and tables.
Nested views do not automatically execute other extensions; the same extension
can describe nested structures, while selecting a different extension for a
typed byte source is a separate explicit action.

No HTML, CSS, SVG markup, DOM handles, JavaScript callbacks, web URLs to fetch,
custom templates, or executable UI are accepted from an extension. Text stays
escaped, resource references are opaque validated handles, and no output causes
a network request. V1 host renderers are tree/table/text/hex; any future image
view must reuse the existing isolated raster-decoding path.

### Network denial and sandbox lifecycle

The extension runtime provides no `fetch`, sockets, WebSocket, XMLHttpRequest,
Node/Deno I/O, DNS API, subprocesses, filesystem, environment, registry,
clipboard, native FFI, WebAssembly, dynamic imports, or string code generation.
Restrict these before any module initialization, not only before calling an
export. Package resources are supplied as bytes through the same bounded channel.

On Windows, require an ephemeral zero-capability AppContainer and a kill-on-close
Job Object with process, memory and UI restrictions. Grant no Internet/private
network capabilities or loopback exemption; inherit only the exact IPC handles
and never sockets. Verify containment before accepting source or captured bytes.
[AppContainer network isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation)
is the OS boundary; removing JavaScript globals alone is insufficient.

The broker has no HTTP, DNS, arbitrary URL, UNC/path, download, replay, or proxy
operation. Only bounded package data and already-retained capture evidence cross
it. Check both runtime API denial and native network denial under the actual
production launch policy, including direct TCP/UDP, DNS, loopback and LAN probes.
Exercise top-level module code as well as handlers and malicious output URLs.
Any discovered network path blocks shipment until closed.

Deadlines/cancellation kill an unresponsive worker and release leases and derived
buffers. Late responses cannot replace a newer selection, reload revision or
closed job. A worker crash leaves raw viewing and capture available. Apply this
policy equally to bundled, installed, and unpacked development extensions. If
the OS boundary is unavailable, report that visualization is unavailable; no
unsandboxed fallback. Non-Windows production execution waits for an equivalent
qualified sandbox.

### Development and host interaction

Register an unpacked local directory containing the same manifest and source
layout; do not automatically discover arbitrary folders. The host watches that
registered directory only when Hot reload is enabled. Resolve paths under its
root, reject reparse-point escapes, and snapshot exact bytes into an immutable
revision before execution. A packaging command and local validator produce the
same normalized manifest/module graph used by production; neither runs install
hooks. Use portable Python 3.11+ entry points in `scripts` for repository tooling.
The development manifest can declare files without fixed payload digests; on
activation/reload the host computes them from the immutable snapshot and then
applies ordinary revision validation. Packaged manifests require those digests.
This permits editing source without manually updating hashes after every save.

Debounce changes and invalidate the candidate revision. With no active visualizer,
file changes update metadata/dirty state without compiling or starting workers.
With an explicitly opened development visualization, hot reload validates and
starts the candidate in a fresh sandbox, then swaps only after success. Failed
reloads preserve the last good view and identify the failed candidate. Retain
tree expansion/selection where IDs still match, reject old asynchronous replies,
and expose manual Reload, diagnostics, source locations, and the active revision.
When live mode is enabled, replace the worker using a bounded snapshot and event
sequence handoff, then resume the feed only after the candidate initializes.
Do not transfer arbitrary JS heap state between revisions. Preserve the old
worker/feed on candidate failure; expose a gap if bounded buffering cannot keep
up. If a new manifest removes live support or changes its scope/matching contract,
stop live delivery and require the user to enable the revised option explicitly.

Add matching body contributions to the existing Body viewer selector and expose
header contributions through a compact host-owned View as control in Headers.
Combined contributions declare the surfaces where they are useful and receive
one coherent selected/paired-message input contract. Selecting one
shows local progress, Cancel, and Retry where appropriate. Render a tree and byte
detail together when space permits, switching between them in a compact pane.
Keep protocol metadata and source/derived distinctions visible without adding a
new workspace. Extension management belongs in the Extensions settings experience.
Preserve raw Save as behavior, copy semantics, keyboard tree navigation, focus,
and selection through expansion and refresh. Follow the
[UX quality routine](ux-quality.md) for these changes.

### Extensions settings

Provide a single Extensions settings experience listing installed packages with
name, version, bundled/installed/development origin, enabled/disabled state and
compatibility or validation errors. Expanding an entry shows its visualizer
registrations, matching criteria and declared header/body access by direction.
Load this information from validated metadata; opening settings must not load
extension source, start workers or read traffic.

- **Install:** choose a `.tmext`, validate it, and register the package atomically.
  Support registering an unpacked development directory and its Hot reload option
  here too. Installation has no executable hooks.
- **Disable / Enable:** apply to all contributions of that extension. Disable
  immediately stops their feeds and unloads all active instances across windows.
  Enable makes matching visualizers available for explicit selection; it does not
  start a worker or resume a previous live subscription. Persist this setting.
- **Uninstall:** unload the extension, remove its registration and host-managed
  installation/cache. Unregistering a development extension never deletes the
  developer's source directory. Bundled packages can be disabled; their physical
  files remain owned by the app installation.

Installed, enabled and actively visualizing are distinct states. Keep management
and declaration details here, with ordinary error feedback and no auditing UI.

### Bundled MAPI parity and implementation sequence

Use the [Office inspector](https://github.com/OfficeDev/Office-Inspectors-for-Fiddler)
as a behavioral reference, porting its parser/data concepts to TypeScript. Its
Fiddler/.NET integration cannot run unchanged in this runtime. Its
[user guide](https://github.com/OfficeDev/Office-Inspectors-for-Fiddler/wiki)
demonstrates expandable fields with linked source-byte highlighting. The
[parser](https://github.com/OfficeDev/Office-Inspectors-for-Fiddler/blob/7a76470574377d14cae612c507f45b34d70e05a0/MAPIInspector/Source/MAPIParser.cs)
also reconstructs context from earlier exchanges; a response-only JSON formatter
would not meet the requested parity. Pin the upstream reference revision and
protocol-spec versions in implementation fixtures and attribution records.

Broad parity means an agreed matrix spanning mailbox and address-book operations,
Execute/ROP request and response structures, properties and flags, auxiliary
buffers, protocol compression/obfuscation, nested structures and FastTransfer,
cross-exchange context/handles, errors, partial captures, and field-to-byte
selection. It does not require reproducing Fiddler's UI, capture hooks or export
integration. Follow [MS-OXCMAPIHTTP](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcmapihttp/2e717239-a22b-4508-a2ee-af64ffc090a2)
and related protocol specifications when upstream behavior is ambiguous; record
intentional differences. Include upstream MIT notices and dependency attribution.

| Stage | Deliverable and exit evidence |
| --- | --- |
| 1. Specify and measure | Freeze the parity matrix and sanitized golden captures; settle package/API schemas, context identity, output provenance, and cold/active resource budgets. Inventory upstream parser families and dependencies before estimating the port. |
| 2. Package and runtime | Contract/package primitives, native admission index, closed TypeScript modules, isolated worker and supervisor implemented. Metadata persistence, retained-evidence/context broker and product-owned subscriptions remain. Production sandbox fixtures exercise activation, byte continuations, failures, network denial and unload. |
| 3. Host renderer and development | Deliver concurrent request/response/header contributions, the tree/table/text/hex model, source highlighting, deferred pages, Live updates control, Extensions settings with declaration details and install/disable/enable/uninstall, unpacked registration and transactional hot reload. Prove both packaged and unpacked forms use the same validation/sandbox. |
| 4. MAPI port | Build shared byte readers/property decoders, transport envelopes, mailbox/NSPI families, ROPs, auxiliary data, compression, FastTransfer and contextual decoding, including incremental request/response handling in live mode. Add differential fixtures as each family lands; no bundled release at partial coverage. |
| 5. Parity and shipment | Run the full matrix against the pinned reference, qualify hostile inputs, resource/performance behavior, native sandbox and UX, then bundle the ordinary `.tmext` with build-generated metadata and attribution. |

Golden tests compare decoded semantics, source ranges and transformation
provenance, not Fiddler's control layout. Include out-of-order inspection, missing
prior sessions, redaction, reused handles, identical-looking independent captures,
unknown ROPs, nonzero protocol errors, multi-buffer compression and nested
FastTransfer. A supported operation must not silently pass as an opaque block;
the matrix records unsupported cases and requires resolution before shipment.

Security/robustness checks cover hostile ZIP64 directories and replacement races,
escaping development paths, malformed module graphs, infinite loops, allocation
bombs, forged IPC/context handles, invalid/cyclic trees, decompression bombs,
pagination budget evasion, crashes, cancellation and reload races. Verify that
ordinary startup, viewer discovery, selecting matching traffic, and Auto viewing
cause zero extension worker launches, source reads, and context body reads when
no extension visualization/subscription has been explicitly activated.
Live-mode tests additionally cover unsupported/off declarations, no feed before
enablement, request/response pairing, delayed and failed bodies, snapshot/event
races, duplicate revisions, body-progress coalescing, slow parsers, queue overflow,
pause/resume gaps, clear/eviction, scope changes and hot reload during delivery.
Verify capture remains responsive, network access stays denied, and snapshot and
live parsing agree for the same retained evidence and protocol context.
Benchmark identical high-volume traffic with zero, one and many installed
extensions while holding the active set fixed. Verify inactive packages add no
per-event callbacks, candidate scans, queues, body reads or IPC; measure active
filtering cost, admitted/rejected counts and per-view resource use. Exercise
switching viewers, hidden mounted workspaces, collapsed inspectors, minimized
windows and hot reload during deactivation. Assert immediate dispatch removal,
bounded worker termination and release of captured-data memory/leases. Reopening
must rebuild without delivering hidden-period events as a continuous live feed.
Verify one package's request, response and header contributions can be active
concurrently, including multiple instances across windows. Match their own sides
and input declarations correctly, preserve duplicate/raw/redacted headers, and
support a request before its response exists. Header-only fixtures with missing
Content-Type and absent/evicted bodies must cause zero body reads. Closing,
canceling, reloading or crashing one instance must not stop another; updating or
removing a package must handle all of its instances explicitly. Check per-package
admission fairness and inactive registrations as well as inactive packages.
Access-contract checks reject missing declarations, wildcard/invalid header names,
case-variant duplicates and undeclared matcher/correlation dependencies. Attempt
undeclared reads through initial payloads, live updates, paired/history access,
continuations, guessed handles, derived metadata and shared caches. Verify `none`
causes zero body leases/reads/decodes, optional bodies stay lazy, required-body
absence is explicit, and declaration changes cannot silently expand active
worker grants. Check settings declaration details against validated manifests,
without adding runtime access tracking. Exercise install, disable/enable and
uninstall, including active instances in multiple windows and preservation of
unpacked source directories. Settings discovery and enabling must start no worker;
disabling/uninstalling must stop all affected feeds and release their resources.

Run formatting, strict Clippy and relevant Rust/TypeScript tests, production
template Playwright checks, and native Windows sandbox/WebView2 acceptance.
Check populated trees, loading/failure/partial states, delayed results, keyboard
operation and 1440x900, 960x720, 800x600/resized layouts. Run
`python scripts/dev-env.py --check` before builds and the complete
[local release qualification](windows-release.md#local-release-qualification-before-dispatch)
before any signed release dispatch. Current sandbox tests check missing runtime
globals and runaway code; new native network probes are required evidence, not
something this plan claims has already passed.

### Remaining design choices for review

- **Package compression:** recommend Stored ZIP64 v1, reserving bounded Deflate
  for a later format feature if measured package size warrants it.
- **Distribution:** recommend bundled packages plus explicit local package/folder
  registration, with visible provenance and explicit development overrides.
  Marketplace, automatic third-party updates and publisher signing are separate.
- **API freeze:** finalize the exact node vocabulary, context correlation rules,
  parser continuation shape and numeric resource limits using the parity corpus
  before calling the visualizer API stable. These are proposals, not existing
  product guarantees.

When implemented, move the lasting UX/development contracts to their topic guide,
record the cross-cutting decision in an ADR, and remove completed stages here.

## Capture and inspection fidelity

- Broader case/rule CLI validation, matching and extraction remain deferred under
  the revision-0 [portable investigation contract](portable-investigations.md), along with
  offline script execution, persisted browser runs and CLI comparison (K5).
  Desktop case previews and saved recorded-request/capture comparisons are implemented.
  Broader project organization and any future format stability decision remain
  separate from the experimental live control protocol.
- Expand safe previews or semantic viewers only after a format-specific parser,
  sandbox, CSP, resource-limit, and hostile-corpus review.

## Platforms and distribution

- Extend platform qualification beyond the Linux container gate to native Linux
  and macOS host integration, networking and process sandboxes.
- Decide whether a Linux or macOS desktop shell has a supportable system WebView
  and process-sandbox story before enabling those products.
- Add macOS caller-process attribution only if it can use a supported and
  testable operating-system contract; the available `libproc` socket APIs are
  currently private and subject to change.
- Review activation of the deferred hosted platform, browser, fuzz, and
  performance workflows in the [CI guide](../ci/README.md).

## Protocol expansion

- HTTP/3 WebSockets.
- Application-owned upstream byte streams for upgraded connections.
- Native downstream HTTP/3, CONNECT-UDP, MASQUE, and WebTransport.
- Reverse-proxy listener products, including certificate provisioning, load
  balancing, and health checks. Existing provider seams do not imply these
  features.
- QUIC 0-RTT and connection migration only after replay and identity policy is
  explicit.

## Chromium transport alignment

The built-in profiles target Chromium 154.0.8037.99. Chromium 155 uses the same
wire targets covered here. Use the [local comparison harness](chromium-comparison.md)
to qualify changes; a matching JA4 hash alone does not establish transport or
certificate-verification equivalence.

Current offers and connection policy are documented in
[Chromium negotiation](chromium-negotiation.md), [ALPS](alps.md), and
[certificate verification](chromium-verifier.md). Dependency patches and their
reproduction commands live in the [transport inventory](../third_party/rust-transport/README.md).

### Expand comparison coverage

The harness compares fresh GET requests over H1.1, H2 and H3. Extend it to cover
connection reuse, HEAD/POST, duplicate headers and cookies, edits and reroutes,
accepted ALPS, and resumption. Keep protocol fallback and missing evidence as
hard errors. Compare randomized ECH padding and QUIC GREASE across samples;
a single random value difference is not a compatibility defect.

### HTTP/2 startup ordering

The initial SETTINGS values, header order, priority and HPACK output match the
target in the fresh-request case. A real SETTINGS ACK still precedes the initial
WINDOW_UPDATE and HEADERS. A peer-settings bootstrap API must preserve flow
control, concurrency limits and ALPS settings before request allocation. Prefer
an upstreamable encoder/connection API; do not rewrite encoded HPACK bytes.

Qualify uploads, trailers, connection reuse and extended CONNECT alongside any
startup-order change.

### HTTP/3 dynamic QPACK

The pinned quiche decoder has incomplete dynamic-table paths. Keep advertised
capacity and blocked-stream limits at zero until bounded updates, insertions,
evictions, blocked streams, acknowledgements, cancellation and malformed-input
handling work against an independent peer. Prefer upstream support; assess a
maintained implementation or adapter migration if necessary.

### HTTP/3 datagrams

H3_DATAGRAM and the QUIC datagram transport parameter remain absent. Supporting
them requires an application consumer, routing and ownership rules, capture
semantics and bounded queues. Review that product scope before implementation.
HTTP/3 extended CONNECT and WebTransport likewise need working end-to-end behavior
before their capabilities can be advertised.

For each change, run the affected interoperability tests and local comparison.
Keep qualification reports and review transcripts in generated evidence, not in
the roadmap.

## Post-quantum trust

- Evaluate PQ interception issuers and Merkle Tree Certificate support as
  Chromium's PKI requirements develop. Define trust installation, protected key
  storage, migration and rollback before changing the interception root.
  Current hybrid exchange and ML-DSA handshake support are documented in the
  [certificate model](chromium-negotiation.md#algorithms-and-clienthello).

## Control protocol stabilization

Control revision 0 is a same-build, breakable internal contract. It may remain
that way indefinitely. Stabilization begins only when a human maintainer
explicitly records that it now makes sense; elapsed time, repository splits,
consumer count, or test coverage do not trigger it.

Once triggered, stop protocol feature work long enough to inventory producers,
consumers, persisted references, and deployment topologies; define compatibility
and support windows; specify canonical serialization and unknown-field behavior;
add conformance and cross-version fixtures; and record the v1 decision in an
ADR before promising compatibility.

## Repository boundaries

The monorepo keeps cross-layer changes atomic while APIs are evolving. Split a
crate into another repository only when independent ownership, release cadence,
security review, or distribution warrants the operational cost. A split must
retain acyclic dependency direction, runnable contract tests, explicit release
coordination, and clear license/security ownership. It does not stabilize the
control protocol by itself.

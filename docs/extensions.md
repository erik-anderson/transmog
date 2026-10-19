# Extension contracts and packages

`transmog-extensions` provides the UI-independent revision-1 visualizer metadata,
native matching, input projection, output validation and `.tmext` container
primitives. `transmog-extension-host` and `transmog-extension-supervisor` provide
the isolated JavaScript worker and its bounded lifecycle. Desktop management,
retention/context integration and the bundled MAPI extension remain in the
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
the planned Extensions settings experience.

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

`visualize`, `resume`, `expand` and declared `onTraffic` calls return synchronously
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

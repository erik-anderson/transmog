# Plan: Traffic inspection, automation, and sandboxed scripting

Status: approved; implementation in progress
Audience: maintainers, application authors, security reviewers, and UI authors
Depends on: Hooks v2, content processing, application/session service, native
capture, and the Windows Tauri/WebUI shell

## Mission

Build the next Transmog product layer into a Fiddler-style traffic workspace
without moving projects, saved responses, body retention, scripts, or editor
state into the proxy core. Users must be able to watch traffic as it passes,
inspect original and effective messages, break and modify exchanges, apply
conditional native rules, serve saved or authored responses, and use a safe
script API for behavior that does not fit the built-in controls.

The common path must remain native and inexpensive. A conditional
`User-Agent` rewrite should not require a JavaScript invocation, but the same
rewrite must also be expressible by a script. Both paths must end in ordinary
identified Hooks v2 actions so ordering, limits, cancellation, and audit
attribution remain consistent.

## Product decisions

1. Transmog provides both built-in automation and scripting. Built-in rules
   cover common predicates, header operations, request rejection, and saved
   responses. Scripts cover advanced conditions and calculations.
2. Native rules and scripts use one ordering model and the same typed action
   vocabulary. Neither receives a privileged path around Hooks v2.
3. Response bodies are retained by default. Users can turn retention off.
   The initial default policy is a one-GiB circular on-disk buffer.
4. Circular eviction removes the oldest completed response bodies first. It
   never removes session metadata, audit evidence, user-created captures,
   saved responses, or exports. An evicted body remains visibly identified as
   evicted rather than missing or empty.
5. Request-body retention remains off by default and is an independent privacy
   choice.
6. Each response boundary can be inspected through multiple representations:
   safe presentation, decoded text, formatted structured text, image preview,
   and bounded bytes. The UI always identifies the selected boundary and byte
   representation.
7. User scripts run outside the desktop and proxy processes in an
   operating-system sandbox. Local authorship is not treated as a reason to
   give scripts ambient machine authority.
8. A script compile, timeout, memory, crash, protocol, or action-validation
   error aborts the affected exchange. Matching traffic is never silently
   passed through when a required script is unavailable.
9. The traffic row and inspector show the script identity, revision, handler,
   phase, safe source location, and remediation guidance for a script-caused
   abort. Error records exclude body content and credential values by default.
10. Monaco is an authoring and inspection component only. Rust remains
    authoritative for compilation, validation, activation, persistence, and
    traffic decisions. Monaco never executes a traffic script.
11. The script source and API schema are versioned immediately. Compatibility
    may remain breakable until a human explicitly triggers stabilization, but
    incompatible saved scripts fail with actionable migration diagnostics.

## Layer ownership

```text
Tauri and WebUI desktop
  traffic grid, inspectors, Monaco editors, previews, commands
                         |
transmog-product-workspace
  projects, rules, script manifests, response assets, body store
           |                                  |
transmog-script-adapter              transmog-script-supervisor
  per-exchange registration snapshot bounded authenticated IPC
           |                                  |
transmog-app and transmog-session    sandboxed transmog-script-host
  live catalog and lifecycle         deno_core and V8
           |
transmog-runtime, content, and core
  protocol correctness, body plans, audit, bounds
```

The exact crate names may change, but the dependency direction may not.
`transmog-core` owns typed hook actions and protocol correctness. The workspace
layer owns persisted rules, scripts, body blobs, response assets, and UI
selection. The script host consumes bounded value objects and returns typed
decisions; it never receives core objects, file paths, sockets, or Tauri
handles. A future CLI reuses the workspace and supervisor layers without
depending on Monaco or Tauri.

## Current foundation and gaps

| Capability | Current foundation | Remaining gap |
| --- | --- | --- |
| Live traffic | Bounded catalog, paging, filters, and lossy update hints | Virtualized live workspace, stable selection, richer filters, timing, tags, and body availability |
| Break and inspect | Request and response head/body phases with bounded decoded edits | Integrated editors, original/effective diffs, automatic pause UX, and clearer deadlines |
| Conditional headers | Native rules support method, host, path-prefix, status, and header set/remove actions | Product persistence, request-header predicates, dynamic activation, richer safe matching, and UI |
| Autoresponse | A request-head hook can return a bounded local response | Response asset store, matching, live activation, completeness checks, UI, and large streaming bodies |
| Response bodies | Observers can receive full chunks and native capture can retain them | Indexed random-access store, default retention, eviction, representation metadata, decoding, and preview |
| Audit | Hook identity, phase, header-name changes, body plan, abort, and local response are recorded | Rule/script revision, source hash, handler, response asset, runtime failure, and source location |
| Scripting | Hooks v2 is a suitable execution target | API schema, compiler, isolated runtime, permissions, supervisor, persistence, diagnostics, and editor |
| Preview | Escaped text and hex summaries exist | Full inspector modes, charset handling, image rasterization, isolated serving, and hostile-input tests |

The application currently constructs an empty caller hook chain before the
session service appends interactive control. Wiring dynamic product hooks into
that construction is therefore application work, not a replacement for core.
The small core extension is a bounded registration provider invoked when an
exchange is admitted. It returns an immutable, finite set of identified
`InterceptorRegistration`s for that exchange. This preserves native Hooks v2
ordering and exact rule or script attribution instead of hiding all product
decisions behind one dispatcher identity.

## Response body retention

### Stored boundaries

For responses, the store distinguishes at least:

- **upstream original**, as received before response hooks; and
- **client effective**, as committed after response hooks.

Each reference records whether bytes are content-coded, the ordered coding
stack, declared media type and charset, observed length, retained length,
completion state, trailers, checksum when complete, and any loss, truncation,
quota, or eviction reason. Equal complete bodies may share a content-addressed
blob, but their boundary records remain distinct.

Decoded presentation bytes are derived on demand under content-processing
limits rather than permanently doubling storage. If a decoded body was
modified and re-encoded, the upstream and client boundary references continue
to identify their distinct exact byte streams.

### Retention modes

- `off`: retain metadata and byte counts only;
- `stop-when-full`: retain until the configured quota is reached, then mark
  subsequent bodies as quota-omitted; and
- `circular`: evict the oldest completed response body blobs to admit newer
  bodies.

The product default is `circular` with a one-GiB aggregate quota. The quota is
configurable within reviewed lower and upper bounds. Active writes are never
selected as eviction victims. A body larger than the total quota retains only
the configured prefix and becomes explicitly truncated; it cannot evict in a
loop or block proxy traffic indefinitely.

Retention-setting changes apply to newly admitted responses. Turning retention
off does not silently erase bytes already retained; clearing the existing cache
is a separate explicit operation whose scope and result are shown before it
runs.

Eviction order is deterministic by completion/admission sequence, not by UI
access. A short read lease keeps a blob alive while the UI or exporter is
streaming it. Crash recovery removes orphan temporary blobs and reconciles
references without treating the cache as a durable capture. Explicit `.tmcap`
captures and saved response assets use separate ownership and are never
circularly evicted.

The live body observer writes through a finite queue with an explicit
backpressure deadline. If storage cannot keep up, proxy traffic continues and
the affected boundary is marked incomplete with its loss reason; capture must
never create unbounded memory growth or silently claim completeness. The body
store uses current-user-only filesystem permissions, does not place body bytes
in logs or routine support diagnostics, and exposes content only through
bounded application APIs. Encryption at rest is a separate product decision,
not implied by the circular cache.

### Inspector representations

The inspector has independent selectors for boundary and representation:

- **Auto** chooses decoded text, structured text, or an allowed image preview
  only after validation;
- **Original text** preserves whitespace and code points without formatting;
- **Formatted** initially supports JSON and other formats only as separately
  reviewed renderers;
- **Image** shows a rasterized preview for allowed formats;
- **Bytes** shows offsets, hexadecimal bytes, and an ASCII gutter for either
  retained encoded bytes or on-demand decoded bytes; and
- **Metadata** remains available when retention is disabled or a body was
  truncated, lost, or evicted.

Text detection handles ASCII, valid UTF-8, and BOM- or declaration-supported
UTF-16 and UTF-32 variants. Invalid sequences are never repaired silently.
The view reports the detected/declared encoding, mismatches, replacement use,
and truncation. A bounded byte prefix may always be requested even when a body
is too large for the editor. The initial display default is 256 KiB with a
reviewed hard ceiling of 16 MiB per UI request; saving or exporting a complete
retained body uses a separate streaming operation.

## Built-in automation

Built-in predicates initially cover:

- original or current effective method, scheme, host, port, path, and query;
- request or response header existence and bounded exact, prefix, suffix, or
  Rust-regex matching;
- response status or status class; and
- an explicit all-traffic predicate for unconditional actions.

Rust-regex matching is used so a crafted pattern cannot introduce
backtracking-based denial of service. Pattern length, count, compiled size, and
evaluation work remain bounded.

Built-in actions initially cover:

- set, append, or remove ordered request and response headers;
- replace or discard a bounded decoded body;
- abort with a safe operator reason; and
- respond with a validated saved or authored response asset.

A conditional `User-Agent` rewrite is a first-class acceptance scenario. A
rule can match a host/path/header condition and set `User-Agent`; the same
operation is available from the script API.

Rules have stable IDs, revisions, priorities, an explicit match boundary, and
declared write sets. Equal-priority overlapping writes are rejected unless a
future explicit composition mode defines their behavior. Higher priority is
applied last in both directions, preserving the existing automation ordering
contract.

Editing does not mutate the running configuration. Validation creates an
immutable candidate snapshot; activation atomically replaces the snapshot used
for new exchanges. Each exchange retains the snapshot and revisions captured
at admission so a mid-flight edit cannot mix behavior.

## Response assets and autoresponse

A response asset contains a stable ID and revision, status, ordered headers,
body reference, byte length, checksum, media metadata, creation provenance,
and completeness state. Assets may be authored directly or derived from an
upstream-original or client-effective response body.

Creating an asset from traffic requires a complete retained body. The UI must
not offer an incomplete, truncated, lossy, or evicted body as if it were a
faithful response. A deliberate partial-response authoring workflow, if added,
must produce a new asset explicitly labeled as authored.

Before serving an asset, Rust validates status, fields, body availability, and
policy limits. Hop-by-hop fields are rejected or removed, stale content length
and digest metadata are repaired, and the chosen content coding is explicit.
Small responses may use the existing bounded `CanonicalResponse`. Large assets
require a new backpressured streaming local-response contract so autoresponse
does not load the complete file into memory.

Head-based autoresponse is supported first. Matching on a complete request
body requires a distinct pre-upstream decision point because the current body
hook cannot synthesize a response after an upstream request has begun. That
core extension must preserve streaming for exchanges that do not opt into
body-dependent matching.

## Script contract

Scripts are TypeScript or JavaScript ES modules with named synchronous
handlers. TypeScript is compiled by an authoritative Rust-side compiler at
validation time; Monaco's language service is advisory. Initial handlers are:

```typescript
export function onRequestHead(context: Context, request: Request): RequestHeadAction;
export function onRequestBody(context: Context, request: Request, body: Body): BodyAction;
export function onResponseHead(context: Context, request: Request, response: Response): ResponseHeadAction;
export function onResponseBody(context: Context, request: Request, response: Response, body: Body): BodyAction;
```

Inputs are immutable bounded value objects. Outputs are deserialized and
validated in Rust before they can affect traffic. The API exposes builders for
the same operations as native automation, including header changes, abort,
bounded body replacement, and response-asset selection.

Each script manifest declares:

- stable ID, revision, API revision, and source hash;
- phase handlers and a native prefilter;
- read capabilities, including whether sensitive headers or bodies are
  supplied;
- write capabilities and declared header/body targets;
- maximum input body bytes, output bytes, log bytes, heap, and duration;
- priority and match boundary; and
- required abort-on-error behavior.

The native prefilter prevents unrelated traffic from entering the script
process. A script authorized for one host and no body access never receives
other hosts or body bytes. Scripts receive response asset IDs and bounded
metadata, never arbitrary paths.

There is initially no filesystem, network, environment, process, clipboard,
registry, dynamic remote import, Node API, Deno namespace, native module,
timer, WebAssembly, or string-code-generation capability. A controlled time
value may be supplied in `Context`; nondeterministic facilities require a
future explicit capability and audit design.

## Script sandbox and supervision

The desktop and future CLI launch `transmog-script-host` as a separate binary.
The default isolation unit is one persistent host process per active script,
subject to a finite global process limit. This prevents one compromised script
from observing another script's source, heap, or traffic inputs. A shared pool
is not introduced without an explicit performance and threat-model decision.

On Windows each host runs:

- in a kill-on-close Job Object with one active process, memory, CPU/time, and
  child-process restrictions;
- under an AppContainer or equivalent restricted token with no network or
  ambient filesystem capabilities;
- with inherited or access-controlled IPC handles available only to the
  supervisor and that host; and
- without UI, clipboard, shell, or certificate/private-key access.

V8 isolate heap limits and termination are defense in depth, not the operating
system security boundary. The V8 and `deno_core` versions are exactly pinned;
prebuilt archives and generated bindings are mirrored or cached, SHA-256
pinned, included in supply-chain evidence, and available to offline builds.
The initial spike compares ordinary JIT and supported reduced-JIT modes for
security, compatibility, latency, and memory before fixing release policy.

The supervisor enforces invocation deadlines independently of the isolate. A
deadline first requests termination and then kills the process if it does not
exit promptly. Crash, protocol corruption, memory exhaustion, and failed
termination discard that host and require a clean process before retry.

## Script failure behavior and remediation

Any error in an enabled script aborts the affected exchange. This includes
compile incompatibility at activation, unavailable sandbox state, timeout,
heap exhaustion, crash, IPC failure, thrown exception, an unexpected
asynchronous return, invalid return value, undeclared mutation, oversized
output, or failed Rust action validation.

An invalid edit never replaces the last activated revision. Activation is
explicit, and the UI clearly distinguishes saved, validated, and active
revisions. If an active process later fails, matching exchanges continue to
abort until the user repairs, rolls back, or deliberately disables the script;
they do not silently bypass it.

The traffic row shows a script-error state. The inspector exposes a bounded
diagnostic containing:

- script display name, stable ID, revision, and source hash;
- handler and hook phase;
- safe error category and message;
- source file, line, and column when available through a source map;
- elapsed time and which resource limit fired; and
- actions to open the revision, validate, roll back, disable, or retry.

Routine diagnostics never include body bytes or credential values. A user may
copy a separately generated, explicitly reviewed diagnostic with additional
context.

## Safe preview policy

Captured content is hostile input. It is never inserted into the Transmog UI
origin as HTML, `srcdoc`, SVG, script, style, or an executable URL.

Initial presentation support is limited to:

- bounded decoded text and original text;
- bounded formatted JSON;
- bounded encoded or decoded byte views; and
- PNG, JPEG, GIF, and WebP images after signature validation and isolated
  decoding/rasterization.

Image decoding enforces source bytes, decoded pixels, dimensions, frame count,
animation duration, color-profile, and wall-time limits. Prefer a separate
restricted preview worker so a decoder failure cannot terminate the proxy or
desktop process. The WebView receives only a raster result through an opaque
preview handle on a separate no-network origin with `default-src 'none'`.

SVG, HTML, XML with active content, PDF, fonts, audio, video, and documents are
not previewed initially. Adding a format requires its own threat analysis,
decoder isolation decision, hostile corpus, resource limits, CSP behavior, and
accessibility fallback.

## Implementation phases

### Phase 0: Architecture decisions and feasibility spikes

Write ADRs for body-store ownership, dynamic hook snapshots, the script trust
model, abort-on-error behavior, sandbox isolation, V8 integration, and Monaco
packaging. Spike locally bundled Monaco workers and one sandboxed V8 invocation.
Measure installer growth, offline reproducibility, cold start, per-process
memory, invocation latency, termination, and Job Object/AppContainer behavior.

Exit gate: reviewed evidence shows that the selected V8 packaging and Windows
sandbox work with the LLVM/Ninja build and do not weaken offline packaging.

### Phase 1: Response body store and retrieval API

Implement boundary-aware body references, streamed writes, exact completion
state, crash cleanup, content-addressed finalization where useful, read leases,
and the three retention modes. Add product settings with response retention on
and one-GiB circular storage by default. Keep request retention independent and
off by default.

Exit gate: quota exhaustion, a single oversized body, concurrent readers,
crash recovery, and circular eviction remain bounded; metadata and explicit
captures survive every eviction.

### Phase 2: Inspector read models and traffic workspace

Replace the JSON inspector with typed boundary, header, body, route, timing,
WebSocket, and audit read models. Add a virtualized live grid with coalesced
updates, stable selection, filters, pause, and body-state badges. Implement
range reads and bounded text/byte viewers before richer previews.

Exit gate: ten thousand retained sessions and sustained updates do not grow the
DOM without bound, and every missing, truncated, lossy, or evicted state is
visually distinct.

### Phase 3: Dynamic native automation

Add a bounded registration provider at proxy construction. At exchange
admission it snapshots the active product registry and expands active native
rules and scripts into their own identified `InterceptorRegistration`s. Each
registration evaluates its phase-appropriate predicate when the required data
exists. The resulting chain is immutable for that exchange, and provider
failure is contained and surfaced instead of producing a partial chain. Extend
the native matcher and action schema, persisted workspace format, compile
validation, atomic activation, revision history, declared write sets, ordering,
and audit. Add built-in conditional header rules, including the `User-Agent`
scenario.

Exit gate: rule edits activate without restarting the proxy, each exchange
uses one immutable, finitely bounded snapshot, every audit entry names the
exact rule or script revision that acted, and conflicts, provider failures, or
invalid conditions cannot become partially active.

### Phase 4: Response assets and autoresponse

Implement the response asset store, create/edit/import operations, session-to-
asset conversion, head-based matching, metadata repair, small bounded local
responses, and streamed large local responses. Add the pre-upstream request-
body decision point only after its API and buffering impact are separately
reviewed.

Exit gate: saved and authored responses work across H1, H2, and H3 egress
scenarios without contacting the origin, and large responses remain streamed
and auditable.

### Phase 5: Runtime-neutral script API and compiler

Define versioned manifests, invocation and action DTOs, generated TypeScript
declarations, authoritative Rust TypeScript compilation, source maps,
capability validation, native prefilters, and a fake runner. Adapt successful
actions into identified Hooks v2 registrations.

Exit gate: conformance tests prove the complete script contract without V8,
and generated declarations cannot drift from Rust validation.

### Phase 6: V8 sandbox host and supervisor

Implement the helper protocol, one-process-per-script lifecycle, Windows
sandbox, Job Object limits, V8 snapshots, heap/deadline termination, bounded
logs, crash recovery, offline dependency packaging, and required error-abort
behavior.

Exit gate: infinite loops, allocation bombs, process spawning, filesystem and
network attempts, malformed IPC, crashes, and supervisor restarts cannot escape
their declared bounds or silently pass matching traffic.

### Phase 7: Monaco scripting and breakpoint editors

Bundle only required Monaco ESM modules and workers. Add generated API types,
completion, diagnostics, templates, source-mapped runtime errors, revision
diffs, and separate save/validate/test/activate controls. Reuse the editor for
structured breakpoint modifications where it improves the workflow.

Exit gate: the editor remains offline, CSP-clean, keyboard navigable, high-
contrast aware, screen-reader usable, and non-authoritative for execution.

### Phase 8: Safe structured and image previews

Add presentation selection, charset diagnostics, JSON formatting, the isolated
preview worker, image limits, opaque preview handles, separate origin, and
preview cache eviction. Keep active document formats disabled.

Exit gate: hostile text, polyglots, malformed images, decompression bombs,
large dimensions, animations, and decoder crashes cannot execute content,
reach the network, or exhaust the desktop/proxy process.

### Phase 9: End-to-end qualification

Run the full workflow through standalone curl and Chromium clients and Nginx,
Apache, and Caddy origins. Cover built-in and scripted conditional headers,
breakpoint edits, small and streamed autoresponses, original/effective body
views, every supported content coding, eviction, script failures, image
preview, capture/export, WebSockets, and application restart.

Exit gate: tests prove that the proxy observed or changed each exchange, script
failures abort visibly, body eviction preserves evidence, and the same project
services operate headlessly without Tauri or Monaco.

## Test strategy

- Unit and property tests cover matcher overlap, rule ordering, write sets,
  body reference transitions, quota arithmetic, eviction order, charset
  detection, DTO validation, and audit attribution.
- Crash and fault-injection tests cover partial blob writes, stale references,
  disk full, read-only storage, helper death, IPC truncation, cancellation, and
  desktop shutdown.
- Fuzz targets cover persisted manifests, rule predicates, script invocations,
  script actions, source maps, MIME sniffing, charset decoders, body ranges,
  and preview inputs.
- Script conformance tests run every handler/action combination against the
  fake runner and V8 runner and require identical Rust-visible decisions.
- Security tests prove the script host cannot access network, arbitrary files,
  environment secrets, clipboard, registry, child processes, CA keys, or
  unrelated script inputs.
- Performance tests measure native-rule overhead separately from script IPC,
  candidate prefilter effectiveness, V8 cold/warm latency, process memory,
  body-store throughput, grid update cost, preview cost, and eviction latency.
- Soak tests exercise sustained body retention and eviction, repeated script
  reload and failure, Monaco model disposal, preview creation, and app close.
- Accessibility tests cover the traffic grid, inspector tabs, Monaco tab
  escape, error navigation, forced colors, scaling, reduced motion, and
  keyboard-only remediation.

## Definition of done

- [ ] Conditional native `User-Agent` changes and equivalent scripts produce
      the same canonical header result and distinct attributed hook IDs.
- [ ] Response retention defaults to a configurable one-GiB circular buffer;
      off and stop-when-full modes behave deterministically.
- [ ] Original upstream and effective client response bodies are separately
      identifiable, with safe text, structured, image, and bounded byte views.
- [ ] Eviction, loss, truncation, unsupported encoding, and unavailable preview
      are never displayed as an empty successful body.
- [ ] Saved or authored responses work through native rules and scripts, and
      large assets stream without complete-body memory growth.
- [ ] Every traffic modification identifies the exact rule or script revision,
      phase, action, and changed structural fields.
- [ ] User scripts run outside the proxy and desktop processes with operating-
      system and V8 resource limits and no ambient capabilities.
- [ ] Every enabled-script error aborts the affected exchange and produces a
      redacted, source-mapped, actionable UI diagnostic.
- [ ] Monaco and all language workers are locally packaged, CSP-clean,
      accessible, and absent from headless products.
- [ ] Active captured content cannot execute in the app origin or initiate
      network, filesystem, navigation, or script activity.
- [ ] Core, content, runtime, standalone interoperability, fuzz, dependency,
      desktop, and packaging gates remain green without hosted CI.

## Design references

- [V8 guidance for untrusted code](https://v8.dev/docs/untrusted-code-mitigations)
- [`rusty_v8` embedding and binary distribution](https://github.com/denoland/rusty_v8)
- [`deno_core::JsRuntime` API](https://docs.rs/deno_core/latest/deno_core/struct.JsRuntime.html)
- [Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Windows AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
- [Monaco ESM integration](https://github.com/microsoft/monaco-editor/blob/main/docs/integrate-esm.md)
- [Monaco accessibility guidance](https://github.com/microsoft/monaco-editor/wiki/Monaco-Editor-Accessibility-Guide)

## Deliberate exclusions

The first scripting milestone does not provide Node compatibility, npm package
installation, arbitrary local modules, remote imports, filesystem or network
APIs, persistent script globals, asynchronous timers, DOM APIs, browser
automation, or active HTML/SVG/PDF preview. It does not stabilize the external
control protocol or the script API. Each exclusion requires a separate product
need and threat-model update before implementation.

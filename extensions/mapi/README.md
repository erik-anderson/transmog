# MAPI extension

The bundled MAPI/HTTP visualizer is an independent strict TypeScript project.
It parses messages into protocol fields, then projects those fields through the
host's tree, value, encoding and source-range contract. The extension supplies
no UI and has no filesystem, network, timer or browser API.
Its app-owned ID is `net.transmog.mapi`. Matching body contributions participate
in Auto for the visible selected message; a complete parse is required before
Auto shows the visualization. Explicit selection remains available for partial
results.

## Authoring and packaging

Install the desktop UI's locked npm dependencies first. From the repository root:

```text
node extensions/mapi/build.mjs
node extensions/mapi/build.mjs --development --watch
```

The build uses the repository's pinned TypeScript and esbuild dependencies.
It type-checks before publishing replacement runtime files and emits ordinary
ES2022. All JavaScript under `dist/` is ignored build output; do not commit it.
The app's build compiles and embeds an ordinary stored ZIP64 `.tmext`, plus a
small metadata sidecar used for discovery without loading the scripts.

For development, register `extensions/mapi/dist/development` in Extensions
settings. The `--development` build gives it a distinct ID and name so it can
coexist with the app's reserved bundled ID; the root authoring manifest is not
modified. Development snapshots measure the current declared files. Enable hot reload in settings to replace active
instances after a successful build. Explicit Reload also lives in settings.
Leaving the visualizer releases its worker, granted data and source watcher.

Use **Package extension…** in Extensions settings to package a loose folder.
Only `manifest.json` and its four explicitly listed payloads are included:

- `dist/runtime/body.js`
- `dist/runtime/headers.js`
- `README.runtime.md`
- `LICENSES/Office-MIT.txt`

The package excludes TypeScript sources, tests, reference code, compiler tools,
source maps and unlisted files. `dist/package-manifest.json` is the build's
length/hash metadata, not an additional archive member. Installed extensions
need neither TypeScript nor .NET. The .NET oracle is a developer test tool only.

**Visualizers and declared inputs → Save to folder…** exports the bundled content
for inspection and editing. The two runtime scripts use readable JavaScript;
they can be changed directly. Give the copied manifest a different ID before
loading it alongside the app-owned extension with **Load unpacked…**.

## Architecture

See [the source guide](typescript/README.md). `BinaryReader` supplies bounded
forward reads, exact 64-bit integers and explicit encodings. `RecordReader`
collects typed protocol facts with original byte ranges. Protocol modules own
HTTP envelopes, mailbox/NSPI messages, ROPs, properties, notifications, auxiliary
buffers, restrictions, rules, synchronization and FastTransfer lexical elements.
Conversation and continuation state belongs to one decode session.

FastTransfer retains bounded byte fragments per stream and handle, assembling
a complete element before decoding it. Derived sources record every contributing
chunk. XOR/LZ77 transformations likewise preserve exact encoded parents and
separate decoded offsets. The output adapter owns bounded deferred expansion;
it does not copy the reference inspector's class hierarchy or presentation tree.
Keep these utilities local until another extension establishes a shared need.

The header contribution has a separate small bundle and no binary-parser
imports. Body contributions share one implementation but have independent
request/response registrations and workers. All contributions declare exact
header names and body access. Native media-type/operation matching determines
applicability. Optional traffic classification supplies a literal `MAPI/HTTP`
label from headers only; it never executes the parser or loads capture bodies.

Body reads use authenticated pages and host-provided HTTP decoding. Retained
context is requested only when needed, and is restricted by the host to the
same capture, origin and conversation. A snapshot holds at most 64 related
exchanges and 8 MiB of body bytes. Live updates are opt-in, filtered before
body access, bounded and revision-aware. Later irrelevant exchanges do not
change the inspected message. Unknown, malformed, incomplete and unavailable
input receives an explicit diagnostic while original Hex remains available.

Parser work is bounded independently for the selected message, its paired request
and related context. Each phase allows 1,000,000 work units, 50,000 fields and 48
levels of nesting; LZ77 copying counts toward work. Context exhaustion leaves
selected-message fields available with a Limited result. The visualizer's ⓘ panel
reports per-phase work categories, input and decoded byte counts, limits and the
largest operation. Copy diagnostics exports those summaries without captured
field values, making budget failures diagnosable without sharing a trace.

## Qualification

The unchanged [Microsoft reference](../../third_party/mapi-inspector/README.md)
is pinned at `7a76470574377d14cae612c507f45b34d70e05a0`. Its 651 source-file
hashes and native Marshal reader provide independent reference evidence. The
C# source is fetched on demand into an ignored cache rather than vendored; the
small oracle adapters are isolated under `test/reference`.
Only enum data is generated from C#; no parser behavior is transpiled.

Run these from the repository root:

```text
python scripts/test-mapi-reference.py
python scripts/generate-mapi-constants.py --check
python scripts/test-mapi-parity.py
python scripts/test-mapi-robustness.py
python scripts/test-mapi-visualizer.py
node --test extensions/mapi/test/reader.test.mjs extensions/mapi/test/protocol.test.mjs extensions/mapi/test/view.test.mjs
```

`test-mapi-parity.py` fetches the hash-pinned reference if needed, builds the developer-only .NET 10 oracle and compares its
fresh output with the committed corpus, then checks the TypeScript decoder.
Use `--golden-only` without .NET. `--write-reference` updates reference results
only after the comparison succeeds. Machine-local timestamps and exception
stacks are excluded from the normalized reference data.

The 721 synthetic probes comprise 707 wire-fact comparisons, 12 continuation
evidence checks and two archived invalid OptionsData response probes. These
are deliberately separate counts. Coverage includes every request/response
HTTP dispatch path for all 23 operations, 128 request ROP shapes, response
variants, populated NSPI data, meaningful mailbox/table responses, scalar and
multivalued properties, Unicode, exact wide integers, handles, auxiliary
buffers, multi-buffer RPC, XOR/LZ77 and FastTransfer continuations. Dispatcher
coverage is checked against the pinned source.

The independent reader/protocol/output tests cover valid and invalid boundaries,
compression overlap, rule counts, synchronization sets, auxiliary offsets,
per-value FastTransfer lengths, UTF-16 validity and serialized output limits.
Robustness checks exercise 93 malformed/boundary inputs with a valid same-worker
recovery after each. The exact production bundles pass 401 full-message viewer
cases plus header isolation, HTTP decode failure, live completion/status changes,
missed updates and resource limits.

Native AppContainer tests run representative reference cases and production
handlers, denied runtime capabilities, exact ranges, authenticated continuations,
header-only inputs, live updates and loose-folder/package roundtrips. Desktop
acceptance uses synthetic SAZ evidence to check exact values, retained rows,
same-origin/capture context isolation, transformed and HTTP-compressed evidence,
source highlighting, concurrent surfaces, static classification and unload.

Intentional corrections to reference behavior have explicit evidence:

- Decoded RPC fields use decoded source coordinates, linked to their encoded
  region; compression has enclosing provenance rather than a false byte mapping.
- FILETIME retains unsigned ticks and explicit UTC metadata with seven fractional
  digits. Optional absent address-book strings remain absent values.
- FastTransfer shows complete continued values and preserves all chunk parents.
  Extended uploads retain their own state. Variable multivalues use each value's
  declared wire length, following Microsoft's lexical grammar.
- MetaTagEcWarning preserves unsigned error bits in both directions.
- OptionsData reads ReturnValue before success fields. The reference's omitted
  status read made two old padded shape probes invalid; separate valid success
  and failure cases verify the protocol layout.
- The Unicode subject fixture uses the protocol's correct discriminator, 4.

No sanitized Office captures are available. Real Office end-to-end validation
is deferred by agreement; synthetic qualification does not establish complete
production-capture compatibility or support for every possible protocol value.

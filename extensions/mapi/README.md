# MAPI extension

The bundled MAPI/HTTP visualizer demonstrates extension authoring in strict
TypeScript. It interprets protocol data and returns typed fields with source
ranges; Transmog owns presentation and supplies declared inputs. The extension
has no filesystem, network, timer or browser API.

Its bundled ID is `net.transmog.mapi`. Body Auto selects it when matching input
parses completely. Select it explicitly to inspect partial results.

## Authoring and packaging

Follow the [development setup](../../docs/building.md). Run these commands from
the repository root:

```text
python scripts/dev-env.py --check
npm ci --ignore-scripts
npm ci --prefix apps/desktop/ui
node extensions/mapi/build.mjs
```

The build type-checks before publishing readable ES2022 modules and source maps.
It uses the locked TypeScript and esbuild dependencies. Files under `dist/` are
ignored build output.

For a development copy that rebuilds on changes:

```text
node extensions/mapi/build.mjs --development --watch
```

Use **Load unpacked…** in Extensions settings to register
`extensions/mapi/dist/development`. Its distinct ID and name let it coexist with
the bundled extension. Enable hot reload to replace active instances after a
successful build, or use Reload in settings. Leaving the visualizer releases its
worker, granted data and source watcher.

**Package extension…** packages a registered folder. The app build also embeds
a `.tmext` package. Both include `manifest.json` and its six declared payloads:

- `dist/runtime/body.js`
- `dist/runtime/headers.js`
- `dist/runtime/body.js.map`
- `dist/runtime/headers.js.map`
- `README.runtime.md`
- `LICENSES/Office-MIT.txt`

Source maps embed the TypeScript for debugging. Tests, reference code and compiler
tools are excluded. `dist/package-manifest.json` records build lengths and hashes;
it is not an extra archive member. Installed extensions need neither TypeScript
nor .NET.

To edit the bundled extension, use **Visualizers and declared inputs → Save to
folder…** in Extensions settings. Change the copied manifest's ID before loading
it alongside the bundled extension. If editing JavaScript directly, remove
`sourceMaps` from the copied manifest so the debugger displays the edited source.

## Architecture

Start with the [source guide](typescript/README.md). `BinaryReader` bounds reads
and preserves exact 64-bit values; `RecordReader` emits protocol facts and byte
ranges. Each decode snapshot owns its protocol and conversation state.
FastTransfer reassembles complete elements and records each contributing chunk.
RPC transformations keep encoded parents separate from decoded offsets.

The header viewer has a separate bundle without binary-parser imports. Request
and response body viewers share code but run in independent workers. Optional
traffic classification reads headers without executing the parser.

The host restricts related traffic to the same capture, origin and conversation.
The extension retains at most 64 related exchanges and 8 MiB of body bytes,
prioritizes selected input and rejects stale live revisions. Named size and
structure limits preserve separate field allowances for selected input, its
paired request and earlier context. The host enforces execution and memory limits.

The ⓘ panel and Copy diagnostics expose parse issues and limits. Original retained
bytes remain available in Hex. Body views finalize only after complete input
parses without issues or clipping. Live updates are opt-in and unavailable for
imported captures.

## Qualification

Full reference checks require the .NET 10 SDK. After building, run these checks
from the repository root:

```text
npm run check
python scripts/test-mapi-reference.py
python scripts/generate-mapi-constants.py --check
python scripts/test-mapi-parity.py
python scripts/test-mapi-robustness.py
python scripts/test-mapi-visualizer.py
node --test extensions/mapi/test/*.test.mjs
python scripts/dev-env.py --run cargo test --locked -p transmog-extension-host --test mapi --test mapi_visualizer
```

The [Microsoft reference](../../third_party/mapi-inspector/README.md) is hash-pinned
and fetched into an ignored cache. Its adapters live under `test/reference`.
Only enum data is generated from C#; parser behavior is written in TypeScript.

Parity compares fresh oracle output with the committed corpus, then checks the
TypeScript decoder. Use `--golden-only` to compare against the committed corpus
without .NET.
Use `--write-reference` to save reviewed oracle results after the TypeScript
comparison succeeds.

Coverage includes HTTP and ROP dispatch, values and byte ranges, encodings,
compression, continuation state, malformed input and recovery in the same worker.
Production-bundle tests also check header isolation, HTTP decoding, live updates
and resource limits. Native tests cover sandbox execution, authenticated reads
and folder/package roundtrips. [Desktop acceptance](../../scripts/test-windows-desktop.ps1)
checks the rendered workflow with synthetic traffic, scoped context, source
highlighting, concurrent viewers and unloading.

Intentional corrections to reference behavior have explicit fixtures:

- Decoded RPC fields use decoded coordinates linked to encoded parents.
  Compression records enclosing provenance rather than an exact byte mapping.
- FILETIME preserves unsigned ticks and UTC with seven fractional digits.
- FastTransfer reassembles continued values, preserves chunk parents and
  reads each variable multivalue's declared length.
- MetaTagEcWarning preserves unsigned error bits in both directions.
- OptionsData reads ReturnValue before success fields. Valid success and failure
  cases cover the layout; padded reference probes remain invalid-input regressions.

See the [protocol regressions](test/protocol.test.mjs) and
[FastTransfer fixtures](test/fast_transfer.py) for the wire examples.

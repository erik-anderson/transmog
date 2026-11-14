# Authoring a binary visualizer

This bundled extension demonstrates API 1 body visualizers. The host supplies
declared inputs, reads retained bytes, decodes HTTP content and renders structured
documents. Extension code returns data; it has no DOM or network access. Start
with [the extension API guide](../../docs/extensions.md) for the manifest and host
contract. [Binary formats](../../docs/binary-visualizers.md) describes supported
formats and user-facing limits.

## Follow the BOND path

1. [bond-entry.ts](typescript/bond-entry.ts) exports `visualize`, `resume`,
   `resumeDecoded` and `expand`. It connects the parser to the host lifecycle and
   preserves incomplete-evidence status during expansion.
2. [runtime.ts](typescript/runtime.ts) requests bounded byte pages from the selected
   message side. It handles content decoding, continuation ownership and missing
   or incomplete evidence before calling the parser.
3. [bond-framing.ts](typescript/bond-framing.ts) distinguishes marshaled headers
   from raw structs and tries Compact Binary v1/v2. It keeps competing valid or
   limited interpretations explicit instead of guessing a version or framing.
4. [bond-inline.ts](typescript/bond-inline.ts) decodes small documents directly into
   API nodes with exact source spans. This path also supports field comparison.
5. When the input or materialized output is too large,
   [bond-index.ts](typescript/bond-index.ts) validates the wire structure and stores
   offsets in typed rows. Fixed-width lists keep offsets without a row per item.
   [bond-viewer.ts](typescript/bond-viewer.ts) projects those rows into bounded
   documents and deferred ranges. Range tokens belong to one visualization;
   opening a range replaces the previously opened path.

[bond-wire.ts](typescript/bond-wire.ts) names the published tags and encoding
layout. [bond-limits.ts](typescript/bond-limits.ts) names BOND's input, index and
display safeguards; [limits.ts](typescript/limits.ts) holds shared limits. There
is no per-operation work counter. Finite inputs, nesting and output bounds remain,
along with the host's worker limits. [unicode.ts](typescript/unicode.ts) validates
complete strings even when their display preview is clipped. Its preview-only
mode is reserved for bytes already validated by the index.

The shared [reader.ts](typescript/reader.ts) checks byte bounds and builds scalar
values and documents. Keep errors, partial evidence and display limits distinct:
reaching a limit cannot disprove an alternative interpretation. Treat captured
strings as inert values, preserve wide integers as decimal strings, and link
displayed values to their original or host-decoded byte ranges.

## Verify changes

From the repository root, with the development tools installed:

```powershell
python scripts/dev-env.py --check
npm ci --ignore-scripts
npm run check
python scripts/test-binary-visualizers.py
```

The binary test command builds the bundle, then exercises all five readers in
isolated JavaScript modules. Build output and source maps are generated under
`dist`; edit TypeScript rather than generated JavaScript.

Browser checks render production templates under Trusted Types:

```powershell
npm --prefix apps/desktop/ui run check
node apps/desktop/ui/scripts/check-workspaces.mjs --visualizers-only
```

On Windows, the focused acceptance check exercises the packaged modules, host
content decoding, deferred BOND ranges, field comparison and constrained layouts:

```powershell
./scripts/test-windows-desktop.ps1 -BinaryChecks
```

Close existing Transmog instances first. The script creates an isolated profile,
uses generated captures and a local reference service, and retains screenshots
and qualification results in its reported evidence directory.

# MAPI parser architecture

This is the authoring source for the bundled MAPI decoder. It is written in
strict TypeScript and compiles to ordinary ES2022 modules. The extension host
does not compile TypeScript, load .NET, or provide browser or network APIs.

The boundaries are deliberately small:

- `reader.ts` reads wire data with explicit bounds and shared work limits.
- `model.ts` describes protocol fields, values, source ranges and parse issues.
- Protocol modules interpret MAPI HTTP envelopes, ROPs, properties, address-book
  messages and FastTransfer streams. Conversation state belongs to one decode;
  there is no global session navigator or mutable protocol singleton.
- The extension adapter obtains only host-granted input and translates protocol
  fields to the host's existing visualization contract. The host owns the UI.
- The pinned Microsoft inspector is a developer-only behavioral reference.
  Synthetic inputs and reference outputs remain independent test evidence.

Do not transliterate the reference's parser class hierarchy or retain its UI
layout as a protocol contract. Compare wire facts, values, boundaries, operation
dispatch and continuation behavior. Presentation differences should be explicit
in the tests; unsupported input must be reported rather than presented as a
complete parse.

Keep the reader and field model local until another extension demonstrates a
shared need. A future authoring library can reuse them without adding host
capabilities or changing the sandbox contract.

Run `node extensions/mapi/build.mjs` from the repository root to compile with
the repository's pinned TypeScript dependency. Compiled modules are build
artifacts and are never checked in. Packaging compiles the project with the
pinned compiler. Tests under `test` exercise boundary conditions and the
independent reference corpus.

Source formatting uses Prettier 3.6.2 and the adjacent `.prettierrc.json`.
Protocol enum tables contain data only; `generate-mapi-constants.py` checks their
provenance against the pinned source. No parser behavior is generated from C#.

The differential runner compares wire values and ranges, not the inspector's UI
nodes. Known UI omissions are checked against explicit protocol field positions.
The old OptionsData response shape is invalid: the reference omits reading its
return code and the generated probe includes a large padding tail. It remains
an archived malformed-input regression; independent valid success/failure tests
verify the new parser's status and payload handling.

FastTransfer continuation uses bounded byte fragments per stream/handle. The
parser reassembles a complete wire element before decoding it, preserving all
chunk ranges as parents of a derived source. It displays a complete value rather
than the reference's final text fragment. The corpus checks the concatenated
bytes, decoded Unicode/binary/integer values and every parent range independently.
MetaTagEcWarning is an unsigned error code in both directions; the reference's
download path displays its bits as a signed integer. Per-element lengths for
variable multivalues follow the protocol's lexical grammar, independently of the
reference's null-terminated multivalue helper.
See Microsoft's [FastTransfer lexical structure](https://learn.microsoft.com/en-us/openspecs/exchange_server_protocols/ms-oxcfxics/bafa8ce0-ff31-4c36-b464-8e8852d22a10).

The production manifest selects the compiled body and header bundles. Explicit
synchronous export wrappers preserve API 1 handler names through minification.
The build rejects binary-parser dependencies in the header bundle. The separate
`testing.ts` reference bridge is never imported by production or packaged.
See [qualification and developer commands](../README.md).

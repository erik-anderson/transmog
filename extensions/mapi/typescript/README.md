# Reading and extending the MAPI visualizer

The MAPI visualizer is a strict TypeScript extension compiled to ES2022 modules.
It interprets protocol data and returns documents; Transmog supplies the UI,
granted inputs, sandbox and worker lifetime.

## Start with the extension contract

Read these files in order:

1. [manifest.json](../manifest.json) declares matching rules, inputs, entry points
   and host limits. Request and response viewers run in separate workers.
2. [headers.ts](headers.ts) is the smallest complete example: read granted headers
   and return a final tree document without loading the binary parser.
3. [body.ts](body.ts) exposes synchronous callbacks typed with the public
   [API 1 SDK](../../../crates/extensions/sdk/transmog-extensions.d.ts).
   Type-only imports add no runtime dependency.
4. [body-session.ts](body-session.ts) requests bytes, HTTP decoding or context
   through `needInput`, `needDecoded` and `needContext`. Resume callbacks validate
   continuation tokens; live updates reject stale revisions.
5. [body-input.ts](body-input.ts) retains granted pages, prioritizes the selected
   body and assembles the decoder snapshot.
6. [decode.ts](decode.ts) enters the parser. [view.ts](view.ts) translates its
   results into host documents and deferred expansion tokens.

Follow a Connect request from `visualize` through body acquisition to
`readHttpMessage` in [http.ts](http.ts). The Connect branch reads UserDn and
numeric fields in wire order. [record.ts](record.ts) records their values and
byte ranges; `present` converts them into a tree. Protocol code needs no access
to the UI or capture store.

## Add an operation

1. Add HTTP layouts in [http.ts](http.ts), or fixed ROP fields in
   [rop-layouts.ts](rop-layouts.ts), in wire order.
2. Add variable ROP payloads in [rop-requests.ts](rop-requests.ts) or
   [rop-responses.ts](rop-responses.ts). [rops.ts](rops.ts) dispatches and pairs
   operations. Use captured request parameters, not emitted field names, to
   interpret responses; `requestParameter` checks required parameters.
3. Use `nested` for structures, `bounded` for length-delimited regions and
   `repeat` for declared counts. Name flags, sentinels and meaningful sizes in
   [wire.ts](wire.ts) or beside the algorithm that owns them. Reuse generated enums.
4. Add fixtures with meaningful values and exact ranges. Cover malformed input,
   missing context and recovery in the same worker. Update manifest matching
   rules when adding an HTTP operation.
5. Run the [qualification checks](../README.md#qualification).

Use a message's own flags to choose its layout when available. Report required
but absent state as `missing-context`, and unknown layouts as `unsupported`.
Expected wire failures use `ParseError`; unexpected exceptions reach host
diagnostics and the debugger.

## Protocol boundaries

[reader.ts](reader.ts) owns bounds, encodings, signedness and exact 64-bit reads.
[model.ts](model.ts) describes fields, values, ranges and issues independently
of the host API. Use `scalarField` when a value also needs units or a timestamp.

[decoder-state.ts](decoder-state.ts) owns one decode snapshot, replaying earlier
exchanges and the selected response's paired request. [conversation.ts](conversation.ts)
retains logon flags, table columns and request queues. Context acquisition is
triggered by missing state or an actual FastTransfer stream payload.

[fast-transfer.ts](fast-transfer.ts) assembles complete elements from fragments
keyed by operation and handle. Derived sources retain every contributing chunk
as a parent. RPC XOR/LZ77 decoding also separates decoded offsets from encoded
parents; compression records enclosing provenance rather than an exact mapping.

## Safeguards and presentation

[limits.ts](limits.ts) names input, collection, nesting, field and derived data
limits. Context, paired-request and selected-message parsing have separate field
and depth limits. The parser has no work-unit accounting; the host enforces
execution deadlines, memory limits, capabilities and output validation.

[view-fields.ts](view-fields.ts) converts values and determines status.
[view-limits.ts](view-limits.ts) names display limits. `view.ts` preserves node IDs
and expanded sections across unrelated field additions without retaining older
decoded trees. It reduces oversized output in a loop that shrinks or terminates.

Clipped output reports Limited and discloses omitted diagnostics.
[text.ts](text.ts) clips text without splitting Unicode code points. Input-limit
diagnostics also report Limited. When context is unavailable, available fields
can still be shown. Only a complete body interpretation of complete input declares
`updates: "final"`. Original retained bytes remain available in Hex.

## Build and reference evidence

See [authoring and packaging](../README.md#authoring-and-packaging) for build
commands. The build checks that the header bundle has no binary-parser imports.
[testing.ts](testing.ts) is a developer-only bridge excluded from production entries
and packages.

[constants.ts](constants.ts) contains generated enum data, not parser behavior.
The pinned Microsoft inspector supplies independent wire-fact evidence.
[Qualification notes](../README.md#qualification) describe the checks and deliberate
corrections to reference behavior. Keep parser utilities local until another
extension demonstrates a shared need.

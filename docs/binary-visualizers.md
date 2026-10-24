# Schema-free binary body visualizers

Choose **Protobuf**, **Bond Compact Binary**, **MessagePack**, **CBOR** or **BSON**
in a request or response's **Body viewer**. The bundled **Binary formats** extension is managed in Settings →
Extensions and uses the ordinary isolated, read-only extension system. It reads
only the selected message side and needs no schema files.

Auto tries Protobuf for `application/protobuf`, `application/x-protobuf` and
`application/vnd.google.protobuf`, and Bond for `application/bond-compact-binary`.
Only a structurally complete result replaces the normal body representation.
Choose a viewer explicitly to inspect partial, malformed or limited results.
Discovery and traffic-list rendering do not execute these parsers. No generic
binary sniffing is performed.

Auto also recognizes MessagePack's `application/msgpack`, `application/x-msgpack`
and `application/vnd.msgpack`, CBOR's `application/cbor` and
`application/cbor-seq`, and BSON's `application/bson` and `application/x-bson`.
These readers show consecutive self-delimiting values/documents in wire order.
Map keys and duplicate occurrences remain visible. MessagePack extension types
are opaque except for the standard timestamp. CBOR tags are preserved without
claiming application semantics; indefinite strings retain their chunks. BSON
shows encoded names and specialized types, with Decimal128 preserved as raw bits.
JavaScript source and regular expressions remain inert text. MongoDB's TCP
protocol, SignalR and Socket.IO envelopes are outside the body viewers' scope.

Protobuf preserves field order and duplicate occurrences. Field numbers, wire
types, lengths and raw unsigned integers are wire evidence. Signed, ZigZag,
Boolean, float/double, UTF-8 and nested-message alternatives are labeled
**Possible**. A length-delimited value might instead be arbitrary bytes or packed
numbers. These viewers cannot recover field names, enum labels, default values,
map/oneof declarations or a root message's identity. Protobuf bodies are treated
as one message; concatenated message boundaries cannot be recovered without
external framing.

Payloads up to 4 KiB also offer **Possible packed** varint, ZigZag, fixed32,
fixed64 and floating-point arrays when their complete payload parses. Each array
previews its first 128 items and retains its full byte range. These alternatives
do not establish a repeated-field declaration or replace the raw payload.

Bond Compact Binary includes scalar and container types, so its view shows
strings, numbers, structs, base boundaries, lists, sets and maps by field ID.
Marshaled v1/v2 headers establish the protocol version. Raw bodies are tried as
both v1 and v2, with the chosen interpretation labeled **inferred**. If both parse
completely, both **Possible** versions appear in an explicitly selected viewer;
the result is partial and Auto falls back. Names, enum labels, and aliases such
as blobs or nullable values need a schema. Fast and Simple Binary are unsupported.
Protocol-header bytes that also parse as raw fields are explicitly ambiguous and
remain partial, so Auto cannot silently choose between the framings.
If a competing version or framing reaches a parser limit, it remains unresolved;
the result stays limited even if another interpretation completes.
Raw Bond bodies may contain consecutive structs. Each struct retains its own
byte range and wire order. A leading protocol header frames a single struct;
the header itself is not counted as a batch item. Both incomplete raw version
interpretations remain available when neither version parses completely.

All viewers request HTTP content decoding from the host, including its
raw-DEFLATE compatibility. Failed decoding is explained and never replaced with
an interpretation of compressed bytes. Fields link to exact byte ranges in the
original or host-decoded source. Incomplete retained evidence is partial even
when it ends at a valid field boundary. Earlier complete fields remain available
when later bytes are malformed; the remainder links to its original byte range.
Complete children of partial collections, groups and structs remain inspectable.
Speculative Protobuf message probes still require a complete parse.

Bond accepts selected bodies up to 16 MiB, with a compact index of up to 500,000
values and 64 levels of nesting. Large field, record and item collections load
in ranges through the shared tree. Opening another range replaces the previously
loaded range, keeping display memory bounded. Deep descendants appear flattened
in wire order with explicit nesting depths. Fixed-width lists retain payload
offsets without allocating a node per item. Large strings are validated in full,
with a bounded preview and access to their complete source bytes in Hex. Copy a
smaller range if a whole subtree cannot be materialized; copying never silently
omits unloaded children.
The host's HTTP decoding and expansion limits still apply to compressed bodies.

Other readers limit each selected decoded body to 2 MiB. Additional work, nesting, node,
collection and total text limits bound parsing and speculative probes. Protobuf
text/nested-message probes consider payloads up to 64 KiB. Text values preview
up to 1,024 UTF-16 code units; larger values remain available through their byte
ranges. Wide integers use exact decimal strings; nonfinite floating point values
use `NaN`/`Infinity` text, and negative zero uses `-0` text to preserve its sign
through JSON. Captured strings are inert. No related traffic or live
subscription is requested. **ⓘ** explains schema limitations and parser diagnostics.

gRPC framing/compression, schema imports, schema inference and message editing
are deferred. These visualizers do not add a dependency on the retired Bond
framework. Their independently implemented readers follow the
[Protobuf encoding specification](https://protobuf.dev/programming-guides/encoding/)
and Microsoft's [Compact Binary encoding reference](https://github.com/microsoft/bond/blob/master/cpp/inc/bond/protocol/compact_binary.h).

In **Compare requests and responses**, open **Compare structured body fields**,
choose a body and format, then **Compare fields**. This uses the comparison's
frozen complete bodies, including HTTP content decoding, up to 2 MiB per side.
Field comparison requires materialized reader output; larger deferred Bond
structures remain inspectable in their individual body viewers.
Completion feedback stays beside the action; results appear below without
moving focus.
The ordinary enabled extension readers parse each side in isolated workers;
native code then matches encoded map keys or field labels and ordered duplicate
occurrences. It reports changes in typed values or wire encoding, including
changes beyond displayed text previews. Incomplete/ambiguous parses never prove
that an unmatched field is absent. Findings are limited to the first 512; the
visible summary reports counts, parse coverage and additional findings. Each
finding starts with **Left** and **Right** values; expand **Evidence** for the
complete field path, change reason and source ranges. Source offsets refer to
decoded snapshot bodies, while **Original byte ranges** retains the frozen
original representation.

The native field-comparison producer and JavaScript readers share the same inert
structured-document contract and host UI: virtualized tree, keyboard navigation,
field details, table/text presentation and inspection copy menus. A native
snapshot has no extension instance, live feed or source-reading capability.
Tree navigation preserves the available reading width: arrow keys, Home and End
move selection, while Enter or a click opens details. Escape closes details;
continuing to navigate keeps them closed. Hex is offered only when the result
has a byte source; native comparisons retain **Original byte ranges** separately.
The UI does not parse binary formats. Workers close after parsing, and closing or
refreshing the comparison cancels obsolete work and rejects late results.

Build and verify with Python 3.11 or newer:

```text
python scripts/build-binary-extension.py
python scripts/test-binary-visualizers.py
```

Desktop builds generate and embed the ordinary `.tmext` bundle. The package
contains five readable ES2022 runtime modules and its runtime README; authoring
sources, tests and tools are excluded. No extra runtime or network access is
required. Parser qualification uses synthetic wire-format fixtures and malformed
inputs. Independent reference serializers are development-only dependencies;
none are shipped in the extension. Production service-capture qualification is
reported separately from synthetic and local-service fixtures.

Windows acceptance also exercises live localhost HTTP responses with gzip for
all five formats and compares retained, decoded bytes with the supplied payloads.
MessagePack, CBOR, BSON and Protobuf use independent encoders; Bond uses the
Microsoft reference wire example. This qualifies local transport and parser
integration without claiming coverage of production service traffic.

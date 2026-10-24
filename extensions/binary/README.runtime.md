# Binary formats

Select **Protobuf** or **Bond Compact Binary** in the request or response Body
viewer. Auto tries them for their declared binary content types and uses only
structurally complete results. Explicit selection also shows partial results.
No schemas are required. These viewers do not support gRPC framing, schema
imports, Bond Fast Binary or Bond Simple Binary.

Protobuf shows field numbers, wire types and raw unsigned numeric values,
preserving order and duplicate fields. Signed, ZigZag, Boolean, floating point,
UTF-8 and embedded-message views are labeled **Possible**. They are alternative
interpretations, not recovered schema types. Length-delimited data can also be
arbitrary bytes or packed numbers. Nested/text probes are limited to 64 KiB;
larger fields remain available as bytes. The body is interpreted as one message;
Protobuf itself cannot establish boundaries between concatenated messages.

Bond shows encoded field IDs and types, structs, base boundaries, lists, sets
and maps. Marshaled Compact Binary v1/v2 headers declare the version. Without a
header, both versions are tried and any chosen version is labeled as inferred.
If both parse completely, both possible interpretations are shown and Auto
keeps the ordinary body view. Field names, enum labels and schema aliases such
as blobs or nullable values cannot be recovered.
Header bytes that also parse as raw fields are labeled as ambiguous framing;
that result also remains partial.

Both viewers use the host's HTTP content decoding and preserve links to exact
source byte ranges. A decoding failure leaves original bytes available in Hex.
Each body is limited to 2 MiB, with separate work, nesting, node and text limits.
Text previews show at most 1,024 UTF-16 code units. Negative zero is displayed as
`-0` text so JSON cannot erase its sign. Missing, malformed, incomplete
and limited input is reported explicitly; a complete parse does not establish a
schema or prove that mislabeled traffic uses this format.
An interpretation that reaches a parser limit cannot rule out that version or
framing. Such results remain limited even if another interpretation completes.

Only the selected side is read. There is no network access, related-traffic
lookup or live subscription. The host owns the UI; captured text remains inert.
Manage the bundled **Protobuf and Bond** extension in Settings → Extensions.

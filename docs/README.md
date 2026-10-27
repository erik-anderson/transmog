# Transmog documentation

These guides describe the checked-in implementation. The [roadmap](roadmap.md)
contains remaining work; Git history holds completed plans and migration notes.

## Start here

- [Architecture](architecture.md), [dependencies](dependencies.md) and [embedding](embedding.md)
- [Windows desktop](product-shell.md) and [current limitations](limitations.md)
- [Build prerequisites](building.md), [testing](testing.md) and [Windows releases](windows-release.md)
- [UX principles](ux-principles.md) and [quality routine](ux-quality.md)

## Traffic and automation

- [Traffic inspection](traffic-inspection.md): headers, bodies, search, selection and replay.
- [Copying inspection values](copying-inspection-values.md): contextual field, subtree, row and report copying.
- [Auth inspection](auth-inspection.md): HTTP/OAuth/JOSE/SAML and Windows auth evidence, masking and interpretation limits.
- [Binary viewers](binary-visualizers.md): schema-free fields, source bytes and interpretation limits.
- [Format converter](format-converter.md): temporary text/byte conversions and independent scratch windows.
- [Extension contracts](extensions.md): sandboxed visualizers, declared inputs, host rendering, `.tmext` packaging, development reload and bundled MAPI/HTTP.
- [Request timings](request-timings.md): waterfalls and shared transport evidence.
- [Streams](stream-inspection.md): WebSocket/SSE inspection and retained messages.
- [Automation](automation.md): auto-responses, header overrides and scripts.
- [Safe previews](safe-previews.md) and [captured-page preview](captured-page-preview.md).
- [Product state and support](product-state-and-support.md): settings, retention, privacy and diagnostics.
- [Client process attribution](client-process-attribution.md).

## TLS and transports

- [Network profiles](network-profiles.md): settings, customization and activation.
- [Certificate model](certificate-model.md): interception CA and recorded evidence.
- [Root stores](root-stores.md): component discovery, local trust and Trust Anchor IDs.
- [Chromium verifier](chromium-verifier.md): path building, issuer retrieval, certificate policy, CT and revocation.
- [Chromium negotiation](chromium-negotiation.md): algorithm offers, GREASE, resumption and ECH.
- [H2/H3 ALPS](alps.md): settings, Client Hints and captured server contents.
- [Client certificates](client-certificates.md): authorization, Windows keys and selection.
- [Connection racing](networking.md) and [WebSocket transport](websocket-inspection.md).
- [Local Chromium comparison](chromium-comparison.md) and [remaining alignment](roadmap.md#chromium-transport-alignment).

## Captures and CLI

- [Save traces](trace-saving.md): formats, encryption, redaction and source metadata.
- [Native capture overview](native-capture-format.md) and [TMCap wire specification](tmcap-format.md).
- [SAZ compatibility](saz-compatibility.md) and [HAR/NetLog](json-archives.md).
- [CLI commands](cli.md) and [guided support capture](cli-support-capture.md).
- [Support capture profiles](support-capture-profiles.md): reproduction steps, issue markers and reviewed evidence export.
- [Portable investigations](portable-investigations.md): revision-0 case/rule APIs, ZIP64/TMCap transfer, encrypted desktop sharing and reviewed disabled import; CLI workflows remain planned.

## Library contracts and decisions

- [Interception lifecycle](interception-lifecycle.md): ordering, short circuits and failures.
- [Application/session service](application-session-service.md): catalog, capture, replay and host integration.
- [Architecture decisions](adr/README.md): accepted constraints and rationale.
- [Native SDK](../sdk/README.md), [C++ example](../examples/cpp-embedding/README.md) and [.NET example](../examples/dotnet-embedding/README.md).
- [Branding and identifiers](branding.md).

## Development and qualification

- [Desktop development](../apps/desktop/README.md) and [UI instructions](../apps/desktop/ui/AGENTS.md).
- [Performance and capture benchmarks](performance.md) and [browser load testing](load-testing.md).
- [Fuzzing](../fuzz/README.md), [hosted workflows](../ci/README.md), and [verification artifacts](../verification/README.md).
- [Transport patches](../third_party/rust-transport/README.md), [PKI snapshot](../third_party/boringssl-pki/README.md),
  [root data](../third_party/chromium-root-store/README.md), [certificate policy data](../third_party/chromium-certificate-policy/README.md)
  and [Public Suffix List](../third_party/chromium-suffix-list/README.md).

## Maintenance

Describe current behavior once in its topic guide and link from overviews.
Use the [product contract map](ux-principles.md#keep-product-contracts-in-their-topic-guides)
for feature ownership. Building owns setup, testing owns routine checks, Windows
release owns candidate qualification/publication, and CI owns hosted execution
and permissions. The native-capture guide explains storage and recovery; the
TMCap specification owns wire layouts and cryptographic parameters.

Keep architectural rationale in ADRs and unfinished work in the roadmap. Link
to source for field inventories, validation, version pins and generated data.
Remove completed plans after their lasting decisions are documented. Keep run
hashes, timings, screenshots and review transcripts in generated evidence or CI
history. Preserve upstream snapshot documents and license text; regenerate notices
through their collectors rather than editing generated inventories by hand.

# Transmog documentation

This directory describes the code that is currently checked in and the work
that is intentionally still ahead. Git history is the archive for completed
implementation plans and migration notes; they are not maintained as parallel
sources of truth.

## Start here

- [Architecture](architecture.md) explains the layers, dependency direction,
  transport model, and failure boundaries.
- [Key dependencies](dependencies.md) identifies the major protocol, crypto,
  application, and build libraries and explains where they are contained.
- [Product shell](product-shell.md) describes the Windows desktop behavior and
  its relationship to the reusable application facade.
- [UX design principles](ux-principles.md) guides interaction, density,
  alignment, accessibility, and review of user-visible changes.
- [Current limitations](limitations.md) states what the implementation does
  not support.
- [Roadmap](roadmap.md) contains only planned or deliberately deferred work.

## Library and protocol behavior

- [Embedding](embedding.md): construct the runtime or application/session
  service without the desktop shell.
- [Interception lifecycle](interception-lifecycle.md): interceptor ordering,
  short circuits, terminal delivery, and observer behavior.
- [Application/session service](application-session-service.md): live catalog
  retention, capture, interactive control, replay and host-integration seams.
- [Automation](automation.md): declarative rules, ordered auto-responses, and
  audit attribution.
- [WebSocket inspection](websocket-inspection.md): transparent and inspected
  upgrade paths.
- [Certificate and trust model](certificate-model.md): upstream verification
  and downstream interception certificates.
- [Origin connection racing](networking.md): bounded Happy Eyeballs behavior
  shared by TCP/TLS and QUIC origin transports.
- [Client process attribution](client-process-attribution.md): local process
  name/PID resolution and remote-client classification.

## Traffic, captures and support

- [Traffic inspection](traffic-inspection.md): header sizes, authentication,
  command copying, search, multiselection, removal and Composer replay.
- [Request timings](request-timings.md): latency interpretation, shared connection
  setup, waterfalls and transport counters.
- [Trace saving](trace-saving.md): merged captures, encryption, redaction and
  original trace metadata.
- [Native capture encoding](native-capture-format.md): chunk compression,
  authenticated frames, lazy body reads and recovery.
- [SAZ compatibility](saz-compatibility.md): archive interoperability and fidelity.
- [CLI support capture](cli-support-capture.md): start, reproduce, stop, review
  and share a trace; certificate ownership and cleanup.
- [Product state and support](product-state-and-support.md): persistent
  preferences, memory/disk circular retention, diagnostics and support bundles.
- [Safe response previews](safe-previews.md): inert workbench content and
  sandboxed image decoding.
- [Captured-page preview](captured-page-preview.md): reconstruct a page from
  captured resources in an isolated Windows browser window.
- [Branding and identifiers](branding.md)

## Build, test, and release

- [Build prerequisites](building.md)
- [Testing](testing.md)
- [Performance harness](performance.md)
- [Windows release and qualification](windows-release.md)
- [Desktop-specific development](../apps/desktop/README.md)
- [Content fuzzing](../fuzz/README.md)
- [Hosted release workflows and deferred automation](../ci/README.md)
- [Verification artifacts](../verification/README.md)

## Architecture decisions

Accepted decisions live in the [ADR index](adr/README.md). They explain
constraints that still shape the implementation; completed task checklists and
superseded API migrations do not belong there.

## Maintenance policy

- Describe shipped behavior in present tense in the relevant guide.
- Put durable architectural rationale in an ADR.
- Link to current source for field inventories, framing details, validation and
  version pins rather than maintaining parallel copies in prose.
- Document a behavior in its topic guide and link to it from overviews.
- Keep per-run timings, hashes, screenshots and qualification transcripts in
  generated evidence or CI run history, rather than maintained documentation.
- Put work that has not been implemented in [the roadmap](roadmap.md).
- Remove completed phase plans after their lasting behavior and decisions are
  represented by the current guides and ADRs.
- Do not use a plan, completion report, or dated status narrative as a second
  specification for code that already exists.

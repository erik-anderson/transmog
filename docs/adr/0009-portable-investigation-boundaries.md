# ADR 0009: Portable investigation boundaries

Status: accepted architecture; format revision 0 is an implementation design.

## Context

A trace preserves network evidence, while response assets, named experiments,
scripts and notes belong to other stores. Copying application state exposes
machine paths and activation state without establishing which bytes a prior run
used. A recipient needs explicit dependencies and source identity, including when
the same capture is imported twice or different captures reuse exchange IDs.

## Decision

1. Define a portable manifest and content-addressed objects separately from
   private desktop persistence, TMCap records, script IPC and live control. Format
   revision 0 is breakable during development. No migration or compatibility
   adapters are required, and this decision does not stabilize control revision 0.
2. Use a case directory for deliberate authoring and a ZIP64 Stored archive for
   transfer. Objects use SHA-256 names; manifests carry typed references, exact
   revisions, lineage, dependency state and authored observations. A rule
   collection uses the same packaging conventions with a smaller manifest.
   Native packages wrap that archive in TMCap for compression, with optional
   encryption. Native TMCap packages are the only supported transfer form;
   bare ZIP case/rule import and export have been removed to keep case sharing
   free of packaging choices. This does not change SAZ traffic compatibility.
3. Identify evidence by actual source-artifact bytes, the source key inside that
   artifact, original exchange identity and occurrence. Runtime import IDs,
   display request numbers, filenames and labels are not portable keys.
4. Opening, validating, matching or importing never activates automation or
   executes scripts. Import stages and validates the whole selected dependency
   closure, remaps identities, and publishes disabled content atomically. Existing
   drafts, rules, global gates and admitted requests retain their state.
5. Freeze external file bytes explicitly. Distinguish an export-time snapshot
   from retained bytes of a historical run. Missing, incomplete, redacted, locked
   and unsupported evidence never becomes an empty or runnable replacement.
6. Compress the complete ZIP byte stream using TMCap's indexed chunks. When
   encryption is selected, protect its manifest and member names with TMCap's
   existing authenticated encryption. Reuse the container layer through a typed,
   seekable artifact payload; do not disguise an archive as HTTP traffic or encode
   it into JSON.
   A mandatory document-kind discriminator distinguishes captures from portable
   archives. K3 must implement and qualify this extension, authenticated
   completion, bounded random access, failure cleanup and clean-machine
   distribution before shipping encryption. No new encryption protocol or
   compatibility reader is introduced. SAZ member encryption stays separate.
7. Keep CLI result schemas separate from desktop command DTOs. New read-only
   commands use the same semantic validators and matchers, with explicit unknown
   states and no network, routing, trust-store or activation side effects.
8. Include case-level configuration/diagnostic snapshots as typed immutable
   context objects, including capture-time context in the new saved format.
   Preserve the original machine, time, purpose and capture association, not the
   current `trace-metadata` / `trace-source` storage layout. Update writers,
   readers and recovery together without legacy readers or dual writes. Opening
   a case never runs the recorded tools. Review context privacy with traffic.

The [portable investigation design](../portable-investigations.md) owns the
revision-0 fields, limits, CLI grammar and staged implementation boundaries.

## Consequences

Large evidence remains streamed and can be inspected without an author's source
checkout or application-state directory. Integrity hashes identify bytes rather
than publishers; redaction or re-encoding creates a new artifact with lineage.
An encrypted source capture can remain encrypted inside a case and retain its
own input-password requirement. Working directories and explicit unpacked copies
are plaintext; package encryption is not an erasure promise for working state.

The wrapper adds a second index and framing overhead, but leaves the inner ZIP
unchanged and avoids a second encryption dependency. The existing implementation
does not yet support this generic payload. K0 contains design fixtures, not a
production case reader or an encryption claim. This records the maintainer's
choice of ZIP64 inside TMCap over a separate outer encryption format.

## Alternatives

- Exporting private workspace JSON couples recipients to local paths, generation
  counters and live activation machinery.
- ZIP member encryption as currently used for SAZ leaves filenames and the
  archive directory exposed. Encrypting the complete ZIP protects all members
  under one package password.
- A separate age envelope would preserve ZIP interoperability but adds another
  encryption implementation and distribution/qualification path. Reuse TMCap's
  existing implementation while reviewing the new artifact path independently.
- Storing individual case objects and manifests directly in TMCap avoids nested
  indexes and could suit future incremental persistence, but couples case
  storage to native records and loses the standard inner archive. Keep authored
  case semantics in the portable manifest and capture bytes as opaque objects.
- A project database, remote service and a stable live-control API are not
  prerequisites for transferring a small local investigation.

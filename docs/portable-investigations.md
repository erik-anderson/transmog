# Portable investigations: revision 0

Status: K0 defines the contract and corpus; K1 implements rule/asset interchange;
K2 adds directory working cases, inventory and evidence inspection through shared
libraries and UI-neutral application APIs. Desktop file actions, archive/encryption
readers and CLI commands remain planned.
[ADR 0009](adr/0009-portable-investigation-boundaries.md) records the architecture;
[the roadmap](roadmap.md) tracks remaining implementation.

## Support policy

Revision 0 is experimental and may change without migration. A consumer supports
only the current documented revision-0 layout and declared capabilities; it
rejects other schema versions rather than guessing a downgrade. Refresh design
fixtures when the contract changes. No TMCap migration or earlier-layout adapter
is part of this work. A supported public v1 and its support window require a
separate recorded decision before release; neither this design nor additional
consumers establishes such a promise. CLI result revision 0 follows the same
experimental policy. Private persistence, script API and live control retain
their own policies; control revision 0 remains outside this decision.

## Current foundation

The audited source is the response-editing closeout on `0df7fcf` (0.1.4).
Revalidate these boundaries when implementing a consumer:

| Area | Reuse and portability boundary |
| --- | --- |
| Automation | [Private workspace](../crates/app/src/automation.rs) schema 1, 4 MiB, validated generations and atomic activation; [named variants](../crates/app/src/experiments.rs) retain exact ordered revisions. Export intent and selected revisions, not generations, Undo state or active hook registrations. |
| Response assets | [Asset storage](../crates/app/src/response_assets.rs) schema 1; explicit copy/link up to 1 GiB. Stored hashes identify encoded entities; linked configuration does not identify historical bytes. Omit absolute paths and copied-from paths. |
| Match and action semantics | [Native compiler](../crates/proxy-automation/src/lib.rs), [URL matcher](../crates/proxy-automation/src/matching.rs) and [literal operation](../crates/proxy-automation/src/text_transform.rs) remain authoritative. Translate explicit portable fields; do not deserialize private rule enums as the interchange contract. |
| Capture provenance | [Save trace](../crates/app/src/trace_save.rs) carries original source/entry associations, body boundaries, availability, actual hook outcomes and streams. [TMCap](tmcap-format.md) remains an independently versioned, experimental evidence format. No migration code is added. |
| Comparison | [Runtime subjects](../crates/app/src/comparison.rs) contain session, draft, asset and preview handles. Export stable evidence references and options, not handles. Preserve unavailable/incomplete/not-compared states. |
| Scripts | [API revision 1](../crates/proxy-script/src/lib.rs) is unstable. Save draft, activation and optional sandbox tests are distinct. Export source, declared handlers/capabilities/limits and asset references, never tokens, compiled state or cached validation authority. |
| Privacy | [Export projection](../crates/app/src/export_privacy.rs) handles selected header/trailer redaction and raw-head omission. It does not sanitize arbitrary URLs, bodies, scripts or notes. |
| Configuration/context | [Network collector](../crates/network/src/context.rs) captures bounded, opt-in tool output with collection time, purpose, platform and notes. [Trace save](../crates/app/src/trace_save.rs) currently distinguishes original source context from the saving machine's new context in `trace-metadata` / `trace-source`. Preserve that provenance in the shared context object model, without requiring this existing record layout. |
| Distribution | [Release packaging](../scripts/package-signed-windows-release.ps1) publishes a separate signed CLI; the desktop installer includes script/preview helpers. It does not bundle the CLI. Script declarations and a standalone CLI script-host distribution are not currently shipped contracts. |
| CLI | [Dispatcher](../crates/proxy-cli/src/main.rs) has profile/record/serve/roots/CA/capture commands. Existing inspect output is KEY=VALUE and errors exit 2. New JSON/results must be explicit additions. [Password input](../crates/proxy-cli/src/passwords.rs) is masked or a protected UTF-8 file, never command-line password text. |

## Package and working-copy decisions

A case directory contains `manifest.json` and regular files at
`objects/sha256/<64 lowercase hex digits>`. The digest is SHA-256 of the exact
object bytes. Paths are derived from digests, never supplied filesystem locators.
There are no absolute paths, URLs to fetch, links, scripts to run or environment
variable substitutions in dependency resolution.

Use `.tmcase` for a bare ZIP/ZIP64 interchange export of a case directory and
`.tmrules` for a rule collection. Writers use Stored members so ZIP entries remain
seekable and TMCap owns package compression. Native captures and many encoded
response assets already compress internally. Readers reject other ZIP compression,
member encryption, directories, duplicate names, unsafe names, reparse points,
symlinks, inconsistent local/central headers, overlapping entries and trailing
archive data. Standard streaming data descriptors and ZIP64 size fields are
allowed: their local placeholders must agree with the descriptor and central
directory under ZIP's rules, rather than being rejected as mismatches. Only
`manifest.json` and declared object members are allowed.
Nested archive-looking asset bytes are ordinary response content, never extracted.

Native transfers default to `.tmcase.tmcap` or `.tmrules.tmcap`: the exact ZIP byte
stream stored as one logical artifact in a compressed TMCap container, with
optional password encryption. Compression does not require encryption. The inner
ZIP is unchanged by wrapping/unwrapping and remains usable by standard archive
tools after explicit unwrap/decryption. The portable manifest does not depend on
the compression/encryption mode. Do not add ZIP member encryption or a second
encryption format. Bare ZIP is an explicit interchange option and leaves its
Stored members uncompressed; it is not the native compression path.

When encryption is enabled, reuse
[TMCap's encryption](tmcap-format.md#encryption-and-integrity): AES-256-GCM,
Argon2id with 64 MiB, three iterations and one lane, domain-separated block keys,
fresh salt/nonces, authenticated indexes and sealing. Reject unsupported KDF
settings before password work; serialize password derivations. The public header
retains its 4 KiB bound. Exposed information includes the TMCap file/document
type, encryption parameters, salt/nonces, ciphertext size, external filename and
filesystem metadata. The case manifest, member names and artifact descriptor
are encrypted. No plaintext title, source list or notes are copied outside.

This requires a **new generic artifact path**, not an existing capture feature:

- Share the file/block/encryption layer below capture-record semantics. Define
  a mandatory public document-kind discriminator for captures versus
  `portable-archive`, authenticated with the rest of the header. Unsupported
  document kinds fail before a capture importer can treat a case as empty
  traffic. K3 specifies the wire extension and updates all current consumers and
  fixtures together, without migration readers or earlier-layout adapters.
- Store one typed logical ZIP payload with media type (case/rules),
  checked unsigned-64-bit length, SHA-256 and a bounded chunk index. Reuse chunks
  of at most 128 KiB and existing integrity checks; encrypt the descriptor with
  the payload when encryption is selected. Do not invent HTTP exchanges,
  embed Base64 archives in metadata, or allocate the entire ZIP as one blob.
- Provide streaming writing and a logical `Read + Seek` view of the exact ZIP
  bytes. ZIP creation must stream directly into the wrapper; inspection must
  verify requested chunks (including authentication when encrypted) before
  exposing them, without a persisted decrypted temporary archive. Bound the ZIP
  directory before library indexing.
- Emit Stored ZIP members; let the outer layer's existing adaptive compression
  handle compressible chunks and bypass ineffective work. Do not decompress and
  rebuild an existing ZIP to wrap it. K3 measures nested framing/index overhead
  and read amplification on large archives and already compressed evidence.
- Require a sealed outer artifact and complete inner ZIP for a valid transfer.
  TMCap checkpoint recovery does not reconstruct a missing ZIP central directory.
  Publish only after completion/integrity checks, authentication when encrypted,
  exact length/hash verification and package validation. Re-encrypting creates
  different outer bytes; inner object hashes and portable source identities do
  not change when their bytes do not.

This choice avoids a new encryption dependency but does not establish correctness
of the new composition. K3 must qualify the generic payload, independent ZIP
readability after unwrap, corruption/truncation detection, password handling,
bounded random access, cancellation and clean-machine distribution. K0 ships no
encrypted case writer/reader and does not change today's TMCap wire format.

### Compression and performance qualification

Keep compression at the TMCap layer for native packages, with independent
level-1 zstd chunks and existing raw bypass. Stored ZIP members allow direct
seeking into an embedded native capture through the logical ZIP view; the
installed ZIP reader's seek API rejects compressed members. Deflating a whole
capture member would require a separate decompression/spooling strategy before
its internal indexes could serve bounded random reads. Double compression also
spends work on captures and encoded response bodies that already compress
internally. Do not apply HTTP paired-boundary deduplication to an archive.

The generic writer must reset/re-evaluate compression at member boundaries, or
use equivalent bounded sampling that prevents an incompressible capture from
suppressing compression of subsequent text/configuration members. Boundary hints
must preserve the exact ZIP bytes and must not force padded physical blocks for
every small member. Measure both whole-stream and boundary-aware policies in K3.

Before shipping, compare Stored ZIP + TMCap against ZIP Deflate with outer
compression disabled on the same synthetic and representative workloads: large
native/encrypted captures, encoded assets, large plain text, small config/notes,
and mixed member order. Record package size, pack/validate/open times, random
entry/body latency, CPU and peak memory; report password derivation separately.
Current capture benchmarks qualify the existing codec, not this future wrapper.
No package-level throughput or size advantage is claimed by K0.

Unencrypted TMCap uses its existing corruption checks, not authenticated
encryption; it provides no confidentiality or publisher identity.

### Opening and working copies

Read-only opening pins the source and uses integrity-checked bounded reads,
authenticated when encrypted; it must not silently persist a decrypted working
copy. Validate authenticated completion when encrypted, and all object hashes
before publishing an imported case. Explicit **Save working copy** / `case unpack`
creates a plaintext directory after disclosure. Source
movement/deletion either leaves pinned reads valid or produces an explicit
unavailable source; it never resolves a new file by its old name.

Working copies have one current manifest and immutable objects. Save stages,
flushes and atomically replaces the manifest; unreferenced staged objects are
recoverable garbage, not a completed revision. Start with 32 explicit authored
manifest revisions, reject further history saves at the limit, and offer explicit
pruning. Do not silently discard evidence. Keep working manifest history under
`history/<revision>/manifest.json` and retain objects referenced by any retained
revision. Transfer includes one selected manifest, its selected experiment
revisions and referenced objects; working manifest history and recovery files
are excluded.

## Manifest model

UTF-8 JSON, no BOM, duplicate keys, non-finite numbers or unknown top-level fields.
Required fields and a concrete example live in the [fixture corpus](../verification/fixtures/portable/README.md).
This is a typed design, not the private persistence DTO or a stable public v1.
Known nested types also reject unknown semantic fields; bounded namespaced
`extensions` are the explicit preservation mechanism. An optional unsupported
experiment can be retained opaquely within the manifest bound, but its unknown
fields are never translated into rules or treated as supported configuration.

| Field | Revision-0 meaning |
| --- | --- |
| `format` | `transmog-investigation`; `schemaVersion` is numeric `0`. |
| `kind` | `case` or `rules`. Rules packages omit evidence, notes, context and runs, not rule dependencies. |
| `id`, `revision` | Explicit portable identity and immutable revision; revision is a canonical positive decimal string. |
| `title`, `createdAt`, `producer` | Authored title, UTC RFC3339 creation time and producer name/version. Descriptive data is not captured evidence. |
| `requiredFeatures` | Package-wide required semantic identifiers. Unknown requirements prevent opening/import before mutation. |
| `objects` | Unique SHA-256, decimal byte length and role (`capture`, `response-body`, `script-source`, `context`). All present members must match length/hash. |
| `sources` | Capture object references, internal source keys, original/derived lineage and explicit loss/privacy context. |
| `assets` | Exact asset ID/revision, status, ordered headers, encoded entity object and provenance/availability. |
| `rules` | Optional ordered ordinary rules, separate from named experiment ownership. Defaults to an empty array. |
| `experiments` | Named variants and ordered immutable rule revisions with selected saved definitions and enabled/paused intent. |
| `notes` | Authored text/reproduction steps and optional stable evidence references. |
| `contexts` | Optional case-only inventory of immutable capture-time and separately collected diagnostic objects, with original source association, collection origin, completeness and explicit failures/omissions. |
| `runs` | Frozen experiment selection, actual retained response references/hashes, engine/script choice and authored outcomes with provenance. |
| `extensions` | Optional namespaced descriptive JSON; bounded and preserved without executing or using it for matching. |

Portable IDs use `[a-z0-9][a-z0-9._-]{0,127}` and are scoped to their owning graph.
Labels may contain Unicode and duplicates. Revisions, byte lengths, ordinals,
timing counters and large numeric identities use canonical decimal strings to
avoid JavaScript precision loss. SHA-256 is exactly 64 lowercase hex digits.
Object digests describe integrity/content identity, not publisher authenticity.
The manifest has no self-hash; its exact stored bytes may be hashed externally.
Within an entity's scope, repeated ID/revision pairs must have identical
definitions. Identity remapping must remain consistent across all selected
variant revisions and asset references. Known named experiments use
`kind="response-variants"`; requirements on an optional experiment can make
that experiment unavailable without invalidating known evidence.

A source reference is `(artifactSha256, sourceKey, exchangeId, occurrence)` plus
the requested message boundary. Hash the actual input artifact including its
encryption/header bytes. `sourceKey="root"` names a standalone artifact without
embedded sources; otherwise preserve its exact embedded source key. Native
exchange IDs use 32 lowercase hex digits; imported non-native original IDs remain
opaque strings and absent original IDs use JSON `null`, distinct from an empty
string. `occurrence` is the zero-based source-local occurrence of that original
ID (or of `null`), including duplicates. Runtime trace IDs and the Traffic
`#` are never join keys. Reimporting the same bytes reuses a portable source
reference; different bytes with the same filename/IDs remain different sources.

Redaction, re-encoding, decryption into a new artifact or selected-entry export
creates a new artifact digest. Record parent digests and explicit derivation
operations. If the original artifact was omitted, keep the parent reference
unavailable rather than pretending the derived object is the original. A derived
file's internal associations and its parent entry mapping remain explicit.

## Rule, asset and dependency semantics

Every rule has an ID/revision, label, kind, evaluation boundary, authored enabled
intent, match definition and typed action. Variants carry ordered rule references.
Auto-responses use array order and first-enabled-match precedence. General header
and literal rules carry an explicit signed numeric priority; equal-priority
overlapping writes use the production compiler's conflict rejection. Preserve
the original rule ID as provenance while allocating/remapping local import IDs.

| Rule kind | Evaluation and action |
| --- | --- |
| `auto-response` | Original client method/URL/headers; `original-client-request`; exact asset ID/revision. |
| `header-edit` | Request or response head at its native position; `effective-request-head` or `effective-response-head`; ordered set/append/remove operations. |
| `literal-response` | `final-effective-response-heads`, fresh-origin response body; UTF-8 literal Find/Replacement, First/All, optional required total count, exact media type and decoded input/output bound. |

This distinction is essential: a captured original request cannot establish
effective-head predicates after earlier native/script edits. `rules test` must
report **unknown** when it lacks that boundary, and must not execute other hooks
to manufacture an answer. It evaluates matches/precedence, not response bodies
or proof of browser behavior. Unavailable is different from no match.

The portable matcher names the method (`null` means Any), address kind and scope,
query mode, and ordered request/response header predicates. Exact URLs normalize
implicit ports as desktop does. Address patterns retain literal scheme/host,
segment placeholders, digit/rest forms and case choice. Query modes are exact,
required decoded pairs (including duplicates), or ignore. Regex declares URL/path
scope, whole/submatch and case choice, using the current bounded Rust matcher.
Status/status-class predicates apply only to response phases. Examples contain
explicit sample request fields and expected match decisions; they never influence
matching. K1 translates explicit portable DTOs through the existing validators,
without expanding which rule kinds the product supports.

Header names are ASCII tokens with original casing preserved. Header values use
canonical padded RFC4648 Base64, including empty bytes. Arrays preserve duplicate
names and order; no JSON dictionary conversion. Predicates use exists/exact/
prefix/suffix/byte-regex. General header edits retain protected-field rejection.
Do not silently omit redacted predicate values: mark that rule unavailable.

Assets contain final status, ordered fields and exact **encoded entity** bytes,
with their coding stack reflected in headers. SHA-256 identifies stored encoded
bytes. Decoded hashes/text comparisons, if recorded, have their own representation
labels. K1 repairs framing through existing response-asset services,
not by treating a bounded editor preview as complete content.

Dependencies carry `state`: `present`, `missing`, `not-exported`, `unsupported` or
`locked`, and a reason when unavailable. Evidence completeness separately records
complete/truncated/redacted/evicted/not-retained/unknown; a present capture file
can still have incomplete evidence. A missing asset is inspectable but prevents
that experiment running. An absent body is not an empty object. Unknown optional
experiment types remain inspectable as metadata with an unavailable result; they
cannot be imported or executed. Unknown package-wide required features fail early.

Linked files are exported as an explicitly acquired snapshot or an unavailable
dependency. Do not export their absolute path, reopen an author's path on import,
or claim an export-time copy was served earlier. A historical run needs retained
boundary bytes matching its actual digest. Freeze failure aborts the selected
experiment export; inspection-only export must explicitly report omissions.

Scripts reference inert UTF-8 source objects, declared API revision, handlers,
capabilities, limits and asset references. Imported scripts stay disabled drafts.
Validation does not execute; explicit sandbox tests/activation use current backend
validation and host startup. K5 must qualify helper discovery/versioning and ship
declarations before advertising standalone script test commands.

## Implemented rule interchange APIs (K1)

[`transmog-portable`](../crates/portable/src/lib.rs) owns independent public
[DTOs](../crates/portable/src/model.rs), strict JSON parsing, native semantic
validation and verified object snapshots. It currently accepts only `kind="rules"`.
Case/evidence/script semantics and unavailable dependencies cannot be imported by
this reader. Unsupported versions, required features, action kinds and action
phases fail explicitly; unknown fields and duplicate keys are invalid.

`Collection::parse` validates metadata and the declared dependency graph without
opening objects. `Collection::encode` applies the same validation and an 8 MiB
output bound. `PreparedCollection::prepare` additionally reads each declared
digest from an explicit caller-supplied local provider into owned temporary
files. It checks exact length and SHA-256 with 128 KiB buffers, syncs files and
removes its staging directory on drop or failure. `manifest()` and
`open_object(digest)` expose only that frozen closure. There is no archive or
directory reader here: a future file adapter must enforce its own safe
name/member/source-access policy before supplying readers. The manifest never
causes a URL, linked file path or source-machine path to be opened.

The [Application facade](../crates/app/src/rule_interchange.rs) exposes:

| API | Result and publication boundary |
| --- | --- |
| `prepare_rule_import(manifest, staging_parent, open, canceled)` | Freezes and verifies the complete collection, then returns an opaque `PreparedRuleImport` with a mutation-free `review()`. |
| `import_rule_collection(prepared, canceled)` | Requires the same workspace and unchanged generation; publishes fresh disabled local authoring content and assets in one automation commit. |
| `export_rule_collection(request, staging_parent, canceled)` | Returns a `PreparedCollection` containing only selected saved rules/revisions and their verified asset closure; writes no transfer package. |

`RuleExportRequest` supplies a collection ID/revision/title, ordinary rule IDs
and exact `ExperimentSelection` values. Selection order does not change saved
rule precedence. Named rules must be selected through their saved variant,
including archived history; projected live rules, drafts and candidate handles
are not export selections. Empty Baseline is included automatically. Unsupported
advanced/mixed rules fail the whole selection instead of disappearing.

Export retains complete encoded entities, ordered duplicate/raw header values
and exact asset revisions. Linked assets become coherent export-time snapshots,
including the configured content-coding stack, labeled `export-snapshot` with
`acquiredAt`. Their paths are excluded. Captured-response assets without a joined
source artifact carry `sourceUnavailableReason`; runtime exchange IDs are never
substituted for evidence references. The descriptive `transmog.rule-origins`
extension retains original imported rule IDs for selected rules only. It does
not affect matching or disclose origins of unselected content.

Review classifies entries as `new`, `identical-source-definition` or
`conflicting` against saved identities and retained import origins. All receive
fresh local identities, including identical definitions; immutable body blobs
deduplicate by digest. Existing content is never overwritten by label or ID.
Original definitions, exact revision strings and mappings persist as provenance.
Ordinary imported rules are disabled; imported named experiments are paused,
unselected and inspectable. Source enabled/paused/archived/active intent remains
metadata. Local variant histories start at revision 1 while exact original
revision numbers remain in the retained source definition. Named saved rule
intent remains usable only after a separate explicit activation.

Imported asset metadata belongs to the automation document, with one persisted
transaction and one in-memory generation shared with the rules. A complete
flushed temporary document replaces its recovery slot before publication; failed
writes cannot truncate an older slot. Imported metadata is not copied
into the independent authored-asset index. Startup validates this catalog and
its source mapping before accepting a saved generation. A failed/canceled import
leaves existing gates, registrations, rules and drafts intact; saved candidates
remain available but require revalidation against the changed generation after a
successful import. Unreferenced immutable body blobs may remain after a failed
commit; they are not visible assets or a partially imported collection.

Application errors distinguish `Unsupported`, `Canceled`, `InvalidInput`,
`Conflict` and unavailable I/O. K1 does not change desktop commands, window roles,
activation controls, file dialogs or CLI output. It adds no TMCap compatibility
or migration code.

### Current wire fields and limits

All known objects reject unknown fields. Optional matcher fields are absent or
`null`; omitted predicate/example arrays are empty. These explicit wire fields
are separate from private Rust rule serialization:

| Object | Fields and supported values |
| --- | --- |
| Matcher | `method`, `address`, optional `scheme`, `host`, `port`, `pathPrefix`, raw `query`, `requestHeaders`, `responseHeaders`, `responseStatus`, `responseStatusClass`, `examples`. Native phase restrictions apply. |
| Address | `{kind:"exact",value:URL}`, `{kind:"pattern",value:{address,query,caseSensitive}}`, or `{kind:"regex",value:{pattern,scope,whole,caseSensitive,query}}`; regex scope is `url` or `path`. |
| Query | `{kind:"exact",value:STRING_OR_NULL}`, `{kind:"parameters",value:[{name,value}]}`, or `{kind:"ignore"}`. Required decoded pairs preserve duplicates. |
| Predicate | `name`, `kind`; `exists` has no payload, `exact`/`prefix`/`suffix` use `valueBase64`, `byte-regex` uses `pattern`. |
| Example | `method`, `url`, ordered `headers` and `expected` (`match`/`no-match`). Native sample headers currently require UTF-8; binary sample values fail as unsupported. Binary predicates and asset fields remain supported. |
| Rule | `id`, decimal `revision`, `label`, `kind`, `evaluation`, `enabledIntent`, `match`, typed `action`; general mutations require signed numeric `priority`. Auto-response priorities, if supplied, must agree with array order. |
| Auto-response action | `assetId`, decimal `revision`. |
| Header action | `operations:[{operation,name,valueBase64}]`; `operation` is `set`/`append`/`remove`; remove has no value. |
| Literal action | `find`, `replacement`, `occurrences` (`first`/`all`), nullable decimal `expectedMatches`, `mediaType`, `encoding`, decimal `maxBytes`; current native UTF-8 and media/body bounds apply. |
| Experiment | `id`, `kind:"response-variants"`, `requiredFeatures`, `label`, `pausedIntent`, optional `archivedIntent` (false), optional `activeIntent:{variantId,revision}`, `variants`. Active intent must reference an included revision. |
| Variant/revision | Variant `id`, `label`, optional `archivedIntent` (false), ordered `revisions:[{revision,rules}]`. An empty `baseline` variant is mandatory. |

K1 recognizes `response-assets/1`, `header-edits/1` and `literal-response/1`.
Namespaced extension keys use the portable ID alphabet, contain a dot and are at
most 128 bytes. All nested JSON shares depth 32 and the 8 MiB manifest bound.
Assets/objects are bounded to 16,384 each, bodies to 1 GiB, headers to 256 fields
and 64 KiB, examples to 64 per rule, experiments to 32, variants to 16 per
experiment, saved revisions to 32 per variant, and rules to 64 per saved revision.
Ordinary compilation units retain the native 1,024-rule and action/regex limits.

The private application workspace still has its 4 MiB persistence limit and
allows at most 64 retained imported collections. Combined native workspace
limits also apply. A valid larger transfer can therefore fail import atomically
without altering existing content; transfer validity is not a promise of available
workspace capacity. Later archive/working-copy APIs have their own qualification.

## Implemented working-case APIs (K2)

[`CaseManifest`](../crates/portable/src/case_model.rs) defines case-only source,
note, marker, context, lineage and run DTOs. `parse` / `encode` apply strict JSON,
identity/reference and native rule validation within the existing 8 MiB/depth-32
bound. The production reader exercises the case corpus, including unavailable
assets, locked native sources, duplicate source labels/IDs, optional future
experiments and different historical/export-time response bytes.

`inventory()` reports selected object bytes, source completeness, missing/locked
dependencies and each experiment's availability. `ready-for-explicit-import`
means supported rules and complete declared asset dependencies; it does not mean
a browser run has occurred. Unsupported optional experiments remain opaque JSON
and inspectable as unavailable. Unknown required package features fail early.
`rules_for_import(ids, ordinary)` selects only supported, complete dependencies
for K1's separate reviewed disabled import. Opening or saving a case does not
copy automation into the workbench or activate anything.

`PreparedCase::prepare` snapshots declared objects from an explicit inert local
provider, verifying exact lengths/hashes with 128 KiB chunks. Captures have no new
small byte ceiling; response bodies retain 1 GiB and context objects 8 MiB limits.
JSON context is parsed within the same depth/size limits; text stays UTF-8 data.
The owned snapshot and its read handles survive source movement/deletion.
Passwords, compiled script state and active hook registrations are not case data.

[`WorkingCase`](../crates/portable/src/working.rs) provides:

| API | Current behavior |
| --- | --- |
| `create(prepared, path, canceled)` | Create-new plaintext directory; existing paths are preserved. A manifest is published only after complete verified objects are present. |
| `open(path, staging, canceled)` | Read-only verified current snapshot; no collection, script execution or application-state import. |
| `save(next, canceled)` | Same case ID and next exact revision, exclusive writer lock and unchanged current-manifest hash; stage/flush objects and history before atomically replacing `manifest.json`. |
| `history()` / `open_revision(revision, staging, canceled)` | List retained prior revisions and explicitly verify/open their object closure. Current inventory does not imply every historical object's integrity has been checked. |
| `prune_history(revisions)` | Explicit removal of selected old manifests; current evidence and immutable objects remain. Recovery data must be inspected before pruning a directory that contains it. |
| `recovery_artifacts()` | Report interrupted owned staging without loading it as the current case or silently deleting it. |

At most 32 committed manifest revisions, including current, are retained. Saving
at the limit fails before publication until the caller explicitly prunes history.
An interrupted save/prune can leave immutable unreferenced objects, temporary
files or an empty/current-revision history directory. The valid current manifest
remains authoritative. Conflicting pending history is preserved for inspection
and explicit repair; retry can reuse an identical completed history snapshot.
Failures preserve existing current content and destinations. The filesystem
adapter rejects links/reparse points, unexpected member names and excessive
metadata/staging counts. No archive extraction is implemented by this adapter.

The [Application case APIs](../crates/app/src/investigations.rs) are:

| API | Result |
| --- | --- |
| `prepare_investigation(request, staging, canceled)` | Frozen selected rules/assets and an optional reviewed native evidence selection, source-linked authored notes, original context objects and incident-marker inventory. |
| `open_investigation_evidence(case, source_id, password, canceled)` | Existing inert Traffic/source inspection with stable-reference-to-runtime navigation links. It shares native importer limits and atomic publication. |

`CaseExportRequest.rules` supplies exact saved selections through K1;
`evidence_review_id` is an existing frozen Traffic export review. Omitted evidence
includes no Traffic. Selected workspace evidence requires explicit
`derived_plaintext=true`: it creates a new plaintext derived native artifact,
including when an earlier source was encrypted. To preserve an encrypted source
as-is, supply its exact encrypted bytes through the case object provider instead.
Working directories are plaintext; archive-wide encryption belongs to K3.

Selected evidence uses the production native session writer: retained boundary
bodies, original entry IDs, raw fields, stream payloads, truncation, local response
snapshot/operation evidence and per-exchange loss remain recorded. Capture-wide
loss is preserved separately from per-request attribution, both in source context
and native derived output. Parent artifact hashes come from pinned input handles,
with bounded streaming and change checks against the indexed file's size/time.
The internal source key inside that exact artifact is retained. Changed originals
fail export; filenames and new runtime trace IDs are not used as origin identity.
Live evidence without an earlier artifact has an explicit unavailable-parent
reason. Hashes identify bytes, not authenticity or proof of historical behavior.

Derived sources get fresh intrinsic keys in their new native artifact. Lineage
parents carry `artifactSha256`, optional `sourceKey`, dependency state and reason.
Original entry IDs remain unchanged. When a subset changes an occurrence number
for a repeated original ID, `entryMappings:[{exchangeId,occurrence,original}]`
records its complete parent evidence reference. Import-time occurrences survive
clearing earlier rows, so the next export cannot silently renumber the parent.
`OpenedCaseEvidence.links` contains temporary navigation handles;
`resolve(reference)` never joins by label or guesses an absent original ID.

Case notes are authored `{id,text,source?}` observations. Case markers retain
`id`, `label`, source artifact/key, `originalId` and nullable decimal
`elapsedMillis`; unknown elapsed time stays `null`. Runs retain exact
`experimentId`, `variantId`, decimal `revision`, declared retained
`responses:[{source,body,provenance}]` and authored `outcome:{origin,text}`. The
current run layout validates selected saved response variants and exact response
object references. Stored declarations are not a browser replay or a substitute
for inspecting captured boundaries. New workspace exports leave runs empty;
they never infer a past run's bytes from today's linked file. Existing per-run
snapshot evidence stays in selected native captures, and the working model can
retain explicitly supplied historical body objects independently of current assets.

No desktop case dialog, new command, browser launcher or case archive is added
by K2. Hosts use their existing read-only viewer role and permission checks when
opening the evidence API; this does not grant a viewer global automation rights.
Synchronous prepare/save APIs should run on a worker in a GUI host. Case evidence
opening is asynchronous and cancels pending publication if its future is dropped.

## Configuration and diagnostic context

Use the same ZIP object store for saved configuration output and diagnostic
snapshots, including capture-time context in the new format. A context attachment
carries an ID, kind (`network-configuration` initially), collection time,
collector name/version, platform, purpose, optional
machine label, dependency state and completeness (`complete`, `partial` or
`unknown`) with issues. Its `content` references exact bytes by object digest;
JSON/text media types remain inert data. A successful collection is a snapshot,
not proof that the same configuration held during every captured request.
Unknown optional context kinds remain inspectable as unavailable metadata within
the same bounds; they never acquire execution semantics. Package-wide required
features still fail early when unsupported.

K2's working-case reader stores these exact JSON/text bytes in context objects.
Newly exported workspace sources use `kind="capture-metadata"` with a bounded
JSON object containing original `context`, `notes` and `format`. That includes
original network-configuration output, incident metadata and source notes. The
embedded derived native capture has empty context/path fields; configuration is
not duplicated there. Existing standalone native Save trace remains unchanged.
Opening a case never invokes a recorded command or collects replacement context
from the receiving computer.

Captured-source `collectedAt` may be `null` when the outer source metadata has no
known collection time. Its nested original timestamps and machine/purpose values
remain intact; an import/export time is not substituted. New
`collected-for-case` context requires a known RFC3339 collection time. Unknown
optional context kinds remain inert objects; they are not restored as native
support context by the current adapter.

Keep capture-time context associated with its source through artifact digest and
internal source key, with collection purpose/time/machine recorded separately.
Never substitute the importing or saving machine's current state for the original
machine's snapshot. A new case-save collection has
`origin.kind="collected-for-case"`, not a captured-source attribution. A
capture-only bundle can use the same case manifest with empty experiments,
assets, notes and runs; it does not need another archive format.

The current native `trace-metadata` / `trace-source` layout is an audit fact, not
a compatibility constraint. When moving saved context to these objects, update
the current writers, readers and recovery together; do not dual-write context,
add fallback readers or keep the old storage solely to open earlier files. K3
must specify recording/checkpoint and final-package behavior so interrupted
recording still recovers its available traffic and context without depending on
an unfinished ZIP directory. Source association is evidence provenance, not a
reason to retain the old record representation.

Collection stays opt-in and uses the existing bounded collector; packaging and
opening do not run tool labels, shell commands or network probes. Preserve
unavailable-tool, nonzero-exit, timeout and output-limit notes. Configuration
output is included in privacy review and whole-package encryption along with
notes and traffic. Privacy omission removes the selected context object/reference;
if deliberately embedded opaque evidence still contains the same information,
it needs an explicitly derived capture with new identity/lineage or a warning
that the information remains. This is a content/privacy rule, not a requirement
to support historical TMCap layouts.
No collectors or existing native context records change in K0. K2 owns the
inventory and source associations; K3 owns safe packaging/privacy qualification.

## Limits, persistence and import

| Resource | Initial processing bound |
| --- | --- |
| UTF-8 manifest, including descriptive extensions | 8 MiB; maximum nesting depth 32. |
| Objects / ZIP file members | 16,384 objects plus one manifest; central directory at most 16 MiB, checked before ZIP index allocation. |
| Member names | Exactly `manifest.json` or the derived ASCII digest path; no aliases, case variants, absolute/drive/UNC paths, `..`, backslashes or alternate data streams. |
| Rules / operations / predicates / regex | 1,024 rules per compilation unit, 256 operations per rule/leg, 64 predicates per rule and 4 KiB per regex, matching current compiler limits. |
| Response body object / script source | 1 GiB / 1 MiB, matching current serving/source limits. |
| Context attachments | At most 256 per case; 8 MiB per JSON/text object and depth 32 for parsed JSON. Existing collector limits still apply (1 MiB stdout per tool, 8 KiB stderr and 10 seconds); report failures without claiming complete output. |
| Notes / IDs / reference text | 64 KiB UTF-8 per note; IDs as above; original IDs/source keys at most 512 UTF-8 bytes. All still share the manifest bound. |
| Object transfer | At most 1 MiB buffered chunks; checked unsigned-64-bit lengths/sums, cancellation, disk-space and I/O failure handling. |
| Captures | No new small byte ceiling; stream exact objects. Existing native/SAZ/HAR metadata, decode and content-processing limits still apply and remain visible. |
| Working history | 32 explicitly saved manifest revisions; immutable objects, explicit prune. |

Do not allocate a buffer from declared object size. Validate the manifest and
names, then stream and hash all selected objects into owned staging before
publishing. Reject digest/length mismatch, missing mandatory structure, unsupported
requirements, excessive metadata and duplicate JSON keys. Optional omissions are
explicit; unexpected/unlisted members are errors. Readers never extract names
directly against a destination. Create-new CLI output is the default; desktop
replacement uses the established native confirmation and atomic publication.
Failure/cancellation preserves originals and removes only owned staging.

An import review distinguishes new, identical, conflicting, unsupported, locked
and missing content. Differing content gets new local identities by default;
remap every dependent reference in one transaction. Never replace by label or
portable ID alone. Import disables rules and scripts, preserves authored intent
as metadata, and leaves existing global gates, active revisions and drafts alone.
Read-only viewers may inspect cases but cannot gain global automation privileges.

Keep source capture encryption intact by default. Package and source passwords
have separate roles; an unlocked outer case does not imply unlocked embedded
captures. Passwords are never packaged. An explicit decrypted/derived copy must
be reviewed and receive a new content identity. Privacy review includes notes,
URLs, source labels, bodies, streams, script sources, assets and configuration
snapshots with their source associations. Exclude private
keys, passwords, supervisor tokens, root ledgers and product-state credentials.
Package encryption does not erase plaintext working copies, previews or dumps.

## Intended CLI interface

These commands are **not implemented by K0–K2**. K4 adds the first group; K5 adds
script/compare tooling only after its dependencies are distributed and qualified.

| Command | Intended behavior |
| --- | --- |
| `case inspect --input FILE --json` | Bounded inventory, source/dependency states and capabilities; no execution or import. |
| `case validate --input FILE --json` | Verify structure, object hashes and references; separately report evidence completeness and runnable capabilities. |
| `case pack --input DIR --output FILE [--container tmcap\|zip] [--encrypt]` | Create-new transactional package of the selected manifest and closure; default TMCap compression, optional encryption. Explicit bare ZIP rejects `--encrypt`. |
| `case unpack --input FILE --output DIR` | Explicit plaintext working copy, create-new directory and safe transactional extraction. |
| `rules validate --input FILE --json` | Portable schema plus authoritative rule/action validation; no activation. |
| `rules test --input FILE --capture FILE --json` | Original-request or available recorded-boundary matching, actual match/no-match/unknown, order and intent; no response/script execution or network. |
| `capture inspect --input FILE --json` | Explicit structured addition; existing KEY=VALUE default remains. |
| `capture compare --left FILE --right FILE --output FILE --json` | Existing comparison semantics with stable portable references; differences succeed unless `--fail-if-different`. |
| `script types`, `script validate`, `script test` | Shipped declarations, side-effect-free validation, then explicitly invoked bounded sandbox tests. |

Use existing support-profile JSON and validation; do not add a second capture
profile language. All groups support `--help` before setup. Reject duplicate or
unknown options before file mutation. Paths with spaces/Unicode remain ordinary
quoted paths. No network service, source checkout, Cargo or Node requirement for
binary-only workflows. K0 does not add a case management SDK or browser launcher.

Password roles: `--input-password-file FILE` for the package, repeated
`--source-password-file SOURCE_ID FILE` for separately encrypted captures, and
`--password-file FILE` for encrypted output only. Use masked prompts when
interactive; noninteractive missing passwords fail explicitly. Preserve the
current 4096-byte UTF-8 file bound and newline trimming. Never accept plaintext
password arguments. Diagnostics/progress/prompts go to stderr for new structured
commands; stdout contains one UTF-8 JSON result and newline. Existing default
output and existing command exit behavior remain unchanged.

Results use `resultVersion:0`, `command`, `status` (`ok`/`error`), `diagnostics`
(category, field path, human message) and command-specific `data`. Machine
categories/counts are explicit; messages may evolve. Source references and large
lengths use the manifest's exact string representations. Validation validity,
evidence completeness and experiment runnability are separate fields. A valid
inspection-only case succeeds with unavailable experiments. Define a separate
streaming result mode before exposing enormous per-exchange JSON inventories.

| Exit | New structured-command meaning |
| --- | --- |
| 0 | Successful operation, including ordinary comparison differences or known evidence gaps. |
| 1 | Comparison differences with explicit `--fail-if-different`. |
| 2 | Usage, invalid structure/integrity, authentication failure or missing password. Authentication errors do not identify a tampered ciphertext as a known wrong password. |
| 3 | Unsupported format, required feature, encryption profile or unavailable execution capability. |
| 4 | I/O, destination collision, disk-space or permission failure. |
| 5 | Explicit sandbox/execution failure. |
| 130 | Cancellation; no published partial output. |

## Implementation and acceptance handoff

K0's checked-in fixtures establish bytes, identities and expected future outcomes;
they do not demonstrate an implemented package parser. The ordinary Python gate
checks fixture consistency, digests, compression and scenario coverage. Current
native readers verify real encrypted/stream/incomplete capture seeds separately.

K1's production parser and application tests cover selected rule/asset export
and reviewed disabled import, native exact/pattern/regex/query matching, header
and literal output, encoded/duplicate-header fixtures, changed/large linked
files, conflict remapping, cancellation, failed persistence, restart recovery,
workspace ownership and generation-bound asset visibility. Case/archive/CLI
scenarios remain future work; passing rule tests does not qualify those readers.

K2's working-case APIs implement evidence/notes/context/run inventory and
transactional directory persistence, including historical manifest inspection.
K3 qualifies
safe archive readers/writers, the typed TMCap wrapper, hostile corpus,
transactional export and clean-profile import. K4 qualifies the documented CLI
using shipped binaries in an empty directory. K5 integrates scripts, preview and
comparison without claiming that packaging launches arbitrary browsers.

Every reader must exercise the corpus through its production path once shipped:
colliding sources and IDs; duplicate/binary headers and compression; missing and
changed linked bytes; sensitive notes; encrypted sources; streams/loss/prefixes;
unknown versions/features; invalid paths, duplicate members and corrupt digests.
Add cancellation/full-disk/password retry and large streaming cases in K3/K4.
Record actual compatibility, completeness and runnable state instead of a single
"portable" success flag. Keep per-run results under ignored `artifacts`.

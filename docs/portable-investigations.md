# Portable investigations: revision 0

Desktop cases share reviewed evidence, rules, assets, notes and context with
optional package encryption. They support disabled rule import, frozen response
previews and saved request/capture comparisons. Library APIs also provide
rule interchange and plaintext working cases with revision history.

The CLI currently supports [native capture context extraction](cli.md#read-recorded-context).
Case/rule commands, offline script execution and persisted browser-run evidence
remain planned. Revision 0 stays experimental.
[ADR 0009](adr/0009-portable-investigation-boundaries.md) records the architecture;
[the roadmap](roadmap.md) tracks remaining implementation.

## Support policy

Revision 0 is experimental and may change without migration. A consumer supports
only the current documented revision-0 layout and declared capabilities; it
rejects other schema versions rather than guessing a downgrade. Refresh design
fixtures when the contract changes. No TMCap migration or earlier-layout adapter
is part of this work. A supported public v1 and its support window require a
separate recorded decision before release; additional consumers do not establish
such a promise. Private persistence, script API and live control retain
their own policies; control revision 0 remains outside this decision.

## Implementation boundaries

Portable manifests are separate from private application persistence. The
[portable crate](../crates/portable/src/lib.rs) owns their validation and object
storage; [application APIs](../crates/app/src/investigations.rs) adapt retained
traffic and saved automation. Matching and actions use the
[native automation compiler](../crates/proxy-automation/src/lib.rs).

Export exact saved definitions and bytes, with source identity and availability.
Local generations, activation handles, Undo history, credentials and filesystem
paths are not portable authority. [Export redaction](../crates/app/src/export_privacy.rs)
removes selected headers/trailers; it does not sanitize URLs, bodies, scripts or notes.

## Package and working-copy decisions

A case directory contains `manifest.json` and regular files at
`objects/sha256/<64 lowercase hex digits>`. The digest is SHA-256 of the exact
object bytes. Paths are derived from digests, never supplied filesystem locators.
There are no absolute paths, URLs to fetch, links, scripts to run or environment
variable substitutions in dependency resolution.

Transfers use `.tmcase.tmcap` for cases and `.tmrules.tmcap` for rule collections.
Each contains a ZIP/ZIP64 payload inside TMCap. Writers use Stored members so
ZIP entries remain seekable and TMCap owns package compression. Native captures
and many encoded response assets already compress internally. Readers reject other ZIP compression,
member encryption, directories, duplicate names, unsafe names, reparse points,
symlinks, inconsistent local/central headers, overlapping entries and trailing
archive data. Standard streaming data descriptors and ZIP64 size fields are
allowed: their local placeholders must agree with the descriptor and central
directory under ZIP's rules, rather than being rejected as mismatches. Only
`manifest.json` and declared object members are allowed.
This profile accepts only ZIP64 extra fields; Unicode path aliases and other
unmodeled ZIP extras are rejected rather than interpreted differently by another
reader. Stored ZIPs emitted here remain readable by standard ZIP tools.
New ZIP members use the export time in UTC, rounded to ZIP's two-second DOS
timestamp precision. This is packaging time, not a claim about when original
evidence was modified. Forwarding an existing package preserves its timestamps.
Nested archive-looking asset bytes are ordinary response content, never extracted.

Transfers store the exact ZIP byte stream as one logical artifact in a compressed
TMCap container, with optional password encryption. Compression does not require
encryption. The inner ZIP is unchanged by wrapping/unwrapping and remains usable by standard archive
tools after explicit unwrap/decryption. The portable manifest does not depend on
the compression/encryption mode. Do not add ZIP member encryption or a second
encryption format. Bare ZIP case and rule packages are unsupported, including
archives renamed with a native extension. The internal ZIP profile remains part
of native package validation. SAZ traffic import/export is a separate format and
retains its own ZIP compression and encryption support.

The `portable-archive` document kind separates packages from capture records.
Its descriptor carries the case/rules media type, exact ZIP length and SHA-256,
plus a bounded chunk index. The wrapper provides streaming writes and seekable,
integrity-checked reads without a persisted decrypted archive. A valid transfer
requires both a sealed wrapper and a complete, validated inner ZIP; capture
checkpoint recovery cannot reconstruct a missing ZIP directory. See the
[wire specification](tmcap-format.md#portable-archive-payload).

Optional [TMCap encryption](tmcap-format.md#encryption-and-integrity) protects the
manifest, member names, descriptor and objects. File/document type, encryption
parameters, ciphertext size, filename and filesystem metadata remain visible.
Unencrypted checksums provide corruption detection, not confidentiality or
publisher identity. Re-encryption changes the outer bytes but preserves inner
object hashes when their bytes are unchanged.

### Compression and performance qualification

Stored ZIP members allow direct seeks into embedded captures. TMCap supplies
independent zstd chunks and adaptive raw bypass, resetting compression sampling
at member boundaries so incompressible evidence cannot suppress compression of
later text. HTTP paired-boundary deduplication does not apply to archives.

The [composition benchmark](performance.md#portable-archive-composition)
compares this policy with whole-stream sampling and ZIP Deflate. It measures
size, pack/open/validation time, random-read amplification, CPU and memory,
separating password derivation. Its synthetic codec results do not establish
desktop end-to-end performance or a cross-machine throughput guarantee.

### Opening and working copies

Read-only opening pins the source and uses integrity-checked bounded reads,
authenticated when encrypted; it must not silently persist a decrypted working
copy. Validate authenticated completion when encrypted, and all object hashes
before publishing an imported case. **Save unencrypted folder…** creates a
plaintext working directory after disclosure. CLI unpacking remains planned. Source
movement/deletion either leaves pinned reads valid or produces an explicit
unavailable source; it never resolves a new file by its old name.

Working copies have one current manifest and immutable objects. Save stages,
flushes and atomically replaces the manifest; unreferenced staged objects are
recoverable garbage, not a completed revision. Start with 32 explicit authored
manifest revisions, reject further history saves at the limit, and offer explicit
pruning. Do not silently discard evidence. Keep working manifest history under
`history/<revision>/manifest.json` and retain objects referenced by any retained
revision. Transfer includes one selected manifest, its selected response set
revisions and referenced objects; working manifest history and recovery files
are excluded.

## Manifest model

UTF-8 JSON, no BOM, duplicate keys, non-finite numbers or unknown top-level fields.
Required fields and a concrete example live in the [fixture corpus](../verification/fixtures/portable/README.md).
This contract is separate from private persistence and is not a stable public v1.
Known nested types also reject unknown semantic fields; bounded namespaced
`extensions` are the explicit preservation mechanism. An optional unsupported
response set can be retained opaquely within the manifest bound, but its unknown
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
| `rules` | Optional ordered ordinary rules, separate from named response set ownership. Defaults to an empty array. |
| `experiments` | Named variants and ordered immutable rule revisions with selected saved definitions and enabled/paused intent. |
| `notes` | Authored text/reproduction steps and optional stable evidence references. |
| `contexts` | Optional case-only inventory of immutable capture-time and separately collected diagnostic objects, with original source association, collection origin, completeness and explicit failures/omissions. |
| `runs` | Frozen response set selection, actual retained response references/hashes, engine/script choice and authored outcomes with provenance. |
| `extensions` | Optional namespaced descriptive JSON; bounded and preserved without executing or using it for matching. |

Portable IDs use `[a-z0-9][a-z0-9._-]{0,127}` and are scoped to their owning graph.
Labels may contain Unicode and duplicates. Revisions, byte lengths, ordinals,
timing counters and large numeric identities use canonical decimal strings to
avoid JavaScript precision loss. SHA-256 is exactly 64 lowercase hex digits.
Object digests describe integrity/content identity, not publisher authenticity.
The manifest has no self-hash; its exact stored bytes may be hashed externally.
Within an entity's scope, repeated ID/revision pairs must have identical
definitions. Identity remapping must remain consistent across all selected
variant revisions and asset references. Known named response sets use
`kind="response-variants"`; requirements on an optional response set can make
that response set unavailable without invalidating known evidence.

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
Saved responses use array order and first-enabled-match precedence. General header
and literal rules carry an explicit signed numeric priority; equal-priority
overlapping writes use the production compiler's conflict rejection. Preserve
the original rule ID as provenance while allocating/remapping local import IDs.

| Rule kind | Evaluation and action |
| --- | --- |
| `auto-response` | Original client method/URL/headers; `original-client-request`; exact asset ID/revision. |
| `header-edit` | Request or response head at its native position; `effective-request-head` or `effective-response-head`; ordered set/override-if-present/append/remove operations. |
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
that response set running. An absent body is not an empty object. Unknown optional
response set types remain inspectable as metadata with an unavailable result; they
cannot be imported or executed. Unknown package-wide required features fail early.

Linked files are exported as an explicitly acquired snapshot or an unavailable
dependency. Do not export their absolute path, reopen an author's path on import,
or claim an export-time copy was served earlier. A historical run needs retained
boundary bytes matching its actual digest. Freeze failure aborts the selected
response set export; inspection-only export must explicitly report omissions.

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
`open_object(digest)` expose only that frozen closure. The separate `Package` and
`WorkingCase` adapters enforce safe member/source-access policies before supplying
readers. The manifest never
causes a URL, linked file path or source-machine path to be opened.

The [Application facade](../crates/app/src/rule_interchange.rs) exposes:

| API | Result and publication boundary |
| --- | --- |
| `prepare_rule_import(manifest, staging_parent, open, canceled)` | Freezes and verifies the complete collection, then returns an opaque `PreparedRuleImport` with a mutation-free `review()`. |
| `import_rule_collection(prepared, canceled)` | Requires the same workspace and unchanged generation; publishes fresh disabled local authoring content and assets in one automation commit. |
| `export_rule_collection(request, staging_parent, canceled)` | Returns a `PreparedCollection` containing only selected saved rules/revisions and their verified asset closure; writes no transfer package. |

`RuleExportRequest` supplies a collection ID/revision/title, ordinary rule IDs
and exact `ExperimentSelection` values. A response-set selection identifies its
owner ID and incarnation plus an exact saved revision: an empty `variantId`
selects current content; a named version ID selects that explicit snapshot.
Selection order does not change saved rule precedence. Owned rules must be
selected through their current collection or named version; projected live
rules, drafts and candidate handles are not export selections. Unsupported
advanced/mixed rules fail the whole selection instead of disappearing.

The portable `experiments` / `variants` envelope remains the serialization
contract. Export represents selected current content with the `current` identity,
represents named versions as separate portable variants, and includes empty
Baseline automatically. These wire objects do not introduce runtime variant
histories or archived local collections.

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
Ordinary imported rules are disabled. Each imported portable response set creates
a fresh local `ResponseSet` with a new owner identity and incarnation, paused and
without an applied live snapshot. The portable `current` identity initializes its
current collection independently of selection order. Other non-Baseline portable
snapshots become explicit named versions with fresh local identities and revision
tokens; a named-only import starts with an empty current collection. Exact original
revision strings and enabled/paused/archived/active intent remain in retained source
metadata. Imported content affects live traffic only after separate explicit
activation.

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
| Header action | `operations:[{operation,name,valueBase64}]`; `operation` is `set`/`override-if-present`/`append`/`remove`; remove has no value. |
| Literal action | `find`, `replacement`, `occurrences` (`first`/`all`), nullable decimal `expectedMatches`, `mediaType`, `encoding`, decimal `maxBytes`; current native UTF-8 and media/body bounds apply. |
| Response set | `id`, `kind:"response-variants"`, `requiredFeatures`, `label`, `pausedIntent`, optional `archivedIntent` (false), optional `activeIntent:{variantId,revision}`, `variants`. Active intent must reference an included revision. |
| Variant/revision | Variant `id`, `label`, optional `archivedIntent` (false), ordered `revisions:[{revision,rules}]`. An empty `baseline` variant is mandatory. |

K1 recognizes `response-assets/1`, `header-edits/1` and `literal-response/1`.
Namespaced extension keys use the portable ID alphabet, contain a dot and are at
most 128 bytes. All nested JSON shares depth 32 and the 8 MiB manifest bound.
Assets/objects are bounded to 16,384 each, bodies to 1 GiB, headers to 256 fields
and 64 KiB, examples to 64 per rule, response sets to 32, variants to 16 per
response set, saved revisions to 32 per variant, and responses to 1,000 per saved revision.
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
response sets and different historical/export-time response bytes.

`inventory()` reports selected object bytes, source completeness, missing/locked
dependencies and each response set's availability. `ready-for-explicit-import`
means supported rules and complete declared asset dependencies; it does not mean
a browser run has occurred. Unsupported optional response sets remain opaque JSON
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
Working directories are plaintext; native transfers optionally encrypt the whole archive.

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

Desktop transfer uses the K3 package APIs below; these working-directory APIs
do not launch browsers or add CLI commands. Hosts use their existing read-only
viewer role and permission checks when opening evidence; this does not grant a
viewer global automation rights.
Synchronous prepare/save APIs should run on a worker in a GUI host. Case evidence
opening is asynchronous and cancels pending publication if its future is dropped.

## Desktop transfer and package APIs (K3)

Traffic's **Files** menu leads with normal import/save actions. Its secondary
**Cases** group offers **Save case…**, **Open case…**, and **Current case…**
while a case is loaded. Normal import, file-open arguments and drag/drop also
recognize case files. ZIP files go to the case reader; native files are routed by
the bounded public document header before any password prompt. Full validation
still happens when opening. Opening cases never imports or enables automation.

Saving defaults to selected requests, or current filtered traffic when nothing
is selected. **Rules and notes only** explicitly omits traffic. The form adds
selected saved rules/exact response set revisions, authored notes and an original
context choice. It does not run new configuration commands. Preparing selected
evidence creates temporary unencrypted copies; the form discloses this before
**Review selected content**. That explicit action prepares the copy without an
additional acknowledgement checkbox. Output encryption protects the final file.

The optional **Rules and response sets** disclosure starts collapsed with its
selected count. When expanded, the picker initially shows saved content that
matches the included requests. **Show all** also exposes
nonmatching and unassessed choices, including overrides for uncaptured resources.
Choosing a version includes its complete ordered rules; rules owned by response sets
do not appear again as standalone choices. Suggestions are not automatically
selected. Selected content stays visible and selected when the evidence scope or
visibility filter changes. With no evidence chosen, every saved choice is shown.

The collapsed **Comparisons** section shows its selected count. Choosing a
comparison includes all required requests, including any outside the selected
traffic scope; the description states this before selection. Matching and the
final export use the union, deduplicated by request identity. Expired comparisons
fail explicitly. The comparison dialogs expose **Save comparison in case…**
under **More**, only when their recorded requests can be saved.

Changing **Traffic to include** reassesses the full scope before table paging/windowing. The
read-only check uses recorded client request and client response heads, including
raw header bytes, independently of enabled/paused state. It never runs actions,
loads bodies or assets, activates automation, or consumes an export review slot.
Matching describes the recorded conditions, not which rules actually ran or
whether a response body transformation will succeed. Suggestions are a snapshot;
live traffic can change before the final export freezes its membership.
Missing heads, redacted predicate values, unknown original targets, invalid
matchers or exhausted assessment bounds produce **Could not assess**, rather than
a false no-match. Metadata is bounded to 32 MiB, at most 100,000 scoped requests,
one million evaluations and a two-second assessment budget. All saved definitions
remain available even when assessment stops. Late results cannot replace a newer
scope or reopen a closed dialog; errors retain selections and offer **Retry
matching**. Selected definitions removed elsewhere remain visible for explicit
deselection before saving.

Review leads with a compact request/capture/content summary and groups requests by capture,
showing each capture's request count when recorded. Case source `requestCount`
is an optional canonical unsigned-64-bit decimal string: the number of requests
included for that source, not its original unfiltered size. Absence means unknown,
not zero. Selected native exports always record it, including on reopened cases.
**Open in Traffic** is the primary fixed-footer action; a capture selector
chooses its source. It closes the review and reveals a request from that capture.
**Current case…** returns to the loaded case and selected capture without reopening its file. Detailed
counts remain under **Content details**. Notes, context files, rules, response files,
response sets and exclusions remain available. **Capture observations** explains
stream events and body records without full content; ordinary capture settings,
protocol behavior and unknown completeness are not presented as export failures.
The raw **Technical manifest (JSON)** contains file inventory and provenance,
not an individual request list. Missing content remains missing;
an optional unsupported response set does not prevent unrelated inspection.
**Ready to import** describes supported rule content, not browser replay
or proof of an earlier response set's outcome.

The native format compresses a Stored ZIP64 stream inside TMCap. Optional password
protection covers the complete ZIP, including notes, context, object names and
manifest. The masked output prompt is separate from source/embedded-source
password prompts. Saving a case or rule collection always uses its native
compressed format; there is no format selector. The password checkbox remains
available for new exports and copies. An encrypted opened package starts with
protection selected when saving a copy.
An error or canceled password prompt never selects plaintext as a fallback.

Opening verifies the complete outer stream and all declared object hashes before
publishing inventory. It creates no decrypted archive or working directory.
**Open in Traffic** uses pinned random reads and the existing inert native
importer. Original internal source keys are checked before Traffic publication.
Source-linked notes can navigate to retained matching requests; absent links are
reported without guessing. **View context** displays retained JSON/text without
executing its command labels. Capture viewers retain their role restrictions.

Closing the dialog preserves its contents and ongoing operations. **Open case…**
always chooses another file and replaces the loaded case only after successful
verification. **More → Unload case** releases its owner, while already
imported Traffic retains the source/key handles needed by lazy bodies. Moving or
deleting the original filename does not reopen another file under that name.

**More → Save a copy…** opens output options, then forwards the exact inner ZIP,
optionally changing its password protection. It preserves ZIP metadata as well as declared contents;
it is not redaction. An encrypted opened package starts with protection selected.
Writing a readable copy requires an explicit output choice. Native dialogs confirm
replacement; failed writes, failed verification and cancellation preserve existing
destinations and remove only owned staging files.
Replacing an opened package atomically publishes a separately verified file.
Existing pinned readers retain their original snapshot; new opens see the new
package. Windows readers allow delete sharing and publication uses the modern
rename API through Rust's standard library. A failed replacement preserves the
existing destination.

**More → Extract to a folder → Save unencrypted folder…** explicitly copies the case into a readable working
folder, including any decrypted evidence. Its adjacent description states this
effect; there is no separate acknowledgement checkbox. It creates a new case
directory at the chosen path and preserves
existing paths. Portable manifest IDs are not used as filesystem names. Archive editing is not implicit. Shared `WorkingCase` APIs own
transactional edits/history; this desktop increment does not add a project editor.

In the main window, selected supported response sets/ordinary rules have a separate
import review showing names, counts and what will be added. Generated identifiers
and identity conflicts remain under **Identity details**. **Import rules** imports rules disabled
and response sets paused/unselected, preserving existing edits and activation.
Rules-only packages are reviewed as one complete dependency graph. Opening,
copying or inspecting never imports automation automatically or executes scripts.

[`Package`](../crates/portable/src/package.rs) is the shared archive adapter:

| API | Boundary |
| --- | --- |
| `open(path, source_password, canceled)` | Pinned, fully verified read-only source; bounded ZIP directory and strict manifest validation. |
| `open_object(digest)` | Independent seekable cursor over declared bytes; no source-machine path resolution. |
| `write_transfer(...)` | Stream the exact inner ZIP into native output, optionally changing its password; verify before publication. |
| `save_working_copy(...)` | Explicit create-new plaintext directory using K2 transactional persistence. |
| `write_case_package` / `write_rules_package` | Stream a frozen dependency closure into Stored ZIP64 inside native TMCap; exclude working history/recovery. |
| `Application::open_package_evidence` | Existing bounded native importer and body sources, with package/source handles pinned for lazy reads. |

The ZIP preflight rejects unsafe/unlisted/duplicate names, links/directories,
unsupported compression/encryption, inconsistent ZIP32/ZIP64 and local/central
metadata, overlaps, incomplete descriptors and trailing data. Directory limits
are checked before indexing; no payload-sized allocation is made. Standard
streamed ZIP64 placeholders/descriptors are supported, including an unsigned
descriptor whose CRC equals the optional signature. Transfer content is read in
128 KiB chunks; the outer protected chunk index has a separate 16 MiB limit.
Failure-injection, blocked replacement, large virtual ZIP64, independent ZIP-reader
and encrypted native desktop tests are described in [testing](testing.md).

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
capture-only bundle can use the same case manifest with empty response sets,
assets, notes and runs; it does not need another archive format.

Active capture documents keep recorder context in committed capture metadata so
prefix/checkpoint recovery can retain it without a ZIP directory. Final case
preparation projects selected evidence into a newly sealed native object with
empty inline source context; original context goes into the source-associated
ZIP objects above. Case opening restores it from those objects, without an
earlier-layout fallback or duplicate context write. These are separate current
document kinds, not a compatibility adapter. Interrupted original captures can
still be inspected through native prefix recovery; an unfinished final ZIP/TMCap
archive is never presented as a completed transfer.

Collection stays opt-in and uses the existing bounded collector; packaging and
opening do not run tool labels, shell commands or network probes. Preserve
unavailable-tool, nonzero-exit, timeout and output-limit notes. Configuration
output is included in privacy review and whole-package encryption along with
notes and traffic. Privacy omission removes the selected context object/reference;
if deliberately embedded opaque evidence still contains the same information,
it needs an explicitly derived capture with new identity/lineage or a warning
that the information remains. This is a content/privacy rule, not a requirement
to support historical TMCap layouts.
Collectors remain opt-in and unchanged. K2 supplies the context inventory and
source associations; K3 packages and protects that same object model.

## Desktop previews and saved comparisons

Open a case through **Files → Open case…** or ordinary file import. Choose a
capture and **Preview…**, select an included complete HTML response and
choose **Captured baseline**, **Saved responses**, or an exact named
variant revision. Read the warning before **Open preview**. Scripts start off
each time. The resource scope defaults to the document’s original source;
**All opened captures in this case** is an explicit choice that can mix captures. Inspect
additional case sources first when they are needed by that broader scope.

Preview preparation reads the package’s verified immutable response objects into
bounded owned snapshots. It neither imports nor activates automation, resolves
original linked-file paths, executes packaged automation scripts, nor changes the live
pause switch. Ordinary saved responses explicitly selected for preview participate
even if their saved enabled intent is false. Named variants retain each rule’s
participation and recorded precedence. A variant containing participating live
header/body transformations cannot run in this captured-response preview.
Unavailable assets or an unsupported selection fail visibly; there is no fallback
to a current workbench definition. Existing 1,000-response, 32 MiB/resource and 256 MiB
scene bounds apply. See [page previews](captured-page-preview.md) for isolation,
network failures and browser limitations.

Each opening creates an independent InPrivate WebView2 preview. The case review’s
**More → Preview diagnostics…** (also available as **Last preview diagnostics…** in Traffic) records the case/revision/variant, actual engine,
script choice, resource hashes, winning rules and intercepted request outcomes.
These are supplied-response observations, not proof that another browser or the
original site completed successfully. Closing the package does not invalidate an
already prepared scene. Open another case preview to use a different selection.

In a recorded-request or capture-scope comparison, choose **Save comparison in
case…**. The case form selects that comparison and all requests it requires;
review and save the content normally. Other retained comparisons can also be
selected. A narrower evidence selection must still contain every referenced
request. Changed source metadata or an expired comparison requires a new
comparison before saving. Temporary draft, response-asset and preview-observation
comparisons are not saved by this increment.

A case’s **Saved comparisons** shows historical summary findings.
**Recompute comparison** opens required available sources, resolves their exact
artifact digest / internal source / original request / occurrence identities,
and opens the existing comparison workspace. Request comparisons restore both
boundaries, header exclusions and newline normalization. Capture comparisons
restore ordered scope membership, pairing query/header/body criteria, and manual
pair / keep-unmatched decisions. Proposed pairs are recomputed in the saved scope
order; body comparison remains on demand. Missing, removed or ambiguous evidence
and unsupported engine revisions fail explicitly. Historical findings never
establish equality for the newly acquired evidence. No runtime handle, workspace
request number or filename is used as a portable comparison identity.

The optional manifest `comparisons` array uses required feature `comparisons/1`.
Each entry has `id`, `label`, `kind` (`exchange` or `captures`), `engine`
(`transmog-comparison/1`), ordered `left`/`right` evidence references,
`requestBoundary`, `settings`, `corrections`, and inert `recordedSummary`.
Settings contain `excludedHeaders`, `normalizeNewlines`, `excludedQueryNames`,
`requestHeaders`, and `requestBodyDigest`; fields inapplicable to the kind must
be empty/false. Response boundaries belong to each evidence reference. Corrections
contain optional `left`/`right` references to their declared sides, with no
repeated member. There are at most 16 saved comparisons, 10,000 members per side,
20,000 corrections and 8 KiB per summary, within the shared manifest bound.
Unknown engine revisions remain inspectable but cannot be recomputed. Payload
bytes stay in the existing shared objects; summaries never contain copied bodies.

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

The implemented [`context` command](cli.md#read-recorded-context) lists or extracts
saved native-capture context. It does not author packages, unpack cases or test
rules. See the [roadmap](roadmap.md#capture-and-inspection-fidelity) for deferred
case/rule/script CLI work; proposed command names and result schemas are not
shipped interfaces.

## Verification

Use the [portable investigation checks](testing.md#portable-investigations) for
fixture consistency, native rule and case readers, hostile archives, encrypted
transfer, transactional publication and desktop flows. Every shipped reader must
exercise its production path; design fixtures alone do not qualify a consumer.

Keep source identity, evidence completeness, dependency availability and runnable
state distinct in results. Cancellation, full-disk, password retry and large
streaming cases belong in affected gates. Per-run reports stay under ignored
`artifacts/`; future CLI/script consumers need their own distribution checks.

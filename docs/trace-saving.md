# Saving and inspecting traces

Use **Traffic → Files → Save copy…** in the main window or a Capture viewer.
Review all retained entries, selected requests, the current filters/incident
window, or an original source. Removed entries are excluded; undo their removal
to include them. Imported originals require a new filename. The file dialog
confirms replacing other exports, which happens only after a complete successful
write. Failed reads or writes leave the destination intact. A progress
bar reports writing and SAZ conversion by entry, with indeterminate progress while
preparing or finalizing. Close the dialog to keep working; Traffic continues to
show save progress, and **Save progress…** reopens the dialog.

The Save copy dialog shows concise help for the selected format; detailed
limitations are under **Format details and capture limits**, and the full
inventory/refresh action is under **Included evidence inventory**. Password,
context and header-removal choices stay visible.

## Choose a format

| Format | Use and limits |
| --- | --- |
| [TMCap](native-capture-format.md) | Native evidence, source associations, stream lifecycle and parsed SSE events. Compressed chunks support on-demand body reads and optional encryption. |
| [SAZ · compatibility](saz-compatibility.md) | Conventional SAZ readers, including retained WebSocket messages reconstructed for Fiddler. Missing heads use labeled placeholders; these are not captured server responses. |
| [SAZ · extended fidelity](saz-compatibility.md) | Conventional SAZ plus Transmog source associations and additional evidence. Both SAZ modes offer encryption and network context; ZIP member names remain visible. |
| [HAR](json-archives.md) | HTTP archive interchange with headers, decoded response bodies and available timings. No stream activity, password encryption or computer network-context option. |

The save report discloses missing or incomplete bodies and format-specific
omissions. TMCap bodies stream from retained storage without collecting the
whole capture in memory. Saved traces have no fixed file-size or record-count
ceiling; metadata indexing and body-processing budgets still apply. Review
private headers, URLs, bodies and notes before sharing; header redaction alone
does not anonymize a trace.

For evidence, frozen rules/assets, notes and context in one transfer, use
**Files → Save case…**. [Portable cases](portable-investigations.md#desktop-transfer-and-package-apis-k3)
open through **Open case…**, normal file import or drag/drop. Inspection,
forwarding, plaintext working copies and reviewed disabled rule import are
separate actions.

## Evidence and source context

Workspace request numbers are local. TMCap and extended SAZ preserve relative
request order and explicit original numbers, but reopening assigns new workspace
numbers. See [request order](traffic-inspection.md#request-order) for fallbacks
and provenance; a workspace number is never a portable source reference.

**Details → Certificates** shows recorded TLS chains and verification failures.
See [certificate evidence](certificate-model.md#recorded-connection-evidence)
for retained fields and format support.

For linked auto-responses, TMCap records the selected encoded entity's length,
SHA-256 and compression stack with the historical rule/asset decision. It saves
the retained client-response bytes independently, including their availability.
Importing or comparing an earlier result never reads the external source path.
The exchange ID, client-response boundary and complete body hash form the byte
reference for future response set packaging. A current path or configuration
revision cannot replace missing, truncated or expired bytes. SAZ/HAR projections
do not preserve the local snapshot selection record; use TMCap for that evidence.

**Encrypt with a password (AES-256)** is optional. New passwords require
confirmation and cannot be recovered. Encrypted imports ask only when needed,
allow retry or Cancel, and leave Traffic unchanged on failure. Passwords are
kept in memory for the operation and cleared from dialogs on close. Native files
compress before encrypting each frame, allowing metadata indexing and body reads
without decrypting the entire file; see [native encoding](native-capture-format.md).

**Include this computer’s network configuration** is optional and unchecked. It
records bounded ipconfig /all output on Windows, interface/DNS/route context on
macOS, or ip and resolvectl output on Linux. Collection time, platform and failures
stay with this context. This option also exists when recording from Capture tools,
and the CLI offers --include-network-context.

Saving merged native traces preserves each original source's metadata, original
request identifier, raw imported heads, timers and terminal time. Select a request
and choose **Details → Capture source** to reach its associated source. The source selector
also offers the saved workspace's own context; network configuration from the
saving computer does not replace the original machine's configuration. Imported
source names, paths and session counts describe the original capture.

Optional capture-level context also survives conventional and extended SAZ
conversions in a namespaced JSON member ignored by ordinary SAZ readers. Native
files preserve the full original source associations and measured transport
observations; SAZ conversion reports its compatibility limitations.

**Capture tools → Inspect / recover** offers a trace drop target and picker that import
into Traffic, plus a separate **Import trace** button for a typed source path.
**Inspect trace metadata** reads the file's original context and shows its full
path without adding Traffic entries. Source paths start empty; browse for an existing
trace or enter its path before inspecting or converting.

**Capture tools → Convert** creates a new file from a saved TMCap file in TMCap,
JSON lines or SAZ format. Choose a source file and an unused destination;
**Convert inspected trace…** carries over the inspected path and suggests a
filename containing `converted`. Conversion leaves the source unchanged.

**Capture tools → Export** explains how to save captured or imported Traffic
through **Traffic → Files → Save copy…**. **Open Traffic Files** goes to Traffic
and opens that menu, preserving the current selection and filters.

Imports bound the index for each source and the combined index and metadata
across merged files. Publication is atomic: rejected files add neither partial
traffic nor partial source metadata. Removing an entry keeps evidence available
for Undo, so its imported index remains retained until Clear releases it or its window
closes. Large Clear operations expire Undo and release source context too.

## Comparison snapshots

Current TMCap recording and **Save copy…** preserve observed request/response
head-edit order, captured hook identities/names and structural header-name
changes, including no-op head replacements. Header values are excluded from
this audit metadata; original and effective heads follow the trace's ordinary
sensitive-header projection. **Details → Timings → Recorded header edits** reads
that historical evidence even when no current local rule exists. Missing audit
metadata does not establish that no hook ran. No legacy-format translation is
performed for these records.

Investigation comparisons retain exact loaded source identities, original IDs,
selected boundaries and acquisition times. Two imports of the same file remain
distinct loaded instances. Filenames, paths, URLs and workspace request numbers
are descriptive and do not become cross-capture identities. Removed evidence is
reported unavailable; a former path does not authorize loading a replacement file.

Saving a trace does not persist comparison state. Use **Save comparison in case…**
to retain supported recorded-request or capture-scope comparisons, including
their evidence references and settings. Temporary draft, asset and preview
comparisons remain window state. See [case comparisons](portable-investigations.md#desktop-previews-and-saved-comparisons)
and [Traffic comparison](traffic-inspection.md#comparing-captured-evidence).

## Incident intake and reviewed evidence export

**Include markers and notes** controls the current marker set independently of
**Include capture details**. TMCap and extended SAZ preserve markers and message
links; HAR and compatibility SAZ disclose their omission. Copies contain current
notes and reviewer provenance, without replaced text or Undo history. Changed
notes require a refreshed inventory. See [marker review and export](support-capture-profiles.md#developer-intake-and-export-review)
for membership, revision checks and unsaved-work handling.

## Literal response edit evidence

Current TMCap recording and Save trace preserve literal operation outcomes,
including explicit count and buffer failures, with historical rule identities,
match counts, decoded lengths and before/after hashes. Reports exclude body
values and authored literals. Bodies still follow ordinary retention controls.
A selected plan without a result does not establish completed execution; codec
failure or interruption can prevent a report. Details and response comparison
read retained boundaries, never today's rule. No TMCap migration or compatibility
translation is added.

<a id="response-experiment-sharing"></a>

## Response set sharing

Save TMCap while complete original/effective response bodies remain retained to
preserve the observed response bytes and historical rule, asset and operation
evidence. A hash or saved configuration alone cannot recover an expired body.
An imported trace shows historical attribution without enabling local automation
or authorizing access to an author's filesystem paths.

Save copy does not export the automation workspace, named variant definitions,
script sources, external file dependencies or temporary preview scenes/reports.
Use [Save case](portable-investigations.md#desktop-transfer-and-package-apis-k3)
to combine reviewed evidence, source context, notes and selected saved response
sets in a package with optional encryption. Importing its automation requires a
separate review and leaves imported rules disabled. Case/rule CLI commands remain
planned. See [response sets](automation.md#response-sets) for authoring and activation.

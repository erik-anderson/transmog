# Saving and inspecting traces

Request order (**#**) is local to the workspace; see
[Request order](traffic-inspection.md#request-order) for numbering and import
fallbacks. Saving a subset preserves source associations and explicit source
numbers, while reopening assigns fresh workspace numbers. Native saves carry
versioned request-order ordinals so a saved investigation retains deterministic
relative order even with equal or backward timestamps. Extended SAZ carries the
same ordering and original numeric provenance in its Transmog evidence. Older
native and extended-SAZ files remain readable without these optional fields.
HAR and conventional third-party SAZ consumers may discard these extensions;
workspace numbers alone are never portable source references.

Use **Traffic → Files → Save trace…** in the main window or a Capture viewer. Review all retained entries, selected requests, the current filters/incident window,
or an original source; removed entries are excluded.
Undo a removal before saving if those entries should be included. The file dialog
confirms overwriting other exports; imported originals require a new filename. An existing destination is replaced only after a complete
successful write. Failed reads or writes leave that destination intact. A progress
bar reports writing and SAZ conversion by entry, with indeterminate progress while
preparing or finalizing. Close the dialog to keep working; Traffic continues to
show save progress, and **Save progress…** reopens the dialog.

Select a request and choose **Details → Certificates** to inspect its original TLS
connections, server and client authentication chains, public PEM/DER and recorded
verification failures. TMCap shares certificate definitions across requests.
Extended SAZ stores them under `transmog/certificates/`; HAR uses a
`_transmogCertificates` dictionary with per-entry `_transmogPerformance`
references. Ordinary readers can ignore these extensions. Imported Chromium
NetLog PEM paths use the same viewer, while SAZ HTTPS flags and HAR security
summaries remain available even when their files omitted certificate bytes.

The Save trace dialog offers TMCap, HAR, SAZ compatibility and SAZ extended
fidelity. Select **SAZ · compatibility** for ordinary SAZ readers, or **SAZ ·
extended fidelity** to preserve original source associations and additional
evidence. SAZ supports optional encryption and network context; ZIP member names
remain visible. Both SAZ profiles include retained WebSocket messages for Fiddler,
with reconstructed frames and normalized HTTP/2 handshakes. TMCap preserves full
stream lifecycle and parsed SSE events; HAR omits stream activity.
SAZ includes each selected exchange. Missing request or response heads
use clearly labeled compatibility placeholders: Fiddler can show the explanation
in session comments and message headers; an uncaptured response uses a labeled 599
status, or 502 for a recorded DNS, TCP or TLS setup failure, and, when
there are no retained response bytes, explanatory text. These are export
placeholders, not captured server responses. The save report counts affected
entries and additional generated TLS connection summaries. Captured opaque CONNECT
tunnels include their recorded destination, peer and byte totals without invented
TLS details. TMCap preserves the original missing-head state without placeholders.
HAR 1.2 preserves HTTP exchanges, original headers, decoded response bodies and
available timings. HAR has no password encryption or computer network-context
option. Missing bodies are
disclosed; binary request bodies use a Transmog extension. See
[JSON archive import](json-archives.md).

Save native traces as `.tmcap`. Each chunk is compressed independently, so the
desktop can read body payloads on demand. Bodies stream from retained storage;
large captures are not collected into one in-memory buffer. Active, missing or
incomplete bodies remain explicit and are counted in the save report. Review
private headers and bodies before sharing. Saved traces have no fixed file-size
or record-count ceiling; metadata indexing and body-processing budgets remain
separate.

For linked auto-responses, TMCap records the selected encoded entity's length,
SHA-256 and compression stack with the historical rule/asset decision. It saves
the retained client-response bytes independently, including their availability.
Importing or comparing an earlier result never reads the external source path.
The exchange ID, client-response boundary and complete body hash form the byte
reference for future experiment packaging. A current path or configuration
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
path without adding Traffic entries. **Capture tools → Convert / export** converts native
files to TMCap, JSONL or SAZ and suggests filenames containing `exported`.

Imports bound the index for each source and the combined index and metadata
across merged files. Publication is atomic: rejected files add neither partial
traffic nor partial source metadata. Removing an entry keeps evidence available
for Undo, so its imported index remains retained until Clear releases it or its window
closes. Large Clear operations expire Undo and release source context too.

## Comparison snapshots

Current TMCap recording and **Save trace…** preserve observed request/response
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

Comparison bodies, pairing decisions and exclusions are bounded transient window
state. Saving a trace does not currently persist them or duplicate payloads in a
comparison-result JSON file. Portable cases and public CLI comparison belong to
their separate API/artifact work. See [Traffic comparison](traffic-inspection.md#comparing-captured-evidence).

## Incident intake and reviewed evidence export

Native source metadata retains support profile revisions, independent incident
markers and original source clocks. **View current imports…** offers incident
intake and source-specific adjustable time navigation. A marker is never an HTTP
exchange or a request-order value. Circular gaps and recovered/unsealed sources
remain explicit. See [support profiles and incident intake](support-capture-profiles.md).

**Save trace…** inventories and freezes all, selected, filtered or source membership
before saving, including offscreen entries. The original-context choice can omit
capture context, markers, source notes and source paths from the copy. HTTP/stream
content is controlled separately by whole-entry selection and sensitive-header
projection. TMCap/extended SAZ preserve source incident context; HAR/compatibility
SAZ report incident-marker omissions. Review is not complete anonymization.


## Literal response edit evidence

Current TMCap recording and Save trace preserve literal operation outcomes,
including explicit count and buffer failures, with historical rule identities,
match counts, decoded lengths and before/after hashes. Reports exclude body
values and authored literals. Bodies still follow ordinary retention controls.
A selected plan without a result does not establish completed execution; codec
failure or interruption can prevent a report. Details and response comparison
read retained boundaries, never today's rule. No TMCap migration or compatibility
translation is added.

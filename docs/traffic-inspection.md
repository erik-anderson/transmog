# Inspect and search traffic

Traffic uses one selected-item inspector for live requests and imported captures.
Selection and scroll position remain stable while new traffic arrives. Inspect
request and response together in a wide pane or use the message selector when space is
limited; the narrow selector sits beside the URL/status. Incomplete, unavailable, redacted and evicted evidence are distinct
states; display previews can be shorter than the retained source.

Traffic uses one continuous list and a native scrollbar spanning all matching
entries. Only rows near the viewport are rendered, so large captures remain
usable. Compact and ordinary rows have consistent heights. Headers and cells
stay on one line and clip with ellipsis when columns are narrow; hover shows
the complete value. Protocol includes the active WebSocket or SSE badge beside
the HTTP version and participates in column fitting. If upstream DNS, TCP, TLS or
connection policy fails before an upstream HTTP protocol is observed, Protocol
shows **Unavailable**. The inspector still preserves the recorded client-side
HTTP version. A failed connection attempt does not hide HTTP evidence from
another attempt. Selection, focus and range anchors use stable IDs and the
complete display order; leaving the rendered window never deselects an entry.

## Request order

The leading pinned **#** column is **Request order**: a positive, workspace-local
number assigned once when a request is admitted. Use it to refer to a request or
restore admission order through **Lowest first** / **Highest first** in its header
menu. Equals, At least and At most accept exact positive decimal integers, including
values larger than JavaScript's safe integer range. Filtering and sorting happen
across all retained matches before the visible rows are selected.

Numbers survive response completion, filtering, scrolling, dismissal and Undo.
Clear, eviction, and stopping/restarting the proxy do not reuse numbers in the same
workspace. Gaps are normal and do not establish capture loss or causal order.
Independent workspaces assign their own numbers. **Start time** remains a separate
column, the default newest-first sort, and the sort used by **Follow latest**.

| Source | Admission order within an import |
| --- | --- |
| Native TMCap | Valid saved request-order metadata, otherwise first surviving exchange appearance in the stream. Opaque IDs and wall-clock times do not determine this fallback. |
| HAR | Entry-array order, including entries without timestamps. |
| SAZ | Conventional numeric session order; extended Transmog archives prefer complete, unique saved request-order metadata. |
| Chromium NetLog | Inferred surviving request-start monotonic time; missing times follow timed requests. Ties use first source appearance, then redirect hop. |

Each import publishes one atomic batch after existing admissions. Reimporting a
file, combining files with overlapping source IDs, and mixing files with live
traffic allocate new workspace numbers without renumbering existing requests.
Partial or circular captures retain only surviving evidence; inferred order does
not claim that missing initial requests or records were captured.

**Details → Capture source** shows the original capture, original request ID, and
an explicit source request number with its provenance when available (for example,
a conventional SAZ session number). An opaque ID, HAR array position, or observer
event sequence is not presented as an original measured request number.
**Copy request reference** in Actions or the row context menu includes the exact
workspace number, its scope, and available source path, original ID and URL.
Workspace numbers and runtime import namespaces alone are not portable case IDs.

**View → Columns** calls the column **Request order (#)**. It supports the existing
hide, pin, resize, fit and move actions. Upgrading an old layout inserts the pinned
column while preserving existing widths, order, visibility, pinning and splits.
Hiding it persists, and Reset columns restores the new default.

## Headers and sizes

Header views preserve ordering and duplicate fields. Value and field byte sizes,
the total header size and **Largest first** help find oversized cookies or other
fields. Sizes describe the recorded HTTP headers, not HPACK/QPACK compression or
IP/TCP packet overhead. Redaction retains original lengths when measured; an
unknown imported length remains unavailable rather than appearing as zero.

Header options shows whether **Authorization** and
**Proxy-Authorization** were present, including when values were redacted.
Sensitive header values are captured by default in the desktop and guided CLI;
Settings offers persistent redaction for new traffic. See
[retention and privacy](product-state-and-support.md#retention-and-privacy).

Body details distinguish observed, retained and decoded sizes when available.
Encoded entity sizes for compressed requests describe HTTP body bytes; physical
transport byte counters can include shared connections and TLS records. Neither
is a count of the application's decoded payload plus all network packet overhead.

Local requests show the best-effort process name and PID. A non-loopback client
is marked remote and its recorded source IP appears in the request inspector. Loopback
addresses do not reserve an extra field. See
[client process attribution](client-process-attribution.md).

## Body viewers and Composer evidence

Hex reaches every retained byte with exact offsets, paging, keyboard navigation
and Go to offset. Text-preview limits do not cap Hex. Clipboard selections have
separate bounds; Save as handles larger data. Temporary byte caches are released
with their readers. Save as leases original bytes; preview formatting never
replaces the source.

Composer preserves later draft edits during sends and keeps the last complete
response until replaced or closed. It uses the same response inspector as
Traffic. A terminal failure without a response displays Failed rather than
Pending. Body completeness is independent of a later exchange failure.

## Copy a request

The context menu and selected request’s Actions menu offer **Copy as cURL**,
**Copy as PowerShell** on Windows, and **Copy all headers** for a single entry.
They only put text on the clipboard; they never execute a request.

cURL commands use plain `curl` with POSIX shell quoting, suitable for Bash.
Windows PowerShell 5.1 aliases `curl`, so use **Copy as PowerShell** there.
PowerShell output supports 5.1 and 7, using `Invoke-WebRequest` where it can
represent the request and .NET APIs where restricted or duplicate headers,
method/body combinations or HTTP version requirements need them. Notices explain
normalization, protocol fallback and proxy-authentication limitations.

Commands preserve recorded headers and encoded body bytes as closely as the tool
allows and regenerate framing. Redacted or missing values use explicit
placeholders. If a body can be represented inline, copying goes directly to the
clipboard; PowerShell can embed binary bytes as base64. A body-file dialog appears
only when a file is needed. **Save as…** saves the complete original bytes and
copies a command referencing that path. It holds an eviction lease while saving.

**Copy all headers** includes the request line, complete request headers, response
status line and complete response headers, with exactly two blank lines between
the blocks. An unrecorded protocol is labeled unavailable rather than guessed.

## Search and select

The live **Filter** matches complete URLs, including text spanning the domain
and path, as well as process metadata. Both Filter and metadata searches in
**Search contents** also match URLs without an explicitly captured default port
(`:443` for HTTPS or `:80` for HTTP). Non-default ports remain part of the URL.

In **Search contents** mode, **Search** or Enter searches retained evidence;
typing alone does not repeatedly read bodies. **Search options** chooses metadata, request headers, response
headers and decoded text request/response bodies, literal text or regex, case
sensitivity and accent handling. Content codings and supported text encodings are decoded.
The bounded regex engine does not support backreferences or look-around.

Binary bodies are skipped quietly. Unavailable text and text over the search
budget contribute one summary count. Search limits are separate from capture
limits, so retaining a large body does not guarantee that all of it is searchable.
Results are a completed snapshot; search again to include later traffic.

**Select all matches after searching** replaces selection with the new matches on
each successful search. **Select all matches** and Ctrl+A in the result list also
work across the whole matching list, including offscreen entries. New traffic
does not silently join the selection. Column filters remain applicable. Failed or
canceled searches preserve the previous results and selections.

**View matches** opens bounded snippets of original decoded evidence, keeping
accents and showing zero-width regex positions. F3 / Shift+F3 moves between
matches; entry navigation preserves traffic multiselection, reveals the current
request in Traffic and updates its inspector. Closing review focuses that
request. Only a bounded number
of occurrences per entry is displayed; refine the query to inspect later matches.
Changed or removed evidence requires searching again. HTML and script fragments
remain inert text in this view.

Ctrl+click adds entries, Shift selects ranges across the full displayed order,
even when intermediate rows are offscreen, Shift+arrow extends selection and
Escape clears it. **Remove selected entries** / Del and **Remove unselected
entries** operate across the whole workspace, including hidden rows. Undo / Ctrl+Z
restores them while their underlying evidence is retained. These actions do not
delete source capture files or saved auto-responses.

## Edit and replay

**Edit and replay** opens the original request in Composer. Small bodies use a
lossless hex editor. Larger captured bodies use **Complete captured body**;
**Replacement file** streams an explicitly chosen regular file. Captured bytes
are leased and copied to an anonymous temporary file for streaming so large
bodies stay outside the WebView editor. Switching modes preserves editor drafts.

Missing bodies require a replacement or an explicit **Use an empty body** choice.
Removing a source never silently turns replay into an empty request. Framing is
recomputed while Content-Encoding is preserved; replacement files therefore need
already encoded bytes when that header is retained.

Sending requires the applicable method-risk and credential acknowledgements.
Replay uses the verifying Rust HTTP/TLS stack with the proxy either running or
stopped. Failure keeps the draft ready to retry. Results and bounded replay
history retain the originating entry, trace and original entry identity after
edits, with links back to source traffic while it remains available.

For latency interpretation see [request timings](request-timings.md). To share or
inspect saved evidence see [trace saving](trace-saving.md).

## Traffic workspace organization

The main field defaults to a live URL and process filter across all retained
metadata. Switch to **Search contents**, then press Search or Enter to inspect
headers and bodies. Content searches remain snapshots; filtering never decodes
bodies on each keystroke.

**Request and response** is the wide default, with the list above request and response
inspectors. Narrow panes show one message and a request/response selector.
Headers show Name and Value by default; **Header options** reveals byte sizes
and ordering. **Body options** groups decoding and preview limits.

**Details** groups Timings, Connection, Certificates, and imported Capture source.
**Actions** groups replay, auto-response creation, page preview, and request copy
commands. **Files** groups importing and saving; **View** groups layout, density,
and columns. Bulk actions appear for multiple selected requests.

## Comparing captured evidence

Select exactly two Traffic requests and choose **Compare requests**, or use
**Set as comparison baseline** and **Compare with baseline** from Actions/the
selection menu. The baseline remains visible and clearable while selecting other
traffic. Selection order determines left and right; **Swap sides** reverses them.
Comparisons are read-only and available in capture viewers as well as the main
window. Closing returns to the original Traffic selection and scroll position.
The existing inspector layout is named **Request and response**.

The comparison opens original client requests and final client-visible responses.
**Boundaries and comparison options** explicitly selects client or upstream stages;
a missing stage remains unavailable. Source/fidelity retains original identifiers,
recorded capture starts and saved-rule/asset attribution where available.
Overview, Request, Response, Timings and
Source/fidelity rows share **Next change / Previous change** (Alt+Down/Alt+Up).
Rows and original byte ranges are paged. Wide windows show values together;
compact windows stack labeled values. Source actions return to original evidence;
if the source was removed or a view switch is canceled, the comparison stays open.

Equality uses complete original fields and bytes, including duplicate header
occurrences, rather than shortened inspector text. Redacted values are unknown.
Exact encoded bytes, decoded bytes and faithfully decoded text have separate
verdicts. Newline normalization is an explicit derived view. Header exclusions are
explicit comma-separated names; none are enabled by default. Differences can be
known while other evidence is incomplete or not compared. Matching retained
prefixes never prove complete-body equality. HTTP 304 has no captured cached
representation; absent evidence is distinct from empty bytes. Imported HAR/NetLog
decoded payloads and source-adjusted bytes do not establish equality of original
encoded entity bytes. Protocol values require observations at the selected boundary.
Available stream lifecycle/count metadata is compared; message/event content is
explicitly excluded, with source stream inspection available for it.

A comparison freezes its evidence. **Source changed or expired** offers a deliberate
refresh; late or canceled work cannot replace a newer result. Canceling preserves
the previous completed result. Closing releases the snapshot. Subjects, criteria,
and manual pairings are transient investigation state, separate from global layout
preferences; they are not saved into captures or restored after app restart.

### Comparing capture scopes

Use **Files → Compare capture scopes…** to select two loaded original sources,
live traffic, the selected requests, or all current filter results. Filters apply
before virtual windowing. Scope counts in a completed result describe its frozen
membership; later traffic does not join silently. Frozen labels retain the source
or filter settings. **Frozen scope times and fidelity** shows recorded start ranges
in each source clock, unavailable times, stored formats and known limitations.
Import another capture into this
window to include it. Removed or expired sources never become invented failures.

Pairing starts with exact method and absolute URL, normalizing host/scheme case
and default ports while preserving path case/escaping and query order/duplicates.
Unique keys and unambiguous shared provenance identify counterparts. Repeated keys
remain proposed/unresolved, including count mismatches and possible missing middle
requests. Occurrence proposals use recorded source order, independent of display
sorting or the order in which requests were selected. Response status or body never chooses a counterpart. **Change pairing…**
searches the frozen opposite scope; **Keep unmatched**, **Undo pairing correction**
and **Reset proposed pairs** allow explicit correction. Each entry has at most one
counterpart. Manual choices survive criteria changes on the same frozen scopes.
Starting another scope comparison creates new membership and pairing state.

Pairing criteria can exclude named query fields, require selected request header
values, or require a complete request-body digest. These criteria choose pairs;
they do not hide differences in a pair's detail. Header exclusions in the detail
are separate. Summaries distinguish changed/equal compared metadata, unresolved,
left-only/right-only and unavailable evidence. Bodies start **not compared**;
**Compare pair** acquires them on demand. Only observed in scope does not mean
another browser never made that request. Timing deltas use matching locally
measured intervals, including preserved native-capture measurements; other imported
definitions remain in source timing details. Capture pairs show a request-header
to local-completion delta only when both sides have that measured interval.

### Comparison limits

Two workers run per window, with cooperative cancellation and a 30-second work
limit checked between bounded reads/processing stages. At most four completed
exchange comparisons and four capture comparisons are retained. Each body snapshot
is bounded to 32 MiB, four bodies per exchange pair (128 MiB, up to 512 MiB across
retained exchange results). Larger/incomplete sources offer a frozen prefix up to
64 KiB and retain the source Hex/Save as flows for complete retained evidence.
Text decoding has a 16 MiB input/output bound with the existing codec expansion
and work limits. Line alignment uses at most 200,000 lines, a 32-line lookahead and
128 text hunks per body. Exact byte verdicts remain separate from diff output.

Original headers are bounded to 4 MiB / 16,384 fields; an exchange's selected
request/response headers share that budget. Request methods/targets have a 64 KiB
input bound and remain unavailable for pairing above it. Results retain
at most 512 evidence rows and 1 MiB of serialized row data; pages contain 64 rows,
and byte pages contain 256 bytes per side. Omitted output is disclosed and does
not change an already established difference verdict. Capture scopes are bounded
to 50,000 entries and 32 MiB of charged metadata per side; pairing pages contain
100 rows. Undo retains at most 20 corrections within a 4 MiB history budget.
Narrow scopes when acquisition exceeds a limit. Limits do not constrain
existing source Hex/Save as. Cancellation cooperates with file/codec work; an
in-progress operating-system file read must return before its worker can stop.

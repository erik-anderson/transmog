# Inspect and search traffic

Right-click read-only fields and structured evidence to copy names,
complete values, pairs or subtrees without changing selection. See
[copying inspection values](copying-inspection-values.md) for formats, context
menus, Auth masking and limits.

The per-message **Auth** view explains captured authentication formats, nested
tokens, masking and source coverage alongside Headers and Body. See
[authentication inspection](auth-inspection.md) for formats, reveal/copy, time
interpretation, related traffic and limits.

Traffic uses one selected-item inspector for live requests and imported captures.
Selection and scroll position remain stable while new traffic arrives. Inspect
request and response together in a wide pane or use the Request/Response pill
when space is limited. The pill switches in one click. In very narrow inspectors,
the controls wrap together to keep the full URL accessible. Incomplete,
unavailable, redacted and evicted evidence are distinct states; display previews
can be shorter than the retained source.

**Expand inspector** temporarily fills the Traffic workspace with the selected
exchange. **Restore** returns to the list and its prior scroll position. Use
**Ctrl+Shift+Enter** for either action. Selection, message modes and the prior
inspection scroll position are preserved; expansion and its request/response
resize do not overwrite saved split positions. Wide windows retain both messages,
while compact windows retain the Request/Response switch.
While Traffic is active, the shortcut works from list, inspector and app header
controls, including focused buttons. Open menus and dialogs retain their keys,
and text editors can handle the shortcut themselves.
The search shortcuts **Ctrl+F** and **Ctrl+Shift+F** restore Traffic before
focusing the filter or content search field.

Body's status line identifies the representation, content decoding and displayed
preview size. Significant preview limits, decoding warnings and unavailable-body
reasons remain visible. **Body options** contains Decode and the preview byte
limit in one disclosure. **Copy body preview** copies only the displayed evidence;
**Copy explanation** copies a warning or missing-body reason, and **Copy body
metadata** identifies the metadata view. **Save as…** saves the full retained
response with content decoding, independently of the displayed preview limit.

Retained-body decoding accepts both zlib-wrapped and raw DEFLATE for
`Content-Encoding: deflate`, including in protocol visualizers and saved response
files. This handles senders such as OneCollector that omit the zlib wrapper.
The initial bytes select the format; a zlib checksum failure remains an error.
Turning Decode off shows the original retained bytes.
In Auto, Decode remains available while viewing raw binary bytes, so it can be
turned back on without changing viewers.

Cancellation appears as **Cancelled** in the Status column, inspector, State
column, Timeline and copied request summary. Cancellation uses a neutral badge;
the retained diagnostics explain whether the caller stopped waiting or the
request was canceled in Transmog.
An observed HTTP status remains in the inspector and copied summary when
cancellation followed response headers. Cancellation does not establish whether
the server processed the request.
Proxy shutdown remains a separate outcome with a proxy-stop explanation.
These distinctions also apply when opening a capture that already recorded its
cancellation category.

**Actions → Cancel request** stops a selected active request, including a long-running
download, SSE stream or open WebSocket. Traffic multiselection offers **Cancel
requests** for active entries in the selection. Completed requests and imported
evidence cannot be canceled. The row and retained headers, body bytes and messages
stay available for inspection. Cancellation cannot establish whether the server
already processed the request.

A response supplied by a saved response rule is highlighted as **Auto-responded**
in the inspector and Request details, with its recorded rule name and revision.
This attribution comes from the captured exchange, independently of later rule edits.

For complete responses, **Actions → Add to response set…** collects the selected
response or multiple selected responses into an existing or new saved debugging
workspace. Review the source responses and destination, then add them without
applying the set to live traffic. **Open response set** continues editing
explicitly. **Actions → Create rule → Serve a saved response…** creates an
ordinary saved-response rule from a captured response.
See [response sets](automation.md#response-sets).

Use [choose an editing workflow](automation.md#choose-an-editing-workflow) to
distinguish saved response replacements from header rules and literal edits to
fresh origin responses. Saving, previewing and live activation are separate
actions; a captured-page preview does not run the standalone live editors.

Traffic uses one continuous list and a native scrollbar spanning all matching
entries. Only rows near the viewport are rendered, so large captures remain
usable. Compact and ordinary rows have consistent heights. Headers and cells
stay on one line and clip with ellipsis when columns are narrow; hover shows
the complete value. Protocol keeps the WebSocket or SSE identity beside the
secondary HTTP version through closure and import, and participates in column fitting.
A narrow Protocol column uses **WS ⌄** while retaining the full WebSocket name
in its accessible description and disclosure; saved column widths are preserved. If upstream DNS, TCP, TLS or
connection policy fails before an upstream HTTP protocol is observed, Protocol
shows **Unavailable**. The inspector still preserves the recorded client-side
HTTP version. A failed connection attempt does not hide HTTP evidence from
another attempt. Selection, focus and range anchors use stable IDs and the
complete display order; leaving the rendered window never deselects an entry.

**View → List columns → Extension columns** optionally adds fixed protocol labels
from installed extensions. Their request/response header rules run natively on
bounded visible-row metadata, without starting extension code or reading bodies.
Choose up to eight; disabling an extension removes its columns. Column choices
update automatically when extensions change. A small extension icon distinguishes
their headings; click one to filter its fixed labels or **No match**. These
filters combine with ordinary Traffic filters and use bounded retained headers
across matching entries, without opening bodies. A **Limited**
cell means its header classification exceeded the metadata budget. Manage their
declared header names and conditions in [Extensions settings](extensions.md).

The Body viewer's **Auto** option can show a matching extension after it reports a
complete parse. Failed or incomplete automatic parses fall back to the ordinary
body representation; choose the extension explicitly to inspect a partial result.
Only the selected, visible message is parsed. Request and response panes remember
their viewer and Headers/Body/Split choice independently. If a protocol viewer is
unavailable on another request, the pane uses its normal mode and restores the
preferred visualization when matching traffic is selected again.

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
**Actions → Copy → Copy request summary** (also in the row context menu) copies
the method, URL, observed status or request state, available duration, exact
workspace request number and capture name. This readable text is useful in notes
and issue reports. **Details → Capture source → Copy source details** additionally
copies the available source path, original ID and source request number.
Workspace numbers and runtime import namespaces alone are not portable case IDs.

Actions groups comparison, rule creation, page previews, flags and copying into
one level of submenus. Right arrow opens a focused submenu; Up/Down and Home/End
move between actions. Left arrow or Escape returns to its parent; Escape again
returns to Actions or the traffic list. Disabled comparison actions explain the
required selection or baseline. The selected baseline is identified by its
workspace request number. **Server response with client response** compares the
upstream and downstream evidence for one exchange, including all recorded edits.

**View → Columns** calls the column **Request order (#)**. It supports the existing
hide, pin, resize, fit and move actions. Upgrading an old layout inserts the pinned
column while preserving existing widths, order, visibility, pinning and splits.
Hiding it persists, and Reset columns restores the new default.

## Markers

The compact **Markers** panel shares horizontal space between its list and the
selected note. Timeline flags show marked times in the overview and zoomed range.
Click a flag or marker row to navigate without applying a filter; **Show nearby
requests** explicitly filters its trace around that time. The inspector's **Actions**
and the request context menu offer **Flag → Flag request…** and **Flag → Flag response…**.
An optional note and exact message link travel with TMCap and extended SAZ copies.
Edit, clear, delete and Undo remain in the panel; **Save copy…** preserves changes.
See [marker review](support-capture-profiles.md#developer-intake-and-export-review)
for limits, missing evidence and time interpretation.

## Timeline

Click the **Timeline** button (waterfall icon) beside **View** to replace the
list with requests aligned on a shared time axis, while keeping the inspector
available. Click it again to return to the existing list and scroll position.
The compact toggle shows when Timeline is on and retains keyboard focus when
switched. List is the default. **Order → Start time** is the default;
**Longest first** brings long requests to the top.
The path is the primary request label; host, source and state are secondary.
The full URL remains in the inspector. Row descriptions and tooltips retain
the display URL and precise duration. Chart labels use milliseconds, seconds,
minutes or hours to keep longer ranges readable.
Axis units follow the visible span so zoomed endpoints remain distinct. Path
labels preserve recorded dot segments, escapes and empty query markers.

With no requests selected, the timeline includes all current Traffic filter
matches, including offscreen entries. With a selection, it shows those selected
matches only. **Show all matches** or Escape clears the selection. Clicking
a timeline row inspects it without changing this scope; Up/Down and Home/End
navigate timeline rows. Return to List to change the selected group.
Ctrl+A selects the requests included in the current timeline range, including
offscreen rows and new traffic. Inspector actions apply to the inspected request
without changing that group. Opening source evidence refreshes the timeline,
fits the range to that request and focuses its row.
If focus moves elsewhere while source evidence loads, it stays there when the
result arrives. Timeline virtualization follows the rendered row size, including
larger text settings.

Drag the overview's window to pan, drag either edge to resize, or drag outside
the window to choose a range. Requests active at any point in the window are
included, even when they started earlier. Unavailable timing stays visible and
is counted separately. Focus the window or either edge and use Left/Right;
Shift moves farther, Home/End moves to a boundary, +/− zooms and 0 resets.
Escape cancels a drag. **Reset range** fits the scope again.
Hover or focus **?** beside the Timeline heading for the absolute start time in
UTC, timing boundaries and keyboard instructions. Click it to keep help open;
Close, Escape or clicking outside dismisses it. Help overlays the view without
moving the chart. Selection, removal and Undo/Redo shortcuts are inactive
inside help. **Compact rows** applies to both List and Timeline;
**List columns** is available only in List. Layout places the **Inspector below**
(default) or **Inspector beside** the traffic view.

Live updates preserve adjusted ranges and do not steal focus. The overview
summarizes overlapping activity; rows are virtualized across the full scope.
Timeline mode, ordering and range are temporary investigation state, retained
while switching workspaces but not saved into captures or preferences.

Bars use a recorded request start and its measured or source-recorded duration.
For measured proxy intervals the anchor is complete client request headers.
An optional darker segment begins at observed upstream response headers; it
includes subsequent forwarding and local work and is not a server-only or pure
download phase. Active stream state can outlive its HTTP request interval.
Source names and states remain visible. Mixed sources carry an explicit clock
notice; their alignment does not establish synchronized or causal chronology.
See [request timings](request-timings.md) for boundaries and imported evidence.

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

Installed extensions can add matching choices to **Body viewer** and **Headers
viewer**. Choose one explicitly to start its isolated parser. Transmog renders its
structure, selected values, tables, format metadata and byte ranges. Changing
exchange or message stage restores Auto and raw headers. Hidden visualizers stop
and release their data; returning offers **Open visualizer** to start a fresh view.
Manage packages and their declared inputs in **Settings → Extensions**. See
[extension visualizers](extensions.md#desktop-visualizers) for source provenance
and current development limitations.

Live traffic updates preserve the selected body viewer, reading position and
text or byte selection while its captured evidence is unchanged. Growing bodies,
retention changes and viewer options refresh the evidence. **Refresh preview**
also retries a failed text or image preview.

Hex reaches every retained byte with exact offsets, paging, keyboard navigation
and Go to offset. In a short inspector, scroll the message area to reach its
body controls and byte/status region, or give it more room with the list divider.
Text-preview limits do not cap Hex. Clipboard selections have
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

**Filters** opens a shared field picker directly. Method, State, HTTP version,
Traffic type, HTTP status families/codes and Fetch destination offer counted
checkbox choices. Column menus open the same picker, and each active field has
one chip with separate Edit and Remove actions. Choices remain a draft until
**Apply filter**; Escape closes the draft and returns focus. Custom contains,
equals and numeric conditions remain available under **Custom match**.

Counts describe the surrounding filters across all retained entries. Zero values
stay selectable; longer zero lists use **More values**, and selected values stay
visible when the picker is reopened. Loading or failed counts are shown as
unavailable rather than zero. Values outside exact filter limits are disclosed;
use a custom contains condition to narrow long text.

Protocol choices group recorded `HTTP/2.0` with **HTTP/2** and `HTTP/3.0` with
**HTTP/3**, for both counts and exact matches. Original recorded spellings remain
visible in traffic and inspection evidence. App-generated fallback names already
use canonical HTTP labels.

Method, Protocol, State and Fetch destination include searched observed values
alongside common choices. **Other** appears when entries have present values
outside the common choices; it includes later arrivals with unfamiliar values,
and remains visible while selected even when its count becomes zero. Missing
method, protocol and fetch-destination evidence have separate choices. Status also
offers all observed codes; its **Other** matches codes outside 100–599, since the usual
families already cover every code in that range. Other overlaps the individual
observed values it includes, just as status families overlap individual codes.

Host, Process and normalized Content type offer searched observed choices with
contextual counts. **Load more** continues the list; after 200 observed values,
search narrows the remaining set. Selected values remain visible outside the
searched page. **Use exact value** accepts a new value before it appears in traffic;
custom Contains and Equals remain available. Content type choices omit parameters
and compare without case. **No process name** means no captured process name,
including remote callers; **Unavailable status** and **Missing content type** are
explicit evidence states, distinct from zero or the literal text of those labels.

Duration and size ranges use inclusive From/To bounds with milliseconds, seconds
or minutes and bytes, KiB, MiB or GiB. Blank ends are unrestricted; decimal values
must resolve exactly to whole milliseconds or bytes. Request-order ranges retain
the complete positive unsigned 64-bit decimal value. Start-time controls use local
calendar time, preserving milliseconds; the picker names the time zone. Skipped
daylight-saving times reject, and repeated times choose the first occurrence.
Presets resolve when chosen and retain a fixed lower bound for later traffic.
Unavailable starts never appear as an observed epoch-zero timestamp. Existing
incident-time restrictions continue to apply alongside these filters.

The row context menu and selected request's **Actions → Filter by** open the same
picker with Host, Process, Status or Content type selected as a draft. Apply is
explicit. Existing selection and inspection remain in place while choosing values.

**View → List columns** offers optional **Stream sent** and **Stream received**
columns. These count observed decoded message bytes, including controls/events;
ordinary HTTP entries show unavailable. **Request size** and **Response size**
continue to describe HTTP bodies. New stream columns start hidden and unpinned;
upgrades preserve existing widths, order, visibility, pins and pane splits.

Filtering preserves Follow newest or the paused inspection state. A following
empty live view automatically displays later matches; a paused view offers new
matches through **Show updates**. Saved viewers remain static, and Search contents
keeps its completed snapshot until searched again. Filters never clear themselves.
Duration uses established stream lifetime for WebSocket/SSE and the HTTP interval
for ordinary requests; **HTTP duration** remains available as a separate column.
The HTTP waterfall and Timeline retain their original HTTP timing boundaries.

Traffic query metadata supports exact typed alternatives: choices within one
dimension use OR, and distinct dimensions and existing text/numeric constraints
use AND. Traffic type (HTTP, WebSocket or SSE) is independent of HTTP version and
stream state, and dimensions do not need visible columns. Common status families
are alternatives alongside exact numeric codes, so selecting 2xx and 404 includes
both. Missing evidence is distinct from measured zero.

Contextual facet counts use all retained traffic, with the edited dimension's own
constraints excluded and all other filters, metadata text, incident window and
completed content-search snapshot retained. Counts describe traffic entries,
not stream messages, and never decode bodies. The API returns bounded options
in stable order with option search and an offset continuation. Each request is
a fresh metadata snapshot; callers restart paging after scope or capture changes.
Fixed presets can merge absent options with zero counts without disabling them.
The picker and backend share `crates/app/src/sessions/filter-catalog.json` for
categorical presets and evidenced external protocol spellings. Internal names
are canonical at their producer; they do not need alias lists. Counts and result
selection use the same normalization and Other predicate, including values beyond
the currently displayed option page. Unknown external Fetch destination values
remain selectable instead of being rejected by a fixed vocabulary.


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
Escape clears the selection, including when a Traffic toolbar control is focused,
so Timeline shows all matching requests again. Editors, menus and dialogs retain
their own Escape handling. Clicking blank space beside or below the columns also
clears the selection and focuses the list. With no focused entry, the first Up or
Down arrow selects the first entry.
Selecting or dragging across Traffic rows does not highlight
their text. Headers, bodies, the full URL popover and reports remain selectable
for copying. **Remove selected entries** / Del and **Remove unselected
entries** operate across the whole workspace, including hidden rows. **Clear all** /
Ctrl+X clears the window's traffic when the list is focused. Undo / Ctrl+Z restores
removed entries while their underlying evidence is retained; Redo / Ctrl+Y removes
those restored entries again, leaving subsequently captured traffic in place.
Ctrl+Z and Ctrl+Y also work while the Undo or Redo button is focused. A new removal
clears Redo history. Redo preserves unrelated selected entries and leaves an empty
selection empty. If selection or
focus changes while Undo is pending, the restored entries appear without replacing
the newer selection or moving focus. Text fields retain their ordinary editing shortcuts.
These actions do not delete source capture files or saved responses.

With the traffic list focused, F5 resumes **Follow newest**: newest-first ordering,
the latest entries in view, and automatic updates as matching requests arrive.
Existing filters still apply; selecting an entry or scrolling away pauses following.
Resuming supersedes older row navigation that is still loading, while allowing its
selected request's inspector to finish without scrolling away from the newest traffic.

Clear and Remove keep open or connecting live WebSockets visible by default. **Settings →
Preferences → Retention → Terminate open WebSockets when clearing Traffic**
changes these actions to terminate the affected connections before removing their
rows. Connections still opening stay visible until they can be terminated; newly
opened connections outside that operation remain visible. Undo restores
retained evidence; it never reconnects a terminated WebSocket. **Cancel request**
can stop one connection while keeping its row.

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

Use **Open in Format converter** from selected text or a request/response header
row to decode, encode or hash the value while preserving the source. Header rows
supply the complete value of that occurrence, including values longer than their
display preview. See [Format converter](format-converter.md) for supported
operations, temporary drafts and independent windows.

The main field defaults to a live URL and process filter across all retained
metadata. Switch to **Search contents**, then press Search or Enter to inspect
headers and bodies. Content searches remain snapshots; filtering never decodes
bodies on each keystroke.

**Inspector below** is the default layout, with the traffic view above request
and response inspectors. Narrow panes show one message and a Request/Response
pill. Its filled half identifies the active message; click either half or use the arrow keys to switch.
Headers show Name and Value by default; **Header options** reveals byte sizes
and ordering. **Body options** groups decoding and preview limits.

**Details** groups Timings, Connection, Certificates, and imported Capture source.
**Actions** groups replay, auto-response creation, page preview, and request copy
commands. **Files** groups importing and saving; **View** groups layout, density,
and columns. Bulk actions appear for multiple selected requests.

## Comparing captured evidence

For linked saved responses, **Details → Timings → Response file evidence** shows
which encoded file snapshot the matched rule selected and whether complete
client-response bytes remain retained. It distinguishes selected and captured
hashes when later processing changes output. Compare the Traffic entries to
compare historical iterations after external edits or deletion; selecting the
saved response configuration instead acquires its current file. Save TMCap while
complete evidence is retained to preserve it. See
[linked-file evidence](automation.md#linked-file-snapshots-and-historical-evidence).

Select exactly two Traffic requests and choose **Compare requests**, or use
**Set as comparison baseline** and **Compare with baseline** from Actions/the
selection menu. The baseline remains visible and clearable while selecting other
traffic. Selection order determines left and right; **Swap sides** reverses them.
Comparisons are read-only and available in capture viewers as well as the main
window. Closing returns to the original Traffic selection and scroll position.
The inspector continues to show request and response together when space permits.

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
occurrences, rather than shortened inspector text. Header order is ignored;
identical names and values are matched before spelling changes. Changed values,
original field-name spelling and added or removed duplicate occurrences remain
visible. Redacted values are unknown.
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


For a targeted edit of server text, choose **Actions → Create rule → Find and replace response text…**
from the selected request's actions. The draft starts disabled in Rules
and keeps the selected method/address. See [literal response editing](automation.md#literal-edits-to-fresh-responses)
for bounds, encoding and activation. Recorded body edits in Details offer the
original/effective response comparison with normal retained-evidence limits.

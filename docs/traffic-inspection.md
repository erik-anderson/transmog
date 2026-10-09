# Inspect and search traffic

Traffic uses one selected-item inspector for live requests and imported captures.
Selection and scroll position remain stable while new traffic arrives. Inspect
request and response together in a wide pane or use pane tabs when space is
limited. Incomplete, unavailable, redacted and evicted evidence are distinct
states; display previews can be shorter than the retained source.

## Headers and sizes

Header views preserve ordering and duplicate fields. Value and field byte sizes,
the total header size and **Largest first** help find oversized cookies or other
fields. Sizes describe the recorded HTTP headers, not HPACK/QPACK compression or
IP/TCP packet overhead. Redaction retains original lengths when measured; an
unknown imported length remains unavailable rather than appearing as zero.

The Authentication disclosure shows whether **Authorization** and
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

## Copy a request

The context menu and selection More menu offer **Copy as cURL**,
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

**Search** or Enter searches retained evidence; typing alone does not repeatedly
read bodies. **Search options** chooses metadata, request headers, response
headers and decoded text request/response bodies, literal text or regex, case
sensitivity and accent handling. Content codings and supported text encodings are decoded.
The bounded regex engine does not support backreferences or look-around.

Binary bodies are skipped quietly. Unavailable text and text over the search
budget contribute one summary count. Search limits are separate from capture
limits, so retaining a large body does not guarantee that all of it is searchable.
Results are a completed snapshot; search again to include later traffic.

**Select all matches after searching** replaces selection with the new matches on
each successful search. **Select all matches** and Ctrl+A in the result list also
work across every matching page. Column filters remain applicable. Failed or
canceled searches preserve the previous results and selections.

**View matches** opens bounded snippets of original decoded evidence, keeping
accents and showing zero-width regex positions. F3 / Shift+F3 moves between
matches; entry navigation preserves traffic multiselection. Only a bounded number
of occurrences per entry is displayed; refine the query to inspect later matches.
Changed or removed evidence requires searching again. HTML and script fragments
remain inert text in this view.

Ctrl+click adds entries, Shift selects ranges, Shift+arrow extends selection and
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

# WebSocket and server-sent events

Select an upgraded WebSocket or a `text/event-stream` response in Traffic to open
**Messages / Events**. The list updates while the connection is open. **HTTP
handshake** returns to the request and response headers and body inspectors.

The inspector names **Sent** and **Received** totals as observed decoded message
bytes. They include observed message/control/event payloads, exclude the HTTP
handshake body, WebSocket framing, compression and transport overhead, and are
separate from Traffic's HTTP Request/Response size columns. Optional Stream sent
and Stream received columns expose the same totals. A stream with no observed
payload bytes can show zero; an ordinary HTTP entry has no stream byte value.
Saved captures preserve their recorded totals, including totals for evicted
messages; missing observer records or source limitations can leave these totals
incomplete. Import never turns them into a claim about all original wire traffic.

Messages show time, sender, type and decoded size. Select one to inspect text,
JSON or hex, copy text, or save its retained bytes. SSE also exposes event name,
last event ID, retry delay and decoded field syntax (normalized to LF). The
Controls checkbox includes ping, pong, close, heartbeat, field-only SSE blocks and inspection diagnostics.
Search messages searches retained payloads, event names and IDs; direction filters
apply before windowing. Traffic's **Search contents** includes retained text
messages and SSE data under the request/client and response/server body scopes.

**Follow latest** follows incoming activity until you select a message or scroll
back. Live refresh preserves the selected payload. The list loads bounded metadata
windows; payloads load separately on selection. Arrow keys, Home and End navigate
the filtered history. Short inspector panes switch between the list and selected
payload with **Messages**; taller or wider panes show both.

## Lifetime and stopping

The HTTP handshake and application stream have separate lifetimes. Traffic's
Duration uses stream lifetime for WebSocket/SSE entries and HTTP exchange duration
for ordinary HTTP. The optional **HTTP duration** column preserves the HTTP
exchange interval; the HTTP waterfall retains its original meaning.

Stream lifetime starts only at an explicit Open observation. Connecting and
never-established streams have unavailable lifetime. Quiet live streams continue
aging on a monotonic clock; Closed, Failed and Interrupted observations stop it.
The final message time is never treated as a close time. Native saves retain
establishment, explicit end and the known observation cutoff separately from
the older first-observation timestamp. Missing older evidence stays unavailable.
Imported open streams use only a saved cutoff or recorded observed interval;
they never age against today's clock. Their interval remains open-at-capture,
not a claim that the connection closed at that boundary.

A WebSocket
remains **active** after its opening response (101 on H1, 200 on H2), including while quiet. SSE remains active
until its response ends. **Protocol** keeps **WebSocket** or **SSE** as the primary
identity for live, closed and imported rows, with the recorded HTTP version beside
it. Click that identity, or focus it and press Enter/Space, for lifecycle and timing
details plus **Filter WebSocket** / **Filter SSE** and **Filter in progress** (or
the recorded state). Opening this disclosure preserves row selection.

**Filters → State → In progress** includes quiet live streams. Closed, failed
and interrupted streams stop matching that choice. Imported open streams show
**Open at capture**, because a saved file is not a live connection. The stream
inspector shows Lifetime alongside the WebSocket handshake or SSE response-header
timing. **Timing boundaries** explains the interval and recorded timestamps.
Live lifetime and duration filters refresh while Traffic is visible even without
messages. Imported evidence never ticks; paused list ordering and inspection stay
in place while new matches are offered through **Show updates**.

Stop restores proxy routing and permits existing WebSocket/SSE streams to finish
without an inactivity deadline. Start resumes that listener while streams remain
open. Application exit cancels open streams within the shutdown bound. Idle HTTP
keep-alive, HTTP/2 and HTTP/3 connections are not application streams and cannot
hold the proxy indefinitely in its draining state.

Open or connecting live WebSockets stay in Traffic when Clear or Remove is used. The default
keeps active connections inspectable. Enable **Terminate open WebSockets when
clearing Traffic** in **Settings → Preferences → Retention** to close the affected
connections before removing their rows. A pending handshake stays visible until
its connection can be terminated; use Clear again after it opens. **Actions → Cancel request** stops one
selected connection or active response transfer while retaining its evidence.
Undo restores removed evidence without reopening connections.

## Capture and limits

Passive WebSocket inspection preserves forwarded wire bytes, reassembles fragmented
messages, and decodes negotiated permessage-deflate in each direction. Existing
message hooks retain their mutation behavior. HTTP/1.1 Upgrade and HTTP/2 extended
CONNECT use the same live inspector and Active filter. H2 clients connect through
intercepted TLS; the origin leg can use H1 or H2 independently. Auto routing uses
H2 when TLS negotiates it and the origin advertises extended CONNECT, otherwise
it falls back to H1 before sending the opening request. Forced H2 requires origin
support. HTTP/3 extended CONNECT is not supported.

SSE decoding handles arbitrary frame boundaries, content compression, UTF-8,
CR/LF/CRLF, multiline data, comments and unterminated tails. Ordinary cumulative
HTTP body and body-idle limits do not end an SSE response. Codec work, window and
expansion bounds still apply. Inspection failure is reported as an unavailable
item while forwarding continues; it never turns captured text into trusted markup.

Live inspection retains complete WebSocket payloads and SSE events while their
session is retained. Message count and history size do not silently discard older
activity. Memory can grow with the recorded session; the HTTP body buffer and
configured whole-session retention remain separate. The message list is paged,
and imported payloads load on demand. Explicit observer prefix policies and
disabled body retention can still omit content; the inspector reports that loss.
TMCap's current individual stream-content object uses a 32-bit byte length
(payload plus optional raw SSE fields and a four-byte prefix); it does not limit
the total size of a recorded session.

Request body privacy settings control subsequently retained client messages;
response body settings control server messages and SSE content. Disabling content
retention keeps lifecycle and size metadata. Existing retained traffic is unchanged.

TMCap stores lifecycle, message metadata and independently indexed
message content. Imported content is authenticated and read lazily from pinned
source files. Save trace saves the retained history and counters, including eviction
and open-at-capture state. Persistent SSE IDs are shared on disk and in memory,
so heartbeats do not repeatedly store a large earlier ID. File recording writes observed items independently of
whole-session retention. Headers only omits
payloads and SSE field content. No compatibility support is provided for earlier
experimental layouts.

SAZ includes retained WebSocket text, binary, ping, pong and close messages in
Fiddler's conventional `_w.txt` member. Frames are reconstructed from decoded
messages, without original fragmentation or compression. HTTP/2 extended CONNECT
handshakes are normalized to HTTP/1.1 upgrades for compatibility. Incomplete or
unavailable message payloads are omitted and counted in session comments.
HAR omits stream activity. Parsed SSE events require TMCap; SAZ and HAR retain
the available HTTP response body. Use TMCap for full stream lifecycle and diagnostics.

Decoded byte totals retain availability per direction. A live observer initializes both totals at stream establishment, so a quiet live stream has known zero totals. Imported state alone does not establish a total; absent counters show unavailable. An explicit saved zero or a decoded empty message establishes zero for that direction, and saving/reimporting preserves missing counters without inventing zero.

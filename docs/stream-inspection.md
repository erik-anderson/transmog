# WebSocket and server-sent events

Select an upgraded WebSocket or a `text/event-stream` response in Traffic to open
**Messages / Events**. The list updates while the connection is open. **HTTP
handshake** returns to the request and response headers and body inspectors.

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

The HTTP handshake and application stream have separate lifetimes. A WebSocket
remains **active** after its opening response (101 on H1, 200 on H2), including while quiet. SSE remains active
until its response ends. Both appear in State contains **active**. Closed, failed
and interrupted streams stop matching that filter. Imported open streams show
**open-at-capture**, because a saved file is not a live connection.

Stop restores proxy routing and permits existing WebSocket/SSE streams to finish
without an inactivity deadline. Start resumes that listener while streams remain
open. Application exit cancels open streams within the shutdown bound. Idle HTTP
keep-alive, HTTP/2 and HTTP/3 connections are not application streams and cannot
hold the proxy indefinitely in its draining state.

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

HAR and SAZ export HTTP evidence but omit WebSocket messages and parsed SSE events.
The Save dialog and exported entry comments disclose that limitation. Use TMCap to
preserve stream activity.

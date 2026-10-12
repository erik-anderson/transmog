# SAZ compatibility

Transmog records live traffic in its own append-only `.tmcap` format and
converts a sealed or crash-recovered artifact to SAZ afterward. SAZ requires a
ZIP central directory, so it is not the streaming persistence format.

The strict exporter follows the documented Fiddler archive structure:

- `[Content_Types].xml`;
- an explicit `raw/` ZIP directory entry;
- `raw/<number>_c.txt` containing the raw client-view HTTP request;
- `raw/<number>_s.txt` containing the raw client-view HTTP response;
- `raw/<number>_m.xml` containing conventional session metadata;
- `raw/<number>_w.txt` containing the WebSocket frame list, when established.

This structure is described by Telerik's
[Fiddler Archives documentation](https://www.telerik.com/fiddler/fiddler-everywhere/documentation/knowledge-base/fiddler-archives).
The ZIP implementation writes Deflate-compressed entries with ZIP64-capable
file options. The selected `zip` crate enables only its existing `flate2`
backend and its AES support. Additional compression codecs remain disabled.

Both export profiles include complete retained WebSocket text, binary, ping,
pong and close messages. The `_w.txt` member uses Fiddler's length-delimited
envelopes, directional `Request-Length`/`Response-Length`, sequential IDs, message
flags and timestamps followed by RFC 6455 frames. Session metadata sets
`IsWebSocketTunnel` (`0x40000`). These file members use the same encryption option
as the HTTP members.

Captured decoded messages become individual uncompressed final frames. Client
frames use a zero masking key only in the archive; no archive bytes are sent to
peers. Compression negotiation is removed from exported handshake headers, and
HTTP/2 extended CONNECT becomes a labeled HTTP/1.1 GET/101 upgrade. Original
header fields remain in extended evidence. `DoneRead` is the local observation
time; unavailable send times use the unset DateTime sentinel. Session comments
and namespaced flags explain these normalizations and count omitted incomplete
or unavailable observations. Message payloads are streamed into the ZIP member,
without a separate history or per-message cap. This does not reconstruct original
fragmentation, compression or remote delivery times. Parsed SSE events and full
stream lifecycle/diagnostics remain TMCap evidence; retained SSE HTTP body bytes
are still exported normally.

The import adapter accepts stored and Deflate-compressed ZIP/ZIP64 archives.
It bounds compressed input, central-directory size and counts before the ZIP
index allocates, plus individual and aggregate declared uncompressed sizes,
HTTP heads and XML metadata. It rejects unsafe member names, collisions,
unsupported compression and encryption schemes. It never extracts archive paths.
Password imports accept the ZIP library’s ZipCrypto and WinZip AES-128/192/256
(AE-1/AE-2) readers. Opt-in encrypted exports use AES-256 for every file member;
ZIP member names remain visible. Passwords stay in memory and are never saved
in preferences. Encrypted members are streamed through a sink to validate
whole-member authentication before publishing heads, then read on demand
from their original archive. No decrypted file copy is created.
Indexing reads headers and metadata. The viewer also streams chunked members
through a sink to retain validated trailers and exact entity sizes, without
allocating their bodies. Other bodies stay lazy; reads remove chunk framing and
verify ZIP CRC.
Content-Encoding and original body bytes are preserved.

The application import API publishes a complete indexed batch with per-file
trace metadata and a unique namespace, so importing the same file twice cannot
collide. Saved bodies remain in pinned, read-only source files and are opened
on demand by the same inspector, command-copy and Composer APIs used for live
traffic. They do not consume or get evicted by the live body-cache quota.
[Native TMCap](native-capture-format.md) independently indexes metadata and body
frames. Missing source protocols, terminal times or measured header lengths remain
unavailable rather than being invented or presented as zero.

Classic origin-form requests use the recorded HTTPS bit and Host header. Missing
scheme evidence is reported rather than guessed. Original start lines, ordered
duplicate headers, timing/metric attributes and session flags are preserved.
Incomplete or malformed sessions are reported individually without discarding
usable neighbors. Dropped-body flags and Content-Length mismatches prevent a
partial body from being presented as complete. Progress and cancellation are
available to the consuming application. Session flag meanings follow the
[FiddlerCore enumeration](https://www.telerik.com/fiddler/fiddlercore/documentation/api/fiddler.sessionflags).

Both export profiles write a recorded caller name and PID to the conventional
`x-processinfo` flag as `name:pid`. A known PID without a recorded name uses
`:pid`; unresolved and remote callers do not receive an invented process flag.
Import recognizes the flag without regard to its name's casing. Extended SAZ
also retains the original caller identity, including unresolved and remote states.

Exports add labeled CONNECT connection summaries for individual upstream physical
connections, deduplicated within each source trace. Recorded failed setup attempts
use 502 diagnostics, while successful connections use 200. Each row describes one
upstream connection and its associated client interception context. Their text separates
client-side interception TLS from origin connections and includes recorded
protocols, ciphers, ALPN, endpoints, physical byte totals and public certificate
details when available. A destination inferred from a matching TLS server name
and HTTPS authority is labeled as inferred. Raw CONNECT headers and ClientHello
bytes are not reconstructed. Generated summaries use HTTP tunnel flags and
Transmog attribution; importing them in Transmog restores the captured exchanges
without adding summaries as traffic or counting their text as transferred bytes.
Summaries and captured requests are interleaved by recorded start time. Summaries
without a measured setup start are placed beside the first associated request.
Generated CONNECT diagnostics omit Content-Type. StartTime and failure Timestamp
use recorded setup phases with an explicit UTC label; Transmog-Last-Observed and
directional byte counts describe the latest recorded snapshot. EndTime is omitted
when no tunnel-end event was recorded, rather than using an HTTP exchange's end
as a physical socket-close time.

Captured opaque CONNECT exchanges retain their original request headers and body
bytes. Successful responses use `200 Connection Established` and Transmog-labeled
headers describing the tunnel, recorded peer and physical byte totals when known.
Recorded setup and sampling times add UTC-labeled StartTime and last-observed
headers; a recorded terminal forwarding event also supplies EndTime. Directional
totals use ClientToServerBytes and ServerToClientBytes. Captured headers take
precedence over generated fields, with case-insensitive collision checks, so
original values and duplicate fields survive repeated exports and imports.
No TLS details are inferred for opaque traffic. Conventional `TunnelInfo` byte
counts and blind-tunnel flags make this context available to SAZ readers.
Uncaptured responses for recorded DNS, TCP or TLS setup failures use labeled 502
diagnostics; active-at-save requests use a labeled 599 diagnostic. Generated
headers and diagnostic text are removed when restoring the captured exchange.
Failure categories are stable labels, separate from their complete explanations.
Explanations retain line breaks and Unicode; archive metadata and native record
budgets bound their storage.
Strict metadata retains physical connection outcomes, including failures without
TLS details, so reimporting and resaving does not change known CONNECT results to
an unavailable outcome.

Raw SAZ response headers place Cache-Control fields before Expires fields without
changing their values or cookie attributes. Extended evidence retains the original
header order. Caching-column presentation also depends on the consuming viewer.

The `raw/` directory must be a ZIP member of its own. The published
[Fiddler DotNetZip importer sample](https://github.com/gocardless/gocardless-legacy-dotnet/blob/master/GoCardlessSdk.Tests/libs/FiddlerCoreAPI/SampleApp/SAZ-DOTNETZIP.cs)
checks for that exact member before looking for requests, so an otherwise valid
ZIP containing only `raw/<number>_*` files is rejected as a non-Fiddler archive.

Strict mode deliberately exports the client-facing request and response. SAZ
cannot represent all four Transmog boundaries or the complete attributed
hook-effect trail. `saz-extended` adds `transmog/manifest.json` with stable
native exchange IDs and completeness state; consumers expecting only classic
members should use strict mode.

Raw messages normalize HTTP/2 and HTTP/3 to textual HTTP/1.1; original protocols
remain in session flags and extended evidence. Recorded HTTP/1.0 and custom
response reasons are retained. Entity bytes are re-chunked when needed for
Transfer-Encoding or trailers. Conventional XML timers contain measured local
observations, with explicit semantic flags: complete request headers and terminal
proxy processing do not imply first client send or remote receipt. Original
imported timer attributes remain unchanged. The timer vocabulary follows
[Telerik SessionMetrics](https://www.telerik.com/fiddler/fiddlercore/documentation/api/fiddler.sessionmetrics).

Fiddler Classic's `Session.LoadMetadata` unconditionally converts six
`SessionTimers` attributes with `XmlConvert.ToDateTime`: `ClientConnected`,
`ClientDoneRequest`, `ServerGotRequest`, `ServerDoneResponse`,
`ClientBeginResponse`, and `ClientDoneResponse`. Omitting any of these passes
null into the XML date parser and prevents metadata from loading. Both export
profiles always write these attributes; an unmeasured value uses
`0001-01-01T00:00:00`, the XML representation of Fiddler's unset
`DateTime.MinValue`. This sentinel is unavailable evidence, not a measured time.
Recorded and original imported values take precedence, and optional unmeasured
timers and durations stay absent. The
[LoadMetadata API documentation](https://www.telerik.com/fiddler/fiddlercore/documentation/api/fiddler.session)
describes the metadata stream as XML.

Redacted fields are omitted from raw headers. When body bytes were not retained,
observer delivery was lost, or the body capture did not finish, conventional
`log-drop-request-body` or `log-drop-response-body` flags disclose that the wire
file is incomplete. Complete boundary captures remain complete after a later
exchange failure. Saved representations require matching retained-byte counts;
recordings retain ordered body-completion markers. The independent
`x-transmog-terminal` session flag preserves the exchange outcome in both SAZ
profiles without marking a fully retained body as dropped.
Every exchange is exported, including exchanges missing request or response
heads. Conventional `ui-comments` and `X-Transmog-Placeholder` headers explain
uncaptured heads. A missing request uses a synthetic GET to
`http://transmog.invalid/uncaptured-request`; a missing response uses
`599 Transmog response not captured`. The response body explains that this is not
a server response only when there were no retained response bytes. Existing
captured messages and retained body prefixes stay unchanged. Namespaced
`x-transmog-missing-request-head`, `x-transmog-missing-response-head` and
`x-transmog-placeholder-response-body` flags let Transmog restore missing evidence
on reimport. Extended evidence records the original optional headers, excluding
placeholder headers. Save reports and CLI output count placeholder sessions.
TMCap remains the complete original evidence format. Unsafe line breaks, invalid
statuses, body-limit overflow, entry-limit overflow, and ZIP failures stop the
conversion rather than producing ambiguous wire text.

Metadata members are bounded to 4 MiB, with the existing aggregate index and ZIP
budgets enforced before publication. Native TMCap remains the complete persistence
choice for very large captures or all four HTTP boundaries.

Use the standalone release CLI with a new destination path (no Cargo or
repository checkout is required):

```powershell
.\transmog-cli.exe capture export `
  --input .\session.tmcap --format saz --output .\session.saz
```

Choose `--format saz-extended` for extra Transmog evidence. Add `--encrypt` for
AES-256 output and an interactive password/confirmation. For unattended
conversion from encrypted native input to encrypted SAZ, supply the input
password through `--source-password-file` and the output password through
`--password-file`. ZIP member names remain visible. See the
[CLI guide](cli.md#input-and-output-passwords) for all password roles.

The CLI converter never overwrites an existing destination and publishes a
file only after successful conversion. Failures leave no partial destination.
Incomplete retained bodies are reported and marked in SAZ. The CLI conversion
limits are 1,000,000 exchanges and 256 MiB retained per direction per exchange;
its source recovery retains decoded records in memory. Native capture remains
the fidelity and recovery source even after a SAZ is produced.

Run `scripts/test-saz-interop.py` to check exports with the independent native
7-Zip implementation. It authenticates strict and extended AES-256 archives,
compares every decrypted file to its unencrypted reference, rejects a wrong
password, and checks Transmog importing a 7-Zip ZipCrypto binary fixture. Windows
can use the checksummed official portable tooling; no installation is required.
This gate runs before signing credentials are available. It also checks required
session timers and XML date parsing through .NET.

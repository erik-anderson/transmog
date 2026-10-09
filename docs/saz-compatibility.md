# SAZ compatibility

Transmog records live traffic in its own append-only `.tmcap` format and
converts a sealed or crash-recovered artifact to SAZ afterward. SAZ requires a
ZIP central directory, so it is not the streaming persistence format.

The strict exporter follows the documented Fiddler archive structure:

- `[Content_Types].xml`;
- an explicit `raw/` ZIP directory entry;
- `raw/<number>_c.txt` containing the raw client-view HTTP request;
- `raw/<number>_s.txt` containing the raw client-view HTTP response;
- `raw/<number>_m.xml` containing conventional session metadata.

This structure is described by Telerik's
[Fiddler Archives documentation](https://www.telerik.com/fiddler/fiddler-everywhere/documentation/knowledge-base/fiddler-archives).
The ZIP implementation writes Deflate-compressed entries with ZIP64-capable
file options. The selected `zip` crate enables only its existing `flate2`
backend; additional archive codecs and encryption remain disabled.

The import adapter accepts stored and Deflate-compressed ZIP/ZIP64 archives.
It bounds compressed input, central-directory size and counts before the ZIP
index allocates, plus individual and aggregate declared uncompressed sizes,
HTTP heads and XML metadata. It rejects unsafe member names, collisions,
encryption and unsupported compression. It never extracts archive paths.
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
Native files use a streaming checksummed frame index; later body reads verify
both CRC and the indexed body digest. Missing native protocol and terminal-time
fields remain unavailable. Older redacted headers with unknown sizes remain
unknown instead of being presented as zero bytes.

Classic origin-form requests use the recorded HTTPS bit and Host header. Missing
scheme evidence is reported rather than guessed. Original start lines, ordered
duplicate headers, timing/metric attributes and session flags are preserved.
Incomplete or malformed sessions are reported individually without discarding
usable neighbors. Dropped-body flags and Content-Length mismatches prevent a
partial body from being presented as complete. Progress and cancellation are
available to the consuming application. Session flag meanings follow the
[FiddlerCore enumeration](https://www.telerik.com/fiddler/fiddlercore/documentation/api/fiddler.sessionflags).

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

Redacted fields are omitted from raw headers. When body bytes were not retained,
observer delivery was lost, or an exchange did not complete cleanly, conventional
`log-drop-request-body` or `log-drop-response-body` flags disclose that the wire
file is incomplete. Exchanges without both request and response heads are
skipped and counted in the conversion report. Unsafe line breaks, invalid
statuses, body-limit overflow, entry-limit overflow, and ZIP failures stop the
conversion rather than producing ambiguous wire text.

Metadata members are bounded to 4 MiB, with the existing aggregate index and ZIP
budgets enforced before publication. Native/gzip remains the complete persistence
choice for very large captures or all four HTTP boundaries.

Use the headless converter with a new destination path:

```powershell
cargo run --locked -p transmog -- capture export `
  --input ./session.tmcap --format saz --output ./session.saz
```

The converter never overwrites an existing destination. Native capture remains
the fidelity and recovery source even after a SAZ is produced.

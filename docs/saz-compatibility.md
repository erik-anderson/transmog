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
backend and its AES support. Additional compression codecs remain disabled.

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
The library exporter counts exchanges without both request and response heads
as skipped in its conversion report. CLI export and desktop **Save trace**
reject that result instead of publishing an archive that omits those exchanges;
use native TMCap to preserve them. Unsafe line breaks, invalid statuses,
body-limit overflow, entry-limit overflow, and ZIP failures stop the
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

Run `scripts/test-saz-interop.ps1` to check exports with the independent native
7-Zip implementation. It authenticates strict and extended AES-256 archives,
compares every decrypted file to its unencrypted reference, rejects a wrong
password, and checks Transmog importing a 7-Zip ZipCrypto binary fixture. Windows
can use the checksummed official portable tooling; no installation is required.
This gate runs before signing credentials are available. It also checks required
session timers and XML date parsing through .NET.

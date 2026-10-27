# Native capture encoding

TMCap is Transmog's indexed, compressed native format. The desktop loads metadata
without decompressing bodies and reads payloads on demand. This guide explains
storage, scheduling and recovery; the [wire specification](tmcap-format.md) owns
byte layouts, encryption derivation, record schemas and reader invariants.

The mandatory document kind is `capture` for append/checkpoint recording or
`portable-archive` for a sealed, seekable ZIP payload. Capture recovery does not
repair an incomplete ZIP. The format is experimental, with no reader for earlier
container layouts; current consumers and fixtures move together.

Application saves preserve [request ordering and source numbers](tmcap-format.md#application-stream-records)
independently of exchange identity. Reopening assigns fresh workspace numbers.

Hook-effect records carry the captured `chain_position` and an optional
`response_snapshot` for frozen local response selections. Its camel-case fields
are `status`, `bodyBytes`, `sha256` and `contentCodings`; the digest identifies
content-encoded entity bytes before subsequent response hooks or HTTP no-body
handling. The hook ID retains the rule/asset revisions. Body records remain
independent evidence of the client-response output and its retention state.
Snapshot metadata never grants filesystem access and contains no external path.

Stream lifecycle and WebSocket/SSE content are independently indexed. Optional
stream-summary metadata retains establishment, terminal observation, observation cutoff
and elapsed lifetime. Its legacy `openedAt` remains first observation and is not
reinterpreted as establishment. Older summaries without explicit timing evidence
remain readable without inventing missing timestamps. Persistent SSE IDs are
stored once and referenced by later messages, avoiding repeated large fields
on disk and in reopened histories.

Circular recording also keeps stream payloads outside JSON metadata, including
in its private memory or disk scratch storage. Export preserves the compressed
content and shares persistent SSE IDs in the resulting TMCap file. When an
explicit circular budget requires eviction, finished HTTP exchanges precede
WebSockets whose handshake has completed but whose stream is still open.

## Privacy and physical layout

```text
public bootstrap | protected geometry | locator A | locator B | fixed-size blocks
                                                      packed objects / footer
```

With encryption enabled, traffic metadata, payloads, indexes and checkpoint
references are encrypted. File type, encryption parameters, filename, filesystem
metadata and total size remain visible; there is no traffic-volume padding.
Saved-file encryption does not hide runtime memory, live filesystem activity or
private circular-cache allocation patterns.

Passwords derive keys through Argon2id; AES-256-GCM authenticates encrypted
blocks and locators. Plain files use checksums that detect corruption, not
malicious rewriting. Failed writes poison the writer, and password/key buffers
are redacted and zeroized on final drop. Exact parameters and nonce/key rules
belong to [encryption and integrity](tmcap-format.md#encryption-and-integrity).

## Metadata index and lazy bodies

The index stores ordered request/response metadata separately from body blobs.
Opening reads the bootstrap and footer; populating Traffic walks compressed index
pages in file order without inflating bodies. Application memory still scales
with the metadata it displays. Writers retain only the current leaf and a
logarithmic branch frontier.

HTTP bodies use chunks of at most 128 KiB. Equal opposite-boundary chunks may
share a blob; near matches may use an earlier independent chunk as a zstd
dictionary, with at most one extra chunk read and no dependency chain. Random
reads verify the selected metadata and body blocks. Full validation checks every
referenced body. Index ranges, allocations and decompression are bounded without
a fixed total file-size or record-count limit. See the [index specification](tmcap-format.md#index-objects-and-footer).

WebSocket/SSE message metadata follows the same index. Message payloads and SSE
field text occupy independent authenticated blobs, fetched only when inspected.
Their record-size bound is separate from the 128 KiB HTTP-body chunk size. See
[stream inspection](stream-inspection.md) for lifetime, retention and format scope.

## Compression and scheduling

Reusable zstd level-1 contexts encode independent chunks. A result is retained
only if it saves at least 2% and 64 bytes; inputs smaller than 256 bytes stay raw.
Request/response headers, shared connection definitions and other metadata use
this same zstd policy on batches, exploiting repeated header names and values
across client/upstream records and exchanges. Metadata never inherits a body's adaptive bypass.

Body sharing is scoped to opposite boundaries of the same exchange and direction,
at matching captured offsets. A bounded table retains 65,536 exact-match
descriptors, and a separate 8 MiB cache holds independent dictionary bytes
(including a per-entry allowance). Near-match encoding is kept only when it
saves at least 256 bytes and 2% beyond the independent encoding, to cover its
reference. It is attempted even when ordinary body compression is bypassed.
Cache eviction or different segmentation can forgo sharing; correctness and
random access do not depend on a cache hit. The format supports shared blobs
without retaining a second full response in memory.

Each body representation has independent adaptive state, reset on a new head and
released at completion or eviction. Once at least 64 KiB has been sampled, weak
aggregate savings or two consecutive misses switch that body to direct raw
storage. The first probe can encompass a whole chunk, up to 128 KiB. This needs
no trusted Content-Length or media-type guess and works for unknown-length video.

Bypassed bodies re-probe up to 64 KiB after every 8 MiB, resuming at 4% savings.
This hysteresis avoids repeated compression attempts while allowing a stream's
content to change. With queued work, up to eight already admitted chunks may
still be compressed after the decision changes. No bytes are omitted.

Live CLI and session capture use a dedicated writer thread. On machines with at
least two available CPUs, a two-worker compression pool starts lazily after a
body demonstrates useful compression; chunks smaller than 4 KiB, initial probes,
metadata and bypassed bytes remain on the writer thread. One-CPU machines use a
single compression context. Up to eight pending records preserve write order and
bound speculation; checkpoints and seals drain them. Benchmark callers can
explicitly select 1–4 workers. See [the performance test](performance.md#tmcap-capture-benchmarks).

The observer and live-capture queues each charge up to 32 MiB of retained payload
and also enforce event-count bounds. One oversized input takes an entire byte
budget; this is flow control, not a hard process-memory cap. Lossless capture
waits for capacity and slows forwarding instead of dropping evidence. Encoder,
index, compression-context and active-exchange memory are additional bounded or
workload-dependent costs. Circular capture uses the same adaptive policy and
preserves authenticated independent compressed chunks where possible. It may
expand a chunk to compare the opposite boundary and try dictionary compression.

## Writes, checkpoints and small traces

The writer packs records into blocks and batches roughly 1 MiB of output per
normal filesystem write. It reuses compression buffers. The session worker
publishes a checkpoint about once a second, including during steady traffic and
idle periods. It writes pending data, index and footer before updating alternating
locator slots. `Write::flush` is used; TMCap does not call `sync_all`/fsync or claim
power-failure durability. The intended process-crash window is below five seconds
while the OS and writer can make progress; an indefinitely blocked filesystem
cannot meet a wall-clock guarantee.

On an interrupted file, readers prefer authenticated locators and recover the
latest complete checkpoint without scanning bodies. Generic append-only outputs
without enabled locators have a bounded backward search of 1024 blocks. A
recovered prefix is unsealed; the final seal is required to claim completion.
Trailing uncommitted bytes are reported. A valid seal does not prove all traffic
was retained, and a saved older checkpoint is not protected against rollback by
an external attacker. CLI `inspect` and `validate` explicitly check all bodies;
desktop opening uses the metadata-only path.

Small finalized captures keep exactly the same data/index/footer structure but
use 1 KiB blocks. A bounded candidate buffer retains up to 256 KiB each of
encoded records and estimated metadata. Before any OS publication, finalization
can directly emit this compact representation. For already checkpointed live
files, stop writes a separate compact file with fresh salt/key material and
atomically replaces the original only after the replacement is complete. A
failed optional replacement leaves the sealed original intact. There is no
in-place ciphertext rewrite. Larger captures discard the candidate buffer.

Roughly 5 KiB overhead is a goal for small traces, not a validity limit. Regression
fixtures cover empty files and incompressible bodies through 64 KiB in both
encryption modes. Metadata/index content and per-block tags still consume space;
large or metadata-heavy captures are not forced to meet a fixed overhead ceiling.
Small compaction preserves metadata-only opening and lazy body reads.

## Retained evidence

Public X.509 certificates and immutable transport/TLS facts use shared,
SHA-256-addressed definitions. Records refer to those definitions instead of
repeating DER or connection metadata per exchange. Changing facts create a new
connection-context definition; circular finalization emits only definitions
needed by retained exchanges. Metadata-only opening resolves them without body
decompression. The Rust API expands references into its performance model.
See [certificate records](tmcap-format.md#application-stream-records) and
[shared connection context](tmcap-format.md#shared-connection-context).

No private key or TLS session secret belongs in the capture schema.

Performance records preserve measured timing, transport evidence and actual HTTP
versions at all four request/response boundaries, including HTTP/1 minor versions.
Record schema, redaction, body-retention limits and incomplete-evidence semantics
remain independent of the container layout. Encryption stays opt-in. Independent
pinned file cursors allow concurrent body readers.

## Runtime body memory

The live memory body store compares a new chunk with the retained range at the
opposite boundary of the same exchange. Equal bytes reuse immutable allocations;
modified bytes own new allocations. Matching works while streaming, in either
direction, across different or interleaved chunk boundaries. There is no lookup
or deduplication across different exchanges. Request and response pairs are
separate. Newly owned prefixes are copied at their retained size so a short
retention limit cannot pin a much larger observer sample.

The aggregate quota charges each allocation once, including while a read lease
keeps it alive after rows are removed. Per-boundary retained lengths and hashes
remain independent. Eviction, clearing and leases cannot invalidate the other
boundary's shared bytes. Disk-cache policy is unchanged. Imported TMCap bodies
remain file references and are decoded only for an active read; they are not
loaded into the live memory cache during import.

## Support incident extensions

The artifact-level `incident-marker` extension uses sequence/exchange ID zero and
a bounded versioned payload, with independent marker/run identity, UTC/monotonic
time, origin, creation kind and plain note. It shares serialized writer/checkpoint
ordering, compression and encryption. Circular metadata is retained independently
of evicted HTTP exchanges. Native import/save/merge preserves original marker
identities inside source context. See [the incident contract](support-capture-profiles.md#recording-and-incident-durability).

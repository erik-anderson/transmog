# Native capture encoding

For byte layouts, cryptographic derivation, record and index schemas, and reader
invariants, see the [TMCap format specification](tmcap-format.md).

Application saves may include an exchange-scoped `request-order` extension with
`{"version":1,"ordinal":"42"}`. The positive decimal `u64` ordinal describes
relative workspace admission order at save time, independently of observer-event
sequence and exchange identity. Import uses a complete, unique set of ordinals;
otherwise it labels its first-surviving-appearance fallback. It always assigns
fresh workspace numbers. Entry provenance may separately retain an
`originalNumber` object (`version`, exact decimal `value`, and `provenance`) for
an explicit source request number, such as a SAZ session number. These optional
extensions do not change the container schema or interpret opaque IDs as numbers.

TMCap is an indexed, block-based capture container. `TMCAP001\0`, header version
`1`, and record schema revision `1` identify the current format. Stream lifecycle
and WebSocket/SSE content are independently indexed. Persistent SSE IDs are
stored once and referenced by later messages, avoiding repeated large fields
on disk and in reopened histories. There is no
compatibility reader for earlier experimental layouts.

Circular recording also keeps stream payloads outside JSON metadata, including
in its private memory or disk scratch storage. Export preserves the compressed
content and shares persistent SSE IDs in the resulting TMCap file. When an
explicit circular budget requires eviction, finished HTTP exchanges precede
WebSockets whose handshake has completed but whose stream is still open.

## Privacy and physical layout

```
public bootstrap | protected geometry | locator A | locator B | fixed-size blocks
                                                      packed objects / footer
```

The public bootstrap contains the nine-byte magic, a little-endian u32 JSON
header length, and at most 4096 bytes of JSON. The header names `indexed-blocks`,
zstd, the cipher, KDF, salt, nonce prefix and supported KDF parameters. For
encrypted files, the public `block_size` is zero; the actual size is a u32 inside
the authenticated bootstrap proof (20 bytes including its GCM tag). Plain files
have a four-byte geometry word matching the header. Supported plaintext block
sizes are 16384 and 1024 bytes, each followed by a 16-byte authentication tag or
checksum. Two fixed 256-byte checkpoint locator slots follow the bootstrap.

Each block's **encrypted** plaintext starts with a one-byte kind and a
little-endian u32 used length, followed by payload and padding. Physical kinds are packed data (0) and footer (3).
Packed data contains body (0), metadata leaf (1) and branch (2) objects. All blocks in a file have
the same physical size. Records can share blocks or cross block boundaries;
blocks do not identify individual requests or responses.

With encryption enabled, URLs, methods, headers, timestamps, certificates,
exchange and record IDs, counts, body lengths, compression flags, index entries,
block kinds and checkpoint references are encrypted. The file type, encryption
parameters, filename, filesystem metadata and total file size remain observable.
There is no traffic-volume padding. This protects a saved file; it does not hide
live filesystem activity, runtime memory, or the allocation pattern of private
circular-storage temporary files from an observer of the running machine.

Passwords derive a 256-bit master key through Argon2id (64 MiB, three iterations,
one lane, fresh 16-byte salt). AES-256-GCM encrypts the bootstrap geometry using
master-key counter zero. HKDF-SHA256 derives domain-separated block keys using
the hash of the public header as salt and the block epoch as context. Each epoch
contains at most 65536 blocks (at most 1 GiB of plaintext). A block nonce combines
the file's four-byte prefix and its big-endian counter within the epoch. AAD binds
the exact public header, global block ordinal and block domain. Moving a block,
changing its bytes, or copying it from another file fails authentication.

Locator updates derive independent keys from fresh random 16-byte salts. Each
slot has 224 encrypted plaintext bytes plus a 16-byte tag; it commits a generation
and footer block ordinal, bound to the slot and file. Updating a slot never
reuses a block or locator encryption nonce/key pair. Unencrypted blocks and
locators have truncated SHA-256 checksums; these detect corruption, not malicious
changes by someone able to rewrite the file. Failed writer operations poison the
writer. Passwords and derived key buffers are redacted and zeroized on final drop.

## Metadata index and lazy bodies

The persisted index stores full record metadata, including ordered request and
response headers, separately from body data. Leaves target 128 KiB of expanded
metadata; branches combine 32 children. The footer contains a bounded root
frontier, record count, its own ordinal and a sealed flag. Only the current leaf
and a logarithmic branch frontier need to remain in writer memory.

Opening reads the bootstrap and final footer. It does not scan data blocks or
inflate body chunks. Populating the traffic list walks compressed index pages
in file order, so it can load request/response metadata in batches. It does not
seek to each body's data location. Memory used by the application's traffic
list still scales with the metadata it chooses to display.

An index entry contains the record ordinal, its canonical metadata and an
optional body reference. Metadata is stored only in compressed pages. Bodies
are at most 128 KiB per chunk; identical opposite-boundary chunks can reference
one blob. Nearly identical chunks can use an earlier independent chunk as a
zstd dictionary, with at most one additional 128 KiB read. No dependency chains
or whole-file decompression are needed.

Packed objects have kind, codec, expanded length and encoded length in a ten-byte
header. Bodies and index pages share physical blocks; only checkpoints pad a
partial block. Metadata-only loading may fetch neighboring body ciphertext in
the page's boundary blocks, but never decompresses body blobs. A random read
authenticates its metadata page and selected body blocks, verifies lengths and
digests, and uses the indexed frame's pinned codec. Full validation reads every
referenced body. All allocations, tree ranges, dictionaries and decompression
are bounded; defaults do not cap total file size or record count.

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

Public X.509 certificates use global `public-certificate` definitions keyed by
SHA-256; TLS evidence refers to them instead of repeating DER per exchange.
Definitions precede references, are validated and shared by readers, and remain
until their last circular-retention reference is evicted. No private key or TLS
session secret belongs to the record schema.

Stable transport and TLS facts use immutable `connection-context` definitions
addressed by SHA-256 of normalized context JSON. Performance snapshots carry a
context reference plus changing counters, measurements and request-relative
timings. Changed facts create a new definition even when the physical connection
ID stays the same. Both definitions and references are encrypted and indexed;
metadata-only opening resolves them without decompressing bodies. Each definition
has one canonical copy in the metadata index. Circular finalization emits only the
contexts needed by retained exchanges. The Rust API expands references back into
the existing performance model. See the [wire schema](tmcap-format.md#shared-connection-context).

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

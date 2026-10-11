# Native capture encoding

For byte layouts, cryptographic derivation, record and index schemas, and reader
invariants, see the [TMCap format specification](tmcap-format.md).

TMCap is an indexed, block-based capture container. `TMCAP001\0`, header version
`1`, and record schema revision `3` are unchanged: this is the initial format,
with no compatibility reader for earlier experimental layouts.

## Privacy and physical layout

```
public bootstrap | protected geometry | locator A | locator B | fixed-size blocks
                                                           data / index / footer
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
little-endian u32 used length, followed by payload and padding. Kinds are data
(0), index leaf (1), index branch (2) and footer (3). All blocks in a file have
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

An index entry includes the logical record ordinal, data offset, encoded length,
and record metadata. A body descriptor includes its retained length, observed
length, boundary, completeness and SHA-256 digest. Bodies are independently
encoded chunks of at most 128 KiB. A body click, byte range or explicit body
search reads only the relevant indexed chunks and authenticates their containing
blocks. Shared-block corruption can affect another record in that same block.
Expanded-size and digest checks apply before exposing body data. There is no
whole-file decompression or plaintext temporary-file requirement.

Data records retain bounded metadata/body pairs. Each inner frame has a u32
encoded length and CRC32 followed by a u64 counter, u32 expanded length, one-byte
codec (raw or zstd), and payload. These descriptors are inside the outer block
encryption. The outer block is the authentication boundary. Explicit full
validation checks indexed metadata against data records and verifies every body.
The public indexed-frame API takes the complete original seekable source,
positioned at the indexed offset; a slice containing only the record is insufficient.

Index objects have a codec byte and u32 expanded length before their payload.
Readers bound header/KDF parameters, index objects (16 MiB), individual metadata
and body allocations, tree depth, node ranges, certificate and connection dictionaries, and
caller-specified file/record limits. Unknown encodings are rejected. Defaults
do not cap total file size or record count.

## Compression and scheduling

Reusable zstd level-1 contexts encode independent chunks. A result is retained
only if it saves at least 2% and 64 bytes; inputs smaller than 256 bytes stay raw.
Request/response headers, shared connection definitions and other metadata use
this same zstd policy. Data records compress metadata independently, while index
pages compress batches of metadata together, exploiting repeated header names
and values across records. Metadata never inherits a body's adaptive bypass.

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
explicitly select 1–4 workers. See [the performance test](tmcap-performance.md).

The observer and live-capture queues each charge up to 32 MiB of retained payload
and also enforce event-count bounds. One oversized input takes an entire byte
budget; this is flow control, not a hard process-memory cap. Lossless capture
waits for capacity and slows forwarding instead of dropping evidence. Encoder,
index, compression-context and active-exchange memory are additional bounded or
workload-dependent costs. Circular capture uses the same adaptive policy and
copies authenticated compressed body frames when finalizing, without another
decompress/recompress pass.

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
metadata-only opening resolves them without touching bodies. Each definition
still has one data copy and one index copy. Circular finalization emits only the
contexts needed by retained exchanges. The Rust API expands references back into
the existing performance model. See the [wire schema](tmcap-format.md#shared-connection-context).

Performance records preserve measured timing, transport evidence and actual HTTP
versions at all four request/response boundaries, including HTTP/1 minor versions.
Record schema, redaction, body-retention limits and incomplete-evidence semantics
remain independent of the container layout. Encryption stays opt-in. Independent
pinned file cursors allow concurrent body readers.

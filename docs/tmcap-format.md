# TMCap format specification

TMCap stores HTTP traffic metadata and independently readable body chunks in an
indexed file. This documents the initial format implemented by
[`transmog-capture`](../crates/proxy-capture/src/lib.rs). There is no compatibility
requirement for earlier experimental layouts. The magic remains `TMCAP001\0`,
the container header version is `1`, and the record revision is `3`.

For design choices, compression heuristics, buffering and performance results,
see [the native capture overview](native-capture-format.md) and
[the compression benchmark](tmcap-performance.md). Those writer policies can
change without changing the wire format described here.

## Conventions

All fixed-width integers are unsigned and little-endian unless explicitly
marked big-endian. Offsets are absolute byte offsets from the beginning of the
file. Block and record ordinals begin at zero. `||` means byte concatenation.
JSON is UTF-8; binary values in JSON are arrays of byte-valued integers, not
base64. JSON object key order and whitespace are generally not canonical (connection
context identities below are the exception). Authenticate
the exact stored header bytes; never reserialize them to construct AAD.

Readers must preserve `u64` and `u128` JSON integers exactly. In particular,
JavaScript implementations must not round exchange IDs or timestamps through
IEEE-754 numbers. Sequence numbers describe observer events, not container
ordinals: body splitting can repeat a sequence number, and artifact records use
zero. They are not a nonce or an index key.

## File envelope

| Order | Size | Contents |
| --- | --- | --- |
| Magic | 9 bytes | ASCII `TMCAP001` followed by NUL |
| Header length | 4 bytes | Length `H`, between 1 and 4096 |
| Public header | `H` bytes | Header JSON below |
| Geometry proof | 4 plain / 20 encrypted bytes | Plaintext is `LE32(B)` |
| Locator A | 256 bytes | Mutable checkpoint slot 0 |
| Locator B | 256 bytes | Mutable checkpoint slot 1 |
| Blocks | Multiples of `B + 16` bytes | Data, index or footer blocks |

`B` is either 1024 or 16384. Let `S = 13 + H + proof_length + 512`;
block ordinal `i` starts at `S + i * (B + 16)`. A final sealed file has no
partial or trailing blocks. An interrupted file can have an uncommitted tail.

The header has exactly these fields. Unknown fields or unsupported combinations
are rejected; readers do not use arbitrary caller-supplied KDF costs.

| Field | Plain file | Encrypted file |
| --- | --- | --- |
| `version` | `1` | `1` |
| `layout` | `"indexed-blocks"` | `"indexed-blocks"` |
| `block_size` | `1024` or `16384` | `0` (actual value is protected) |
| `compression` | `"zstd"` | `"zstd"` |
| `cipher` | `"none"` | `"aes-256-gcm"` |
| `kdf` | `"none"` | `"argon2id"` |
| `salt` | 16 random bytes | 16 random bytes |
| `nonce_prefix` | 4 random bytes | 4 random bytes |
| `memory_kib` | `0` | `65536` |
| `iterations` | `0` | `3` |
| `lanes` | `0` | `1` |

In a plain file the geometry proof must equal the public `block_size`. In an
encrypted file its authenticated value selects the geometry. A reader must
validate the proof before using it for allocation or seeking.

## Encryption and integrity

All AES-GCM operations use AES-256, a 12-byte nonce, and the full 16-byte tag.
Ciphertext is stored immediately followed by its tag. Authentication must
succeed before parsing decrypted contents or decompressing them.

The password is its UTF-8 byte sequence, with no implicit Unicode normalization.
Empty passwords are rejected. Argon2id version `0x13`, with the fixed parameters
above and a 32-byte output, derives master key `M`. There is no pepper or optional
Argon2 associated data. Generate a fresh salt and prefix for every new file,
including a compact replacement of an existing capture.

Define:

```text
AAD(counter, domain) = header_bytes || LE64(counter) || domain
Nonce(counter)      = nonce_prefix || BE64(counter)
```

The geometry proof is:

```text
AES-256-GCM(M, Nonce(0), LE32(B), AAD(0, empty))
```

Blocks use a separate key for each epoch of 65536 blocks:

```text
epoch       = floor(i / 65536)
epoch_info  = ASCII("TMCap block epoch") || NUL || LE64(epoch)
epoch_key   = HKDF-SHA256(IKM=M, salt=SHA256(header_bytes), info=epoch_info, L=32)
block_nonce = Nonce(i mod 65536)
block_aad   = AAD(i, ASCII("TMCap block"))
encoded     = AES-256-GCM(epoch_key, block_nonce, block_plaintext, block_aad)
```

HKDF means extract followed by expand, as defined in
[RFC 5869](https://www.rfc-editor.org/rfc/rfc5869). This limits one block key to
at most 1 GiB of plaintext, separates it from the geometry key, and authenticates
the file context and global ordinal. No writer may encrypt different published
block contents under the same key and nonce. The implementation is append-only
for blocks and poisons a writer after a failed append, checkpoint or seal.
It does not resume writing an existing capture after reopening it.

Plain blocks append `SHA256(block_aad || block_plaintext)[0..16]` instead of
encrypting. This is corruption detection, not authentication against an attacker
who can rewrite the file.

These primitives follow [NIST SP 800-38D](https://csrc.nist.gov/pubs/sp/800/38/d/final),
[RFC 5869](https://www.rfc-editor.org/rfc/rfc5869), and the Argon2 definition in
[RFC 9106](https://www.rfc-editor.org/rfc/rfc9106). The format's own composition
and implementation still require testing and review; use of these standards
does not itself establish that an implementation is correct.

## Block plaintext

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | Kind: data `0`, leaf `1`, branch `2`, footer `3` |
| 1 | 4 | Used payload length, at most `B - 5` |
| 5 | Used length | Payload |
| Remaining | To `B` bytes | Padding (writer emits zeros) |

Kind, used length and padding are all protected by the block operation. Padding
does not participate in logical frame lengths. The reader ignores authenticated
padding bytes. Unknown block kinds cannot satisfy an index or data reference.

## Data records and body chunks

Data-block payloads contain inner frames. A record comprises one metadata frame
and, when `body_index` is present, one immediately following body frame. Frames
can cross consecutive data blocks; they never cross an index or footer block.

| Offset in frame | Size | Field |
| --- | --- | --- |
| 0 | 4 | Encoded payload length `N`, excluding these first eight bytes |
| 4 | 4 | IEEE CRC32 of the following `N` bytes |
| 8 | 8 | Counter: `2 * record_ordinal + 1` for metadata, `+ 2` for body |
| 16 | 4 | Expanded payload length |
| 20 | 1 | Codec: `0` raw, `1` zstd |
| 21 | `N - 13` | Encoded payload |

Counters must not overflow. Inner frames are not separately encrypted in the
public container: their outer blocks supply encryption and authentication. The
private circular-storage scratch representation is an implementation detail,
not another supported TMCap file layout.

Raw payload length must equal the expanded length. Zstd output must equal the
declared expanded length and stay within configured allocation limits. Each
compressed chunk is independently decodable, without a preceding dictionary.
Writers split retained bodies into chunks of at most 128 KiB. Readers additionally
enforce their configured record bound; changing a compression level, probe
threshold or chunking policy does not change how a frame is decoded.

Metadata expands to a stored-record JSON object:

```json
{
  "revision": 3,
  "sequence": 42,
  "exchange_id": 123,
  "kind": "body-segment",
  "payload": {
    "boundary": "client-response",
    "byte_count": 5,
    "bytes": null,
    "truncated": false
  },
  "body_index": {
    "frame_bytes": 26,
    "bytes": 5,
    "digest": [44, 242, 77, 186, 95, 176, 163, 14, 38, 232, 59, 42, 197, 185, 226, 158, 27, 22, 30, 92, 31, 167, 66, 94, 115, 4, 51, 98, 147, 139, 152, 36]
  }
}
```

This example describes a raw body containing UTF-8 `hello`; its body frame is
21 descriptor/framing bytes plus five payload bytes. `body_index` is absent for
records without retained body bytes. Its digest is SHA-256 of the uncompressed
retained bytes; `frame_bytes` includes the entire body frame. A body index is
valid only on a `body-segment`, whose metadata `bytes` must be null. Empty retained
bodies and absent retained bodies are distinct. `byte_count` is observed traffic;
`body_index.bytes` is retained data. Splitting must preserve both totals.

Known kinds include `exchange-started`, `request-head`, `response-head`,
`body-segment`, `trailers`, `performance`, `hook-effect`, `route-selected`,
`route-attempt`, `loss`, `completed`, `failed`, `hook-initialization-skipped` and
`seal`. Boundary names are `client-request`, `upstream-request`,
`upstream-response`, and `client-response`. Headers preserve ordering and
duplicates. See `StoredRecord`, the payload types, `store`, and `load` in
[`lib.rs`](../crates/proxy-capture/src/lib.rs) for the complete record schema.
Unknown kind strings and their JSON payloads are preserved; unsupported record
revisions are rejected.

The `public-certificate` extension has `{ "sha256": lowercase_hex, "der": [...] }`,
uses exchange ID zero, and precedes references in TLS performance evidence. Its
digest must match, and DER must parse and reencode as exactly one X.509
certificate without trailing bytes. Definitions are shared by digest. A
`body-completed` extension marks completion of one body representation.

## Shared connection context

The `connection-context` kind is a global immutable definition. Its sequence and
exchange ID are both zero, and it cannot have a `body_index`. Its payload is:

```text
{ "sha256": lowercase_hex_digest, "context": ConnectionContext }
```

`ConnectionContext` has these camelCase fields:

| Fields | Meaning |
| --- | --- |
| `leg`, `connectionId` | Client/upstream leg and physical connection identity |
| `outcome`, `tunnelDestination`, `peer`, `local` | Setup result, optional tunnel authority and endpoints |
| `tlsVersion`, `tlsResumed`, `cipher`, `alpn` | Optional negotiated TLS/transport facts |
| `tls` | Optional `TlsEvidence`: server name, certificate digest references, offered settings, trust snapshot, verification result, errors and imported details |

TLS evidence contains public certificate digest references, never inline DER.
Certificate definitions precede connection definitions; connection definitions
precede performance records that reference them. Identical contexts are stored
once. The connection ID is one part of the identity: if facts change for the same
connection, the writer emits another immutable definition. Earlier snapshots keep
their original meaning. Identical repeated definitions are accepted and do not
consume additional dictionary budget; inconsistent digests are rejected.

The digest is SHA-256 of compact UTF-8 JSON of the **typed, normalized context**:
deserialize the context schema, serialize all its fields (including null optional
fields and default empty TLS `details`), recursively sort object keys by UTF-8
lexicographic order, preserve array order, and omit whitespace. Integers use exact
decimal notation. Strings use JSON escapes for quote, backslash and control
characters (`\b`, `\t`, `\n`, `\f`, `\r`, and lowercase `\u00xx` for the
remaining controls); other Unicode characters and `/` are emitted directly.
There are no floating-point fields. Certificate `der` is excluded from the
schema and digest. The reference implementation is
[`connections.rs`](../crates/proxy-capture/src/connections.rs). Unknown context,
definition and transport-sample fields are rejected. This content digest is an
identity/integrity check, not a substitute for the container's authentication.

A `performance` payload still has `points`, `transports`, `protocols` and `work`.
Each member of `transports` now has a `context` digest reference plus these
per-snapshot fields:

```text
sampledOffsetMicros, shared, setupTimings,
dnsMicros, tcpMicros, tlsMicros, bytesRead, bytesWritten,
tcp, tcpSampledOffsetMicros, socketIo, quic
```

These retain the types and validation rules of `TransportObservation`. In
particular, setup offsets and waits are request-relative, and byte counters or
TCP/QUIC observations can change. They must not be deduplicated as stable facts.
Readers reject missing/forward context references, more than 16 transports per
snapshot, and evidence that fails the core performance/TLS shape limits. Both
writer and reader bound the context dictionary with a 256 MiB accounting budget
that charges serialized data and estimated object/key overhead per unique
definition. This is independent of the certificate dictionary and record limits;
it is not a hard bound on total process memory.

Definitions are ordinary indexed metadata, so traffic-list opening resolves them
from index pages without reading body blocks. The Rust reader returns expanded
`PerformanceEvidence` to preserve the application API. A returned performance
frame's `codec` pins only its referenced immutable contexts; use that frame's
codec when rereading it independently. Body-frame codecs pin no connection
dictionary. Full reads still check data/index agreement after resolving context.
Definitions and references receive the same encryption as other metadata.

As with all metadata, each definition has a data-record copy and an index copy;
there is no copy for each request. Private circular scratch storage uses expanded
snapshots; final TMCap output builds the dictionary from retained snapshots, so
contexts belonging only to evicted exchanges are absent. The initial format
version and record revision are unchanged.

## Index objects and footer

A leaf, branch or footer payload is an object envelope:

```text
codec: u8 | expanded_length: u32 | encoded JSON bytes
```

Codec values and length checks are the same as for data frames. A leaf or branch
object can span consecutive blocks of its kind; the footer fits in one block.
Both the encoded object and expanded index JSON are bounded to 16 MiB. Footer
JSON is bounded to 64 KiB. Writers normally batch approximately 128 KiB of
metadata into a leaf and combine 32 children into a branch.

A leaf expands to an array of entries:

```text
Entry = { offset:u64, frame_bytes:u64, index:u64, stored:StoredRecord }
```

`offset` points into a data-block payload at the first byte of a metadata frame.
`frame_bytes` is the logical byte length of metadata plus optional body frame,
excluding outer block headers, tags and padding. `stored` duplicates the metadata
so opening the traffic list requires no body-block reads.

A branch expands to an array of nodes; the footer uses the same node descriptors:

```text
Node = { block:u64, blocks:u32, bytes:u32, first:u64, count:u64, level:u8 }
Commit = { roots:[Node...], count:u64, sealed:bool, block:u64 }
```

`bytes` is the encoded object size including its five-byte envelope; `blocks`
equals `ceil(bytes / (B - 5))`. `first` is the first record ordinal and `count`
the number of covered records. Level zero is a leaf; a branch's children have
exactly one lower level. Depth is at most 15. Node, entry and commit descriptors
reject unknown fields.

Validate all arithmetic before seeking or allocating. In particular:

- Node counts are nonzero. Sibling record intervals are contiguous and exactly
  cover their parent's interval. Root intervals cover `[0, commit.count)`.
- Child physical ranges end before their parent block, and roots end before the
  footer. References cannot cycle. Branches contain at most 32 children.
- Leaf entry count and first ordinal match the node. Iteration requires every
  ordinal exactly once, in order, and exactly the committed record count.
- Entry offsets point into payloads, not headers or tags. Record ranges are
  ordered and nonoverlapping, contain at least the inner descriptor, and end
  before their leaf. Body frame lengths must fit within the whole record.
- Referenced objects and blocks have the required kinds and exact lengths.
  Body content and data/index agreement are checked when those data are read.
- A footer's `block` equals its actual block ordinal. A sealed footer has at
  least one record and is at the physical end of the file.
- A sealed capture's last record is `seal` with `record_count` equal to its own
  ordinal (all preceding records). A seal cannot appear earlier or in an
  unsealed checkpoint. Only writer finalization generates seals.

For lazy reading, authenticate the requested data blocks, decode its metadata and
body, check counters and lengths, and verify the body digest against the trusted
index. Full validation additionally compares every data record's metadata with
its index copy. Metadata-only opening intentionally does not prove that unread
body blocks are intact. `CaptureReader::sealed()` describes the selected footer;
callers must iterate and validate records before claiming full-file validity.

## Checkpoint locators and crash recovery

Each 256-byte locator is:

```text
random salt:16 | protected plaintext:224 | tag/checksum:16
```

The plaintext is `LE64(generation) || LE64(footer_block) || zeros[208]`.
Generation is nonzero. Slot number is zero for A and one for B. For encryption:

```text
key = HKDF-SHA256(IKM=M, salt=locator_salt,
                  info=ASCII("TMCap checkpoint locator key"), L=32)
aad = AAD(slot, ASCII("TMCap checkpoint locator"))
protected || tag = AES-256-GCM(key, zeros[12], plaintext, aad)
```

Generate a fresh random salt for every update, including a retry. The key is
independent of block and geometry keys. Plain locators use
`SHA256(aad || salt || plaintext)[0..16]`. An unused all-zero slot is invalid.
The writer increments generation and updates slot `generation mod 2` only after
writing complete data, index and footer blocks to the OS. It preserves the
previous slot during this update.

Opening first tries an aligned final block, then valid locator targets in
descending generation order, then a bounded backward search of 1024 blocks for
generic append-only outputs. A locator is a hint: its target still must be a
valid authenticated footer. A damaged or incomplete block is not parsed as
plaintext. Once an authenticated footer is selected, structural corruption is
an error rather than permission to silently ignore its invalid index.

The selected footer defines the committed prefix. An unsealed prefix can be
returned after a crash; trailing uncommitted bytes are reported. A sealed prefix
with trailing bytes is invalid. Recovery does not constitute whole-file
authentication and cannot distinguish an attacker restoring an older valid
checkpoint from an actual interruption. Preventing rollback would require an
external trusted state or commitment.

Checkpoints drain pending compression work and write buffered bytes through the
OS API. The product aims to checkpoint about once a second, below the requested
five-second crash-loss window while the writer and OS can make progress. It
does not use fsync or promise power-loss durability. This timing policy is not a
wire-format requirement.

## Small files, limits and security boundaries

Small finalized files use the same structure with 1024-byte blocks. An optional
compact replacement is written with fresh cryptographic parameters and published
atomically; never rewrite published ciphertext in place under its old nonce.
Approximately 5 KiB of overhead is a goal, not an encryption or validity rule.

The default per-record allocation limit is 8 MiB. Header, index, footer, depth,
certificate-size (1 MiB), and certificate-dictionary (256 MiB) limits apply in
addition to caller-selected file-size and record-count limits. Defaults leave
total file size and count unlimited. Applications must also bound retained UI
metadata and queues; a streaming file format does not bound whole-application
memory. Malformed input must produce an error rather than a panic or unchecked
allocation.

The viewer charges retained request targets, caller names, header storage,
session rows and performance metadata against its import index budget. Shared
certificate DER is charged once per import as well: it can outlive the reader's
temporary dictionaries. Body segments cannot retain more bytes than their
observation count, and accumulated counts must not overflow. A decoded saved
body may exceed its original wire size; its `body-representation` observation
count describes the original transfer, independently of segment counts.
Request and response heads must use their corresponding boundaries, and
response status codes must be in the HTTP range 100–999. Failed or canceled
imports publish no partial session/body/source state. Cancellation is checked
again under the catalog lock immediately before committing the batch; after
that commit point the batch is published atomically. Dropping the awaiting
import task also cancels its worker, which retains its concurrency slot until
it exits. Format detection and lazy body reads use the same pinned source file.

Encryption hides record kinds, exchanges, headers, timestamps, individual body
sizes, compression flags, counts, index contents and locators. It exposes format
identification, encryption/KDF parameters, total file size and filesystem
metadata. It does not pad total volume, hide a running application's write
activity, authenticate the identity of the recorder, guarantee capture
completeness, or protect an unlocked process's memory. Password strength still
matters against offline guessing. Unencrypted checksums provide no adversarial
integrity.

## Verification and implementation map

- [`container.rs`](../crates/proxy-capture/src/container.rs): blocks, writer,
  index, checkpoint selection, compaction and lazy reads.
- [`encoding.rs`](../crates/proxy-capture/src/encoding.rs): bootstrap, Argon2,
  HKDF, AES-GCM, locators and inner frame encoding.
- [`compression.rs`](../crates/proxy-capture/src/compression.rs): adaptive zstd
  and bounded compression workers.
- [`container_tests.rs`](../crates/proxy-capture/src/container_tests.rs) and
  [`review_tests.rs`](../crates/proxy-capture/src/review_tests.rs): random access,
  truncation, short writes, poisoning, bounds and malformed structural cases.
- [`fuzz/README.md`](../fuzz/README.md): reproducible sanitizer campaigns for
  malformed files, repaired checksums, structured indices, zstd, X.509 and
  encrypted-body tampering.

The test suite also compares block ciphertext with an independent BoringSSL
AES-GCM implementation on both sides of the key-epoch boundary. Fuzzing and
regressions supply evidence, not a proof of cryptographic or parser correctness.

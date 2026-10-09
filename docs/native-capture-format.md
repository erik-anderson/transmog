# Native capture encoding

Product recordings and saved traces use `TMCAP05`, with independently encoded
metadata/body pairs. The container version is independent of the record schema.
The prior encrypted layout is replaced directly; it has no migration layer.

Every logical record has a bounded JSON metadata frame, compressed with raw
DEFLATE and optionally encrypted with AES-256-GCM. A body record's metadata holds
its boundary, observed and retained sizes, completeness, SHA-256 digest and the
following encoded payload's length. Body bytes live in a separate compressed and
optionally encrypted frame. Incompressible frames are stored raw. All sensitive
index metadata, including exchange IDs, headers and body digests, stays encrypted
when a password is used. Public framing exposes lengths and encoding parameters.

Opening a native trace reads and authenticates metadata only, then seeks over body
payloads without reading, decrypting or decompressing them. The index retains
bounded descriptors and original file offsets. On-demand body reads independently
verify framing, CRC, authentication, expanded size and the indexed digest. Corrupt
body bytes are reported when accessed, rather than blocking metadata-only opening.
An incomplete final metadata/body pair is recovered as a truncated tail. Explicit
full validation still reads and authenticates every payload.

The bounded JSON file header names the container version, compression, cipher,
KDF and KDF parameters. Encrypted files use Argon2id v1.3 (64 MiB, three iterations,
one lane), a fresh 128-bit salt and a random 32-bit nonce prefix. The derived key
is 256 bits. Counter zero authenticates the header. Logical record n uses counter
2n+1 for metadata and 2n+2 for its body; counters are checked with overflow rejection.
Header bytes, counters, expanded lengths and encoding flags are authenticated.
Unknown algorithms or unsupported KDF parameters are rejected. Partial writes
poison the writer so a metadata/body pair cannot reuse an exposed nonce.

Passwords and derived keys remain in memory with redacted Debug output and are
wiped when their final owner drops. They never enter preferences or trace metadata.
Encryption is opt-in. Filenames and file sizes remain visible. Independent pinned
file cursors support indexed seeks without expanding a plaintext temporary trace.
An optional outer `.tmcap.gz` wrapper requires container expansion and therefore
opens more slowly; encrypted expansion still contains ciphertext frames.

The desktop defaults to chunk-compressed `.tmcap`. Password prompts appear only
when required, and password fields are cleared on close. CLI circular retention
keeps metadata/body pairs together, then writes retained exchanges at stop.

Trace output has no fixed file-size or record-count ceiling. Chunks have bounded
encoded/expanded lengths. Metadata indexing retains its independent memory bounds.
Callers can explicitly request a finite input or recording budget; the desktop and
guided CLI do not impose one.

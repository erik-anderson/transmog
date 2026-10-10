# Native capture encoding

Product recordings and saved traces use `TMCAP001`, with independently encoded
metadata/body pairs. The container version is independent of the record schema.

The preamble is nine bytes: ASCII `TMCAP`, a three-digit container version and
a NUL terminator. The JSON file header uses numeric version `1`.

Every logical record has a bounded JSON metadata frame, compressed with zstd
and optionally encrypted with AES-256-GCM. A body record's metadata holds
its boundary, observed and retained sizes, completeness, SHA-256 digest and the
following encoded payload's length. Body bytes live in a separate compressed and
optionally encrypted frame. Incompressible frames are stored raw. All sensitive
index metadata, including exchange IDs, headers and body digests, stays encrypted
when a password is used. Public framing exposes lengths and encoding parameters.

The desktop indexes a native trace by reading and authenticating metadata only,
then seeking over body payloads without reading, decrypting or decompressing them.
The index retains
bounded descriptors and original file offsets. On-demand body reads independently
verify framing, CRC, authentication, expanded size and the indexed digest. Corrupt
body bytes are reported when accessed, rather than blocking metadata-only opening.
An incomplete final metadata/body pair is recovered as a truncated tail. Explicit
full validation still reads and authenticates every payload.

CLI `capture inspect` and `capture validate` read and verify body payloads one
frame at a time, releasing each payload before the next frame. Inspection keeps
exchange IDs for its unique-exchange count; validation does not. These commands
therefore check body integrity as well as metadata, unlike metadata-only desktop
opening. A valid final seal does not imply that all traffic or body bytes were
retained. See [CLI inspection and recovery](cli.md#inspect-validate-and-recover-a-native-capture).

The bounded JSON file header names the container version, compression, cipher,
KDF and KDF parameters, permitting explicit algorithm changes. Encrypted files
use Argon2id-derived keys, fresh salt, a random nonce prefix and separate checked
counters for metadata and payload frames. Header, framing and encoding parameters
are authenticated. Unknown algorithms or unsupported KDF parameters are rejected;
a partial write poisons the writer to prevent nonce reuse. The
[encoding implementation](../crates/proxy-capture/src/encoding.rs) owns the exact
header schema, derivation parameters and framing.

Passwords and derived keys remain in memory with redacted Debug output and are
wiped when their final owner drops. They never enter preferences or trace metadata.
Encryption is opt-in. Filenames and file sizes remain visible. Independent pinned
file cursors support indexed seeks without expanding a plaintext temporary trace.

The desktop defaults to chunk-compressed `.tmcap`. Password prompts appear only
when required, and password fields are cleared on close. CLI circular retention
keeps metadata/body pairs together, then writes retained exchanges at stop.

Trace output has no fixed file-size or record-count ceiling. Chunks have bounded
encoded/expanded lengths: bodies larger than a chunk are split automatically,
so a chunk bound does not truncate the body. Oversized individual metadata records
fail explicitly. Metadata indexing retains its independent memory bounds.
Callers can explicitly request a finite input or recording budget; the desktop and
guided CLI do not impose one.

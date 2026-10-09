# Native capture encoding

New product recordings and saved traces use `TMCAP04` with independently encoded
frames. Legacy `TMCAP01` artifacts and schema revisions 1–3 remain readable.
The record schema remains revision 3; the container's version is independent.

Each record is serialized, compressed using raw DEFLATE, and optionally encrypted
with AES-256-GCM. Incompressible records are stored raw rather than expanded.
A frame has the existing length/CRC envelope plus an authenticated counter,
expanded length, encoding flag and payload. Both encoded and expanded sizes are
bounded before allocation. A partial final frame is recoverable; complete corrupt
frames are errors. An interrupted trace is clearly marked unsealed.

The bounded JSON file header names the container version, compression, cipher,
KDF and KDF parameters. Encrypted files use Argon2id v1.3 (64 MiB, three iterations,
one lane), a fresh 128-bit salt and a random 32-bit nonce prefix. The derived key
is 256 bits. A header authentication tag validates the password before publishing
traffic. Each frame uses a unique 64-bit counter appended to the file's nonce
prefix, with the header, counter, length and encoding flag as associated data.
Counter zero is reserved for the header proof. Unknown algorithms or unsupported
KDF parameters are rejected, allowing explicit future codec migrations without
silently interpreting unknown cryptography. Partial write failures poison the
writer to prevent reusing an exposed nonce.

Passwords and derived keys stay in memory, have redacted Debug output and are
wiped when their final owner drops. Passwords never enter preferences or saved
trace metadata. Encryption is opt-in. Filenames and file sizes remain visible;
the capture's traffic and trace metadata are inside encrypted frames.

Import indexes metadata in a bounded streaming pass. Body bytes are discarded
after each frame, retaining offsets, physical indexes, codec context and digests.
Later body reads seek to and authenticate/decode just the referenced frames.
The reader pins an immutable file boundary and independent cursors, rather than
memory-mapping or expanding the entire capture. This keeps memory bounded and
avoids a plaintext temporary file for an encrypted native trace.

Legacy `.tmcap.gz` remains supported and needs a bounded container expansion;
new `.tmcap` files already compress their individual frames. Encrypted `.tmcap.gz`
expansion contains encrypted frames, never a fully decrypted capture.

The desktop’s Save trace defaults to this chunk-compressed `.tmcap` container.
An outer gzip wrapper is optional and explained as slower to open. Password
flows ask only when required and never retain passwords in UI preferences.

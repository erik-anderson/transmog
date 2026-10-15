# Transport dependency patches

These are complete registry source archives, with small opt-in extensions for
Chromium serialization and HTTP/3 TLS application settings. The BoringSSL
provider remains the single pinned provider in the workspace.

`sources.json` pins each registry version and the archive checksum from the
original Cargo.lock. Original licensing and copyright files remain in each
crate. `patches/` is the complete local delta, and each generated snapshot has
a receipt covering its source, patch and every file. Portable build preflight
rejects modified or incomplete snapshots.

Desktop Credits and generated third-party notices include these path dependencies.
The CycloneDX SBOM records the original registry archive and its checksum as the
ancestor, embeds the exact local patch, and records patch and snapshot-receipt
checksums. A `patch_sha256` package-URL qualifier distinguishes the patched build
from the registry release of the same version.

Verify with `python scripts/vendor-transport.py --check`. Regenerate with
`--regenerate`, optionally supplying `--archive-dir` pointing at Cargo's registry
cache. Regeneration verifies checksums, rejects unsafe archive entries, applies
patches in an isolated staging tree and replaces each snapshot recoverably.

To change a patch, edit the vendor tree, then use `--record-patches --archive-dir
PATH`. This records the entire difference and regenerates the snapshots; review
the patch before committing. To roll upstream, update the exact version and
independently verified registry archive checksum in `sources.json`, adjust the
workspace version and patch, regenerate, update Cargo.lock, and run the affected
wire/interoperability tests and local Chromium comparison. Remove each local
patch as soon as an upstream release exposes the required capability.

Run `python scripts/test-transport-patches.py` for focused quiche regressions in
an isolated source copy. It verifies the shared TLS pins and uses a separate
test-only lockfile (including upstream development dependencies). Use
`--update-lock` after a roll and review that lockfile separately; it never
changes production Cargo.lock. `--package` and `--filter` select other upstream
unit checks.

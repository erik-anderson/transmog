# Portable investigation test corpus

Synthetic fixtures for [revision 0](../../../docs/portable-investigations.md).
All content, identifiers, notes, tokens, paths and passwords are synthetic and
public test data. No customer capture, private key or real credential is included.
Git attributes preserve every corpus file byte-for-byte across platforms; JSON
and text-body hashes must not change through Windows newline conversion.

`catalog.json` inventories file hashes, lengths and expected parser/import
outcomes. Python checks fixture consistency; the Rust reader and application
tests below exercise production behavior. Authored CLI examples remain separate
from shipped command support.

- `manifests/` contains case/rule manifests. Their objects live
  in the shared `objects/sha256/` fixture store; this corpus root is not itself a
  standalone case working directory. A consumer must stage only one manifest's
  declared closure when testing an actual case.
- `packages/simple.tmcase` contains an internal Stored ZIP payload fixture.
  `zip64-streamed.tmcase` also uses actual ZIP64 local size fields and streaming
  data descriptors; its logical members match the simple example.
  Other packages deliberately contain unsafe names, duplicate members, unlisted
  content or corrupt bytes. Never extract fixture member names directly.
  These raw archives are not supported transfer files. Package tests wrap them
  in valid native TMCap containers to exercise the inner archive validation.
- `seeds/` contains real current-format TMCap files. Two reuse root/exchange IDs
  and labels but contain different body bytes. They include ordered duplicate
  and non-UTF-8 header values, a retained response prefix, loss markers and SSE
  lifecycle/content. The third is password-encrypted using the existing native
  writer. Its public fixture password is `portable-fixture-password`.
- `examples/match-boundaries.json` shows the intended structured match/unknown
  distinction. It is an authored result example, not output from a shipped command.
- `manifests/network-context.json` adds an inert synthetic configuration snapshot
  with Unicode/CRLF output, collection origin/purpose and partial-tool failure.
  It collects no real machine configuration and does not rewrite source captures.

An encrypted TMCap source object alone does not demonstrate whole-case encryption.
Artifact/package tests separately qualify the typed outer wrapper, unchanged
inner ZIP, authenticated seek, independent ZIP reading, hostile archives and
transactional publication. Current native seeds require the mandatory document
kind discriminator and are regenerated with their hashes/manifest references.

The native reader tests compare every stored observation with authored expected
records, check wrong/missing passwords and authenticated sealing, and distinguish
the two colliding sources. They test only the current format, with no historical
TMCap translation or compatibility corpus.

From the repository root, with Python 3.11+ and configured native tools:

```powershell
python scripts/dev-env.py --check
python scripts/portable_fixtures.py --check
python -m unittest discover -s scripts/tests -p test_portable_fixtures.py
python scripts/dev-env.py --run cargo test --locked -p transmog-capture --test portable_seeds
```

To intentionally regenerate the current native seeds and their dependent corpus:

```powershell
python scripts/dev-env.py --run cargo test --locked -p transmog-capture --test portable_seeds regenerate_portable_capture_seeds -- --ignored --exact
python scripts/portable_fixtures.py --write
```

Native encryption/container salts are random, so seed regeneration changes
artifact hashes, manifests and packages even when observations stay the same.
Review the complete generated diff and rerun both gates. Ordinary tests and
`--check` never rewrite fixtures. Both modes reject unlisted files. When changing
seed hashes, explicitly review and remove obsolete generated objects before
`--write`; the generator never deletes files. Refresh these seeds when the
experimental native format changes; do not add migration readers for this corpus.

Run the [production reader and application checks](../../../docs/testing.md#portable-investigations)
after changing the corpus. These cover rule imports with raw duplicate headers
and exact encoded bodies, case history and interrupted saves, and encrypted
packages with hostile inner archives.

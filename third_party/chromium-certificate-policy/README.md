# Chromium certificate policy sources

Public source snapshots from Chromium 154.0.8037.99. The unmodified SPKI
blocklist is compiled into the shared verifier; the CT log JSON and wire schemas
generate ct_config.pb, the verifier's bundled CT baseline. CT signatures and
the pinned Chromium policy are enforced for public paths while the list is
timely. The BSD license is retained in Credits and the
third-party notices. No signed Google or Microsoft updater package is bundled.

Use `python scripts/update-chromium-certificate-policy.py --chromium-version
154.0.8037.99` to refresh the sources from one exact release, then qualify the
result. `--check` verifies hashes and reproduces the generated protobuf offline
without modifying files. `scripts/test-chromium-certificate-policy.py` exercises
the read-only check. See [the verifier contract](../../docs/chromium-verifier.md)
for freshness and sourcing.

# Chromium certificate policy sources

Public source snapshots from Chromium 154.0.8037.99. The unmodified SPKI
blocklist is compiled into the shared verifier; the CT log JSON/schema supplies
the baseline for subsequent CT integration. Including CT source data does not
by itself mean CT is enforced. The BSD license is retained in Credits and the
third-party notices. No signed Google or Microsoft updater package is bundled.

Use `python scripts/update-chromium-certificate-policy.py --chromium-version
154.0.8037.99` to refresh the sources from one exact release, then qualify the
result. `--check` verifies hashes offline without modifying files.

# Chromium root-store data

This snapshot is generated from public Chromium source at commit
`6baa586063d26e723f468ec8f213069fdb2d120c`, root-store version 41. The original
source files, schema and BSD license are included here. This is not Google's
PKIMetadata updater package. Preserve the full LICENSE in source and binary
distribution notices; the supply-chain generator includes it in Credits.

Run `python scripts/generate-chromium-root-store.py` with Python 3.11 or newer to
reproduce `crs.pb` and `snapshot.json` offline. The generator resolves every
certificate hash against the two public PEM bundles, preserves the represented
policy fields, and fails on unsupported fields or syntax. `snapshot.json` pins
the source revision and input/output SHA-256 hashes. When updating the bundle,
fetch all five inputs from one immutable Chromium revision, update the generator's
revision, regenerate, and qualify the resulting TLS offer.

Version 41 produces the 28 classical Trust Anchor IDs captured from Chromium
154.0.8037.99. Certificates are untrusted path-building helpers only. Chrome's
root policy, additional certificate trust, EV, constraints and Merkle Tree
Certificate (MTC) trust are not installed or claimed by Transmog.

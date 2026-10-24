# BoringSSL PKI

The unmodified PKI target from BoringSSL commit
`427ec40cc8edc545253289232e885708c409e5ea`, the submodule used by our
`cloudflare/boring` revision `2bea8245d9b21776bc9f767b4e82dc2a78a5a262`.
Chromium uses this PKI implementation for certificate path building and RFC 5280
verification. The Rust binding currently builds SSL and crypto but does not
expose this separate target. We compile PKI only and link it to that same crypto
instance; there is no second crypto implementation or legacy verifier fallback.

Run `python scripts/vendor-boringssl-pki.py --check` to verify the byte hashes.
To regenerate, pass `--source` pointing to the matching BoringSSL checkout.
The checkout must be clean. Generation validates a fresh sibling staging tree
before replacing the snapshot, removes obsolete upstream files and restores the
previous directory if publication fails. Checks reject unlisted files and links.
`manifest.json` records the upstream source list and exact file hashes. Update
this snapshot together with the pinned boring family, never independently.

Upstream: https://boringssl.googlesource.com/boringssl/+/427ec40cc8edc545253289232e885708c409e5ea/pki/
Licenses are in `LICENSE` and each original source header.

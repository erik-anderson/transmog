# Test-only ML-DSA issuer

Synthetic certificates generated with OpenSSL 3.5.7. The CA uses ML-DSA-44;
the localhost leaf uses ECDSA P-256 and has an ML-DSA certificate signature.
The leaf private key is public test material. The issuer private key was
discarded. Nothing here is installed into the operating system's trust store.

This distinguishes certificate-path signatures from the TLS CertificateVerify
signature: Chromium's QUIC ClientHello does not advertise ML-DSA handshake
signatures, but its shared certificate verifier can validate an ECDSA leaf
issued by an ML-DSA CA. The fixture has a long validity interval to avoid
clock-sensitive tests of this private-PKI algorithm behavior.

# MAPI reference provenance

`source.json` pins the 651 reference source hashes at Microsoft Office Inspectors
for Fiddler commit `7a76470574377d14cae612c507f45b34d70e05a0`. The C# source tree
is not vendored in this repository. The MIT notice remains because the bundled
TypeScript visualizer uses protocol enum data derived from this reference.
Desktop Credits names that shipped enum data rather than a .NET runtime.

Ordinary app builds and synthetic tests use the checked-in TypeScript and
reference results, without .NET or a source download. Explicit fresh native
comparisons and enum regeneration obtain only the pinned reference into the
ignored `.tools/mapi-build/reference` cache:

```text
python scripts/mapi_reference.py --fetch
python scripts/mapi_reference.py --verify
python scripts/test-mapi-reference.py
python scripts/test-mapi-parity.py
```

The fetcher uses a fixed GitHub repository and immutable revision, bounded
archive/member sizes, and exact per-file SHA-256 verification before publishing
the cache. It extracts only the recorded inventory and never modifies tracked
sources. A changed or incomplete cached reference fails verification.

The small developer-only oracle and headless adapters live under
`extensions/mapi/test/reference`. They retain the independent native reader and
remove Fiddler UI/global-session fallback and legacy JSON export. Fresh native
comparisons require .NET 10; `test-mapi-parity.py --golden-only` does not.
No .NET code or reference source is packaged in `.tmext` or the application.

See [implementation and qualification](../../extensions/mapi/README.md).
Real Office capture validation remains deferred.

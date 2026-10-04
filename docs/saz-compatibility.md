# SAZ compatibility

rustymiddle records live traffic in its own append-only `.rmcap` format and
converts a sealed or crash-recovered artifact to SAZ afterward. SAZ requires a
ZIP central directory, so it is not the streaming persistence format.

The strict exporter follows the documented Fiddler archive structure:

- `[Content_Types].xml`;
- `raw/<number>_c.txt` containing the raw client-view HTTP request;
- `raw/<number>_s.txt` containing the raw client-view HTTP response;
- `raw/<number>_m.xml` containing conventional session metadata.

This structure is described by Telerik's
[Fiddler Archives documentation](https://www.telerik.com/fiddler/fiddler-everywhere/documentation/knowledge-base/fiddler-archives).
The ZIP implementation uses stored entries and ZIP64-capable file options. The
selected `zip` crate is built without optional compression, encryption, time,
or native-code features.

Strict mode deliberately exports the client-facing request and response. SAZ
cannot represent all four rustymiddle boundaries or the complete attributed
hook-effect trail. `saz-extended` adds `rustymiddle/manifest.json` with stable
native exchange IDs and completeness state; consumers expecting only classic
members should use strict mode.

Redacted fields are omitted from raw headers. When body bytes were not retained,
observer delivery was lost, or an exchange did not complete cleanly, conventional
`log-drop-request-body` or `log-drop-response-body` flags disclose that the wire
file is incomplete. Exchanges without both request and response heads are
skipped and counted in the conversion report. Unsafe line breaks, invalid
statuses, body-limit overflow, entry-limit overflow, and ZIP failures stop the
conversion rather than producing ambiguous wire text.

Use the headless converter with a new destination path:

```powershell
cargo run --locked -p rustymiddle -- capture export `
  --input ./session.rmcap --format saz --output ./session.saz
```

The converter never overwrites an existing destination. Native capture remains
the fidelity and recovery source even after a SAZ is produced.

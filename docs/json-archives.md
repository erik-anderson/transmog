# HAR and Chromium NetLog

Import a HAR or Chromium NetLog through Traffic → Import, drag-and-drop, or a
desktop command-line file argument. Imported requests use the ordinary headers,
body inspector, timings, connection and source-metadata views. NetLog is
import-only; there is no NetLog exporter.

HAR 1.x input preserves duplicate headers, HTTP versions, status text, page
context, original timings and available payloads. HAR response content is decoded
data, including base64 binary data; its original compression headers are retained
as evidence without decoding its payload twice. Missing text stays unavailable.
URL-encoded post parameters can be reconstructed; multipart parameters alone
cannot supply exact framing bytes. Save trace offers HAR 1.2 output. Required
HAR timings remain nonnegative and sum to the elapsed total; unavailable optional
timings use -1. Binary request bytes use `_transmogBodyBase64` because HAR has no
standard binary request-body encoding.

NetLog event/source IDs are resolved through the file's constants. Request heads,
response heads, redirect hops, errors and linked connection context are mapped
to existing Traffic views. Its source clock and phase durations remain labeled
as imported timing evidence. When all filtered response bytes and a successful
request end are logged, those decoded bytes are available for inspection.
Encrypted socket bytes are never reconstructed as HTTP payloads. Other bodies
and unrecorded measurements remain unavailable.

Detection reads at most the first 4 KiB: native magic, or a first JSON key of
`log` for HAR or `constants` for NetLog. Whitespace and a UTF-8 BOM are allowed.
Otherwise the extension selects the importer. ZIP alone does not identify SAZ,
so SAZ uses its extension. Reordered JSON uses its recognized extension.

`node scripts/generate-browser-archives.mjs` produces browser-generated HAR and
NetLog files in `target/browser-archives`. It serves a local placeholder site and
uses the locked Playwright Chromium installation. The NetLog switches follow
[Chromium's capture instructions](https://www.chromium.org/for-testers/providing-network-details/).
The mapping follows the [official NetLog viewer](https://chromium.googlesource.com/catapult/+/master/netlog_viewer/).
HAR semantics follow the [HAR 1.2 specification](https://w3c.github.io/web-performance/specs/HAR/Overview.html).
Regression tests use placeholder hosts and values.

After generating the samples, set `TRANSMOG_BROWSER_ARCHIVES` to the absolute
`target/browser-archives` directory and run
`cargo test -p transmog-app browser_produced_archives -- --ignored` to verify
real browser heads and response payloads with both importers.

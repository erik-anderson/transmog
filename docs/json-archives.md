# HAR and Chromium NetLog

Import a HAR or Chromium NetLog through Traffic → Import, drag-and-drop, or a
desktop command-line file argument. Imported requests use the ordinary headers,
body inspector, timings, connection and source-metadata views. NetLog is
import-only; there is no NetLog exporter.

Certificate context uses the same Certificates viewer as live traffic. Chromium's
received PEM chain and nested verified path follow its
[certificate event schema](https://chromium.googlesource.com/chromium/src/+/6eb6e4909a960030aa01b17ab3cca9987b5ceae2/net/log/net_log_event_type_list.h).
Recorded browser verification, TLS errors and client-certificate counts remain
original evidence. A count does not supply missing client certificate DER.

HAR 1.x input preserves duplicate headers, HTTP versions, status text, page
context, original timings and available payloads. HAR response content is decoded
data, including base64 binary data; its original compression headers are retained
as evidence without decoding its payload twice. Missing text stays unavailable.
URL-encoded post parameters can be reconstructed; multipart parameters alone
cannot supply exact framing bytes. Save trace offers HAR 1.2 output. Required
HAR timings remain nonnegative and sum to the elapsed total; unavailable optional
timings use -1. Binary request bytes use `_transmogBodyBase64` because HAR has no
standard binary request-body encoding.

NetLog accepts both the JSON document and Chromium's typed JSONL records
(`constants`, `event`, optional `polledData`, and `end`). The parser follows
[Chromium's writer](https://chromium.googlesource.com/chromium/src/+/1793f994a9d0e0d2597375b6db8c8e9b0b7d190b/net/log/file_net_log_observer.cc).
An interrupted JSONL file retains complete preceding records and reports its
missing end marker or incomplete tail in metadata. Invalid records elsewhere
fail before adding any Traffic entries. UTF-8 BOMs, CRLF and blank lines are accepted.

NetLog event/source IDs are resolved through the file's constants. Request heads,
response heads, redirect hops, errors and linked connection context are mapped
to existing Traffic views. Its source clock and phase durations remain labeled
as imported timing evidence. When all filtered response bytes and a successful
request end are logged, those decoded bytes are available for inspection.
Encrypted socket bytes are never reconstructed as HTTP payloads. Other bodies
and unrecorded measurements remain unavailable.

Detection reads at most the first 4 KiB: native magic, or a first JSON key of
`log` for HAR, `constants` for NetLog JSON, or a typed `constants` record for
NetLog JSONL. Whitespace and a UTF-8 BOM are allowed. `.jsonl` is supported by
desktop opening, file selection and drop targets.
Otherwise the extension selects the importer. ZIP alone does not identify SAZ,
so SAZ uses its extension. Reordered JSON uses its recognized extension.

`node scripts/generate-browser-archives.mjs` produces sanitized browser-generated
HAR and NetLog fixtures in `crates/app/tests/fixtures/browser`. It serves a local placeholder site and
uses the locked Playwright Chromium installation. The NetLog switches follow
[Chromium's capture instructions](https://www.chromium.org/for-testers/providing-network-details/).
The mapping follows the [official NetLog viewer](https://chromium.googlesource.com/catapult/+/master/netlog_viewer/).
HAR semantics follow the [HAR 1.2 specification](https://w3c.github.io/web-performance/specs/HAR/Overview.html).
Regression tests use placeholder hosts and values.

Run `cargo test --locked -p transmog-app browser_produced_archives` to verify
real browser heads and response payloads with the tracked fixtures.

`node scripts/generate-browser-archives.mjs --jsonl` generates only the sanitized
Chromium JSONL fixture using `--net-log-file-format=ndjson`. The ordinary browser
archive regression checks all three formats.

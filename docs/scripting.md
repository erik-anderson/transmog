# Traffic script API

Traffic scripts export synchronous functions and import types or pure helpers
from `transmog:api`. The editor supplies the [complete declarations](../crates/proxy-script/generated/transmog-script.d.ts).
The current manifest API revision is 2. Older script contracts are not supported.

```ts
import { pass, readJson, replaceJson } from "transmog:api";
import type { ResponseBodyEvent, ResponseBodyResult } from "transmog:api";

export function onResponseBody(event: ResponseBodyEvent): ResponseBodyResult {
  if (!event.parameters.enabled) return pass();
  const original = readJson(event.body, event.response.headers);
  return {
    ...replaceJson({ original, inspected: true }),
    annotations: [{ note: "Added inspection flag", tags: ["inspected"] }],
  };
}
```

For this example, declare a boolean `enabled` parameter, allow reading and
replacing bodies and adding annotations, and include `content-type` in writable
headers. Set the response body media-type filter to `application/json`.

## Events and results

Each handler receives one immutable event containing `context`, `request`,
`parameters`, `state`, and `localResponse`. `context` includes the exchange ID,
handler name, and invocation time in Unix milliseconds. Additional fields are
present as described below; inapplicable fields are `null`.

| Export | Additional input | Available decisions |
| --- | --- | --- |
| `onRequestHead` | None | Continue, edit headers/method/URL, return a response, select a response asset, abort |
| `onRequestBody` | Complete decoded `body` | Continue, edit headers/body, discard body, abort |
| `onResponseHead` | `response` | Continue, edit headers/status, return a response, abort |
| `onResponseBody` | `response`, complete decoded `body` | Continue, edit headers/status/body, discard body, abort |
| `onCompleted` | Final `response` | Continue with evidence |
| `onFailed` | `failure` category | Continue with evidence |

Return `{ decision, annotations?, diagnostics?, state? }`. Related edits belong
in one decision, for example:

```ts
return { decision: { action: "edit", edits: {
  status: 202,
  headers: [setHeader("content-type", "application/json")],
  bytes: utf8Encode('{"accepted":true}'),
} } };
```

The host validates the entire result before applying it. An invalid field,
missing capability, exception, or exceeded limit aborts the affected exchange.
Terminal callbacks cannot change traffic; their failures become diagnostics.

Request URL edits require an absolute HTTP(S) URL without credentials or a
fragment. Cross-origin edits additionally require rerouting permission and pass
normal route authorization. Status edits accept 200–599. The engine owns Host,
content length, transfer/content encoding and connection-management headers;
scripts cannot write those fields. HTTP body-forbidden responses suppress output.

## Selection, state and limits

The native request prefilter admits a script once, before its request-head
callback. Later callbacks remain selected after request rewrites. Body filters
then select exact media types, response statuses, and whether to include local
responses **before decoding or buffering**. Empty media/status lists allow all;
local response bodies are excluded by default.

A selected body hook buffers the complete message before sending its headers.
Related header, status and body edits therefore reach the transport together.
Unmatched bodies keep streaming. Do not select indefinite streams such as SSE.
Body-forbidden responses do not invoke body handlers. Multiple body hooks see
the preceding hook's body and header edits in execution order.

Bodies are complete decoded byte arrays, never truncated previews. Input and
output body limits default to 1 MiB and can be configured up to 16 MiB;
runtime, content-decoding, heap and timeout limits also apply. Precommit buffering
is capped at 16 MiB of encoded input and output, and respects lower runtime limits.
Exceeding a bound fails the exchange instead of silently bypassing the hook.

Parameters support text, optional text choices, booleans, and bounded integers.
Defaults and saved overrides are captured with the applied revision. Tests use
those same values. They are ordinary saved configuration, not a secret store.

Returning `state` replaces this script's exchange-local JSON object; omission
retains it. State is limited to 128 keys and 64 KiB, serialized between callbacks,
and released at termination. Module globals belong to the script host and are
shared across exchanges; use `event.state` for request/response correlation.

## Helpers and evidence

- `pass`, `setHeader`, `appendHeader`, and `removeHeader` construct results or
  ordered header operations. `headerValues` and `headerText` preserve duplicates;
  withheld sensitive values throw instead of appearing as empty values.
- `utf8Encode`, `utf8Decode`, `base64Encode`, and `base64Decode` convert bytes.
  UTF-8 decoding is strict by default; base64 accepts canonical padded encoding.
- `requestUrl`, `parseQuery`, `formatQuery`, and `setQuery` handle URLs and ordered
  query pairs. Malformed query escapes fail rather than silently losing bytes.
- `readText` and `readJson` require UTF-8 and reject conflicting or incompatible
  charset declarations. `replaceText` and `replaceJson` edit both bytes and content
  type, requiring body-write and `content-type` permissions.
- `respondText` and `respondJson` create inline responses with the appropriate
  UTF-8 content type. The response capability also permits
  `{ action: "respond", assetRef: "response-id@revision" }` at request head.

Annotations require permission and contain a note, optional tags, and an optional
`info`, `warning`, or `error` severity. Each invocation can return 16 annotations
(2 KiB per note, eight tags of up to 64 bytes). Traffic shows script findings with
revision and phase attribution. Its metadata filter searches annotation notes
and tags across retained entries. Native saved traces retain annotations.

Diagnostics are explicit strings, bounded by `maxLogBytes` and 32 messages per
invocation. They appear in sandbox results and live findings but are excluded
from saved traces. Neither annotations nor diagnostics change traffic by
themselves. Script-authored messages can contain sensitive data; choose their
contents deliberately.

The sandbox provides no network, filesystem, timers, UI, dynamic imports or
string-code generation. Only the bundled `transmog:api` module can be imported.
See [authoring and activation](automation.md#traffic-script-authoring-and-execution)
for drafts, sandbox tests, applying a build, and recovery.

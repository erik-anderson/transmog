# Plan: discoverable ordered auto-responses

Status: completed 2026-10-05
Audience: maintainers, UX authors, application and automation-layer authors
Depends on: native automation, retained response bodies, response assets, and
the desktop traffic inspector

## Outcome

Transmog will make the common auto-response workflow start from captured
traffic rather than from internal asset or rule identifiers. A user selects a
completed exchange and chooses **Create auto-response**, or drags its row into
the Auto-responses workspace. Transmog copies the exact client-visible
response into an immutable response asset, pre-fills an understandable rule,
and inserts that rule at the top of a visible first-match list.

The feature remains application-layer composition over ordinary identified
proxy hooks. The proxy core owns deterministic hook ordering, local responses,
and audit evidence; it does not own projects, rule editing, persistence, or UI
state.

## Approved product decisions

1. The first URL condition is a case-sensitive exact comparison against the
   complete normalized absolute URL. The serialized condition is tagged so a
   bounded Rust regular-expression variant can be added later.
2. A captured source means the client-visible response: final status, ordered
   final headers, and exact final encoded body. Incomplete, truncated, lost,
   disabled, or evicted bodies cannot be offered as faithful sources.
3. Captured text may be edited in decoded form. The default preserves the
   original content-coding stack and re-encodes the edit, including Brotli,
   gzip, deflate, and Zstandard. The user may deliberately remove content
   encoding instead. Exact replay remains available for binary and oversized
   bodies.
4. Matching observes the original client request. Auto-response rules run
   before request mutations such as a User-Agent override.
5. Method and exact URL form the default match. Exact request-header filters
   are exposed only for POST in the initial built-in editor. Request-body
   matching is excluded.
6. Rules are visibly ordered and the first enabled match wins. New rules are
   inserted first. Drag-and-drop reorders them, while Move up/down controls
   provide an accessible keyboard alternative.

## UX flow

The Traffic inspector provides a primary **Create auto-response** action only
when the selected exchange has a complete retained client response. Every
traffic row can also be dragged to the Automation destination or its explicit
drop zone. These are shortcuts to the same pre-filled editor, not separate
behaviors.

The Auto-responses workspace explains its first-match behavior before listing
rules. Every row shows:

- evaluation position and drag handle;
- friendly name and enabled state;
- method, exact URL, and number of header conditions;
- immutable response source;
- Edit criteria, Enable/Disable, Move, and Remove controls.

The editor never asks for rule IDs, response-asset IDs, revisions, or numeric
priorities. Captured sources show their response status, retained byte count,
and content codings. Creating from scratch remains available, but captured
traffic is the recommended path because it preserves protocol-relevant
headers.

## Matching and audit contract

The application assigns unique ordered priorities in a reserved built-in
range. Native request registrations run low priority first and a local
response terminates request-hook evaluation, producing deterministic
first-match behavior. Disabled rules are absent from newly compiled snapshots.
Rule documents retain stable IDs, friendly names, monotonic revisions, and
immutable response-asset references.

When an auto-response wins, the session summary and detail models identify the
friendly rule name, stable ID, revision, evaluation position, response asset,
status, and body size. The traffic row and response inspector display a clear
`AUTO` marker and link the result conceptually to the winning rule. Captured
hook evidence remains immutable even if that rule is later renamed, reordered,
disabled, or deleted.

The protected response-asset path may opt in to exact sensitive response
headers so a cloned client-visible response remains faithful. General session
observers and presentation models remain redacted by default; exact headers
are never returned through UI DTOs, logs, or diagnostics.

## Verification gates

- exact full-URL matching distinguishes query strings and falls through to the
  next ordered rule;
- disabled rules do not match and do not require their response asset to be
  present;
- POST exact-header predicates use the original client request;
- client-visible status, duplicate headers, sensitive response fields, empty
  bodies, and encoded body bytes survive capture-to-asset conversion;
- decoded edits round-trip through every supported content coding, including
  Brotli;
- only the winning rule appears in structured session attribution;
- new, reordered, enabled, disabled, and removed rules activate atomically;
- desktop projection, TypeScript checks, native WebView smoke tests, Rust
  tests, and Clippy pass with the checked-in bundle current.

## Deferred work

- bounded Rust regular-expression URL matching and its explicit UX mode;
- request-body conditions;
- rich structured header editing for cloned responses;
- list virtualization once measured traffic volume requires it;
- bulk import/export of rule collections and conflict-resolution UX.

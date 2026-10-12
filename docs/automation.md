# Automation and auto-responses

`transmog-automation` is an optional declarative layer over the core
interception API. It is not a second execution engine and is not required by
`transmog-core`. Embedding applications may register their own
`ExchangeInterceptor` implementations instead of, or alongside, compiled
rules.

The crate compiles a finite rule set into ordinary identified registrations.
Every applied request or response edit therefore appears in the core
hook-effect audit trail under a stable identity such as
`automation/<rule-id>/<request|response>`. Rules cannot bypass interception
deadlines, cancellation, resource limits, or effect attribution.

The application layer persists validated rule generations, response assets,
and script revisions. Activation is atomic: an invalid candidate never replaces
the last working snapshot, and an in-flight exchange keeps the immutable
snapshot with which it started.

## General rule ordering

General mutation rules are ordered by ascending numeric priority and then by
stable rule ID. Higher-priority edits are applied last on both request and
response legs. The compiler rejects equal-priority rules whose predicates can
overlap and write the same header or body. Callers must give those rules
distinct priorities or make their predicates disjoint.

The desktop presents built-in operations, such as conditional User-Agent
changes, as task-specific editors. Internal IDs, revisions, and priorities are
kept out of ordinary authoring UI while remaining available for persistence,
stale-update detection, and audit evidence.

## User-Agent and client hints

Header overrides contains an ordered list of User-Agent rules. The first enabled
matching rule wins, even if the request has no headers to replace. Host filters
are exact and case-insensitive; path prefixes are case-sensitive. Drag the
handle or use Move up and Move down to change precedence. Saving filters already
used by another rule asks whether to replace that rule, retaining its position.
Deleting a rule offers Undo. List changes affect newly admitted requests.

The editor includes 48 presets in ten groups and accepts custom values. `%s`
and `%full%` resolve to the source browser's major version followed by `.0.0.0`;
`%major%` resolves to the major alone. An optional major or dotted version
overrides this automatic choice. Unknown source versions preserve fields that
need version substitution. Choosing the Googlebook preset resolves its Android
platform version using [Chromium's calendar heuristic](https://chromium.googlesource.com/devtools/devtools-frontend/+/HEAD/front_end/models/emulation/DeviceModeModel.ts): year minus 2011 through
September, year minus 2010 from October. The resolved value is saved with the
rule and remains editable, so an existing override does not change with the date.

Replacements affect existing User-Agent and selected Sec-CH-UA fields only,
preserving duplicate fields and never adding missing headers. Client hints can
be preserved, overridden from preset metadata or custom values, or stripped.
Preserve is the default when no hint overrides are authored. The Automation
navigation count includes enabled non-auto-response rules and active script
modules; Header overrides shows its enabled header-rule count.

## Ordered auto-responses

Auto-response rules form a compact top-to-bottom list beside a resizable
properties pane. The first enabled rule that matches wins and bypasses the
origin. Selecting one rule opens its properties; selecting several exposes
bulk actions through More. Search and enabled/disabled/shadowed filters preserve
the underlying priority order. Drag-and-drop or Move earlier/Move later reorders
a selected group while preserving its internal order. New rules are inserted
first. Disabled copies provide a convenient starting point for a variant.

The primary creation flow starts from a completed traffic item. **Create
auto-response** copies the exact client-visible status, ordered response headers,
and encoded body into an immutable response asset, then pre-fills a method and
exact normalized absolute-URL match.

HTTP port 80 and HTTPS port 443 are implicit in created rules; matching accepts
either spelling. Nondefault ports remain explicit. Revert restores the captured
draft and its source. Status, content type, headers and file replacement are
available before the first save, with protected header bytes retained.
The primary Automation action opens Traffic to choose responses, or uses the
current reusable response. Multiple
selected rows open a review with eligibility,
priority and duplicate explanations in a compact table. Excluded responses stay
visible, and refresh keeps their inclusion choices. The review can skip
responses or retain only the earliest or most recent response per identical
match. Creation leases
every eligible source before copying and activates the whole batch atomically;
a failed source never leaves a partially activated batch. Creating a response
from scratch is available in the secondary More menu. Captured responses remain
the primary path because they retain protocol-relevant headers.

Captured sources require a complete retained client response. Disabled,
truncated, evicted, missing, or lossy bodies cannot be represented as faithful
sources. Text can be edited in decoded form and re-encoded with its original
gzip, Brotli, deflate, or zstd stack, or the user can deliberately choose
identity output. Binary and oversized sources retain exact replay behavior.

Matching uses the original client request, before request mutations. Method and
case-sensitive exact URL are the default conditions. Captured response headers,
including Vary, do not add implicit request-header or User-Agent restrictions.
One modified response can therefore serve different browsers; add a request-header
condition explicitly when a rule should target only one client. Methods include Any and
custom tokens. Request-header conditions are available for every method;
request bodies are not inspected. Exact URLs, address patterns and bounded
regular expressions use the same backend matcher in live requests and in the
network-free editor tester. Saved expected match/nonmatch examples retain their
own method and explicit headers. Generalizing a selected path segment and
editing a placeholder show the same syntax inline.

Address patterns keep the scheme and host literal. In the path, `{id}` or `{}`
matches one nonempty segment; `{id:digits}` or `{:digits}` matches decimal
digits; a final `{path...}` or `{...}` matches one or more remaining segments.
Names are optional annotations and have no effect on matching. Repeated names
do not require equal values. Literal path portions are case sensitive by
default. Query matching is explicit: exact (including absence and ordering),
required decoded name/value pairs (including repeated names), or ignore.
Regex rules explicitly select URL or path scope, whole-value or substring
matching, and case sensitivity. Expressions compile once under finite bounds.

The persisted autoresponse gate controls registrations for newly admitted
requests without changing any rule's enabled state, revision, response or
priority. Pausing leaves other native automation active. Existing exchanges
retain their original hook snapshot. New workspaces start with the gate on.
Validation and activation reject stale generations.
The same On/Paused control appears in Automation and Traffic.

Guaranteed shadowing diagnostics compare equivalent matching behavior in
first-match order, ignoring annotations and regression examples. They identify
the earlier enabled rule; arbitrary regular-expression equivalence is not
inferred from sample URLs.

Saved response status, content type, ordered headers and text remain editable
after the original Traffic entry disappears. Body replacement from a file also
supports binary responses. Serve response body from file copies files up to
25 MB on save; larger files retain their absolute path and read current bytes for
each request. File mode disables text editing and offers Clear file choice.
Missing files retain their rule and path across restart, fail the matched request
without falling through to the origin, and explain recovery beside the rule.
Content-Length always reflects the final encoded
body bytes. Compressed edits rebuild Content-Encoding from the applied stack;
identity edits remove it. Apply original compression type names the original
stack and applies it to text and binary replacement files. Copied files store
encoded bytes; linked files stream encoding with bounded backpressure and
unknown encoded length, without reusing a stale Content-Length. Pasted framing and encoding
fields cannot override these choices in the editor. Response edits create an
immutable configuration revision; copied bytes, earlier revisions and captured evidence remain
intact. The response body editor
starts at twelve lines. The open Response section groups status, content type,
ten-line editable headers and neutral original-response context; rule usage is
shown with the rule properties. A friendly source link opens the original request when
it is still retained; otherwise the label remains plain text. Match counts and
last-match times describe only retained Traffic.

Saved-rule enabled switches apply immediately. Property edits remain drafts
until Save (Ctrl+S); Revert resets them. Changing rule selection with unsaved
edits offers Save and continue, Discard changes or Keep editing. Traffic and
rule lists share Ctrl+click, Shift ranges, Shift+arrow, Ctrl+Shift+arrow, Ctrl+A
and Escape. Focus can move independently with Ctrl+arrow. Del removes selected
list entries, while text fields retain their normal editing shortcuts. List
actions can be undone with Undo or Ctrl+Z. Traffic selection survives page and
refresh changes and reports selected entries on other pages; changing a filter
or search clears it. Removing Traffic entries hides them from the live catalog
without deleting capture files or saved responses. Undo succeeds while the
underlying entries remain retained.

When a rule wins, session summary and detail data retain its friendly name,
stable ID, revision, evaluation position, response asset, status, and body
size. The traffic row is marked `AUTO`, and the response inspector links to the
matching rule. That immutable evidence remains meaningful after a rule is
renamed, reordered, disabled, or deleted.

Scripts have their own Automation tab. Validation and sandbox testing describe
only the current source and capability settings; editing either invalidates the
candidate until checked again. Saving a draft does not enable it. Active revision
status remains visible while a newer draft is edited, and comparison or pausing
is available in More. Sandbox input can be authored or copied from a selected
Traffic request with sensitive headers omitted.

## Body and script safety

Body replacements operate on the declared decoded representation. Request-body
replacement is enabled by default only for methods known to be idempotent. A
rule must explicitly acknowledge modification of a non-idempotent request; the
acknowledgement records the risk but cannot make the operation intrinsically
safe.

Compilation enforces caller-selected finite limits for rule count, header
operations, regular-expression size, and replacement-body bytes. Scripts use
the same validated action vocabulary but run in the separately sandboxed script
host described by [ADR 0008](adr/0008-traffic-workspace-and-script-isolation.md).
Script errors abort the affected exchange and produce bounded remediation
diagnostics.

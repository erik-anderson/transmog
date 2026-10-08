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
exact normalized absolute-URL match. A row can also be dragged to the
Auto-responses workspace. Multiple selected rows open a review with eligibility,
priority and duplicate explanations. The review can skip responses or retain
only the earliest or most recent response per identical match. Creation leases
every eligible source before copying and activates the whole batch atomically;
a failed source never leaves a partially activated batch. Creating a response
from scratch remains available but is secondary because it is easier to omit
protocol-relevant headers.

Captured sources require a complete retained client response. Disabled,
truncated, evicted, missing, or lossy bodies cannot be represented as faithful
sources. Text can be edited in decoded form and re-encoded with its original
gzip, Brotli, deflate, or zstd stack, or the user can deliberately choose
identity output. Binary and oversized sources retain exact replay behavior.

Matching uses the original client request, before request mutations. Method and
case-sensitive exact URL are the default conditions. Methods include Any and
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
retain their original hook snapshot. Older workspaces default to the gate
being on. Validation and activation reject stale generations.
The same On/Paused control appears in Automation and Traffic.

Guaranteed shadowing diagnostics compare equivalent matching behavior in
first-match order, ignoring annotations and regression examples. They identify
the earlier enabled rule; arbitrary regular-expression equivalence is not
inferred from sample URLs.

Saved response status, content type, ordered headers and text remain editable
after the original Traffic entry disappears. Body replacement from a file also
supports binary responses. Response edits create an immutable asset revision;
earlier revisions and captured evidence remain intact. The response body editor
starts at twelve lines. A friendly source link opens the original request when
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

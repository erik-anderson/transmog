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

Auto-response rules form a visible top-to-bottom list. The first enabled rule
that matches wins and bypasses the origin. Drag-and-drop reorders the list;
Move up and Move down provide the keyboard-accessible equivalent. New rules are
inserted first.

The primary creation flow starts from a completed traffic item. **Create
auto-response** copies the exact client-visible status, ordered response headers,
and encoded body into an immutable response asset, then pre-fills a method and
exact normalized absolute-URL match. A row can also be dragged to the
Auto-responses workspace. Creating a response from scratch remains available
but is secondary because it is easier to omit protocol-relevant headers.

Captured sources require a complete retained client response. Disabled,
truncated, evicted, missing, or lossy bodies cannot be represented as faithful
sources. Text can be edited in decoded form and re-encoded with its original
gzip, Brotli, deflate, or zstd stack, or the user can deliberately choose
identity output. Binary and oversized sources retain exact replay behavior.

Matching uses the original client request, before request mutations. Method and
case-sensitive exact URL are the default conditions. The desktop adds
exact request-header conditions only for POST and does not inspect request
bodies. Native rules support exact URLs, address patterns and bounded regular
expressions through the same matcher used by network-free editor tests.

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

Guaranteed shadowing diagnostics compare equivalent matching behavior in
first-match order, ignoring annotations and regression examples. They identify
the earlier enabled rule; arbitrary regular-expression equivalence is not
inferred from sample URLs.

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

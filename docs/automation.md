# Optional automation

`rustymiddle-automation` is a convenience layer over Hooks v2. It is not a
second interception engine and it is not required by `rustymiddle-core`.
Applications with domain-specific behavior should continue to register their
own `ExchangeInterceptor` implementations directly.

The crate compiles a finite set of declarative rules into ordinary identified
hook registrations. As a result, every applied request or response edit is
reported by the core hook-effect audit trail using an ID of the form
`automation/<rule-id>/<request|response>`. No privileged rule path can bypass
hook deadlines, cancellation, resource limits, or effect attribution.

## Ordering and conflicts

Rules are ordered by ascending numeric priority and then by stable rule ID.
Higher-priority edits are applied last on both the request and response legs.
The compiler rejects equal-priority rules whose predicates can overlap and
which write the same header or body. Give such rules distinct priorities or
make their predicates disjoint instead of relying on incidental input order.

## Body safety

Body replacements operate on the decoded content representation selected by
Hooks v2. Request-body replacement is enabled by default only for methods known
to be idempotent. A rule must explicitly opt in to modifying a non-idempotent
request. This is a replay-risk acknowledgement; it does not make an unsafe
operation safe.

Compilation enforces caller-selected finite limits for rule count, header
operations, and replacement-body bytes. Persistence, a rule-authoring user
interface, project configuration, and hot reload belong in higher layers.

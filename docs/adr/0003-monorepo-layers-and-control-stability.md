# ADR 0003: Monorepo layers and control-protocol stability

Date: 2026-10-03

## Status

Accepted.

## Context

rustymiddle will initially build the proxy engine, content processing, control,
capture, rules, and product layers in one monorepo. Atomic changes are valuable
while these APIs are being discovered, but accidental coupling would make a
later repository split expensive. A control boundary will eventually be useful
for a developer-tool UI, yet stabilizing speculative messages before content,
capture, and rule consumers exist would freeze the wrong model.

## Decision

Crate boundaries enforce dependency direction even while all layers share one
repository. Core does not own product state, and its internal Serde forms are
not a wire protocol.

An experimental control v0 may be introduced when a consumer needs it. It may
break through coordinated monorepo changes and incompatible peers must reject
one another through a revision/build handshake.

Control-protocol stabilization has one trigger: a human maintainer explicitly
indicates that stabilizing it now makes sense. No automated condition or amount
of elapsed time triggers stability. If that human decision never occurs, v0
may remain breakable indefinitely. When it does occur, a separate ADR defines
v1 compatibility and support policy before the promise is made.

## Consequences

- Higher layers can evolve atomically without premature compatibility shims.
- A deliberate control-model boundary still prevents UI or IPC concerns from
  leaking into the proxy core.
- Same-build deployment is required during v0 unless a later explicit policy
  says otherwise.
- Persisted capture schemas must not reuse live control envelopes as durable
  storage records.
- Splitting repositories does not itself stabilize the control protocol.


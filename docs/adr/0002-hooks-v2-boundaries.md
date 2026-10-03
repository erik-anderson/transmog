# ADR 0002: Hooks v2 exchange boundaries

Date: 2026-10-03

## Status

Implemented.

## Context

The first interception API uses one `BreakpointHandler` for every exchange and
represents all phases with optional event fields plus one decision enum. It
proved that HTTP/1.1, HTTP/2, and HTTP/3 can share a canonical modification
path, but it permits invalid phase/action combinations and encourages global
state keyed by session ID.

The next API must support interactive developer tools and embedded applications
without moving capture storage, UI state, IPC, or product configuration into
the transport core.

## Decision

### One interceptor instance per exchange

An `InterceptorFactory` creates isolated interceptor instances. Instances are
`Send + Sync` and callbacks receive `&self`, because duplex protocols may
produce a response while a request body is still streaming. Implementations
that mutate per-exchange state use local synchronization, but never need a
global map keyed by exchange ID.

### Typed lifecycle methods and actions

Request head, request body planning, response head, response body planning,
completion, and failure use distinct methods and types. Completion and failure
cannot return traffic decisions. Illegal phase/action pairs are not represented
in the public API.

### Nested chain order

Requests enter interceptors in registration order and responses unwind in
reverse order. A request short-circuit enters no later interceptor. The local
response passes through response hooks for the interceptor that produced it and
all earlier entered interceptors. Terminal callbacks run once in reverse entered
order even when an earlier cleanup callback fails.

### Body behavior selected before pumping

Each entered interceptor selects a bounded body plan before the corresponding
body pump starts. The core composes pass-through, transform, buffer, replace,
and discard stages and owns protocol flow control, queues, limits, deadlines,
trailers, and framing repair. Transforms never receive Hyper or quiche objects.

### Observation is separate from mutation

Observers receive immutable typed events through finite queues. Registration
declares observation interest and an explicit delivery policy. Metadata-only,
redacted observation is the default. Observer failure does not change traffic
unless the caller deliberately selects bounded backpressure.

### Client intent is separate from routing

The original client target remains immutable evidence. Interceptors may modify
the effective request, but changing the network destination requires a typed
reroute action. A route selector applies destination authorization and produces
an auditable upstream plan. Ordinary Host-header edits never silently redirect
the socket destination.

### Upstream execution is protocol-neutral

The default network path uses the existing Hyper and quiche clients, and
`HyperUpstreamService` plus `H3UpstreamService` expose those clients through the
same canonical contract used by an embedding application's streaming upstream
service. Retry and fallback orchestration remains centralized above an
individual attempt.

### Tower is not the canonical API

`tower-service` may be adapted at the boundary, but Hooks v2 uses its own types
because it needs explicit exchange metadata, route plans, body plans, terminal
outcomes, and cancellation semantics that are not represented by a generic
request/response service alone.

### Internal models are not a control protocol

Serde support on canonical types is an implementation convenience, not a wire
compatibility promise. A future external controller receives a separately
versioned protocol that converts to and from Hooks v2 types.

### Breaking migration before 1.0

The final workspace contains one Hooks v2 execution path. The generic event,
generic decision, runner, invalid-decision branches, and temporary adapters are
removed.

## Consequences

- Interceptors can retain exchange-local state without cross-exchange lookup.
- Duplex callbacks may overlap, so interceptor implementations must synchronize
  their own local mutable state.
- Type signatures carry more concepts, but invalid behavior moves from runtime
  errors to construction-time constraints.
- Body and observer resource costs become explicit and testable.
- Higher layers can implement rules, capture, interactive control, and UI
  independently.
- Reverse-proxy ingress remains a later feature, but certificate, route, and
  upstream seams no longer assume that every embedding is a forward MITM proxy.

## Rejected alternatives

### Keep the generic event and decision enums

This preserves a small surface but keeps optional payloads, invalid decisions,
and runtime phase checks in the production path.

### Give hooks mutable access to transport adapters

This would couple public API stability to Hyper and quiche, let extensions
violate flow-control invariants, and prevent protocol-neutral testing.

### Put history and interactive queues in the core

Those policies vary by product and can retain sensitive data. The core provides
bounded observers and pausable interceptors instead.

### Serialize every callback through one mutable interceptor reference

That is ergonomic for simple state but unnecessarily couples duplex request and
response progress and can introduce head-of-line blocking within one exchange.
Per-exchange `&self` callbacks preserve isolation while allowing concurrency.

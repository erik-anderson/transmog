# ADR 0002: Interception boundaries

Date: 2026-10-03
Status: accepted

## Context

Interactive developer tools and embedding applications need one canonical way
to observe, modify, short-circuit, and terminate exchanges without coupling the
proxy core to capture storage, UI state, IPC, product configuration, Hyper, or
quiche.

## Decision

### One interceptor instance per exchange

An `InterceptorFactory` creates isolated interceptor instances. Instances are
`Send + Sync` and callbacks receive `&self`, because duplex protocols may
produce a response while a request body is still streaming. Implementations
that mutate exchange-local state synchronize it explicitly; the runtime does
not require a global exchange-ID map.

### Typed lifecycle methods and actions

Request head, request body planning, response head, response body planning,
completion, and failure use distinct methods and types. Completion and failure
cannot return traffic decisions, so invalid phase/action pairs are absent from
the public API.

### Nested chain order

Requests enter interceptors in registration order and responses unwind in
reverse order. A request short-circuit enters no later interceptor. A local
response passes through response hooks for its producer and all earlier entered
interceptors. Terminal callbacks run once in reverse entered order even if a
cleanup callback fails.

### Body behavior is selected before pumping

Each entered interceptor selects a bounded body plan before the corresponding
body pump starts. The core composes pass-through, transform, buffer, replace,
and discard stages and owns protocol flow control, queues, limits, deadlines,
trailers, and framing repair. Transforms never receive transport objects.

### Observation is separate from mutation

Observers receive immutable typed events through finite queues. Registration
declares observation interest and an explicit delivery policy. Metadata-only,
redacted observation is the default. Observer failure does not change traffic
unless the caller deliberately selects bounded backpressure.

### Client intent is separate from routing

The original client target remains immutable evidence. Interceptors may modify
the effective request, but changing the network destination requires a typed
reroute action. A route selector applies destination authorization and produces
an auditable upstream plan. A Host-header edit never silently redirects the
socket destination.

### Upstream execution is protocol-neutral

The default network path uses Hyper and quiche clients. Their adapters expose
the same canonical contract as an application-owned streaming upstream service.
Retry and fallback orchestration remains centralized above an individual
attempt.

### Internal models are not a control protocol

Serde support on canonical types is an implementation convenience, not a wire
compatibility promise. An external controller must use a separately versioned
protocol that translates at the control boundary.

## Consequences

- Interceptors retain exchange-local state without cross-exchange lookup.
- Duplex callbacks may overlap, so local mutable state needs synchronization.
- Type signatures carry the lifecycle concepts and prevent invalid actions.
- Body and observer resource costs are explicit and testable.
- Rules, capture, interactive control, and UI remain higher-layer concerns.
- Reverse-proxy ingress remains future work, but certificate, route, and
  upstream seams do not require a forward-MITM product.

## Rejected alternatives

- A generic event and decision pair permits invalid combinations and relies on
  runtime phase checks.
- Mutable access to transport adapters couples the API to Hyper and quiche and
  allows extensions to violate flow-control invariants.
- Putting history or interactive queues in core introduces product retention
  policy and sensitive state into the protocol engine.
- Serializing all callbacks through one mutable reference creates unnecessary
  head-of-line blocking within duplex exchanges.

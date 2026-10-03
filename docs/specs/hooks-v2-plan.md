# Plan: Production-grade Hooks v2 interception API

Status: local implementation and hardening complete; cross-platform CI pending
Audience: implementation agent and reviewers
Baseline: commit `8045959`
Primary scope: `rustymiddle-core` and the exchange orchestration in `rustymiddle-runtime`

## Mission

Replace the current phase-generic breakpoint callback with a typed, composable,
per-exchange interception API suitable for a production proxy library. The new
API must preserve rustymiddle's protocol-neutral model, bounded streaming,
backpressure, cancellation, and fail-closed behavior while allowing future
products to add interactive breakpoints, rule engines, capture storage, IPC,
and user interfaces outside the core.

The resulting library must support both of these consumers without favoring
one through hidden assumptions:

- a Charles/Fiddler-style explicit interception proxy built as higher layers;
- an application-owned proxy or gateway that embeds the exchange engine and
  supplies its own interceptors, routing, observers, certificates, or upstream
  service.

Do not stop after defining traits or compiling an adapter. Migrate every runtime
path, port the existing CLI proof behavior, expand the deterministic protocol
matrix, update documentation, and pass all local and live verification gates.

## Current state and reason for change

The existing `BreakpointHandler` proves that all protocol paths can pause and
modify a canonical exchange. It intentionally uses one generic event and one
generic decision enum. That shape now creates avoidable ambiguity:

- event payload validity depends on `BreakpointPhase` and is represented with
  `Option` fields;
- decisions that are illegal for a phase remain representable and are rejected
  only at runtime;
- one shared handler encourages global maps keyed by session ID for state that
  belongs to a single exchange;
- observation, mutation, lifecycle notification, and interactive pausing share
  one callback boundary;
- a streaming transform is selected only after a body frame has arrived,
  complicating pump construction and transform composition;
- editing the logical request target and selecting the network destination are
  not distinct public operations;
- the public model is serializable but is not a versioned external control
  protocol, a distinction that must remain clear.

Hooks v2 must make illegal states difficult to express and make resource costs
visible at construction time.

## Architectural boundary

The core data path is:

```text
H1/H2/H3 ingress adapters
          |
          v
canonical exchange engine
  lifecycle, limits, cancellation,
  backpressure, framing, retry safety
          |
          v
ordered interceptor chain
          |
          v
route selector and upstream plan
          |
          v
H1/H2/H3 network upstream
or application-owned upstream service
```

Optional higher layers consume public hooks but are not dependencies of the
core:

```text
capture recorder   rule engine   control bridge   developer UI
        \               |              |             /
                 Hooks v2 public API
```

### State owned by the core

The core owns state required to preserve network correctness:

- stable exchange, connection, and stream identifiers;
- protocol parsing, framing, translation, and flow-control state;
- connection pools, route attempts, retry state, and replayability state;
- request and response pump state;
- cancellation, deadlines, byte limits, queue limits, and pause permits;
- immutable trust generation and the pool generation using it;
- certificate and Alt-Svc caches whose behavior affects correctness;
- one interceptor instance and one extensions store per exchange;
- exactly-once terminal outcome state.

### State outside the core

The core does not own:

- captured-session databases or long-term body retention;
- saved rules, projects, profiles, tags, comments, or UI selection state;
- an interactive breakpoint queue presented to a user;
- IPC connections or a permanent control-protocol schema;
- HAR libraries, replay collections, or sanitized sharing workflows;
- plugin installation, scripting runtimes, or desktop application lifecycle;
- reverse-proxy configuration files, load-balancer health state, or ACME
  account state.

Higher layers may implement those features through interceptors, observers,
route selectors, infrastructure providers, and upstream services.

## Non-goals

This milestone does not implement:

- a desktop or web UI;
- a capture database;
- an external HTTP, WebSocket, or RPC control server;
- a serialized wire protocol for external controllers;
- a full declarative rule language;
- content decoding for gzip, Brotli, deflate, or zstd;
- WebSocket frame inspection;
- a reverse-proxy listener, ACME, health checks, or load balancing;
- transparent browser QUIC interception;
- a general plugin or scripting runtime.

Small test adapters are allowed when they validate that Hooks v2 can support a
future control bridge or embedded application without adding those products to
the core.

## Required invariants

Every design and implementation decision must preserve these invariants:

1. Hyper and quiche types never appear in the stable interception API.
2. A hook instance is scoped to exactly one exchange unless an implementation
   explicitly shares state behind its factory.
3. Every body queue and complete-body buffer has a configured finite bound.
4. Pass-through bodies stream without whole-body buffering.
5. Pausing one H2 or H3 stream does not pause unrelated streams.
6. Hook timeout, cancellation, panic, or drop terminates only the affected
   exchange where the protocol permits.
7. Every started exchange produces exactly one terminal success or failure
   outcome for its entered interceptors and registered observers.
8. Response bytes are never retried or replaced after commitment in a way that
   violates HTTP semantics.
9. A request is never replayed unless the existing replayability policy permits
   it and no response has started.
10. Header and body modifications continue through the centralized translation
    and framing-repair logic.
11. The original client target is immutable evidence. Rerouting is a distinct,
    typed decision and cannot occur accidentally through a Host-header edit.
12. Observer failure does not change traffic unless the caller explicitly
    chooses a documented backpressuring delivery policy.
13. Sensitive body or credential capture is opt-in. Routine telemetry remains
    metadata-only and redacted.
14. A trust or certificate failure is never converted to success by a hook
    default or observer failure.
15. No hook, observer, or route selector may create an unbounded task, queue,
    map, or retry loop inside the core.

## Terminology

- **Exchange**: one logical request and its response, including all allowed
  upstream route attempts.
- **Original target**: the immutable scheme, authority, host, port, path, and
  query derived from client ingress.
- **Effective request**: the request after interceptor changes, before protocol
  translation.
- **Upstream plan**: a typed network or application destination selected for an
  effective request.
- **Interceptor factory**: shared registration object that creates isolated
  per-exchange interceptor instances.
- **Exchange interceptor**: a stateful object invoked only for its exchange.
- **Body plan**: the bounded pass-through, replacement, transformation,
  buffering, or discard behavior selected before a body pump starts.
- **Observer**: an immutable event consumer that cannot mutate or terminate
  traffic.
- **Entered interceptor**: an interceptor whose request-head callback ran for
  the exchange. Response and terminal callbacks use the entered stack.

## Target API shape

The exact names may change during implementation review, but the type safety
and lifecycle semantics in this section are mandatory.

### Exchange metadata and context

Introduce immutable metadata available for the lifetime of an exchange:

```rust
pub struct ExchangeMetadata {
    pub exchange_id: ExchangeId,
    pub downstream_connection_id: ConnectionId,
    pub downstream_stream_id: StreamId,
    pub client_addr: SocketAddr,
    pub listener_addr: SocketAddr,
    pub ingress: IngressSummary,
    pub original_target: OriginalTarget,
    pub started_at: SystemTime,
}
```

Keep protocol stream IDs distinct from locally allocated HTTP/1 sequence IDs if
that distinction is needed for correct telemetry. Do not make higher layers
infer it from numeric patterns.

Each exchange receives an extensions store for application-specific transient
state. It must be `Send`, remain local to the exchange, and must not be cloned or
serialized implicitly. Prefer a small owned type-map abstraction over exposing
transport internals.

Context supplied to hooks must also expose:

- a cancellation signal;
- the remaining hook deadline or budget;
- immutable exchange metadata;
- read/write access to exchange extensions where lifecycle ordering makes that
  safe;
- the current route-attempt summary when applicable;
- capability queries rather than protocol downcasts.

### Per-exchange interceptor factory

Use an object-safe asynchronous boundary. Native `async fn` syntax may be used
internally, but public trait objects must have a stable object-safe form without
requiring a macro dependency solely for convenience.

Conceptually:

```rust
pub trait InterceptorFactory: Send + Sync {
    fn create(&self, metadata: &ExchangeMetadata)
        -> Result<Box<dyn ExchangeInterceptor>, HookInitError>;
}

pub trait ExchangeInterceptor: Send {
    fn on_request_head(&mut self, event: RequestHeadEvent)
        -> BoxHookFuture<'_, RequestHeadAction>;
    fn on_response_head(&mut self, event: ResponseHeadEvent)
        -> BoxHookFuture<'_, ResponseHeadAction>;
    fn on_completed(&mut self, outcome: CompletedExchange)
        -> BoxHookFuture<'_, ()>;
    fn on_failed(&mut self, failure: ExchangeFailure)
        -> BoxHookFuture<'_, ()>;
}
```

Do not include optional request head, response head, and body fields in one
event. Each callback receives a phase-specific event with required fields.

Factory initialization failure must be typed and handled according to explicit
registration policy. The default fail-closed behavior rejects the exchange; an
optional interceptor may instead be skipped with an observer-visible warning.

### Typed head actions

Request-head actions may express only:

- continue with the original head;
- continue with a replacement head;
- explicitly reroute with a new logical target or route directive;
- respond locally before upstream commitment;
- abort the exchange with a typed public reason;
- attach a request body plan.

Response-head actions may express only:

- continue with the original head;
- continue with a replacement head;
- replace the not-yet-committed response with a bounded local response;
- abort before downstream commitment;
- attach a response body plan.

Completion and failure callbacks return no traffic decision. There must be no
equivalent of `InvalidBreakpointDecision` in normal Hooks v2 execution because
illegal phase/action pairs are not representable.

### Original target and rerouting

Split immutable client intent from network routing:

```text
OriginalTarget (immutable evidence)
        |
        +--> EffectiveRequest (interceptor-visible logical request)
                         |
                         v
                   RouteSelector
                         |
                         v
                   UpstreamPlan
```

Changing ordinary headers, including `Host`, must not silently change the
socket destination. An authority change requires a typed reroute action. The
route selector receives both the original target and effective request, applies
destination authorization, and produces an `UpstreamPlan`.

An upstream plan must contain enough information to audit and pool correctly:

- destination scheme, host, port, and resolved policy identity;
- required or automatic HTTP version policy;
- trust generation and relevant TLS policy identity;
- upstream proxy or direct-network selection when later supported;
- replayability and fallback constraints;
- a redacted reason suitable for evidence.

The plan must not contain resolved secrets or mutable references to product
configuration.

### Body plans

Select body behavior before constructing the corresponding body pump. Required
plans are:

- `PassThrough`: forward frames with no complete-body buffering;
- `Transform`: pass frames through one bounded stateful transform;
- `Buffer`: collect up to an explicit byte limit, invoke a completion handler,
  then continue, replace, or abort;
- `Replace`: ignore the incoming body and emit a bounded replacement source;
- `Discard`: consume incoming frames as required for protocol correctness but
  emit no body.

Body plans must preserve trailers or reject them with a typed outcome. They must
define behavior for empty data frames, duplicate trailers, data after trailers,
consumer cancellation, producer failure, timeout, and limit overflow.

The core owns body pumps and flow-control credit. A transform receives and
returns canonical frames but never polls Hyper, quiche, or Tokio channels
directly. An asynchronous transform may pause only its exchange and remains
subject to the per-call timeout and global pause limit.

Provide reusable helpers for common safe patterns:

- bounded complete-body edit;
- streaming frame map/filter;
- metadata-only byte counting;
- body replacement from `Bytes` and from a bounded canonical stream;
- trailer-preserving transformation;
- identity/no-body semantics for HEAD, 1xx, 204, 304, and CONNECT.

Do not add compression decoding in this milestone. Design the body-plan API so
a later codec middleware can wrap raw bodies without changing protocol
adapters.

### Interceptor chain semantics

Registration order defines nested middleware behavior:

```text
request:  A -> B -> C -> upstream
response: A <- B <- C <- upstream
```

The implementation must guarantee:

- request heads and request body transforms run in registration order;
- response heads and response body transforms run in reverse order;
- if B responds locally or aborts during the request, C is not entered;
- a local response from B flows through response hooks for B and A;
- terminal callbacks run once in reverse entered order;
- observers see the complete lifecycle independently of short-circuiting;
- an interceptor cannot dynamically reorder or remove other interceptors;
- chain configuration becomes immutable before the listener starts.

Document whether terminal callbacks have their own smaller cleanup deadline.
They must not keep a committed response open indefinitely. Failure in one
terminal callback is reported to observers and does not suppress later cleanup
callbacks.

### Observers

Define observation separately from mutation. Observers receive immutable,
typed lifecycle events such as:

- connection opened/closed;
- exchange started;
- request head finalized;
- route selected and route attempt started/finished;
- response head finalized;
- bounded body observation selected by explicit interest;
- exchange completed or failed;
- hook timeout, panic, cancellation, or observer data loss.

Observer registration declares an `ObservationInterest` so the core does not
copy bodies or expensive TLS details unnecessarily. Credential values and body
bytes are excluded by default.

Every observer adapter uses a finite queue and one explicit delivery policy:

- backpressure the exchange under configured limits;
- drop newest and increment a visible loss counter;
- disconnect the observer and emit one diagnostic.

There is no silent unbounded buffering. Observer errors do not fail traffic by
default. A caller that deliberately chooses backpressure must be able to bound
the delay and memory cost.

Events carry a per-exchange monotonic sequence number so an asynchronous
consumer can restore order. Internal observation types are not declared to be
a stable external wire format.

### Route selector and upstream service

Introduce protocol-neutral boundaries that allow the default network stack and
application-owned services to share the exchange engine:

```rust
pub trait RouteSelector: Send + Sync {
    fn select(&self, input: RouteInput)
        -> BoxRouteFuture<'_, Result<UpstreamPlan, RouteError>>;
}

pub trait UpstreamService: Send + Sync {
    fn execute(&self, request: StreamingRequest, plan: UpstreamPlan)
        -> BoxUpstreamFuture<'_, Result<StreamingResponse, UpstreamError>>;
}
```

The existing Hyper and quiche adapters remain the default network upstream.
Route selection and retry orchestration stay centralized so an interceptor
cannot accidentally create an unsafe replay loop.

An application-owned upstream service may return a canonical streaming response
without opening a network connection. It receives the same limits and
cancellation signal as the network implementation.

Do not turn `tower-service` types into the canonical public API unless doing so
materially improves composition and preserves the body, cancellation, and error
semantics defined here. Record the decision in an ADR.

### Infrastructure provider seams

Define or preserve replaceable provider boundaries for:

- upstream trust snapshots and reload generations;
- downstream certificate resolution;
- DNS resolution;
- clock and randomness used by IDs, timeouts, and tests;
- socket or connector creation where applications require custom networking.

Hooks v2 does not need to implement reverse ingress, but `ProxyServer::bind`
must not force future reverse-mode users to construct a MITM CA when a supplied
certificate resolver would be appropriate. If fully abstracting that dependency
would make this milestone unsafe or too broad, introduce the provider trait and
adapt the existing CA implementation without adding reverse listener behavior.

### Structured outcomes and failures

Replace stringly typed cross-layer failures with a public structured outcome
model. Preserve private source chains for diagnostics without exposing secrets.
At minimum classify:

- client cancellation;
- hook initialization, timeout, panic, or explicit abort;
- body limit, idle timeout, or invalid frame sequence;
- request translation or response translation failure;
- route selection or destination authorization failure;
- DNS, TCP, UDP, TLS, HTTP/1, HTTP/2, HTTP/3, and upstream cancellation;
- unsafe retry refusal;
- shutdown.

Failure reports identify the lifecycle stage, whether request or response bytes
were committed, the selected route attempt, and a redacted operator message.
They must not include authorization values, cookies, private keys, or body data.

### Panic and cancellation containment

Contain hook panics at the per-exchange boundary when the active panic strategy
allows unwinding. Convert them to a typed hook failure, run remaining terminal
cleanup, and close only the affected exchange where possible. Do not use unsafe
code to implement panic containment.

Dropping a request future, response consumer, hook future, body transform, or
external adapter must release pause permits and flow-control resources. All
spawned per-exchange tasks need explicit ownership and cancellation; detached
tasks may not retain exchanges indefinitely.

## Proposed source organization

Keep the existing crate boundaries. Do not create a crate per trait. A suggested
`rustymiddle-core` layout is:

```text
src/
  intercept/
    mod.rs
    action.rs
    body.rs
    chain.rs
    context.rs
    failure.rs
    lifecycle.rs
  observe.rs
  route.rs
  upstream.rs
  extensions.rs
  body.rs
  header.rs
  message.rs
  protocol.rs
  translation.rs
```

Move runtime exchange orchestration out of the monolithic `proxy.rs` into a
focused module once the new engine has stable tests:

```text
proxy-runtime/src/
  exchange.rs
  ingress.rs
  routing.rs
  proxy.rs
  lib.rs
```

Do not perform a mechanical file split before behavior is covered. File moves
must clarify ownership rather than only reduce line counts.

## Migration strategy

The workspace is pre-1.0 and can make a deliberate breaking API change. Do not
maintain two complete interception engines.

1. Characterize all current `BreakpointHandler` behavior with tests before
   changing runtime orchestration.
2. Add Hooks v2 types and unit tests without wiring them to network paths.
3. Build the interceptor chain and body-plan engine as protocol-independent
   components.
4. Add a temporary internal adapter from the current proof handler only if it
   allows incremental runtime migration. Do not expose that adapter as a
   long-term public compatibility promise.
5. Migrate ordinary HTTP, CONNECT/H1, CONNECT/H2, and H3 egress paths to one
   shared exchange engine.
6. Port the CLI proof behavior to a per-exchange interceptor factory.
7. Remove the old runner, generic events, generic decisions, invalid-decision
   errors, and duplicate orchestration after all tests use Hooks v2.
8. Update rustdoc, architecture, testing, examples, and limitations in the same
   change series.

Do not leave protocol paths on different hook generations at milestone exit.

## Implementation phases

### Phase 0: Characterize the existing behavior

- Run and record the complete local gate from the baseline commit.
- Add missing black-box tests for every currently supported breakpoint action
  before refactoring it.
- Trace request and response lifecycle ordering in buffered, streaming, local
  response, abort, timeout, cancellation, and failure paths.
- Inventory every place in `proxy-runtime` that invokes a breakpoint or emits
  terminal evidence.
- Write ADR 0002 describing Hooks v2 boundaries, chain order, target/routing
  separation, observer delivery, and whether `tower-service` is used.

Exit gate: the old behavior is fully characterized, the ADR has no unresolved
semantic placeholders, and `scripts/test.ps1` plus `scripts/test-live.ps1` pass.

### Phase 1: Add typed lifecycle and action types

- Add exchange metadata, original target, context, extensions, structured
  outcome, and structured failure types.
- Add object-safe interceptor factory and per-exchange interceptor traits.
- Add phase-specific events and action enums.
- Add an explicit no-op factory/interceptor.
- Make all public types documented; keep Hyper and quiche out of signatures.
- Add compile-fail or API-shape tests proving illegal phase/action pairs cannot
  be constructed through normal public APIs.

Exit gate: core-only tests cover construction, state transitions, exact terminal
outcome, extensions isolation, and public documentation. No runtime path uses
the new API yet.

### Phase 2: Implement ordered interceptor composition

- Create one interceptor instance per exchange.
- Implement request-forward and response-reverse ordering.
- Implement entered-stack semantics for local responses and aborts.
- Apply per-call deadlines and the global paused-exchange bound.
- Contain initialization errors and hook panics according to registration
  policy.
- Guarantee terminal cleanup continues after one cleanup callback fails.
- Add deterministic cancellation tests for every await boundary.

Exit gate: exhaustive core tests prove chain ordering, short-circuiting,
isolation, timeout, panic, cancellation, and exactly-once terminal behavior.

### Phase 3: Implement body plans and pumps

- Define pass-through, transform, buffer, replace, and discard plans.
- Centralize request and response body-pump construction.
- Preserve trailers and bodyless-response semantics.
- Enforce per-plan, per-exchange, and global resource limits.
- Compose request transforms in registration order and response transforms in
  reverse order.
- Ensure dropping any endpoint releases permits and protocol flow-control
  credit.
- Port existing `BoundedBodyBuffer` behavior into reusable Hooks v2 helpers
  rather than duplicating it.

Exit gate: body plans pass unit tests, model tests, limit tests, and concurrent
stream tests without any network adapter-specific branch in the body engine.

### Phase 4: Add immutable observation

- Define typed observer events, observation interest, sequence numbers, and
  delivery policies.
- Add bounded dispatcher adapters for backpressure, drop-newest, and disconnect
  behavior.
- Keep body observation disabled by default and bounded when enabled.
- Emit visible counters or diagnostics for dropped observer events.
- Ensure observer failure and disconnect do not mutate traffic.

Exit gate: observer ordering, filtering, redaction defaults, queue saturation,
data-loss reporting, and disconnect behavior pass deterministic tests.

### Phase 5: Separate logical requests from routing

- Add immutable original target and typed reroute decisions.
- Add route selector, destination authorization, upstream plan, and structured
  route evidence.
- Adapt the existing `RoutePolicy` and Alt-Svc behavior behind the selector.
- Preserve safe fallback and no-response-started rules.
- Add an upstream service boundary and adapt the current Hyper/quiche clients.
- Add a small in-process upstream fixture to prove application-owned service
  composition without introducing a reverse listener.

Exit gate: ordinary target edits cannot silently redirect sockets, explicit
reroutes are auditable, retry safety remains centralized, and both network and
in-process upstream implementations pass the same contract tests.

### Phase 6: Migrate every runtime path

- Introduce one shared exchange engine used by plain HTTP and intercepted HTTPS.
- Migrate H1 and H2 ingress with H1, H2, and H3 egress.
- Remove phase-specific duplicated orchestration from `proxy.rs` where the
  shared engine now owns it.
- Port local responses, request and response modifications, aborts, timeouts,
  streaming transforms, terminal events, and evidence generation.
- Port the CLI proof handler to a factory that owns state per exchange.
- Keep live report fields stable unless the new model adds strictly better
  evidence; update the report schema deliberately when fields change.

Exit gate: all six ingress/egress matrix rows use Hooks v2, the old execution
path is deleted, and no `InvalidBreakpointDecision` branch remains.

### Phase 7: Provider seams and embedded-library example

- Adapt the existing downstream CA through a certificate-resolver/provider
  interface without weakening CONNECT identity validation.
- Preserve trust snapshot and reload behavior through explicit provider types.
- Add a bounded in-process channel bridge that demonstrates an external
  controller pausing a hook and returning a correlated decision. This is a
  contract test adapter, not a permanent IPC or wire-protocol design.
- Test bridge disconnect, stale or duplicate decisions, command timeout,
  cancellation, queue saturation, and late replies after an exchange ends.
- Add an embedded example that supplies an interceptor, observer, route
  selector, and in-process or custom upstream service.
- Ensure the example requires no CLI parsing or global singleton.
- Document thread-safety, cancellation, ownership, and shutdown behavior for
  embedders.

Exit gate: an application can run the exchange engine with its own components
without depending on CLI modules or transport-private types, and the bounded
channel bridge proves that a future out-of-process controller can implement
interactive pause/resume without changing the core lifecycle.

### Phase 8: Hardening and public API review

- Remove temporary compatibility adapters and dead v1 types.
- Review all public names, ownership, lifetime, error, and extensibility choices.
- Add `#[must_use]` where dropping an action, plan, or control handle would be a
  bug.
- Deny missing documentation for the public Hooks v2 surface once migration is
  complete.
- Audit logs and errors for credentials and body leakage.
- Add fuzz targets, Miri-compatible core tests where practical, long-running
  concurrency stress tests, and deterministic shutdown-race tests.
- Add benchmarks for pass-through, one no-op interceptor, multi-interceptor
  chains, body transforms, and observers. Record a baseline; do not hide
  regressions behind larger default buffers.
- Update dependency policy and notices for any accepted dependency.

Exit gate: the complete definition of done below passes from a clean checkout.

## Required test strategy

### Core state and chain tests

Exhaustively test:

- factory success, required failure, and optional failure;
- one distinct interceptor instance per exchange;
- request order A/B/C and response order C/B/A;
- local response and abort at A, B, and C;
- request-head replacement and explicit reroute;
- response-head replacement and local replacement before commitment;
- hook timeout at every phase;
- hook panic at every phase;
- cancellation while waiting for a permit and while inside a hook;
- exactly one completed or failed callback for every entered interceptor;
- cleanup callback failure without suppression of remaining cleanup;
- extensions visibility within one exchange and isolation across exchanges;
- concurrent exchanges through one factory without shared-state races.

### Body-plan tests

For request and response bodies, test:

- zero frames, empty data frames, one frame, and many frames;
- trailers, duplicate trailers, and data after trailers;
- pass-through streaming before producer completion;
- streaming transform expansion, contraction, drop, and flush output;
- complete-body buffer below, exactly at, and above its limit;
- replacement without consuming unbounded peer input;
- discard while preserving protocol flow-control correctness;
- consumer cancellation and producer cancellation;
- transform timeout, panic, explicit abort, and dropped future;
- HEAD, 1xx, 204, 304, and CONNECT body semantics;
- framing repair after length-changing modifications;
- independent progress for two H2 streams and two H3 streams;
- aggregate memory and pause-permit bounds under many concurrent exchanges.

### Observer tests

Test:

- event sequence numbers and lifecycle order;
- metadata-only default interest;
- credential and body redaction defaults;
- explicitly bounded body observation;
- backpressure timeout and cancellation;
- drop-newest counts with no traffic failure;
- observer disconnect with one diagnostic and no task leak;
- slow observer isolation from unrelated exchanges;
- terminal event delivery after local response, abort, upstream failure, and
  client cancellation.

### Routing and upstream tests

Test:

- original target immutability;
- Host-header edit without destination change;
- explicit authorized reroute;
- rejected destination authorization;
- route selection timeout and cancellation;
- pool keys include all required plan and trust identities;
- safe GET/HEAD fallback before response start;
- refusal to replay non-replayable bodies;
- refusal to retry after response commitment;
- network upstream and in-process upstream contract parity;
- trust reload creates a new upstream generation without changing in-flight
  exchanges.

### Protocol integration matrix

Run the following rows through the same Hooks v2 engine:

| Ingress | Egress | Required coverage |
|---|---|---|
| H1 | H1 | all head actions, every body plan, local response, abort, observer events |
| H1 | H2 | translation, trailers, cancellation, routing evidence |
| H2 | H1 | concurrent ingress streams, serialization, isolation |
| H2 | H2 | pause isolation, reset/GOAWAY behavior, graceful drain |
| H1 | H3 | quiche streaming, cancellation, no fallback in H3-only mode |
| H2 | H3 | multiplexing, pause isolation, H3 telemetry and trailers |

Every row must include:

- no-op pass-through;
- request-head and response-head replacement;
- request-body and response-body streaming transform;
- bounded complete-body edit and limit rejection;
- local response;
- abort before upstream send;
- abort during response streaming;
- hook timeout and panic;
- client cancellation;
- upstream failure;
- multiple concurrent exchanges;
- exactly-once terminal interceptor and observer outcomes.

### Fuzzing and model testing

Add focused fuzz targets for:

- interceptor lifecycle commands and state transitions;
- body-frame sequences and plan composition;
- route directives and target normalization;
- observer event serialization only if a temporary test schema is introduced;
- header modifications feeding the existing translation layer.

Seed fuzz corpora with every deterministic regression case. Fuzz targets must
enforce allocation and input-size limits. A short smoke duration runs in CI;
longer campaigns may run on a schedule. Any crash becomes a checked-in
regression test before the fix is accepted.

### Performance and leak testing

Measure at minimum:

- streaming throughput and time to first byte with no interceptors;
- overhead of one and several no-op interceptors;
- bounded transform throughput for H1, H2, and H3;
- observer queue saturation behavior;
- task, handle, and memory return to baseline after cancellation and shutdown.

Benchmarks must use fixed inputs and checked-in instructions. Do not set a
numerical release threshold until a stable baseline exists, but report material
regressions and explain any accepted tradeoff in the ADR.

## Documentation requirements

Update or add:

- rustdoc for every public Hooks v2 type and error;
- an architecture section showing interceptor, observer, route, and upstream
  boundaries;
- a lifecycle document with normal, local-response, abort, failure, and
  cancellation sequences;
- an embedding example with no CLI dependency;
- migration notes from `BreakpointHandler` to Hooks v2;
- explicit resource-bound and timeout semantics;
- guidance on storing state in a per-exchange interceptor versus shared factory;
- guidance that internal serde types are not a stable external control protocol;
- updated testing and limitation documents.

All examples must compile in CI. Examples must not disable certificate
verification, create unbounded buffers, or expose remote listeners by default.

## Dependency and API policy

- Prefer the standard library and current workspace dependencies.
- Any new dependency must pass `cargo deny`, preserve the one-BoringSSL graph,
  and have a documented reason that cannot be met cleanly in-repository.
- Do not add an async-trait macro solely to shorten signatures.
- Do not expose Tokio channel types as the only way to implement a hook.
- Do not expose Hyper, quiche, BoringSSL connection objects, or protocol stream
  objects through public hooks.
- Keep `unsafe_code = "forbid"`.
- No global mutable registry or singleton runtime.
- Do not treat serde derives on internal models as a wire compatibility promise.
- Prefer owned or reference-counted immutable metadata at async boundaries;
  avoid cloning complete bodies or certificate chains implicitly.

## Working rules for the implementation agent

1. Work phase by phase but continue through the final gates. A trait-only PR or
   one migrated protocol is not completion.
2. Keep the workspace buildable and tests passing after each phase.
3. Add characterization tests before changing behavior.
4. Centralize exchange orchestration; do not copy the Hooks v2 lifecycle into
   each protocol adapter.
5. Preserve unrelated behavior and the live Chromium proof.
6. Use typed errors and explicit limits; never solve a failing test with an
   unbounded queue, larger hidden buffer, certificate bypass, or silent retry.
7. Treat hook and observer implementations as untrusted application code at
   cancellation, panic, timeout, and redaction boundaries.
8. Do not make product-layer persistence or IPC decisions in the core to make a
   demo easier.
9. Record meaningful semantic decisions in ADR 0002 before they become
   difficult to change.
10. Run focused tests during development and the complete gates before every
    milestone handoff.
11. Do not claim cross-platform success from a Windows-only run; rely on the
    checked-in CI matrix for platforms not locally available and report its
    status accurately.
12. Before completion, review the final public API as an embedding application
    author, a rule-engine author, and an external-control-bridge author.

## Required verification commands

On Windows, use the checked-in Clang/Ninja/Windows SDK environment:

```powershell
. ./scripts/dev-env.ps1 -Check
pwsh ./scripts/test.ps1
cargo build --workspace --all-features --release --locked
pwsh ./scripts/test-live.ps1
```

Also run all added fuzz smoke, Miri-compatible, benchmark-compilation, and
example-compilation commands documented by the implementation. The live gate
must continue using normal Chromium certificate verification with no Playwright
route interception.

## Definition of done

- [x] Every protocol path uses one shared Hooks v2 exchange engine.
- [x] The old generic breakpoint event, decision, runner, and invalid-decision
      runtime branches are removed.
- [x] Per-exchange interceptor factories and state isolation are proven.
- [x] Phase-specific action types make illegal decisions unrepresentable.
- [x] Request-forward and response-reverse chain ordering is documented and
      exhaustively tested.
- [x] Local response, abort, timeout, panic, cancellation, and cleanup semantics
      produce exactly one terminal outcome.
- [x] Pass-through, transform, buffer, replace, and discard body plans are
      bounded and tested on requests and responses.
- [x] Pausing or transforming one H2/H3 stream does not block unrelated streams.
- [x] Immutable observers have bounded queues, explicit delivery policy,
      redaction defaults, and visible loss reporting.
- [x] Original client target, effective request, explicit reroute, and upstream
      plan are separate types with destination-authorization tests.
- [x] Network and application-owned upstream services pass shared contract
      tests.
- [x] Trust reload, Alt-Svc, safe fallback, and no-DIRECT behavior remain intact.
- [x] The CLI proof handler uses only Hooks v2 and no global per-session map.
- [x] The embedding example compiles and demonstrates custom components.
- [x] Public API rustdoc is complete and contains no transport-private types.
- [x] Fuzz smoke, deterministic stress, cancellation, and shutdown-race tests
      pass.
- [x] Formatting, strict clippy, locked tests, documentation tests, examples,
      dependency policy, and one-BoringSSL graph checks pass.
- [x] Windows release build succeeds with Clang, Ninja, and the Windows SDK.
- [ ] Linux and macOS CI build and test gates pass.
- [x] All five live Chromium cases still pass without certificate bypass or
      Playwright interception.
- [x] Architecture, lifecycle, migration, testing, and limitation documentation
      describe the final implementation rather than the proposed design.
- [x] The final working tree contains no temporary adapters, ignored failures,
      debug credential/body logging, or undocumented feature flags.

The remaining unchecked item is a deliberate gate, not a presumed success: the
checked-in Linux/macOS CI jobs have not been observed for this change set from
the local Windows run.

## Expected risks and mitigations

| Risk | Mitigation |
|---|---|
| Generic hook flexibility recreates illegal phase/action combinations | Use phase-specific events and actions; add compile-fail and exhaustive lifecycle tests |
| Per-exchange objects add allocation overhead | Benchmark no-op paths, permit small optimized factories later, and never trade correctness for speculative optimization |
| Multiple interceptors make body ownership unclear | Select plans before pump construction and centralize composition in one body engine |
| Interactive adapters stall unrelated traffic | Per-exchange futures, finite pause permits, deadlines, and H2/H3 isolation tests |
| Observer capture becomes an accidental unbounded recorder | Explicit interest, finite queues, bounded body taps, and visible loss policy |
| Target edits create SSRF or silent reroutes | Immutable original target, typed reroute, destination authorization, and route evidence |
| Runtime migration duplicates behavior | Use a short-lived internal adapter only; delete the old path before phase exit |
| Hook panics or drops leak tasks and flow-control credit | Per-exchange containment, structured cancellation ownership, and leak/stress tests |
| Public serde derives freeze the wrong wire format | State explicitly that internal models are not the external protocol; add a separate versioned schema later |
| Provider abstraction expands into reverse-proxy scope | Add seams and adapters only; keep reverse listener behavior a separate milestone |

## Completion report

The implementation agent's final report must include:

- a concise public API summary;
- the final lifecycle and chain-order semantics;
- files and ADRs added or materially changed;
- old APIs removed and any intentional migration break;
- deterministic, fuzz, stress, benchmark, CI, and live-browser results;
- any platform gate not executed locally;
- dependency graph changes and policy results;
- remaining limitations that belong above or beyond the core library.

Do not describe Hooks v2 as production-grade if any required protocol path still
uses the old engine, if cancellation or observer saturation is untested, or if
the final live browser gate was skipped.

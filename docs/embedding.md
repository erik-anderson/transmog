# Embedding rustymiddle

The [`embedded` example](../crates/proxy-runtime/examples/embedded.rs) constructs
the proxy without CLI modules or global state. It supplies an interceptor,
bounded observer, route selector, certificate resolver, and canonical streaming
application upstream:

```powershell
. ./scripts/dev-env.ps1
cargo run --locked -p rustymiddle-runtime --example embedded
```

`ProxyComponents` is consumed by bind and cannot be mutated after the listener
starts. Clone configuration into factories before bind. Each factory invocation
must return an exchange-scoped interceptor. Shared application state is allowed
only when the factory explicitly owns synchronized `Arc` state.

An `UpstreamService` receives a bounded `BodyStream`, immutable `UpstreamPlan`,
and exchange cancellation signal. It must stop work promptly when cancelled and
return another bounded stream. It must not create an unbounded queue or retry a
request independently; route attempts and replay safety belong to the exchange
engine.

Observers are asynchronous and use finite queues. Metadata and redacted heads
are enabled by default; body bytes require an explicit bounded prefix interest.
Call `ProxyServer::serve` through shutdown so accepted connections and observer
queues receive their bounded graceful drain. `ProxyControl` exposes trust reload
and observer counters without granting transport access.

For an interactive controller, use the generic `DecisionBridge` inside an
interceptor. Its finite queue and deadline make saturation, disconnect,
cancellation, stale replies, and duplicate replies explicit. It is an in-process
contract adapter, not a stable IPC schema.

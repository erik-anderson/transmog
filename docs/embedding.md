# Embedding rustymiddle

The [`embedded` example](../crates/proxy-runtime/examples/embedded.rs) constructs
the proxy without CLI modules or global state. It supplies an interceptor that
modifies decoded request and response bytes, bounded observer, route selector,
certificate resolver, immutable content policy, and canonical streaming
application upstream:

```powershell
. ./scripts/dev-env.ps1
cargo run --locked -p rustymiddle-runtime --example embedded
```

`ProxyComponents` is consumed by bind and cannot be mutated after the listener
starts. Clone configuration into factories before bind. Each factory invocation
must return an exchange-scoped interceptor. Shared application state is allowed
only when the factory explicitly owns synchronized `Arc` state.

Content processing is disabled by default. Configure it before bind with one
of these immutable policies:

- `ContentPolicy::inspect_to_identity(limits)` decodes supported content for
  semantic hooks and sends identity output;
- `ContentPolicy::preserve_original_output(limits)` decodes for hooks and
  re-encodes the modified body with its original coding stack;
- `ContentPolicy::disabled()` preserves coded bodies without invoking codecs.

With disabled policy, a required decoded hook fails on a coded body before body
bytes are processed. An optional decoded hook is declined and preserves the
complete representation exactly. Unknown codings follow the same required
failure/optional bypass rule; a stack is never partially decoded. Identity
bodies remain available to decoded hooks in every mode because no codec is
needed. Applications choose policy and finite limits, not codec implementation
objects.

An `UpstreamService` receives a bounded `BodyStream`, immutable `UpstreamPlan`,
and exchange cancellation signal. It must stop work promptly when cancelled and
return another bounded stream. It must not create an unbounded queue or retry a
request independently; route attempts and replay safety belong to the exchange
engine.

`rustymiddle-http::HyperUpstreamService` and
`rustymiddle-h3::H3UpstreamService` adapt the built-in pooled clients to this
same contract. Both reject a plan whose authorized destination differs from
the canonical scheme, authority, host, or port before network I/O. The HTTP/3
adapter performs one H3 attempt; an embedding host that selects `Auto` remains
responsible for applying the central replay/fallback policy around attempts.

Observers are asynchronous and use finite queues. Metadata and redacted heads
are enabled by default; body bytes require an explicit bounded prefix interest.
Call `ProxyServer::serve` through shutdown so accepted connections and observer
queues receive their bounded graceful drain. `ProxyControl` exposes trust reload
and observer counters without granting transport access.

For an interactive controller, use the generic `DecisionBridge` inside an
interceptor. Its finite queue and deadline make saturation, disconnect,
cancellation, stale replies, and duplicate replies explicit. It is an in-process
contract adapter, not a stable IPC schema.

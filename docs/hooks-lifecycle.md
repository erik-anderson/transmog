# Hooks v2 lifecycle

The chain is immutable after listener bind. For registrations A, B, and C, a
normal exchange has this order:

```text
factory A, B, C
request head A -> B -> C
request body plans A -> B -> C
route selection and upstream attempt
response head C -> B -> A
response body plans C -> B -> A
completed C -> B -> A (exactly once)
```

Factories create distinct interceptor instances for every exchange. A callback
receives an exchange-local `HookContext` containing immutable metadata, a typed
extensions store, a cancellation signal, and the callback timeout. Because
duplex request and response work can overlap, an interceptor uses internal
synchronization for mutable state shared by its own callbacks. State shared
across exchanges must be an explicit factory-owned `Arc`; the runtime does not
provide a global registry.

## Short circuits

If B returns a local response, C is not entered. The response unwinds through B
then A, including their response body plans, followed by terminal completion in
the same reverse order. If B aborts, C is not entered and failure callbacks run
B then A. An abort or local replacement after downstream response commitment is
not representable as a head action.

## Failures and cancellation

A required factory failure rejects the exchange; an optional factory failure is
skipped and emitted to observers. Hook timeout, panic, cancellation, shutdown,
and explicit abort are distinct outcomes. Body limits and invalid trailer order,
request or response translation, routing authorization, and upstream failures
retain their lifecycle stage and commitment flags.

Terminal callbacks use their own finite cleanup timeout. A failure or panic in
one cleanup callback is reported but does not suppress later cleanup callbacks.
The terminal guard accepts the first completed-or-failed outcome only.

Dropping a hook, route, upstream, or observer boundary future aborts its child
task. Dropping a body consumer makes its bounded producer fail and release the
exchange. H2 and H3 callbacks hold per-exchange pause permits, so a paused stream
does not pause unrelated streams.

## Observation

Observers see monotonically sequenced events: exchange start, optional hook
initialization diagnostics, finalized request head, selected route, finalized
response head, requested body observations, and one terminal event. Credential
headers are removed before dispatch. Body observation is metadata-only unless a
finite prefix is explicitly requested. Graceful observer shutdown drains events
already accepted by its bounded queue.

These Rust event types are internal library contracts, not a stable serialized
control protocol. A future external controller should version its own schema and
translate it at the bridge boundary.

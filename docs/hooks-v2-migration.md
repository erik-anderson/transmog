# Migrating to Hooks v2

Hooks v2 replaces the pre-0.1 generic breakpoint callback. This is an intentional
pre-1.0 source break; the old event, decision, handler, runner, and invalid-phase
runtime errors have been removed.

| Previous concept | Hooks v2 |
|---|---|
| one shared `BreakpointHandler` | `InterceptorFactory` creating one `ExchangeInterceptor` per exchange |
| `BreakpointEvent` plus optional fields | phase-specific request/response head and body events |
| one `BreakpointDecision` enum | phase-specific action types |
| session-ID map for transient state | fields on the per-exchange interceptor or `HookContext::extensions` |
| body transform chosen after a frame arrives | bounded `BodyPlan` chosen before pump construction |
| editing `Host` to redirect | explicit `RequestHeadAction::Reroute` plus destination authorization |
| capture and mutation in one callback | interceptors mutate; observers receive immutable redacted events |

Implement `InterceptorFactory::create` and return a fresh `Arc<dyn
ExchangeInterceptor>`. Put exchange-local flags on that interceptor. Keep only
deliberately shared, synchronized configuration on the factory. Return
`BodyPlan::Buffer` with an explicit nonzero limit for complete-body editing, or
`BodyPlan::Transform` for streaming changes. Wrap each changing plan with the
appropriate request/response action constructor: `raw`, `decoded`, or
`decoded_if_supported`. Use `pass_through` for a neutral byte-exact path. Mixed
raw and decoded plans now fail deterministically before the body pump starts.

`ProxyServer::bind` remains a convenience for one required interceptor and a
MITM CA. Use `InterceptorChainFactory` plus `ProxyServer::bind_with_chain` for
multiple interceptors. Use `ProxyComponents` and
`ProxyServer::bind_with_components` to supply observers, routing, certificates,
infrastructure providers, or an application-owned upstream service.

The compatibility evidence fields named `request_breakpoint_fired` and
`response_breakpoint_fired` remain in the current live-report schema so existing
automation can read historical reports. They now mean that Hooks v2 request and
response phases completed; they are not a retained v1 execution API.

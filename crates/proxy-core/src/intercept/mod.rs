//! Typed, protocol-neutral Hooks v2 interception surface.

mod action;
mod audit;
mod body;
mod bridge;
mod chain;
mod context;
mod failure;

use std::{future::Future, pin::Pin, sync::Arc};

pub use action::{
    RequestBodyAction, RequestBodyEvent, RequestHeadAction, RequestHeadEvent, ResponseBodyAction,
    ResponseBodyEvent, ResponseHeadAction, ResponseHeadEvent,
};
pub use audit::{
    BodyPlanKind, HeaderChanges, HookAuditTrail, HookEffect, HookEffectAction, HookPhase,
    InterceptorId, InterceptorIdentity, RequestHeadChanges, ResponseHeadChanges,
};
pub use body::{
    BodyFilter, BodyHookError, BodyPipeline, BodyPipelineError, BodyPipelineLimits, BodyPlan,
    BodyPlanSelection, BodyRepresentation, BodyRepresentationError, BoxBodyFuture, BufferedBody,
    BufferedBodyHandler, HookAbort,
};
pub use bridge::{
    BridgeCommand, BridgeCorrelationId, BridgeError, BridgeReplyError, DecisionBridge,
    DecisionController,
};
pub use chain::{
    ChainExecutionError, ChainInitError, ExchangeChain, HookLimits, InitializationDiagnostic,
    InterceptorChainFactory, InterceptorRegistration, InterceptorRegistrationProvider,
    InterceptorRequirement, RequestHeadOutcome, ResponseHeadOutcome, TerminalReport,
};
pub use context::{
    ExchangeCancellation, ExchangeId, ExchangeMetadata, HookContext, OriginalTarget,
};
pub use failure::{
    CompletedExchange, ExchangeFailure, ExchangeFailureKind, ExchangeStage, HookExecutionError,
    HookInitError,
};

/// Boxed asynchronous hook result.
pub type BoxHookFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Shared registration that creates one isolated interceptor per exchange.
pub trait InterceptorFactory: Send + Sync {
    /// Creates an interceptor instance for one exchange.
    ///
    /// # Errors
    ///
    /// Returns [`HookInitError`] when a required interceptor cannot initialize.
    fn create(
        &self,
        metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError>;
}

/// Stateful interception hooks scoped to one exchange.
///
/// Request and response callbacks may overlap on duplex protocols. Mutable
/// implementations must synchronize their exchange-local state.
pub trait ExchangeInterceptor: Send + Sync {
    /// Inspects or changes the request head before route selection.
    fn on_request_head(&self, _event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        Box::pin(async { RequestHeadAction::Continue })
    }

    /// Selects request body behavior before its pump starts.
    fn on_request_body(&self, _event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        Box::pin(async { RequestBodyAction::pass_through() })
    }

    /// Inspects or changes an uncommitted response head.
    fn on_response_head(&self, _event: ResponseHeadEvent) -> BoxHookFuture<'_, ResponseHeadAction> {
        Box::pin(async { ResponseHeadAction::Continue })
    }

    /// Selects response body behavior before its pump starts.
    fn on_response_body(&self, _event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        Box::pin(async { ResponseBodyAction::pass_through() })
    }

    /// Receives successful terminal cleanup notification.
    fn on_completed(&self, _outcome: CompletedExchange) -> BoxHookFuture<'_, ()> {
        Box::pin(async {})
    }

    /// Receives failed terminal cleanup notification.
    fn on_failed(&self, _failure: ExchangeFailure) -> BoxHookFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// Factory for a pass-through per-exchange interceptor.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopInterceptorFactory;

impl InterceptorFactory for NoopInterceptorFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(NoopInterceptor))
    }
}

/// Pass-through Hooks v2 interceptor.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopInterceptor;

impl ExchangeInterceptor for NoopInterceptor {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        ConnectionId, HeaderBlock, HttpLegVersion, RequestHead, SessionId, SessionMetadata,
        StreamId, Target,
    };

    use super::*;

    fn metadata(id: u128) -> ExchangeMetadata {
        ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(id),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse().unwrap(),
                client_identity: crate::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2000".parse().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: None,
            },
            Target {
                scheme: "https".to_owned(),
                authority: "example.test".to_owned(),
                host: "example.test".to_owned(),
                port: 443,
                path: "/".to_owned(),
                query: None,
            },
        )
    }

    #[tokio::test]
    async fn no_op_factory_creates_isolated_pass_through_instances() {
        let first = NoopInterceptorFactory.create(&metadata(1)).unwrap();
        let second = NoopInterceptorFactory.create(&metadata(2)).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        let context = HookContext::new(metadata(1), Duration::from_secs(1));
        let head = RequestHead {
            method: "GET".to_owned(),
            target: context.metadata().original_target.as_target().clone(),
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http2,
        };
        assert!(matches!(
            first
                .on_request_head(RequestHeadEvent { context, head })
                .await,
            RequestHeadAction::Continue
        ));
    }
}

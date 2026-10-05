use std::{
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use thiserror::Error;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::timeout,
};

use crate::{BodyFrame, CanonicalResponse, RequestHead, ResponseHead, Target, task::AbortOnDrop};

use super::{
    BodyPipeline, BodyPipelineError, BodyPipelineLimits, BodyPlanSelection, CompletedExchange,
    ExchangeFailure, ExchangeInterceptor, ExchangeMetadata, HookContext, HookEffect,
    HookEffectAction, HookExecutionError, HookInitError, HookPhase, InterceptorFactory,
    InterceptorId, InterceptorIdentity, RequestBodyEvent, RequestHeadAction, RequestHeadEvent,
    ResponseBodyEvent, ResponseHeadAction, ResponseHeadEvent,
    audit::{body_plan_effect, request_changes, response_changes, response_summary},
};

const TERMINAL_OPEN: u8 = 0;
const TERMINAL_COMPLETING: u8 = 1;
const TERMINAL_FAILED: u8 = 2;

/// Resource limits shared by an interceptor chain.
#[derive(Clone, Copy, Debug)]
pub struct HookLimits {
    /// Maximum duration of one traffic-affecting callback.
    pub callback_timeout: Duration,
    /// Maximum duration of one terminal cleanup callback.
    pub terminal_timeout: Duration,
    /// Maximum number of hook callbacks concurrently holding pause permits.
    pub max_paused_exchanges: NonZeroUsize,
}

impl Default for HookLimits {
    fn default() -> Self {
        Self {
            callback_timeout: Duration::from_secs(30),
            terminal_timeout: Duration::from_secs(2),
            max_paused_exchanges: NonZeroUsize::new(256).expect("256 is nonzero"),
        }
    }
}

/// Whether failure to construct an interceptor rejects the exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterceptorRequirement {
    /// Reject the exchange when creation fails or panics.
    Required,
    /// Skip the interceptor and retain an initialization diagnostic.
    Optional,
}

/// One immutable interceptor-chain registration.
#[derive(Clone)]
pub struct InterceptorRegistration {
    factory: Arc<dyn InterceptorFactory>,
    requirement: InterceptorRequirement,
    id: InterceptorId,
    name: Arc<str>,
}

impl std::fmt::Debug for InterceptorRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InterceptorRegistration")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("requirement", &self.requirement)
            .finish_non_exhaustive()
    }
}

impl InterceptorRegistration {
    /// Creates one named registration.
    pub fn new(
        id: impl Into<Arc<str>>,
        factory: Arc<dyn InterceptorFactory>,
        requirement: InterceptorRequirement,
    ) -> Self {
        let id = id.into();
        Self {
            factory,
            requirement,
            id: InterceptorId::new(Arc::clone(&id)),
            name: id,
        }
    }

    /// Creates one registration with distinct stable and display identities.
    pub fn named(
        id: impl Into<Arc<str>>,
        name: impl Into<Arc<str>>,
        factory: Arc<dyn InterceptorFactory>,
        requirement: InterceptorRequirement,
    ) -> Self {
        Self {
            factory,
            requirement,
            id: InterceptorId::new(id),
            name: name.into(),
        }
    }

    /// Stable identity used by audit and capture consumers.
    pub fn id(&self) -> &InterceptorId {
        &self.id
    }

    /// Stable operator-facing name for diagnostics.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Initialization policy for this registration.
    pub fn requirement(&self) -> InterceptorRequirement {
        self.requirement
    }
}

/// Product- or embedder-owned source of immutable registrations for one
/// admitted exchange.
///
/// Implementations must return a finite deterministic snapshot and must not
/// retain a reference to `metadata`. The core owns no product configuration.
pub trait InterceptorRegistrationProvider: Send + Sync {
    /// Produces registrations for one exchange before any traffic callback.
    ///
    /// # Errors
    /// Returns a redaction-safe initialization failure. Provider panics are
    /// contained and converted to the same fail-closed result.
    fn registrations(
        &self,
        metadata: &ExchangeMetadata,
    ) -> Result<Vec<InterceptorRegistration>, HookInitError>;
}

#[derive(Clone)]
enum RegistrationSource {
    Static(InterceptorRegistration),
    Dynamic {
        name: Arc<str>,
        provider: Arc<dyn InterceptorRegistrationProvider>,
        max_registrations: NonZeroUsize,
    },
}

impl std::fmt::Debug for RegistrationSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(registration) => registration.fmt(formatter),
            Self::Dynamic {
                name,
                max_registrations,
                ..
            } => formatter
                .debug_struct("DynamicRegistrationProvider")
                .field("name", name)
                .field("max_registrations", max_registrations)
                .finish_non_exhaustive(),
        }
    }
}

/// Immutable chain configuration shared by all exchanges.
#[derive(Clone, Debug)]
pub struct InterceptorChainFactory {
    sources: Arc<[RegistrationSource]>,
    runner: CallbackGate,
}

impl InterceptorChainFactory {
    /// Freezes registrations and resource limits for listener use.
    pub fn new(registrations: Vec<InterceptorRegistration>, limits: HookLimits) -> Self {
        Self {
            sources: registrations
                .into_iter()
                .map(RegistrationSource::Static)
                .collect::<Vec<_>>()
                .into(),
            runner: CallbackGate::new(limits),
        }
    }

    /// Adds one registration after all existing hooks while preserving the
    /// chain's callback limits and shared pause-permit pool.
    #[must_use]
    pub fn with_registration(self, registration: InterceptorRegistration) -> Self {
        let mut sources = self.sources.iter().cloned().collect::<Vec<_>>();
        sources.push(RegistrationSource::Static(registration));
        Self {
            sources: sources.into(),
            runner: self.runner,
        }
    }

    /// Adds a bounded per-exchange registration snapshot at this exact chain
    /// position.
    #[must_use]
    pub fn with_registration_provider(
        self,
        name: impl Into<Arc<str>>,
        provider: Arc<dyn InterceptorRegistrationProvider>,
        max_registrations: NonZeroUsize,
    ) -> Self {
        let mut sources = self.sources.iter().cloned().collect::<Vec<_>>();
        sources.push(RegistrationSource::Dynamic {
            name: name.into(),
            provider,
            max_registrations,
        });
        Self {
            sources: sources.into(),
            runner: self.runner,
        }
    }

    /// Creates isolated interceptor instances for one exchange.
    ///
    /// Optional initialization failures are returned as diagnostics. A required
    /// initialization failure rejects construction.
    ///
    /// # Errors
    ///
    /// Returns [`ChainInitError`] when a required factory fails or panics.
    pub fn create_exchange(
        &self,
        metadata: ExchangeMetadata,
    ) -> Result<ExchangeChain, ChainInitError> {
        let mut registrations = Vec::new();
        for source in self.sources.iter() {
            match source {
                RegistrationSource::Static(registration) => {
                    registrations.push(registration.clone());
                }
                RegistrationSource::Dynamic {
                    name,
                    provider,
                    max_registrations,
                } => {
                    let supplied =
                        catch_unwind(AssertUnwindSafe(|| provider.registrations(&metadata)))
                            .map_err(|_| ChainInitError {
                                name: Arc::clone(name),
                                source: HookInitError::new("registration provider panicked"),
                            })?
                            .map_err(|source| ChainInitError {
                                name: Arc::clone(name),
                                source,
                            })?;
                    if supplied.len() > max_registrations.get() {
                        return Err(ChainInitError {
                            name: Arc::clone(name),
                            source: HookInitError::new(
                                "registration provider exceeded its finite limit",
                            ),
                        });
                    }
                    registrations.extend(supplied);
                }
            }
        }
        let context = HookContext::new(metadata, self.runner.callback_timeout());
        let mut interceptors = Vec::with_capacity(registrations.len());
        let mut diagnostics = Vec::new();

        for (chain_position, registration) in registrations.iter().enumerate() {
            let created = catch_unwind(AssertUnwindSafe(|| {
                registration.factory.create(context.metadata())
            }));
            let result = match created {
                Ok(result) => result,
                Err(_) => Err(HookInitError::new("interceptor factory panicked")),
            };
            match result {
                Ok(interceptor) => interceptors.push(ChainEntry {
                    identity: InterceptorIdentity {
                        id: registration.id.clone(),
                        name: Arc::clone(&registration.name),
                        chain_position,
                    },
                    interceptor,
                }),
                Err(source) if registration.requirement == InterceptorRequirement::Optional => {
                    diagnostics.push(InitializationDiagnostic {
                        name: Arc::clone(&registration.name),
                        message: source.message,
                    });
                }
                Err(source) => {
                    return Err(ChainInitError {
                        name: Arc::clone(&registration.name),
                        source,
                    });
                }
            }
        }

        Ok(ExchangeChain {
            context,
            interceptors,
            entered: 0,
            diagnostics,
            runner: self.runner.clone(),
            terminal: AtomicU8::new(TERMINAL_OPEN),
        })
    }

    /// Stops new callbacks from acquiring pause permits.
    pub fn shutdown(&self) {
        self.runner.shutdown();
    }
}

/// Failure constructing a required interceptor.
#[derive(Clone, Debug, Error)]
#[error("required interceptor {name} failed to initialize: {source}")]
pub struct ChainInitError {
    /// Registration name.
    pub name: Arc<str>,
    /// Typed initialization failure.
    #[source]
    pub source: HookInitError,
}

/// Non-fatal failure from an optional interceptor factory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializationDiagnostic {
    /// Registration name.
    pub name: Arc<str>,
    /// Redacted reason.
    pub message: String,
}

#[derive(Clone)]
struct ChainEntry {
    identity: InterceptorIdentity,
    interceptor: Arc<dyn ExchangeInterceptor>,
}

impl std::fmt::Debug for ChainEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChainEntry")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Result of applying the request-head chain.
#[derive(Debug)]
pub enum RequestHeadOutcome {
    /// Continue to routing with the effective request and optional explicit target.
    Continue {
        /// Final effective request head.
        head: RequestHead,
        /// Explicit reroute target requiring authorization.
        reroute: Option<Target>,
    },
    /// Complete locally. The response must still unwind through response hooks.
    Respond {
        /// Effective request at the short-circuit point.
        request_head: RequestHead,
        /// Locally generated response.
        response: CanonicalResponse,
    },
    /// Terminate before contacting an upstream.
    Abort(super::HookAbort),
}

/// Result of applying the reverse response-head chain.
#[derive(Debug)]
pub enum ResponseHeadOutcome {
    /// Continue downstream with a final head and an optional bounded body replacement.
    Continue {
        /// Final response head.
        head: ResponseHead,
        /// Replacement body supplied by a response hook, when any.
        replacement_body: Option<Vec<BodyFrame>>,
        /// Whether the current response was generated locally.
        local_response: bool,
    },
    /// Terminate before downstream response commitment.
    Abort(super::HookAbort),
}

/// Failure from one named hook callback.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("interceptor {name} failed: {source}")]
pub struct ChainExecutionError {
    /// Registration name.
    pub name: Arc<str>,
    /// Typed execution failure.
    #[source]
    pub source: HookExecutionError,
}

/// Errors collected while continuing reverse-order terminal cleanup.
#[derive(Clone, Debug, Default)]
pub struct TerminalReport {
    errors: Vec<ChainExecutionError>,
}

impl TerminalReport {
    /// Cleanup errors in callback order.
    pub fn errors(&self) -> &[ChainExecutionError] {
        &self.errors
    }

    /// Whether every entered interceptor completed cleanup successfully.
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Isolated, stateful interceptor chain for one exchange.
#[derive(Debug)]
pub struct ExchangeChain {
    context: HookContext,
    interceptors: Vec<ChainEntry>,
    entered: usize,
    diagnostics: Vec<InitializationDiagnostic>,
    runner: CallbackGate,
    terminal: AtomicU8,
}

impl ExchangeChain {
    /// Shared exchange-local hook context.
    pub fn context(&self) -> &HookContext {
        &self.context
    }

    /// Optional initialization failures skipped for this exchange.
    pub fn initialization_diagnostics(&self) -> &[InitializationDiagnostic] {
        &self.diagnostics
    }

    /// Returns every hook effect recorded for this exchange so far.
    pub fn hook_effects(&self) -> Vec<HookEffect> {
        self.context.audit().snapshot()
    }

    /// Returns hook effects not previously published by the runtime.
    pub fn take_unpublished_hook_effects(&self) -> Vec<HookEffect> {
        self.context.audit().take_unpublished()
    }

    /// Number of interceptors whose request-head callback was entered.
    pub fn entered_len(&self) -> usize {
        self.entered
    }

    /// Applies request-head callbacks in registration order.
    ///
    /// # Errors
    ///
    /// Returns a named timeout, cancellation, panic, or shutdown failure.
    pub async fn request_head(
        &mut self,
        mut head: RequestHead,
    ) -> Result<RequestHeadOutcome, ChainExecutionError> {
        let mut reroute = None;
        while self.entered < self.interceptors.len() {
            let entry = self.interceptors[self.entered].clone();
            self.entered += 1;
            let event = RequestHeadEvent {
                context: self.context.clone(),
                head: head.clone(),
            };
            let action = self
                .runner
                .request_head(Arc::clone(&entry.interceptor), event, &self.context)
                .await
                .map_err(|source| ChainExecutionError {
                    name: Arc::clone(&entry.identity.name),
                    source,
                })?;
            match action {
                RequestHeadAction::Continue => {}
                RequestHeadAction::Replace(replacement) => {
                    let changes = request_changes(&head, &replacement);
                    let did_change = !changes.is_empty();
                    self.context.audit().record(
                        entry.identity,
                        HookPhase::RequestHead,
                        HookEffectAction::ReplaceRequestHead(changes),
                        did_change,
                    );
                    head = replacement;
                }
                RequestHeadAction::Reroute {
                    head: mut replacement,
                    target,
                } => {
                    replacement.target = target.clone();
                    let changes = request_changes(&head, &replacement);
                    let did_change = !changes.is_empty() || reroute.as_ref() != Some(&target);
                    self.context.audit().record(
                        entry.identity,
                        HookPhase::RequestHead,
                        HookEffectAction::Reroute {
                            changes,
                            target: target.clone(),
                        },
                        did_change,
                    );
                    head = replacement;
                    reroute = Some(target);
                }
                RequestHeadAction::Respond(response) => {
                    self.context.audit().record(
                        entry.identity,
                        HookPhase::RequestHead,
                        response_summary(&response),
                        true,
                    );
                    return Ok(RequestHeadOutcome::Respond {
                        request_head: head,
                        response,
                    });
                }
                RequestHeadAction::Abort(reason) => {
                    self.context.audit().record(
                        entry.identity,
                        HookPhase::RequestHead,
                        HookEffectAction::Abort(reason.clone()),
                        true,
                    );
                    return Ok(RequestHeadOutcome::Abort(reason));
                }
            }
        }
        Ok(RequestHeadOutcome::Continue { head, reroute })
    }

    /// Selects request body plans in request registration order.
    ///
    /// # Errors
    ///
    /// Returns a named timeout, cancellation, panic, or shutdown failure.
    pub async fn request_body_plans(
        &self,
        head: &RequestHead,
    ) -> Result<Vec<BodyPlanSelection>, ChainExecutionError> {
        let mut plans = Vec::with_capacity(self.entered);
        for entry in &self.interceptors[..self.entered] {
            let event = RequestBodyEvent {
                context: self.context.clone(),
                head: head.clone(),
            };
            let action = self
                .runner
                .request_body(Arc::clone(&entry.interceptor), event, &self.context)
                .await
                .map_err(|source| ChainExecutionError {
                    name: Arc::clone(&entry.identity.name),
                    source,
                })?;
            if let Some(effect) = body_plan_effect(action.0.plan(), action.0.representation()) {
                self.context.audit().record(
                    entry.identity.clone(),
                    HookPhase::RequestBody,
                    effect,
                    true,
                );
            }
            plans.push(action.0);
        }
        Ok(plans)
    }

    /// Selects and composes request body plans in registration order.
    ///
    /// # Errors
    ///
    /// Returns a named callback execution failure or a typed representation
    /// conflict between selected plans.
    pub async fn request_body_pipeline(
        &self,
        head: &RequestHead,
        limits: BodyPipelineLimits,
    ) -> Result<BodyPipeline, BodyPipelineError> {
        let plans = self.request_body_plans(head).await?;
        Ok(BodyPipeline::from_plans(
            plans,
            self.context.clone(),
            self.runner.clone(),
            limits,
        )?)
    }

    /// Applies response-head callbacks in reverse entered order.
    ///
    /// `replacement_body` carries a bounded local-response body through earlier
    /// response hooks without forcing network responses to buffer.
    ///
    /// # Errors
    ///
    /// Returns a named timeout, cancellation, panic, or shutdown failure.
    pub async fn response_head(
        &self,
        request_head: &RequestHead,
        mut head: ResponseHead,
        mut replacement_body: Option<Vec<BodyFrame>>,
        mut local_response: bool,
    ) -> Result<ResponseHeadOutcome, ChainExecutionError> {
        for entry in self.interceptors[..self.entered].iter().rev() {
            let event = ResponseHeadEvent {
                context: self.context.clone(),
                request_head: request_head.clone(),
                head: head.clone(),
                local_response,
            };
            let action = self
                .runner
                .response_head(Arc::clone(&entry.interceptor), event, &self.context)
                .await
                .map_err(|source| ChainExecutionError {
                    name: Arc::clone(&entry.identity.name),
                    source,
                })?;
            match action {
                ResponseHeadAction::Continue => {}
                ResponseHeadAction::Replace(replacement) => {
                    let changes = response_changes(&head, &replacement);
                    let did_change = !changes.is_empty();
                    self.context.audit().record(
                        entry.identity.clone(),
                        HookPhase::ResponseHead,
                        HookEffectAction::ReplaceResponseHead(changes),
                        did_change,
                    );
                    head = replacement;
                }
                ResponseHeadAction::Respond(response) => {
                    self.context.audit().record(
                        entry.identity.clone(),
                        HookPhase::ResponseHead,
                        response_summary(&response),
                        true,
                    );
                    head = response.head;
                    replacement_body = Some(response.body);
                    local_response = true;
                }
                ResponseHeadAction::Abort(reason) => {
                    self.context.audit().record(
                        entry.identity.clone(),
                        HookPhase::ResponseHead,
                        HookEffectAction::Abort(reason.clone()),
                        true,
                    );
                    return Ok(ResponseHeadOutcome::Abort(reason));
                }
            }
        }
        Ok(ResponseHeadOutcome::Continue {
            head,
            replacement_body,
            local_response,
        })
    }

    /// Selects response body plans in reverse entered order.
    ///
    /// # Errors
    ///
    /// Returns a named timeout, cancellation, panic, or shutdown failure.
    pub async fn response_body_plans(
        &self,
        request_head: &RequestHead,
        response_head: &ResponseHead,
        local_response: bool,
    ) -> Result<Vec<BodyPlanSelection>, ChainExecutionError> {
        let mut plans = Vec::with_capacity(self.entered);
        for entry in self.interceptors[..self.entered].iter().rev() {
            let event = ResponseBodyEvent {
                context: self.context.clone(),
                request_head: request_head.clone(),
                response_head: response_head.clone(),
                local_response,
            };
            let action = self
                .runner
                .response_body(Arc::clone(&entry.interceptor), event, &self.context)
                .await
                .map_err(|source| ChainExecutionError {
                    name: Arc::clone(&entry.identity.name),
                    source,
                })?;
            if let Some(effect) = body_plan_effect(action.0.plan(), action.0.representation()) {
                self.context.audit().record(
                    entry.identity.clone(),
                    HookPhase::ResponseBody,
                    effect,
                    true,
                );
            }
            plans.push(action.0);
        }
        Ok(plans)
    }

    /// Selects and composes response body plans in reverse registration order.
    ///
    /// # Errors
    ///
    /// Returns a named callback execution failure or a typed representation
    /// conflict between selected plans.
    pub async fn response_body_pipeline(
        &self,
        request_head: &RequestHead,
        response_head: &ResponseHead,
        local_response: bool,
        limits: BodyPipelineLimits,
    ) -> Result<BodyPipeline, BodyPipelineError> {
        let plans = self
            .response_body_plans(request_head, response_head, local_response)
            .await?;
        Ok(BodyPipeline::from_plans(
            plans,
            self.context.clone(),
            self.runner.clone(),
            limits,
        )?)
    }

    /// Delivers successful terminal cleanup exactly once in reverse order.
    pub async fn completed(&self, outcome: CompletedExchange) -> TerminalReport {
        if self
            .terminal
            .compare_exchange(
                TERMINAL_OPEN,
                TERMINAL_COMPLETING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return TerminalReport::default();
        }
        let mut report = TerminalReport::default();
        for entry in self.interceptors[..self.entered].iter().rev() {
            if let Err(source) = self
                .runner
                .completed(Arc::clone(&entry.interceptor), outcome.clone())
                .await
            {
                report.errors.push(ChainExecutionError {
                    name: Arc::clone(&entry.identity.name),
                    source,
                });
            }
        }
        report
    }

    /// Delivers failed terminal cleanup exactly once in reverse order.
    pub async fn failed(&self, failure: ExchangeFailure) -> TerminalReport {
        if self
            .terminal
            .compare_exchange(
                TERMINAL_OPEN,
                TERMINAL_FAILED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return TerminalReport::default();
        }
        let mut report = TerminalReport::default();
        for entry in self.interceptors[..self.entered].iter().rev() {
            if let Err(source) = self
                .runner
                .failed(Arc::clone(&entry.interceptor), failure.clone())
                .await
            {
                report.errors.push(ChainExecutionError {
                    name: Arc::clone(&entry.identity.name),
                    source,
                });
            }
        }
        report
    }
}

#[derive(Clone, Debug)]
pub(super) struct CallbackGate {
    permits: Arc<Semaphore>,
    callback_timeout: Duration,
    terminal_timeout: Duration,
}

impl CallbackGate {
    pub(super) fn new(limits: HookLimits) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limits.max_paused_exchanges.get())),
            callback_timeout: limits.callback_timeout,
            terminal_timeout: limits.terminal_timeout,
        }
    }

    fn callback_timeout(&self) -> Duration {
        self.callback_timeout
    }

    fn shutdown(&self) {
        self.permits.close();
    }

    pub(super) async fn permit(
        &self,
        context: &HookContext,
    ) -> Result<OwnedSemaphorePermit, HookExecutionError> {
        tokio::select! {
            permit = Arc::clone(&self.permits).acquire_owned() => {
                permit.map_err(|_| HookExecutionError::ShuttingDown)
            }
            () = context.cancellation().cancelled() => Err(HookExecutionError::Cancelled),
        }
    }

    pub(super) async fn await_task<T: Send + 'static>(
        &self,
        context: &HookContext,
        task: JoinHandle<T>,
        permit: OwnedSemaphorePermit,
    ) -> Result<T, HookExecutionError> {
        let mut task = AbortOnDrop::new(task);
        let result = tokio::select! {
            joined = timeout(self.callback_timeout, task.handle()) => match joined {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) if error.is_panic() => Err(HookExecutionError::Panicked),
                Ok(Err(_)) => Err(HookExecutionError::Cancelled),
                Err(_) => Err(HookExecutionError::TimedOut),
            },
            () = context.cancellation().cancelled() => Err(HookExecutionError::Cancelled),
        };
        if result.is_err() && !task.is_finished() {
            task.abort_and_wait().await;
        }
        drop(permit);
        result
    }

    async fn terminal_permit(&self) -> Result<OwnedSemaphorePermit, HookExecutionError> {
        timeout(
            self.terminal_timeout,
            Arc::clone(&self.permits).acquire_owned(),
        )
        .await
        .map_err(|_| HookExecutionError::TimedOut)?
        .map_err(|_| HookExecutionError::ShuttingDown)
    }

    async fn await_terminal<T: Send + 'static>(
        &self,
        task: JoinHandle<T>,
        permit: OwnedSemaphorePermit,
    ) -> Result<T, HookExecutionError> {
        let mut task = AbortOnDrop::new(task);
        let result = match timeout(self.terminal_timeout, task.handle()).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) if error.is_panic() => Err(HookExecutionError::Panicked),
            Ok(Err(_)) => Err(HookExecutionError::Cancelled),
            Err(_) => Err(HookExecutionError::TimedOut),
        };
        if result.is_err() && !task.is_finished() {
            task.abort_and_wait().await;
        }
        drop(permit);
        result
    }

    async fn request_head(
        &self,
        interceptor: Arc<dyn ExchangeInterceptor>,
        event: RequestHeadEvent,
        context: &HookContext,
    ) -> Result<RequestHeadAction, HookExecutionError> {
        let permit = self.permit(context).await?;
        let task = tokio::spawn(async move { interceptor.on_request_head(event).await });
        self.await_task(context, task, permit).await
    }

    async fn request_body(
        &self,
        interceptor: Arc<dyn ExchangeInterceptor>,
        event: RequestBodyEvent,
        context: &HookContext,
    ) -> Result<super::RequestBodyAction, HookExecutionError> {
        let permit = self.permit(context).await?;
        let task = tokio::spawn(async move { interceptor.on_request_body(event).await });
        self.await_task(context, task, permit).await
    }

    async fn response_head(
        &self,
        interceptor: Arc<dyn ExchangeInterceptor>,
        event: ResponseHeadEvent,
        context: &HookContext,
    ) -> Result<ResponseHeadAction, HookExecutionError> {
        let permit = self.permit(context).await?;
        let task = tokio::spawn(async move { interceptor.on_response_head(event).await });
        self.await_task(context, task, permit).await
    }

    async fn response_body(
        &self,
        interceptor: Arc<dyn ExchangeInterceptor>,
        event: ResponseBodyEvent,
        context: &HookContext,
    ) -> Result<super::ResponseBodyAction, HookExecutionError> {
        let permit = self.permit(context).await?;
        let task = tokio::spawn(async move { interceptor.on_response_body(event).await });
        self.await_task(context, task, permit).await
    }

    async fn completed(
        &self,
        interceptor: Arc<dyn ExchangeInterceptor>,
        outcome: CompletedExchange,
    ) -> Result<(), HookExecutionError> {
        let permit = self.terminal_permit().await?;
        let task = tokio::spawn(async move { interceptor.on_completed(outcome).await });
        self.await_terminal(task, permit).await
    }

    async fn failed(
        &self,
        interceptor: Arc<dyn ExchangeInterceptor>,
        failure: ExchangeFailure,
    ) -> Result<(), HookExecutionError> {
        let permit = self.terminal_permit().await?;
        let task = tokio::spawn(async move { interceptor.on_failed(failure).await });
        self.await_terminal(task, permit).await
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime},
    };

    use bytes::Bytes;

    use crate::{ConnectionId, HeaderBlock, HttpLegVersion, SessionId, SessionMetadata, StreamId};

    use super::*;
    use crate::intercept::{
        BoxHookFuture, ExchangeFailureKind, ExchangeStage, HookAbort, RequestBodyAction,
        ResponseBodyAction,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Behavior {
        Continue,
        Respond,
        Abort,
        Wait,
        PanicRequest,
        PanicTerminal,
        Reroute,
    }

    struct RecordingFactory {
        name: &'static str,
        behavior: Behavior,
        log: Arc<Mutex<Vec<String>>>,
        created: Arc<AtomicUsize>,
    }

    impl InterceptorFactory for RecordingFactory {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            self.created.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(RecordingInterceptor {
                name: self.name,
                behavior: self.behavior,
                log: Arc::clone(&self.log),
            }))
        }
    }

    struct RecordingInterceptor {
        name: &'static str,
        behavior: Behavior,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingInterceptor {
        fn record(&self, phase: &str) {
            self.log
                .lock()
                .unwrap()
                .push(format!("{phase}:{}", self.name));
        }
    }

    impl ExchangeInterceptor for RecordingInterceptor {
        fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
            self.record("request-head");
            Box::pin(async move {
                match self.behavior {
                    Behavior::Continue | Behavior::PanicTerminal => RequestHeadAction::Continue,
                    Behavior::Respond => RequestHeadAction::Respond(local_response()),
                    Behavior::Abort => RequestHeadAction::Abort(HookAbort::Rejected),
                    Behavior::Wait => pending().await,
                    Behavior::PanicRequest => panic!("request hook panic"),
                    Behavior::Reroute => {
                        let mut target = event.head.target.clone();
                        target.host = "rerouted.test".to_owned();
                        target.authority = "rerouted.test:8443".to_owned();
                        target.port = 8443;
                        RequestHeadAction::Reroute {
                            head: event.head,
                            target,
                        }
                    }
                }
            })
        }

        fn on_request_body(
            &self,
            _event: RequestBodyEvent,
        ) -> BoxHookFuture<'_, RequestBodyAction> {
            self.record("request-body");
            Box::pin(async { RequestBodyAction::pass_through() })
        }

        fn on_response_head(
            &self,
            _event: ResponseHeadEvent,
        ) -> BoxHookFuture<'_, ResponseHeadAction> {
            self.record("response-head");
            Box::pin(async { ResponseHeadAction::Continue })
        }

        fn on_response_body(
            &self,
            _event: ResponseBodyEvent,
        ) -> BoxHookFuture<'_, ResponseBodyAction> {
            self.record("response-body");
            Box::pin(async { ResponseBodyAction::pass_through() })
        }

        fn on_completed(&self, _outcome: CompletedExchange) -> BoxHookFuture<'_, ()> {
            self.record("completed");
            Box::pin(async move {
                assert_ne!(
                    self.behavior,
                    Behavior::PanicTerminal,
                    "terminal hook panic"
                );
            })
        }

        fn on_failed(&self, _failure: ExchangeFailure) -> BoxHookFuture<'_, ()> {
            self.record("failed");
            Box::pin(async {})
        }
    }

    struct FailFactory {
        panic: bool,
    }

    struct FixedRegistrationProvider {
        registrations: Mutex<Vec<InterceptorRegistration>>,
        calls: AtomicUsize,
    }

    impl InterceptorRegistrationProvider for FixedRegistrationProvider {
        fn registrations(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Vec<InterceptorRegistration>, HookInitError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.registrations.lock().unwrap().clone())
        }
    }

    struct FailingRegistrationProvider {
        panic: bool,
    }

    impl InterceptorRegistrationProvider for FailingRegistrationProvider {
        fn registrations(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Vec<InterceptorRegistration>, HookInitError> {
            assert!(!self.panic, "provider panic");
            Err(HookInitError::new("provider unavailable"))
        }
    }

    struct DropTrackingFactory {
        entered: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicBool>,
    }

    struct DropTrackingInterceptor {
        entered: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicBool>,
    }

    struct DropMarker(Arc<AtomicBool>);

    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    impl InterceptorFactory for DropTrackingFactory {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(DropTrackingInterceptor {
                entered: Arc::clone(&self.entered),
                dropped: Arc::clone(&self.dropped),
            }))
        }
    }

    impl ExchangeInterceptor for DropTrackingInterceptor {
        fn on_request_head(
            &self,
            _event: RequestHeadEvent,
        ) -> BoxHookFuture<'_, RequestHeadAction> {
            Box::pin(async move {
                let _marker = DropMarker(Arc::clone(&self.dropped));
                self.entered.notify_one();
                pending().await
            })
        }
    }

    impl InterceptorFactory for FailFactory {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            assert!(!self.panic, "factory panic");
            Err(HookInitError::new("unavailable"))
        }
    }

    fn metadata(id: u128) -> ExchangeMetadata {
        ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(id),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse().unwrap(),
                proxy_addr: "127.0.0.1:2000".parse().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: None,
            },
            request_head().target,
        )
    }

    fn request_head() -> RequestHead {
        RequestHead {
            method: "GET".to_owned(),
            target: Target {
                scheme: "https".to_owned(),
                authority: "example.test".to_owned(),
                host: "example.test".to_owned(),
                port: 443,
                path: "/original".to_owned(),
                query: None,
            },
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http2,
        }
    }

    fn response_head() -> ResponseHead {
        ResponseHead {
            status: 200,
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http2,
        }
    }

    fn local_response() -> CanonicalResponse {
        CanonicalResponse::local(202, HeaderBlock::new(), Bytes::from_static(b"local"))
    }

    fn registration(
        name: &'static str,
        behavior: Behavior,
        log: &Arc<Mutex<Vec<String>>>,
        created: &Arc<AtomicUsize>,
    ) -> InterceptorRegistration {
        InterceptorRegistration::new(
            name,
            Arc::new(RecordingFactory {
                name,
                behavior,
                log: Arc::clone(log),
                created: Arc::clone(created),
            }),
            InterceptorRequirement::Required,
        )
    }

    fn limits(timeout: Duration, max_paused: usize) -> HookLimits {
        HookLimits {
            callback_timeout: timeout,
            terminal_timeout: timeout,
            max_paused_exchanges: NonZeroUsize::new(max_paused).unwrap(),
        }
    }

    fn completed(
        chain: &ExchangeChain,
        request: RequestHead,
        response: ResponseHead,
    ) -> CompletedExchange {
        CompletedExchange {
            metadata: Arc::clone(chain.context().metadata()),
            request_head: request,
            response_head: response,
        }
    }

    fn failed(chain: &ExchangeChain) -> ExchangeFailure {
        ExchangeFailure {
            metadata: Arc::clone(chain.context().metadata()),
            stage: ExchangeStage::Upstream,
            kind: ExchangeFailureKind::Upstream,
            request_committed: true,
            response_committed: false,
            message: "upstream unavailable".to_owned(),
        }
    }

    #[tokio::test]
    async fn chain_is_forward_on_request_and_reverse_on_response_and_terminal() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let factory = InterceptorChainFactory::new(
            ["A", "B", "C"]
                .map(|name| registration(name, Behavior::Continue, &log, &created))
                .into(),
            limits(Duration::from_secs(1), 4),
        );
        let mut chain = factory.create_exchange(metadata(1)).unwrap();
        let request = match chain.request_head(request_head()).await.unwrap() {
            RequestHeadOutcome::Continue {
                head,
                reroute: None,
            } => head,
            outcome => panic!("unexpected request outcome: {outcome:?}"),
        };
        assert_eq!(chain.request_body_plans(&request).await.unwrap().len(), 3);
        let response = match chain
            .response_head(&request, response_head(), None, false)
            .await
            .unwrap()
        {
            ResponseHeadOutcome::Continue { head, .. } => head,
            outcome @ ResponseHeadOutcome::Abort(_) => {
                panic!("unexpected response outcome: {outcome:?}");
            }
        };
        assert_eq!(
            chain
                .response_body_plans(&request, &response, false)
                .await
                .unwrap()
                .len(),
            3
        );
        assert!(
            chain
                .completed(completed(&chain, request, response))
                .await
                .is_clean()
        );
        assert!(chain.failed(failed(&chain)).await.is_clean());

        assert_eq!(created.load(Ordering::SeqCst), 3);
        assert_eq!(
            *log.lock().unwrap(),
            [
                "request-head:A",
                "request-head:B",
                "request-head:C",
                "request-body:A",
                "request-body:B",
                "request-body:C",
                "response-head:C",
                "response-head:B",
                "response-head:A",
                "response-body:C",
                "response-body:B",
                "response-body:A",
                "completed:C",
                "completed:B",
                "completed:A",
            ]
        );
    }

    #[tokio::test]
    async fn dynamic_registrations_are_bounded_ordered_and_snapshotted_per_exchange() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(FixedRegistrationProvider {
            registrations: Mutex::new(vec![InterceptorRegistration::named(
                "rule/conditional-user-agent@7",
                "Conditional User-Agent",
                Arc::new(RecordingFactory {
                    name: "B",
                    behavior: Behavior::Reroute,
                    log: Arc::clone(&log),
                    created: Arc::clone(&created),
                }),
                InterceptorRequirement::Required,
            )]),
            calls: AtomicUsize::new(0),
        });
        let factory = InterceptorChainFactory::new(
            vec![registration("A", Behavior::Continue, &log, &created)],
            HookLimits::default(),
        )
        .with_registration_provider(
            "active product snapshot",
            provider.clone(),
            NonZeroUsize::new(2).unwrap(),
        )
        .with_registration(registration("C", Behavior::Continue, &log, &created));

        let mut first = factory.create_exchange(metadata(1)).unwrap();
        provider.registrations.lock().unwrap().clear();
        let mut second = factory.create_exchange(metadata(2)).unwrap();

        let RequestHeadOutcome::Continue { reroute, .. } =
            first.request_head(request_head()).await.unwrap()
        else {
            panic!("expected the first exchange to continue");
        };
        assert_eq!(reroute.unwrap().host, "rerouted.test");
        assert_eq!(
            *log.lock().unwrap(),
            ["request-head:A", "request-head:B", "request-head:C"]
        );
        assert_eq!(first.hook_effects().len(), 1);
        assert_eq!(
            first.hook_effects()[0].interceptor.id.as_str(),
            "rule/conditional-user-agent@7"
        );
        assert_eq!(first.hook_effects()[0].interceptor.chain_position, 1);

        log.lock().unwrap().clear();
        assert!(matches!(
            second.request_head(request_head()).await.unwrap(),
            RequestHeadOutcome::Continue { reroute: None, .. }
        ));
        assert_eq!(*log.lock().unwrap(), ["request-head:A", "request-head:C"]);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn dynamic_registration_provider_failures_are_fail_closed_and_contained() {
        for panic in [false, true] {
            let factory = InterceptorChainFactory::new(Vec::new(), HookLimits::default())
                .with_registration_provider(
                    "broken provider",
                    Arc::new(FailingRegistrationProvider { panic }),
                    NonZeroUsize::new(1).unwrap(),
                );
            let error = factory.create_exchange(metadata(1)).unwrap_err();
            assert_eq!(&*error.name, "broken provider");
            assert_eq!(
                error.source.message,
                if panic {
                    "registration provider panicked"
                } else {
                    "provider unavailable"
                }
            );
        }
    }

    #[test]
    fn dynamic_registration_provider_cannot_exceed_its_finite_limit() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(FixedRegistrationProvider {
            registrations: Mutex::new(vec![
                registration("A", Behavior::Continue, &log, &created),
                registration("B", Behavior::Continue, &log, &created),
            ]),
            calls: AtomicUsize::new(0),
        });
        let error = InterceptorChainFactory::new(Vec::new(), HookLimits::default())
            .with_registration_provider(
                "oversized provider",
                provider,
                NonZeroUsize::new(1).unwrap(),
            )
            .create_exchange(metadata(1))
            .unwrap_err();
        assert_eq!(
            error.source.message,
            "registration provider exceeded its finite limit"
        );
    }

    #[tokio::test]
    async fn local_response_unwinds_only_the_entered_stack() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let factory = InterceptorChainFactory::new(
            vec![
                registration("A", Behavior::Continue, &log, &created),
                registration("B", Behavior::Respond, &log, &created),
                registration("C", Behavior::Continue, &log, &created),
            ],
            limits(Duration::from_secs(1), 2),
        );
        let mut chain = factory.create_exchange(metadata(1)).unwrap();
        let (request, local) = match chain.request_head(request_head()).await.unwrap() {
            RequestHeadOutcome::Respond {
                request_head,
                response,
            } => (request_head, response),
            outcome => panic!("unexpected request outcome: {outcome:?}"),
        };
        assert_eq!(chain.entered_len(), 2);
        let response = chain
            .response_head(&request, local.head, Some(local.body), true)
            .await
            .unwrap();
        assert!(matches!(
            response,
            ResponseHeadOutcome::Continue {
                local_response: true,
                ..
            }
        ));
        assert_eq!(
            *log.lock().unwrap(),
            [
                "request-head:A",
                "request-head:B",
                "response-head:B",
                "response-head:A"
            ]
        );
    }

    #[test]
    fn optional_initialization_failure_is_diagnostic_and_required_failure_rejects() {
        let optional = InterceptorRegistration::new(
            "optional",
            Arc::new(FailFactory { panic: false }),
            InterceptorRequirement::Optional,
        );
        let required = InterceptorRegistration::new(
            "required",
            Arc::new(FailFactory { panic: false }),
            InterceptorRequirement::Required,
        );
        let optional_chain = InterceptorChainFactory::new(vec![optional], HookLimits::default())
            .create_exchange(metadata(1))
            .unwrap();
        assert_eq!(optional_chain.initialization_diagnostics().len(), 1);
        let error = InterceptorChainFactory::new(vec![required], HookLimits::default())
            .create_exchange(metadata(2))
            .unwrap_err();
        assert_eq!(&*error.name, "required");

        let panicking = InterceptorRegistration::new(
            "panicking",
            Arc::new(FailFactory { panic: true }),
            InterceptorRequirement::Required,
        );
        let error = InterceptorChainFactory::new(vec![panicking], HookLimits::default())
            .create_exchange(metadata(3))
            .unwrap_err();
        assert_eq!(error.source.message, "interceptor factory panicked");
    }

    #[tokio::test]
    async fn reroute_is_explicit_and_updates_the_effective_target() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let factory = InterceptorChainFactory::new(
            vec![registration("A", Behavior::Reroute, &log, &created)],
            HookLimits::default(),
        );
        let mut chain = factory.create_exchange(metadata(1)).unwrap();
        let RequestHeadOutcome::Continue {
            head,
            reroute: Some(target),
        } = chain.request_head(request_head()).await.unwrap()
        else {
            panic!("expected explicit reroute");
        };
        assert_eq!(head.target, target);
        assert_eq!(target.host, "rerouted.test");
        assert_eq!(
            chain.context().metadata().original_target.as_target().host,
            "example.test"
        );
        let effects = chain.hook_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].sequence, 1);
        assert_eq!(effects[0].interceptor.id.as_str(), "A");
        assert_eq!(&*effects[0].interceptor.name, "A");
        assert_eq!(effects[0].interceptor.chain_position, 0);
        assert_eq!(effects[0].phase, HookPhase::RequestHead);
        assert!(effects[0].changed);
        assert!(matches!(
            &effects[0].action,
            HookEffectAction::Reroute { target: recorded, changes }
                if recorded == &target && changes.target_changed
        ));
        assert_eq!(chain.take_unpublished_hook_effects(), effects);
        assert!(chain.take_unpublished_hook_effects().is_empty());
        assert_eq!(chain.hook_effects(), effects);
    }

    #[test]
    fn registration_can_separate_stable_id_from_display_name() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let registration = InterceptorRegistration::named(
            "org.example.audit-id",
            "Friendly editor",
            Arc::new(RecordingFactory {
                name: "friendly",
                behavior: Behavior::Continue,
                log,
                created,
            }),
            InterceptorRequirement::Required,
        );
        assert_eq!(registration.id().as_str(), "org.example.audit-id");
        assert_eq!(registration.name(), "Friendly editor");
    }

    #[tokio::test]
    async fn callback_timeout_and_panic_are_contained() {
        for (behavior, expected) in [
            (Behavior::Wait, HookExecutionError::TimedOut),
            (Behavior::PanicRequest, HookExecutionError::Panicked),
        ] {
            let log = Arc::new(Mutex::new(Vec::new()));
            let created = Arc::new(AtomicUsize::new(0));
            let factory = InterceptorChainFactory::new(
                vec![registration("A", behavior, &log, &created)],
                limits(Duration::from_millis(10), 1),
            );
            let mut chain = factory.create_exchange(metadata(1)).unwrap();
            let error = chain.request_head(request_head()).await.unwrap_err();
            assert_eq!(error.source, expected);
        }
    }

    #[tokio::test]
    async fn cancellation_works_inside_a_hook_and_while_waiting_for_a_permit() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let factory = InterceptorChainFactory::new(
            vec![registration("A", Behavior::Wait, &log, &created)],
            limits(Duration::from_secs(10), 1),
        );
        let mut first = factory.create_exchange(metadata(1)).unwrap();
        let first_cancel = first.context().cancellation().clone();
        let first_task = tokio::spawn(async move { first.request_head(request_head()).await });
        while log.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }

        let mut second = factory.create_exchange(metadata(2)).unwrap();
        let second_cancel = second.context().cancellation().clone();
        let second_task = tokio::spawn(async move { second.request_head(request_head()).await });
        tokio::task::yield_now().await;
        assert_eq!(
            log.lock().unwrap().len(),
            1,
            "second hook started without a permit"
        );
        second_cancel.cancel();
        assert_eq!(
            second_task.await.unwrap().unwrap_err().source,
            HookExecutionError::Cancelled
        );

        first_cancel.cancel();
        assert_eq!(
            first_task.await.unwrap().unwrap_err().source,
            HookExecutionError::Cancelled
        );
    }

    #[tokio::test]
    async fn terminal_panic_does_not_suppress_cleanup_and_terminal_is_exactly_once() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let factory = InterceptorChainFactory::new(
            vec![
                registration("A", Behavior::Continue, &log, &created),
                registration("B", Behavior::PanicTerminal, &log, &created),
                registration("C", Behavior::Continue, &log, &created),
            ],
            limits(Duration::from_secs(1), 2),
        );
        let mut chain = factory.create_exchange(metadata(1)).unwrap();
        let request = match chain.request_head(request_head()).await.unwrap() {
            RequestHeadOutcome::Continue { head, .. } => head,
            outcome => panic!("unexpected request outcome: {outcome:?}"),
        };
        let response = response_head();
        let report = chain.completed(completed(&chain, request, response)).await;
        assert_eq!(report.errors().len(), 1);
        assert_eq!(report.errors()[0].source, HookExecutionError::Panicked);
        assert!(chain.failed(failed(&chain)).await.is_clean());
        assert_eq!(
            *log.lock().unwrap(),
            [
                "request-head:A",
                "request-head:B",
                "request-head:C",
                "completed:C",
                "completed:B",
                "completed:A"
            ]
        );
    }

    #[test]
    fn metadata_timestamp_is_populated() {
        assert!(metadata(1).started_at <= SystemTime::now());
    }

    #[tokio::test]
    async fn abort_stops_later_interceptors() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let factory = InterceptorChainFactory::new(
            vec![
                registration("A", Behavior::Abort, &log, &created),
                registration("B", Behavior::Continue, &log, &created),
            ],
            HookLimits::default(),
        );
        let mut chain = factory.create_exchange(metadata(1)).unwrap();
        assert!(matches!(
            chain.request_head(request_head()).await.unwrap(),
            RequestHeadOutcome::Abort(HookAbort::Rejected)
        ));
        assert_eq!(*log.lock().unwrap(), ["request-head:A"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_exchange_stress_releases_permits_and_keeps_state_isolated() {
        const EXCHANGES: usize = 256;
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(AtomicUsize::new(0));
        let factory = InterceptorChainFactory::new(
            vec![registration("isolated", Behavior::Continue, &log, &created)],
            limits(Duration::from_secs(2), 8),
        );
        let mut tasks = Vec::with_capacity(EXCHANGES);
        for index in 0..EXCHANGES {
            let factory = factory.clone();
            tasks.push(tokio::spawn(async move {
                let mut chain = factory
                    .create_exchange(metadata(index as u128 + 1))
                    .unwrap();
                let RequestHeadOutcome::Continue { head: request, .. } =
                    chain.request_head(request_head()).await.unwrap()
                else {
                    panic!("continue interceptor changed request outcome");
                };
                let ResponseHeadOutcome::Continue { head: response, .. } = chain
                    .response_head(&request, response_head(), None, false)
                    .await
                    .unwrap()
                else {
                    panic!("continue interceptor changed response outcome");
                };
                assert!(
                    chain
                        .completed(completed(&chain, request, response))
                        .await
                        .is_clean()
                );
                chain.context().metadata().exchange_id
            }));
        }
        let mut ids = std::collections::HashSet::new();
        for task in tasks {
            assert!(ids.insert(task.await.unwrap()));
        }
        assert_eq!(ids.len(), EXCHANGES);
        assert_eq!(created.load(Ordering::SeqCst), EXCHANGES);
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.as_str() == "completed:isolated")
                .count(),
            EXCHANGES
        );
    }

    #[tokio::test]
    async fn dropping_boundary_future_aborts_the_spawned_hook_task() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let factory = InterceptorChainFactory::new(
            vec![InterceptorRegistration::new(
                "drop-tracking",
                Arc::new(DropTrackingFactory {
                    entered: Arc::clone(&entered),
                    dropped: Arc::clone(&dropped),
                }),
                InterceptorRequirement::Required,
            )],
            limits(Duration::from_secs(30), 1),
        );
        let mut chain = factory.create_exchange(metadata(1)).unwrap();
        let boundary = tokio::spawn(async move { chain.request_head(request_head()).await });
        entered.notified().await;
        boundary.abort();
        let _ = boundary.await;
        timeout(Duration::from_secs(1), async {
            while !dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("hook task retained exchange state after boundary future was dropped");
    }
}

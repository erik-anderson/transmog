use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use transmog_control_model::{
    BreakpointPhase, Capability, DecisionAction, DecisionCommand, Handshake,
};
use transmog_control_transport::TransportConfig;
use transmog_session::{
    ApplicationSessionService, ControlPhase, ControlPolicy, ControllerMessage, PendingDecision,
};

use crate::{AppError, ErrorCategory};

const MAX_BODY_EDIT_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING_DECISIONS: usize = 256;

/// Breakpoint phase enabled by the product.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BreakpointPhaseInput {
    /// Request head before routing.
    RequestHead,
    /// Complete decoded request body.
    RequestBody,
    /// Response head before downstream commitment.
    ResponseHead,
    /// Complete decoded response body.
    ResponseBody,
}

/// Finite interactive controller settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointSettings {
    /// Explicitly enabled phases.
    pub phases: BTreeSet<BreakpointPhaseInput>,
    /// Maximum complete body edit size.
    pub body_limit: usize,
    /// Maximum simultaneous queued decisions.
    pub max_pending: usize,
    /// Per-decision fail-closed deadline in milliseconds.
    pub timeout_ms: u64,
}

impl Default for BreakpointSettings {
    fn default() -> Self {
        Self {
            phases: BTreeSet::from([
                BreakpointPhaseInput::RequestHead,
                BreakpointPhaseInput::ResponseHead,
            ]),
            body_limit: 4 * 1024 * 1024,
            max_pending: 64,
            timeout_ms: 30_000,
        }
    }
}

/// One paused exchange with bounded editable input.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PausedExchange {
    /// Controller-local single-use decision identifier.
    pub decision_id: u64,
    /// Opaque exchange identifier.
    pub exchange_id: String,
    /// Exact hook phase.
    pub phase: BreakpointPhaseInput,
    /// Editable request head, when applicable.
    pub request_head: Option<transmog_control_model::RequestHead>,
    /// Editable response head, when applicable.
    pub response_head: Option<transmog_control_model::ResponseHead>,
    /// Complete decoded body as hexadecimal, when applicable.
    pub body_hex: Option<String>,
    /// Stable Hooks v2 attribution recorded for any modification.
    pub hook_id: &'static str,
    /// Producer deadline as a Unix timestamp in milliseconds for display.
    pub expires_at_unix_ms: u64,
}

/// Current breakpoint-controller state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointStatus {
    /// Whether the exclusive controller worker is attached.
    pub enabled: bool,
    /// Bounded decisions awaiting replies.
    pub paused: Vec<PausedExchange>,
}

/// One correlated operator action.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointDecision {
    /// Controller-local decision identifier.
    pub decision_id: u64,
    /// Opaque exchange identifier from [`PausedExchange`].
    pub exchange_id: String,
    /// Continue, replace, or abort action.
    pub action: DecisionAction,
}

struct State {
    enabled: bool,
    pending: BTreeMap<u64, PendingDecision>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct BreakpointManager {
    service: ApplicationSessionService,
    build_id: Arc<str>,
    state: Arc<Mutex<State>>,
}

impl BreakpointManager {
    pub(crate) fn new(service: ApplicationSessionService, build_id: Arc<str>) -> Self {
        Self {
            service,
            build_id,
            state: Arc::new(Mutex::new(State {
                enabled: false,
                pending: BTreeMap::new(),
                worker: None,
            })),
        }
    }

    pub(crate) fn enable(
        &self,
        settings: &BreakpointSettings,
    ) -> Result<BreakpointStatus, AppError> {
        validate_settings(settings)?;
        {
            let mut state = self.lock();
            if state.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                state.worker.take();
                state.enabled = false;
                state.pending.clear();
            }
            if state.enabled {
                return Err(AppError::new(
                    ErrorCategory::Conflict,
                    "interactive breakpoint controller is already enabled",
                    false,
                ));
            }
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "interactive breakpoint controller requires an async runtime",
                false,
            )
        })?;
        let control_phases = settings
            .phases
            .iter()
            .copied()
            .map(control_phase)
            .collect::<BTreeSet<_>>();
        let mut peer = Handshake::v0(self.build_id.to_string(), settings.body_limit);
        peer.capabilities = settings
            .phases
            .iter()
            .copied()
            .map(capability)
            .chain(std::iter::once(Capability::HookEffects))
            .collect();
        let controller = self
            .service
            .connect_controller(
                &peer,
                TransportConfig {
                    event_capacity: NonZeroUsize::new(256).expect("constant is nonzero"),
                    decision_capacity: NonZeroUsize::new(settings.max_pending)
                        .expect("validated as nonzero"),
                    decision_timeout: Duration::from_millis(settings.timeout_ms),
                },
                ControlPolicy {
                    phases: control_phases,
                    body_limit: NonZeroUsize::new(settings.body_limit)
                        .expect("validated as nonzero"),
                },
            )
            .map_err(|error| AppError::new(ErrorCategory::Conflict, error.to_string(), false))?;
        let state = Arc::clone(&self.state);
        let worker = runtime.spawn(async move {
            let mut controller = controller;
            while let Some(message) = controller.recv().await {
                if let ControllerMessage::Decision(decision) = message {
                    let id = decision.request().decision_id.0;
                    let mut state = state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if state.pending.len() >= MAX_PENDING_DECISIONS {
                        drop(decision);
                    } else {
                        state.pending.insert(id, decision);
                    }
                }
            }
            let mut state = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pending.clear();
            state.enabled = false;
        });
        let mut state = self.lock();
        state.enabled = true;
        state.worker = Some(worker);
        Ok(status(&state))
    }

    pub(crate) async fn disable(&self) -> BreakpointStatus {
        let worker = self.lock().worker.take();
        if let Some(worker) = worker {
            worker.abort();
            let _ = worker.await;
        }
        let mut state = self.lock();
        state.pending.clear();
        state.enabled = false;
        status(&state)
    }

    pub(crate) fn status(&self) -> BreakpointStatus {
        let mut state = self.lock();
        state.pending.retain(|_, decision| decision.is_pending());
        status(&state)
    }

    pub(crate) fn decide(
        &self,
        decision: BreakpointDecision,
    ) -> Result<BreakpointStatus, AppError> {
        let exchange = parse_exchange_id(&decision.exchange_id)?;
        let pending = {
            let mut state = self.lock();
            let pending = state.pending.get(&decision.decision_id).ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Conflict,
                    "breakpoint decision is stale or already answered",
                    false,
                )
            })?;
            if pending.request().exchange_id.0 != exchange {
                return Err(AppError::new(
                    ErrorCategory::InvalidInput,
                    "breakpoint correlation does not match",
                    false,
                ));
            }
            validate_action(pending.request().phase, &decision.action)?;
            state
                .pending
                .remove(&decision.decision_id)
                .expect("validated pending decision")
        };
        pending
            .reply(DecisionCommand {
                decision_id: pending.request().decision_id,
                exchange_id: pending.request().exchange_id,
                action: decision.action,
            })
            .map_err(|error| AppError::new(ErrorCategory::Conflict, error.to_string(), false))?;
        Ok(self.status())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for BreakpointManager {
    fn drop(&mut self) {
        // An enabled worker holds one state reference. The last manager owner
        // therefore sees either one (idle) or two (enabled) strong references.
        if Arc::strong_count(&self.state) <= 2 {
            let mut state = self.lock();
            if let Some(worker) = state.worker.take() {
                worker.abort();
            }
            state.pending.clear();
            state.enabled = false;
        }
    }
}

fn status(state: &State) -> BreakpointStatus {
    BreakpointStatus {
        enabled: state.enabled,
        paused: state.pending.values().map(paused).collect(),
    }
}

fn paused(pending: &PendingDecision) -> PausedExchange {
    let request = pending.request();
    PausedExchange {
        decision_id: request.decision_id.0,
        exchange_id: format!("{:032x}", request.exchange_id.0),
        phase: phase_input(request.phase),
        request_head: request.request_head.clone(),
        response_head: request.response_head.clone(),
        body_hex: request.body.as_ref().map(|body| {
            body.iter()
                .take(MAX_BODY_EDIT_BYTES)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        }),
        hook_id: transmog_session::INTERACTIVE_CONTROL_HOOK_ID,
        expires_at_unix_ms: u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .saturating_add(pending.remaining())
                .as_millis(),
        )
        .unwrap_or(u64::MAX),
    }
}

fn validate_settings(settings: &BreakpointSettings) -> Result<(), AppError> {
    if settings.phases.is_empty()
        || settings.body_limit == 0
        || settings.body_limit > MAX_BODY_EDIT_BYTES
        || settings.max_pending == 0
        || settings.max_pending > MAX_PENDING_DECISIONS
        || !(100..=300_000).contains(&settings.timeout_ms)
    {
        Err(AppError::new(
            ErrorCategory::InvalidInput,
            "breakpoint limits or phase selection are invalid",
            false,
        ))
    } else {
        Ok(())
    }
}

fn parse_exchange_id(value: &str) -> Result<u128, AppError> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "breakpoint exchange identifier is invalid",
            false,
        ));
    }
    u128::from_str_radix(value, 16).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "breakpoint exchange identifier is invalid",
            false,
        )
    })
}

fn validate_action(phase: BreakpointPhase, action: &DecisionAction) -> Result<(), AppError> {
    let valid = matches!(
        action,
        DecisionAction::Continue | DecisionAction::Abort { .. }
    ) || matches!(
        (phase, action),
        (
            BreakpointPhase::RequestHead,
            DecisionAction::ReplaceRequestHead { .. }
        ) | (
            BreakpointPhase::ResponseHead,
            DecisionAction::ReplaceResponseHead { .. }
        ) | (
            BreakpointPhase::RequestBody | BreakpointPhase::ResponseBody,
            DecisionAction::ReplaceBody { .. }
        )
    );
    if valid {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCategory::InvalidInput,
            "breakpoint action is not valid for this phase",
            false,
        ))
    }
}

const fn phase_input(value: BreakpointPhase) -> BreakpointPhaseInput {
    match value {
        BreakpointPhase::RequestHead => BreakpointPhaseInput::RequestHead,
        BreakpointPhase::RequestBody => BreakpointPhaseInput::RequestBody,
        BreakpointPhase::ResponseHead => BreakpointPhaseInput::ResponseHead,
        BreakpointPhase::ResponseBody => BreakpointPhaseInput::ResponseBody,
    }
}

const fn control_phase(value: BreakpointPhaseInput) -> ControlPhase {
    match value {
        BreakpointPhaseInput::RequestHead => ControlPhase::RequestHead,
        BreakpointPhaseInput::RequestBody => ControlPhase::RequestBody,
        BreakpointPhaseInput::ResponseHead => ControlPhase::ResponseHead,
        BreakpointPhaseInput::ResponseBody => ControlPhase::ResponseBody,
    }
}

const fn capability(value: BreakpointPhaseInput) -> Capability {
    match value {
        BreakpointPhaseInput::RequestHead => Capability::RequestHead,
        BreakpointPhaseInput::RequestBody => Capability::RequestBody,
        BreakpointPhaseInput::ResponseHead => Capability::ResponseHead,
        BreakpointPhaseInput::ResponseBody => Capability::ResponseBody,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_action_validation_rejects_cross_phase_edits() {
        assert!(validate_action(BreakpointPhase::RequestHead, &DecisionAction::Continue).is_ok());
        assert!(
            validate_action(
                BreakpointPhase::RequestHead,
                &DecisionAction::ReplaceBody { body: vec![] }
            )
            .is_err()
        );
        assert!(
            validate_action(
                BreakpointPhase::ResponseBody,
                &DecisionAction::ReplaceBody { body: vec![1] }
            )
            .is_ok()
        );
    }

    #[test]
    fn breakpoint_settings_are_strictly_bounded() {
        let mut settings = BreakpointSettings::default();
        assert!(validate_settings(&settings).is_ok());
        settings.max_pending = MAX_PENDING_DECISIONS + 1;
        assert!(validate_settings(&settings).is_err());
    }

    #[test]
    fn enabling_without_a_runtime_returns_an_error_and_can_be_retried() {
        let application = crate::Application::new(crate::AppConfig::default()).unwrap();
        assert_eq!(application.status().lifecycle, crate::AppLifecycle::Stopped);
        let error = application
            .enable_breakpoints(&BreakpointSettings::default())
            .unwrap_err();
        assert_eq!(error.category, ErrorCategory::Unavailable);
        assert!(!application.paused_exchanges().enabled);
        assert!(application.paused_exchanges().paused.is_empty());
        assert!(application.breakpoints.lock().worker.is_none());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(
                application
                    .enable_breakpoints(&BreakpointSettings::default())
                    .unwrap()
                    .enabled
            );
            application.shutdown().await.unwrap();
            assert!(!application.paused_exchanges().enabled);
        });
    }

    #[tokio::test]
    async fn invalid_edits_keep_pending_decisions_and_cancelled_requests_leave_the_queue() {
        let application = crate::Application::new(crate::AppConfig::default()).unwrap();
        let handshake = Handshake::v0("queue-test", 4);
        let (producer, mut controller) =
            transmog_control_transport::connect(&handshake, &handshake, TransportConfig::default())
                .unwrap();
        let cancellation = transmog_control_transport::RequestCancellation::new();
        let trigger = cancellation.clone();
        let waiting = tokio::spawn(async move {
            producer
                .request(
                    transmog_control_model::BreakpointInput {
                        exchange_id: transmog_control_model::ControlExchangeId(3),
                        phase: BreakpointPhase::RequestHead,
                        request_head: None,
                        response_head: None,
                        body: None,
                    },
                    &cancellation,
                )
                .await
        });
        let pending = controller.recv_decision().await.unwrap();
        let id = pending.request().decision_id.0;
        application.breakpoints.lock().pending.insert(id, pending);
        let paused = application.breakpoints.status().paused;
        assert_eq!(paused.len(), 1);
        assert!(
            paused[0].expires_at_unix_ms
                > u64::try_from(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                )
                .unwrap()
        );
        assert!(
            application
                .breakpoints
                .decide(BreakpointDecision {
                    decision_id: id,
                    exchange_id: format!("{:032x}", 3),
                    action: DecisionAction::ReplaceBody { body: vec![] }
                })
                .is_err()
        );
        assert_eq!(application.breakpoints.status().paused.len(), 1);
        trigger.cancel();
        assert_eq!(
            waiting.await.unwrap(),
            Err(transmog_control_transport::TransportError::Cancelled)
        );
        assert!(application.breakpoints.status().paused.is_empty());
        application.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn controller_attachment_is_exclusive_and_reenable_is_clean() {
        let application = crate::Application::new(crate::AppConfig::default()).unwrap();
        assert!(
            application
                .enable_breakpoints(&BreakpointSettings::default())
                .unwrap()
                .enabled
        );
        assert!(
            application
                .enable_breakpoints(&BreakpointSettings::default())
                .is_err()
        );
        assert!(!application.disable_breakpoints().await.enabled);
        assert!(
            application
                .enable_breakpoints(&BreakpointSettings::default())
                .unwrap()
                .enabled
        );
        application.shutdown().await.unwrap();
    }
}

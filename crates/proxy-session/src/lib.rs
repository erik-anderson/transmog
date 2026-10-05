//! Headless application/session composition for Transmog.
//!
//! The crate owns bounded live state and application lifecycle. It deliberately
//! contains no UI types and performs no ambient operating-system mutation.

mod capture;
mod catalog;
mod control;
mod host;
mod observer;
mod replay;
mod service;

pub use capture::{
    CaptureFailure, CaptureManager, CaptureServiceError, CaptureStart, CaptureStatus, SealedCapture,
};
pub use catalog::{
    BodySnapshot, CatalogApply, CatalogCounters, CatalogCursor, CatalogDelta, CatalogError,
    CatalogPage, CatalogQuery, CatalogSubscription, ObservedRequestHead, ObservedResponseHead,
    SessionCatalog, SessionFilter, SessionLimits, SessionSnapshot, SessionTerminal,
    SubscriptionEvent,
};
pub use control::{
    AttachedController, ControlConnectionError, ControlConnector, ControlPhase, ControlPolicy,
    ControlStats, INTERACTIVE_CONTROL_HOOK_ID,
};
pub use host::{HostIntegration, HostIntegrationError, HostIntegrationPlan, HostRestoreToken};
pub use observer::{SessionObserver, session_observer_config};
pub use replay::{
    BoxReplayFuture, ReplayCredentialPolicy, ReplayError, ReplayExecutionError, ReplayExecutor,
    ReplayLimits, ReplayRequest, ReplayResponse, ReplayRisk, ValidatedReplayRequest,
    execute_replay,
};
pub use service::{ApplicationSessionService, ServiceConfig, ServiceError, ServiceStatus};
pub use transmog_control_transport::{ControllerMessage, PendingDecision};

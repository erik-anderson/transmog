//! Headless application/session composition for `rustymiddle`.
//!
//! The crate owns bounded live state and application lifecycle. It deliberately
//! contains no UI types and performs no ambient operating-system mutation.

mod capture;
mod catalog;
mod observer;

pub use capture::{
    CaptureFailure, CaptureManager, CaptureServiceError, CaptureStart, CaptureStatus, SealedCapture,
};
pub use catalog::{
    BodySnapshot, CatalogApply, CatalogCounters, CatalogCursor, CatalogDelta, CatalogError,
    CatalogPage, CatalogQuery, CatalogSubscription, ObservedRequestHead, ObservedResponseHead,
    SessionCatalog, SessionFilter, SessionLimits, SessionSnapshot, SessionTerminal,
    SubscriptionEvent,
};
pub use observer::{SessionObserver, session_observer_config};

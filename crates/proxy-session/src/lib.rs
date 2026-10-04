//! Headless application/session composition for `rustymiddle`.
//!
//! The crate owns bounded live state and application lifecycle. It deliberately
//! contains no UI types and performs no ambient operating-system mutation.

mod catalog;

pub use catalog::{
    BodySnapshot, CatalogApply, CatalogCounters, CatalogCursor, CatalogDelta, CatalogError,
    CatalogPage, CatalogQuery, CatalogSubscription, ObservedRequestHead, ObservedResponseHead,
    SessionCatalog, SessionFilter, SessionLimits, SessionSnapshot, SessionTerminal,
    SubscriptionEvent,
};

//! quiche-based HTTP/3 origin egress and private Alt-Svc state.

mod alt_svc;
mod client;
mod config;
mod service;

pub use alt_svc::{AltSvcCache, AltSvcEntry, AltSvcError, Origin};
pub use client::{
    H3OriginClient, H3OriginError, H3OriginResponse, H3StreamingResponse, H3Telemetry,
};
pub use config::{H3ConfigError, H3TransportLimits, build_quiche_config};
pub use service::H3UpstreamService;

/// Confirms at compile time that this adapter is built against quiche.
pub const QUICHE_HTTP3_ALPN: &[&[u8]] = quiche::h3::APPLICATION_PROTOCOL;

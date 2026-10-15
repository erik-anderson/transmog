//! Hyper-based downstream and upstream HTTP/1.1 and HTTP/2 adapters.

mod alps;
mod authority;
mod client;
mod header_wire;
mod metrics;
mod service;
mod tls_connector;
mod tls_stream;
mod upstream;

pub use authority::{AuthorityError, ConnectAuthority};
pub use client::{
    HyperOriginClient, HyperOriginError, HyperUpgradeResponse, WebSocketIo, WebSocketUpgrade,
};
pub use header_wire::header_block_from_hyper;
pub use service::HyperUpstreamService;
pub use tls_connector::ChromiumHttpsConnector;
pub use tls_stream::MaybeHttpsStream;
pub use upstream::{
    HappyEyeballsResolver, HttpsOriginConnector, HyperEgressMode, build_https_connector,
    build_https_connector_with_happy_eyeballs,
};

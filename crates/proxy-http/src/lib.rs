//! Hyper-based downstream and upstream HTTP/1.1 and HTTP/2 adapters.

mod alps;
mod authority;
mod client;
mod metrics;
mod service;
mod tls_connector;
mod upstream;

pub use authority::{AuthorityError, ConnectAuthority};
pub use client::{
    HyperOriginClient, HyperOriginError, HyperUpgradeResponse, WebSocketIo, WebSocketUpgrade,
};
pub use service::HyperUpstreamService;
pub use tls_connector::ChromiumHttpsConnector;
pub use upstream::{
    HappyEyeballsResolver, HttpsOriginConnector, HyperEgressMode, build_https_connector,
    build_https_connector_with_happy_eyeballs,
};

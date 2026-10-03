//! Hyper-based downstream and upstream HTTP/1.1 and HTTP/2 adapters.

mod authority;
mod client;
mod upstream;

pub use authority::{AuthorityError, ConnectAuthority};
pub use client::{HyperOriginClient, HyperOriginError};
pub use upstream::{HyperEgressMode, build_https_connector};

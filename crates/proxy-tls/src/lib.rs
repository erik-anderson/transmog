//! The one BoringSSL-backed trust and certificate implementation used by every protocol.

mod ca;
mod downstream;
mod trust;

pub use ca::{
    EndpointIdentity, IdentityError, IssuedLeaf, LeafCache, LeafCacheError, ProxyCa, ProxyCaError,
    normalize_connect_identity,
};
pub use downstream::{DownstreamTlsContextFactory, DownstreamTlsError, DownstreamTlsPolicy};
pub use trust::{
    LoadedTrust, SystemTrustSource, TrustError, TrustSnapshot, TrustSource,
    UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

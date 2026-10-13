//! The one BoringSSL-backed trust and certificate implementation used by every protocol.

mod algorithms;
mod blocking_job;
mod ca;
mod certificate_compression;
/// Request-driven client identity selection and asynchronous signing.
pub mod client_authentication;
/// Connection and pool authorization for request-driven client certificates.
pub mod client_connections;
/// Bounded nonmodal certificate decisions owned by physical handshakes.
pub mod client_prompts;
mod downstream;
mod evidence;
mod resolver;
mod trust;
/// Current-user Windows software CNG identity inventory and change monitoring.
#[cfg(windows)]
pub mod windows_client_certificates;

pub use ca::{
    EndpointIdentity, IdentityError, IssuedLeaf, LeafCache, LeafCacheError, LeafKeyAlgorithm,
    MlDsaVariant, ProxyCa, ProxyCaError, normalize_connect_identity,
};
pub use downstream::{DownstreamTlsContextFactory, DownstreamTlsError, DownstreamTlsPolicy};
pub use evidence::TlsCapture;
pub use resolver::{
    CachedMitmCertificateResolver, CertificateResolverError, DownstreamCertificateResolver,
};
pub use trust::{
    CompositeTrustSource, LoadedTrust, PemTrustSource, SystemTrustSource, TrustError,
    TrustSnapshot, TrustSource, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

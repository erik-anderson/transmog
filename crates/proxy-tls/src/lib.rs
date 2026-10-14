//! The one BoringSSL-backed trust and certificate implementation used by every protocol.

mod algorithms;
mod blocking_job;
mod ca;
mod certificate_compression;
/// Request-driven client identity selection and asynchronous signing.
pub mod client_authentication;
/// Connection and pool authorization for request-driven client certificates.
pub mod client_connections;
mod client_hello;
/// Bounded nonmodal certificate decisions owned by physical handshakes.
pub mod client_prompts;
mod downstream;
/// HTTPS service-binding discovery for encrypted ClientHello.
pub mod ech;
mod evidence;
/// Monitored local certificate roles and pool admission identity.
pub mod local_trust;
mod resolver;
/// Bounded TLS tickets shared by the TCP and QUIC adapters.
pub mod session_cache;
mod trust;
/// Public Chromium PKI metadata and its selectable public root store.
pub mod trust_anchors;
/// Shared Chromium CT and CRLSet component inputs.
pub mod verification_data;
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
pub use transmog_tls_native::configure_quic_server_name;
pub use transmog_tls_native::decode_session_for_context;
pub use transmog_tls_native::restore_client_session;
pub use transmog_tls_native::{authenticated_ech_retry, configure_ech};
pub use transmog_tls_native::{configure_h2_application_settings, peer_application_settings};
pub use trust::{
    CompositeTrustSource, LoadedTrust, PemTrustSource, SystemTrustSource, TrustError,
    TrustSnapshot, TrustSource, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

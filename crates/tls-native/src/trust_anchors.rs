//! Trust Anchor ID negotiation and shared Chromium PKI verification hooks.
use crate::verifier::{CompiledPolicy, Verification};
use boring::{
    error::ErrorStack,
    ex_data::Index,
    ssl::{
        NameType, Ssl, SslAlert, SslContext, SslContextBuilder, SslRef, SslVerifyError,
        SslVerifyMode,
    },
    x509::{X509, X509Ref},
};
use foreign_types::ForeignTypeRef;
use std::{
    collections::HashSet,
    sync::{Arc, OnceLock},
};
type Issuers = Arc<Vec<X509>>;
/// Frozen ordinary verification inputs. Helpers never confer certificate trust.
#[derive(Clone, Debug, Default)]
pub struct CertificatePolicy {
    /// A bootstrap context has no current local inventory and must reject TLS.
    pub unavailable: bool,
    /// Untrusted issuer candidates.
    pub issuers: Issuers,
    /// Normal anchor DER; a leaf-only entry cannot authorize issuer paths.
    pub anchors: Arc<HashSet<Vec<u8>>>,
    /// Public Chromium anchors confer issuer trust, never exact leaf trust.
    pub anchor_only: Arc<HashSet<Vec<u8>>>,
    /// Anchors whose encoded X.509 constraints must be enforced.
    pub constrained_anchors: Arc<HashSet<Vec<u8>>>,
    /// Public anchors whose validity period is not a trust constraint.
    pub ignore_anchor_expiry: Arc<HashSet<Vec<u8>>>,
    /// Eligible Windows ROOT certificates for path provenance.
    pub local_anchors: Arc<HashSet<Vec<u8>>>,
    /// Eligible self-signed leaves; never usable to authorize issued certificates.
    pub trusted_leaves: Issuers,
    /// Exact certificate distrust, regardless of certificate role or TLS usage.
    pub disallowed: Arc<HashSet<Vec<u8>>>,
}

struct BoundPolicy {
    input: Arc<CertificatePolicy>,
    compiled: Arc<CompiledPolicy>,
}
type Policy = Arc<BoundPolicy>;
type Observer = Arc<dyn Fn(&SslRef, &Verification) + Send + Sync>;
static SSL_INDEX: OnceLock<Result<Index<Ssl, Policy>, ErrorStack>> = OnceLock::new();
static CTX_INDEX: OnceLock<Result<Index<SslContext, Policy>, ErrorStack>> = OnceLock::new();
static SSL_NAME: OnceLock<Result<Index<Ssl, String>, ErrorStack>> = OnceLock::new();
static CTX_NAME: OnceLock<Result<Index<SslContext, String>, ErrorStack>> = OnceLock::new();
static SSL_OBSERVER: OnceLock<Result<Index<Ssl, Observer>, ErrorStack>> = OnceLock::new();
static CTX_OBSERVER: OnceLock<Result<Index<SslContext, Observer>, ErrorStack>> = OnceLock::new();

/// Installs the single PKI verifier on an upstream context.
/// # Errors
/// Returns native allocation or callback-data failures.
pub fn configure_context_certificate_policy(
    context: &mut SslContextBuilder,
    policy: Arc<CertificatePolicy>,
) -> Result<(), ErrorStack> {
    let compiled = crate::verifier::compile(&policy)?;
    let index = CTX_INDEX
        .get_or_init(SslContext::new_ex_index::<Policy>)
        .clone()?;
    context.set_ex_data(
        index,
        Arc::new(BoundPolicy {
            input: policy,
            compiled,
        }),
    );
    context.set_custom_verify_callback(SslVerifyMode::PEER, verify_peer);
    Ok(())
}
/// Overrides frozen PKI inputs on a fresh physical TCP handshake.
/// # Errors
/// Returns native allocation or callback-data failures.
pub fn configure_ssl_certificate_policy(
    ssl: &mut SslRef,
    policy: Arc<CertificatePolicy>,
) -> Result<(), ErrorStack> {
    let compiled = crate::verifier::compile(&policy)?;
    let index = SSL_INDEX.get_or_init(Ssl::new_ex_index::<Policy>).clone()?;
    ssl.set_ex_data(
        index,
        Arc::new(BoundPolicy {
            input: policy,
            compiled,
        }),
    );
    ssl.set_custom_verify_callback(SslVerifyMode::PEER, verify_peer);
    Ok(())
}
/// Supplies the expected canonical hostname independently of SNI. Required for
/// TCP IP literals, which do not send SNI; DNS connections may use SNI fallback.
/// # Errors
/// Returns a native callback-data allocation failure.
pub fn configure_ssl_peer_name(ssl: &mut SslRef, name: String) -> Result<(), ErrorStack> {
    ssl.set_ex_data(
        SSL_NAME.get_or_init(Ssl::new_ex_index::<String>).clone()?,
        name,
    );
    Ok(())
}
/// Supplies the expected canonical hostname to a connection-specific QUIC context.
/// # Errors
/// Returns a native callback-data allocation failure.
pub fn configure_context_peer_name(
    context: &mut SslContextBuilder,
    name: String,
) -> Result<(), ErrorStack> {
    context.set_ex_data(
        CTX_NAME
            .get_or_init(SslContext::new_ex_index::<String>)
            .clone()?,
        name,
    );
    Ok(())
}
/// Observes the actual PKI path/result without participating in authorization.
/// # Errors
/// Returns a native callback-data allocation failure.
pub fn observe_ssl_verification(
    ssl: &mut SslRef,
    observer: impl Fn(&SslRef, &Verification) + Send + Sync + 'static,
) -> Result<(), ErrorStack> {
    ssl.set_ex_data(
        SSL_OBSERVER
            .get_or_init(Ssl::new_ex_index::<Observer>)
            .clone()?,
        Arc::new(observer),
    );
    Ok(())
}
/// Observes the actual PKI path/result on a connection-specific context.
/// # Errors
/// Returns a native callback-data allocation failure.
pub fn observe_context_verification(
    context: &mut SslContextBuilder,
    observer: impl Fn(&SslRef, &Verification) + Send + Sync + 'static,
) -> Result<(), ErrorStack> {
    context.set_ex_data(
        CTX_OBSERVER
            .get_or_init(SslContext::new_ex_index::<Observer>)
            .clone()?,
        Arc::new(observer),
    );
    Ok(())
}

fn verify_peer(ssl: &mut SslRef) -> Result<(), SslVerifyError> {
    // Never allow Rust panics to cross the TLS callback boundary.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = verify_peer_inner(ssl);
        let observer = SSL_OBSERVER
            .get()
            .and_then(|i| i.as_ref().ok())
            .and_then(|i| ssl.ex_data(*i))
            .or_else(|| {
                CTX_OBSERVER
                    .get()
                    .and_then(|i| i.as_ref().ok())
                    .and_then(|i| ssl.ssl_context().ex_data(*i))
            });
        if let Some(observer) = observer {
            observer(ssl, &result);
        }
        if result.accepted {
            Ok(())
        } else {
            Err(SslVerifyError::Invalid(match result.error_code {
                9 | 10 => SslAlert::CERTIFICATE_EXPIRED,
                20 => SslAlert::UNKNOWN_CA,
                23 => SslAlert::CERTIFICATE_REVOKED,
                _ => SslAlert::BAD_CERTIFICATE,
            }))
        }
    }))
    .unwrap_or(Err(SslVerifyError::Invalid(SslAlert::INTERNAL_ERROR)))
}
fn verify_peer_inner(ssl: &SslRef) -> Verification {
    let policy = SSL_INDEX
        .get()
        .and_then(|i| i.as_ref().ok())
        .and_then(|i| ssl.ex_data(*i))
        .or_else(|| {
            CTX_INDEX
                .get()
                .and_then(|i| i.as_ref().ok())
                .and_then(|i| ssl.ssl_context().ex_data(*i))
        });
    let Some(policy) = policy else {
        return Verification::failure("No PKI verifier inputs configured");
    };
    let certificates = crate::verifier::peer_certificate_chain(ssl);
    let mut result = crate::verifier::verify(&policy.compiled, &policy.input, &certificates, false);
    let name = SSL_NAME
        .get()
        .and_then(|i| i.as_ref().ok())
        .and_then(|i| ssl.ex_data(*i))
        .or_else(|| {
            CTX_NAME
                .get()
                .and_then(|i| i.as_ref().ok())
                .and_then(|i| ssl.ssl_context().ex_data(*i))
        })
        .map(String::as_str)
        .or_else(|| ssl.servername(NameType::HOST_NAME));
    if result
        .names
        .as_ref()
        .is_some_and(|names| !name.is_some_and(|name| crate::hostname::matches(names, name)))
    {
        if result.accepted {
            result.error_code = 62;
            result.depth = Some(0);
            result.diagnostic = "Certificate SAN does not match the expected server name".into();
        }
        result.accepted = false;
        result.trust_role = None;
    }
    result
}

/// Whether DER declares certificate-authority capability. This is an import
/// classification helper, never a substitute for PKI verification.
pub fn certificate_is_ca(certificate: &X509Ref) -> bool {
    // SAFETY: read-only classification of a borrowed public certificate.
    unsafe { boring_sys::X509_check_ca(certificate.as_ptr()) != 0 }
}
/// Publishes server recovery IDs in decreasing preference order.
/// # Errors
/// Returns native validation/configuration errors for the wire list.
pub fn configure_available_trust_anchors(
    context: &mut SslContextBuilder,
    ids: &[u8],
) -> Result<(), ErrorStack> {
    // SAFETY: live, exclusively borrowed context; set1 copies the supplied list.
    if unsafe {
        boring_sys::SSL_CTX_set1_available_trust_anchors(context.as_ptr(), ids.as_ptr(), ids.len())
    } == 1
    {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

/// Offers certificate-based IDs. PKI helpers and trust are configured separately.
/// # Errors
/// Returns native allocation/configuration errors, including malformed ID lists.
pub fn configure_context_trust_anchors(
    context: &mut SslContextBuilder,
    ids: &[u8],
) -> Result<(), ErrorStack> {
    // SAFETY: context is exclusively borrowed. The setter copies the wire list;
    // callback data is owned by typed ex-data, not a borrowed user pointer.
    let accepted = unsafe {
        boring_sys::SSL_CTX_set1_requested_trust_anchors(context.as_ptr(), ids.as_ptr(), ids.len())
    };
    if accepted == 1 {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

/// Freezes a fresh metadata snapshot on a TCP SSL object, overriding its pool's
/// older context without changing established connections or session trust.
/// # Errors
/// Returns native configuration/allocation failures.
pub fn configure_ssl_trust_anchors(ssl: &mut SslRef, ids: &[u8]) -> Result<(), ErrorStack> {
    // SAFETY: exclusively borrowed SSL; the set1 function copies the slice.
    if unsafe {
        boring_sys::SSL_set1_requested_trust_anchors(ssl.as_ptr(), ids.as_ptr(), ids.len())
    } == 1
    {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

/// Copies handshake-only server facts before `BoringSSL` releases them. A matched
/// marker is negotiation evidence and never an authorization to trust a chain.
pub fn peer_trust_anchors(ssl: &SslRef) -> (bool, Vec<u8>) {
    // SAFETY: synchronous read of live SSL-owned handshake storage; nothing
    // borrowed escapes and the protocol caps the vector at 65535 bytes.
    unsafe {
        let matched = boring_sys::SSL_peer_matched_trust_anchor(ssl.as_ptr()) != 0;
        let mut data = std::ptr::null();
        let mut length = 0;
        boring_sys::SSL_get0_peer_available_trust_anchors(
            ssl.as_ptr(),
            &raw mut data,
            &raw mut length,
        );
        let bytes = if length == 0 || length > 65_535 || data.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data, length).to_vec()
        };
        (matched, bytes)
    }
}

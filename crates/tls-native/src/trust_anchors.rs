//! Trust Anchor ID negotiation and ordinary verification with untrusted helpers.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    ssl::{Ssl, SslContext, SslContextBuilder, SslRef},
    stack::Stack,
    x509::{X509, X509Ref, X509StoreContext, X509StoreContextRef},
};
use foreign_types::{ForeignType, ForeignTypeRef};
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
    /// Eligible Windows ROOT certificates for path provenance.
    pub local_anchors: Arc<HashSet<Vec<u8>>>,
    /// Eligible self-signed leaves; never usable to authorize issued certificates.
    pub trusted_leaves: Issuers,
    /// Exact certificate distrust, regardless of certificate role or TLS usage.
    pub disallowed: Arc<HashSet<Vec<u8>>>,
}
type Policy = Arc<CertificatePolicy>;
static SSL_INDEX: OnceLock<Result<Index<Ssl, Policy>, ErrorStack>> = OnceLock::new();
static CTX_INDEX: OnceLock<Result<Index<SslContext, Policy>, ErrorStack>> = OnceLock::new();
fn ssl_index() -> Result<Index<Ssl, Policy>, ErrorStack> {
    SSL_INDEX.get_or_init(Ssl::new_ex_index::<Policy>).clone()
}
fn ctx_index() -> Result<Index<SslContext, Policy>, ErrorStack> {
    CTX_INDEX
        .get_or_init(SslContext::new_ex_index::<Policy>)
        .clone()
}
fn policy(context: &X509StoreContextRef) -> Option<Policy> {
    X509StoreContext::ssl_idx()
        .ok()
        .and_then(|i| context.ex_data(i))
        .and_then(|ssl| {
            ssl_index()
                .ok()
                .and_then(|i| ssl.ex_data(i))
                .or_else(|| ctx_index().ok().and_then(|i| ssl.ssl_context().ex_data(i)))
                .cloned()
        })
}

/// Installs ordinary chain helpers and local certificate policy independently of IDs.
/// # Errors
/// Returns native ex-data allocation failures.
pub fn configure_context_certificate_policy(
    context: &mut SslContextBuilder,
    policy: Policy,
) -> Result<(), ErrorStack> {
    context.set_ex_data(ctx_index()?, policy);
    // SAFETY: exclusively borrowed context; callback reads owned typed ex-data.
    unsafe {
        boring_sys::SSL_CTX_set_cert_verify_callback(
            context.as_ptr(),
            Some(verify),
            std::ptr::null_mut(),
        );
    }
    Ok(())
}
/// Overrides the pool's policy for one physical TCP handshake.
/// # Errors
/// Returns native ex-data allocation failures.
pub fn configure_ssl_certificate_policy(
    ssl: &mut SslRef,
    policy: Policy,
) -> Result<(), ErrorStack> {
    ssl.set_ex_data(ssl_index()?, policy);
    Ok(())
}
/// Checks the current path certificate against exact local distrust.
pub fn certificate_is_allowed(context: &X509StoreContextRef) -> bool {
    let Some(policy) = policy(context) else {
        return true;
    };
    if policy.unavailable {
        return false;
    }
    let Some(cert) = context.current_cert() else {
        return policy.disallowed.is_empty() && policy.trusted_leaves.is_empty();
    };
    let Ok(der) = cert.to_der() else {
        return false;
    };
    if policy.disallowed.contains(&der) {
        return false;
    }
    let leaf_only = policy
        .trusted_leaves
        .iter()
        .any(|leaf| leaf.to_der().is_ok_and(|bytes| bytes == der));
    let anchor = policy.anchors.contains(&der);
    if leaf_only && context.error_depth() > 0 && !anchor {
        return false;
    }
    if leaf_only || anchor {
        // PARTIAL_CHAIN considers stored certificates trusted and skips some
        // extension checks. Explicit local trust still requires appropriate
        // server usage and no unsupported critical extensions at every depth.
        // SAFETY: read-only purpose/extension checks on the live path certificate.
        unsafe {
            if boring_sys::X509_check_purpose(
                cert.as_ptr(),
                boring_sys::X509_PURPOSE_SSL_SERVER,
                i32::from(context.error_depth() > 0),
            ) != 1
                || boring_sys::X509_get_extension_flags(cert.as_ptr())
                    & u32::from_ne_bytes(boring_sys::EXFLAG_CRITICAL.to_ne_bytes())
                    != 0
            {
                return false;
            }
        }
    }
    true
}
/// Role of the terminal certificate on the verified path, when local policy exists.
pub fn verified_trust_role(context: &X509StoreContextRef) -> Option<&'static str> {
    let policy = policy(context)?;
    let chain = context.chain()?;
    let cert = chain.iter().next_back()?;
    let der = cert.to_der().ok()?;
    if chain.len() == 1
        && policy
            .trusted_leaves
            .iter()
            .any(|leaf| leaf.to_der().is_ok_and(|bytes| bytes == der))
    {
        Some("Exact locally trusted server certificate")
    } else if policy.local_anchors.contains(&der) {
        Some("Anchor present in Windows ROOT")
    } else if policy.anchors.contains(&der) {
        Some("Chromium public anchor")
    } else {
        None
    }
}

/// Whether public DER declares a certificate-authority capability.
pub fn certificate_is_ca(certificate: &X509Ref) -> bool {
    // SAFETY: read-only CA classification on a live, borrowed certificate.
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

/// Offers certificate-based IDs and supplies public CA certificates as untrusted chain
/// helpers. The configured `X509_STORE`, hostname, purpose and time checks apply.
/// # Errors
/// Returns native allocation/configuration errors, including malformed ID lists.
pub fn configure_context_trust_anchors(
    context: &mut SslContextBuilder,
    ids: &[u8],
    issuers: Issuers,
) -> Result<(), ErrorStack> {
    configure_context_certificate_policy(
        context,
        Arc::new(CertificatePolicy {
            issuers,
            ..Default::default()
        }),
    )?;
    // SAFETY: context is exclusively borrowed. The setter copies the wire list;
    // callback data is owned by typed ex-data, not a borrowed user pointer.
    let accepted = unsafe {
        boring_sys::SSL_CTX_set_cert_verify_callback(
            context.as_ptr(),
            Some(verify),
            std::ptr::null_mut(),
        );
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
pub fn configure_ssl_trust_anchors(
    ssl: &mut SslRef,
    ids: &[u8],
    issuers: Issuers,
) -> Result<(), ErrorStack> {
    configure_ssl_certificate_policy(
        ssl,
        Arc::new(CertificatePolicy {
            issuers,
            ..Default::default()
        }),
    )?;
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

unsafe extern "C" fn verify(
    pointer: *mut boring_sys::X509_STORE_CTX,
    _: *mut std::ffi::c_void,
) -> i32 {
    std::panic::catch_unwind(|| {
        // SAFETY: BoringSSL supplies an initialized, exclusively driven context.
        let context = unsafe { X509StoreContextRef::from_ptr_mut(pointer) };
        let Some(policy) = policy(context) else {
            // SAFETY: the original initialized context and untrusted stack stay live.
            return unsafe { boring_sys::X509_verify_cert(pointer) };
        };
        // SAFETY: this verification is synchronous on its initialized context.
        // The guard forwards the SSL owner's callback after enforcing local roles.
        unsafe {
            boring_sys::X509_STORE_CTX_set_verify_cb(pointer, Some(guarded_verify));
        }
        let Ok(mut chain) = Stack::new() else {
            return 0;
        };
        if let Some(original) = context.untrusted() {
            for certificate in original {
                if certificate
                    .to_der()
                    .is_ok_and(|der| policy.disallowed.contains(&der))
                {
                    continue;
                }
                if chain.push(certificate.to_owned()).is_err() {
                    return 0;
                }
            }
        }
        for certificate in policy.issuers.iter() {
            if chain.push(certificate.clone()).is_err() {
                return 0;
            }
        }
        // SAFETY: the temporary stack owns all references and remains live for
        // the entire synchronous verification. Restore the original borrowed
        // pointer before dropping the temporary stack; the verified path owns
        // its independent references. No helper enters the trusted store.
        unsafe {
            let original = boring_sys::X509_STORE_CTX_get0_untrusted(pointer);
            boring_sys::X509_STORE_CTX_set_chain(pointer, chain.as_ptr());
            let accepted = boring_sys::X509_verify_cert(pointer);
            boring_sys::X509_STORE_CTX_set_chain(pointer, original);
            accepted
        }
    })
    .unwrap_or(0)
}

unsafe extern "C" fn guarded_verify(
    accepted: i32,
    pointer: *mut boring_sys::X509_STORE_CTX,
) -> i32 {
    std::panic::catch_unwind(|| {
        // SAFETY: BoringSSL supplies its initialized current path context.
        let context = unsafe { X509StoreContextRef::from_ptr_mut(pointer) };
        let allowed = certificate_is_allowed(context);
        if !allowed {
            context.set_error(Err(boring::x509::X509VerifyError::CERT_REJECTED));
        }
        let accepted = if allowed { accepted } else { 0 };
        let callback = X509StoreContext::ssl_idx()
            .ok()
            .and_then(|i| context.ex_data(i))
            .and_then(|ssl| {
                // SAFETY: read-only SSL callback lookup; the SSL owns this context.
                unsafe { boring_sys::SSL_get_verify_callback(ssl.as_ptr()) }
            });
        // SAFETY: original SSL callback receives the same live context. No borrowed
        // pointer escapes. A caller cannot override explicit local distrust.
        let result = callback.map_or(accepted, |callback| unsafe { callback(accepted, pointer) });
        if allowed { result } else { 0 }
    })
    .unwrap_or(0)
}

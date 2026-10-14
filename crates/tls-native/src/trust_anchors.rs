//! Trust Anchor ID negotiation and ordinary verification with untrusted helpers.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    ssl::{Ssl, SslContext, SslContextBuilder, SslRef},
    stack::Stack,
    x509::{X509, X509Ref, X509StoreContext, X509StoreContextRef},
};
use foreign_types::{ForeignType, ForeignTypeRef};
use std::sync::{Arc, OnceLock};

type Issuers = Arc<Vec<X509>>;
static SSL_INDEX: OnceLock<Result<Index<Ssl, Issuers>, ErrorStack>> = OnceLock::new();
static CTX_INDEX: OnceLock<Result<Index<SslContext, Issuers>, ErrorStack>> = OnceLock::new();
fn ssl_index() -> Result<Index<Ssl, Issuers>, ErrorStack> {
    SSL_INDEX.get_or_init(Ssl::new_ex_index::<Issuers>).clone()
}
fn ctx_index() -> Result<Index<SslContext, Issuers>, ErrorStack> {
    CTX_INDEX
        .get_or_init(SslContext::new_ex_index::<Issuers>)
        .clone()
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

/// Offers classical IDs and supplies public CA certificates as untrusted chain
/// helpers. The configured `X509_STORE`, hostname, purpose and time checks apply.
/// # Errors
/// Returns native allocation/configuration errors, including malformed ID lists.
pub fn configure_context_trust_anchors(
    context: &mut SslContextBuilder,
    ids: &[u8],
    issuers: Issuers,
) -> Result<(), ErrorStack> {
    context.set_ex_data(ctx_index()?, issuers);
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
    ssl.set_ex_data(ssl_index()?, issuers);
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
        let helpers = X509StoreContext::ssl_idx()
            .ok()
            .and_then(|i| context.ex_data(i))
            .and_then(|ssl| {
                ssl_index()
                    .ok()
                    .and_then(|i| ssl.ex_data(i))
                    .or_else(|| ctx_index().ok().and_then(|i| ssl.ssl_context().ex_data(i)))
                    .cloned()
            });
        let Some(helpers) = helpers else {
            // SAFETY: the original initialized context and untrusted stack stay live.
            return unsafe { boring_sys::X509_verify_cert(pointer) };
        };
        let Ok(mut chain) = Stack::new() else {
            return 0;
        };
        if let Some(original) = context.untrusted() {
            for certificate in original {
                if chain.push(certificate.to_owned()).is_err() {
                    return 0;
                }
            }
        }
        for certificate in helpers.iter() {
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

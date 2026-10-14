//! Safe native boundary for `BoringSSL` certificate hooks and `ClientHello` configuration.
#![deny(unsafe_op_in_unsafe_fn)]

mod application_settings;
mod credentials;
mod hostname;
mod trust_anchors;
mod verifier;
pub use application_settings::{configure_h2_application_settings, peer_application_settings};
pub use credentials::{
    MlDsaVariant, add_server_credential, add_server_credential_for_trust_anchor,
    selected_credential_chain,
};
pub use trust_anchors::{
    CertificatePolicy, certificate_is_ca, configure_available_trust_anchors,
    configure_context_certificate_policy, configure_context_peer_name,
    configure_context_trust_anchors, configure_ssl_certificate_policy, configure_ssl_peer_name,
    configure_ssl_trust_anchors, observe_context_verification, observe_ssl_verification,
    peer_trust_anchors,
};
pub use verifier::{
    Verification, certificate_is_parseable, peer_certificate_chain, verify_certificate_authority,
};

use boring::{
    error::ErrorStack,
    ex_data::Index,
    ssl::{
        PrivateKeyMethod, PrivateKeyMethodError, Ssl, SslContext, SslContextBuilder, SslRef,
        SslVerifyMode,
    },
    stack::StackRef,
    x509::{X509Name, X509Ref},
};
use foreign_types::ForeignTypeRef;
use std::{
    ffi::c_void,
    panic::AssertUnwindSafe,
    sync::{Arc, OnceLock},
};

/// Configures a reserved GREASE entry in outgoing `ClientHello` signature algorithms.
///
/// This is independent of ordinary GREASE and does not change the real signature
/// preferences or add GREASE to server `CertificateRequest` messages.
pub fn set_signature_algorithm_grease(context: &mut SslContextBuilder, enabled: bool) {
    // SAFETY: the exclusively borrowed context is live. The setter stores one
    // boolean synchronously and neither retains a borrowed pointer nor fails.
    unsafe { boring_sys::SSL_CTX_set_grease_sigalgs_enabled(context.as_ptr(), i32::from(enabled)) };
}

/// Owned constraints from the current server's actual certificate request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CertificateRequest {
    /// Negotiated protocol version. An unknown version cannot authorize an identity.
    pub tls_version: Option<boring::ssl::SslVersion>,
    /// Constraints could not be represented within supported bounds. Never select
    /// a certificate from incomplete constraints; continuing without one is safe.
    pub constraints_omitted: bool,
    /// DER distinguished names of acceptable issuers. Empty means unrestricted.
    pub issuers: Vec<Vec<u8>>,
    /// TLS `SignatureScheme` values the peer can verify.
    pub signature_algorithms: Vec<u16>,
    /// Historical TLS 1.2 certificate types; TLS 1.3 leaves this empty.
    pub certificate_types: Vec<u8>,
}

/// The result of one synchronous invocation of a resumable selection hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionResult {
    /// The callback installed a credential or deliberately cleared credentials.
    Ready,
    /// Suspend this SSL object with `WANT_X509_LOOKUP` until the owner retries.
    Pending,
    /// Fail the handshake, for cancellation or an internal failure.
    Failed,
}

type Callback = Arc<dyn Fn(&mut SslRef, &CertificateRequest) -> SelectionResult + Send + Sync>;
static SSL_INDEX: OnceLock<Result<Index<Ssl, Callback>, ErrorStack>> = OnceLock::new();
static CTX_INDEX: OnceLock<Result<Index<SslContext, Callback>, ErrorStack>> = OnceLock::new();

fn ssl_index() -> Result<Index<Ssl, Callback>, ErrorStack> {
    SSL_INDEX.get_or_init(Ssl::new_ex_index::<Callback>).clone()
}
fn context_index() -> Result<Index<SslContext, Callback>, ErrorStack> {
    CTX_INDEX
        .get_or_init(SslContext::new_ex_index::<Callback>)
        .clone()
}

/// Registers a connection-specific hook. Called only after `CertificateRequest`.
/// The closure is owned by SSL ex-data and destroyed with the SSL object.
/// # Errors
/// Returns a native ex-data allocation error.
pub fn set_client_certificate_callback(
    ssl: &mut SslRef,
    callback: impl Fn(&mut SslRef, &CertificateRequest) -> SelectionResult + Send + Sync + 'static,
) -> Result<(), ErrorStack> {
    ssl.set_ex_data(ssl_index()?, Arc::new(callback));
    // SAFETY: ssl is borrowed exclusively for registration. No raw user-data
    // pointer escapes; the trampoline reads typed, SSL-owned ex-data.
    unsafe { boring_sys::SSL_set_cert_cb(ssl.as_ptr(), Some(trampoline), std::ptr::null_mut()) };
    Ok(())
}

/// Registers a hook on a connection-specific QUIC context.
/// Do not share a context carrying per-connection approval between destinations.
/// # Errors
/// Returns a native ex-data allocation error.
pub fn set_context_client_certificate_callback(
    context: &mut SslContextBuilder,
    callback: impl Fn(&mut SslRef, &CertificateRequest) -> SelectionResult + Send + Sync + 'static,
) -> Result<(), ErrorStack> {
    context.set_ex_data(context_index()?, Arc::new(callback));
    // SAFETY: the builder is exclusively borrowed. Typed ex-data owns the closure
    // through every SSL reference to this context; no borrowed argument escapes.
    unsafe {
        boring_sys::SSL_CTX_set_cert_cb(context.as_ptr(), Some(trampoline), std::ptr::null_mut());
    };
    Ok(())
}

unsafe extern "C" fn trampoline(pointer: *mut boring_sys::SSL, _: *mut c_void) -> i32 {
    // Never unwind into C, including on a poisoned owner or embedding callback.
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: BoringSSL calls this with its live, exclusively driven SSL.
        // The reference and all constraint pointers are confined to this call.
        let ssl = unsafe { SslRef::from_ptr_mut(pointer) };
        if ssl.is_server()
            || !ssl.verify_mode().contains(SslVerifyMode::PEER)
            || ssl.verify_result().is_err()
            || ssl.peer_certificate().is_none()
        {
            return 0;
        }
        let callback = ssl_index()
            .ok()
            .and_then(|index| ssl.ex_data(index))
            .cloned()
            .or_else(|| {
                context_index()
                    .ok()
                    .and_then(|index| ssl.ssl_context().ex_data(index))
                    .cloned()
            });
        let Some(callback) = callback else { return 0 };
        let mut algorithms = std::ptr::null();
        let mut types = std::ptr::null();
        // SAFETY: these getters are used only in cert_cb, as required by
        // BoringSSL. Their arrays remain SSL-owned until callback return.
        let (algorithm_count, type_count, issuers) = unsafe {
            (
                boring_sys::SSL_get0_peer_verify_algorithms(pointer, &raw mut algorithms),
                boring_sys::SSL_get0_certificate_types(pointer, &raw mut types),
                boring_sys::SSL_get_client_CA_list(pointer),
            )
        };
        let mut request = CertificateRequest {
            tls_version: ssl.version2(),
            constraints_omitted: algorithm_count > 256 || type_count > 256,
            ..CertificateRequest::default()
        };
        if algorithm_count != 0 && algorithm_count <= 256 {
            // SAFETY: native getter guarantees algorithm_count readable values.
            request.signature_algorithms =
                unsafe { std::slice::from_raw_parts(algorithms, algorithm_count) }.to_vec();
        }
        if type_count != 0 && type_count <= 256 {
            // SAFETY: native getter guarantees type_count readable values.
            request.certificate_types =
                unsafe { std::slice::from_raw_parts(types, type_count) }.to_vec();
        }
        if !issuers.is_null() {
            // SAFETY: the borrowed stack is owned by SSL for this callback and
            // contains X509_NAME entries. It is neither mutated nor freed.
            let issuers = unsafe { StackRef::<X509Name>::from_ptr(issuers) };
            if issuers.len() > 256 {
                request.constraints_omitted = true;
            } else {
                for name in issuers {
                    let Ok(der) = name.to_der() else {
                        request.constraints_omitted = true;
                        break;
                    };
                    if der.len() > 64 * 1024 {
                        request.constraints_omitted = true;
                        break;
                    }
                    request.issuers.push(der);
                }
            }
        }
        match callback(ssl, &request) {
            SelectionResult::Ready => 1,
            SelectionResult::Pending => -1,
            SelectionResult::Failed => 0,
        }
    }))
    .unwrap_or(0)
}

/// Removes all local credentials from this SSL connection, including chains.
pub fn clear_client_credentials(ssl: &mut SslRef) {
    // SAFETY: the exclusively borrowed SSL remains alive for the synchronous
    // call; BoringSSL releases only its own credential references.
    unsafe { boring_sys::SSL_certs_clear(ssl.as_ptr()) };
}

/// Clears a previously cached client session so the verified peer must ask again.
/// # Errors
/// Returns a native TLS configuration error.
pub fn clear_client_session(ssl: &mut SslRef) -> Result<(), ErrorStack> {
    // SAFETY: exclusive live SSL; null is the documented removal of its session.
    if unsafe { boring_sys::SSL_set_session(ssl.as_ptr(), std::ptr::null_mut()) } == 1 {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

/// Checks the leaf's EKU, key usage and CA constraints for TLS client signing.
pub fn permits_client_authentication(certificate: &X509Ref) -> bool {
    // SAFETY: borrowed certificate is live. BoringSSL may populate its internal
    // extension cache, which uses its own locking; no data escapes the call.
    unsafe {
        boring_sys::X509_check_purpose(certificate.as_ptr(), boring_sys::X509_PURPOSE_SSL_CLIENT, 0)
            == 1
            // The generic client-purpose check also permits key agreement,
            // but this bridge authenticates only with CertificateVerify signing.
            // The getter returns all bits set when Key Usage is absent.
            && (boring_sys::X509_get_key_usage(certificate.as_ptr())
                & boring_sys::X509v3_KU_DIGITAL_SIGNATURE.unsigned_abs()) != 0
    }
}

/// Identifies a CA signed by its own public key, rather than merely self-issued.
/// Uses public certificate data only; invalid or unsupported signatures return false.
pub fn is_self_signed_ca(certificate: &X509Ref) -> bool {
    // SAFETY: the borrowed certificate stays live. BoringSSL synchronizes its
    // internal extension cache, and no native pointers escape this call.
    let is_ca = unsafe { boring_sys::X509_check_ca(certificate.as_ptr()) != 0 };
    is_ca
        && certificate.issued(certificate).is_ok()
        && certificate
            .public_key()
            .is_ok_and(|key| certificate.verify(&key).unwrap_or(false))
}

/// Public TLS events relevant to client authentication; excludes all secrets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientAuthenticationEvent {
    /// Exact leaf-first public certificate message queued by TLS, possibly empty.
    CertificateQueued(Vec<Vec<u8>>),
    /// An incoming TLS alert's description byte, without record or key material.
    PeerAlert(u8),
}
type Observer = Arc<dyn Fn(ClientAuthenticationEvent) + Send + Sync>;
static OBSERVER_SSL_INDEX: OnceLock<Result<Index<Ssl, Observer>, ErrorStack>> = OnceLock::new();
static OBSERVER_CTX_INDEX: OnceLock<Result<Index<SslContext, Observer>, ErrorStack>> =
    OnceLock::new();
fn observer_ssl_index() -> Result<Index<Ssl, Observer>, ErrorStack> {
    OBSERVER_SSL_INDEX
        .get_or_init(Ssl::new_ex_index::<Observer>)
        .clone()
}
fn observer_context_index() -> Result<Index<SslContext, Observer>, ErrorStack> {
    OBSERVER_CTX_INDEX
        .get_or_init(SslContext::new_ex_index::<Observer>)
        .clone()
}

/// Observes only outgoing public certificate messages and incoming alert codes.
/// # Errors
/// Returns a native ex-data allocation error.
pub fn observe_client_authentication(
    ssl: &mut SslRef,
    observer: impl Fn(ClientAuthenticationEvent) + Send + Sync + 'static,
) -> Result<(), ErrorStack> {
    ssl.set_ex_data(observer_ssl_index()?, Arc::new(observer));
    // SAFETY: the SSL-owned ex-data keeps the observer alive. The callback has
    // no raw user-data pointer, and no native message buffer escapes the call.
    unsafe {
        boring_sys::SSL_set_msg_callback(ssl.as_ptr(), Some(message));
    }
    Ok(())
}
/// Observes public authentication events on a connection-specific QUIC context.
/// # Errors
/// Returns a native ex-data allocation error.
pub fn observe_context_client_authentication(
    context: &mut SslContextBuilder,
    observer: impl Fn(ClientAuthenticationEvent) + Send + Sync + 'static,
) -> Result<(), ErrorStack> {
    context.set_ex_data(observer_context_index()?, Arc::new(observer));
    // SAFETY: context-owned ex-data survives all SSL references to the context.
    unsafe {
        boring_sys::SSL_CTX_set_msg_callback(context.as_ptr(), Some(message));
    }
    Ok(())
}
unsafe extern "C" fn message(
    is_write: i32,
    version: i32,
    content_type: i32,
    buffer: *const c_void,
    length: usize,
    pointer: *mut boring_sys::SSL,
    _: *mut c_void,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if buffer.is_null()
            || length == 0
            || length > 2 * 1024 * 1024
            || !((content_type == 22 && is_write == 1) || (content_type == 21 && is_write == 0))
        {
            return;
        }
        // SAFETY: BoringSSL supplies this live SSL and a message buffer valid for
        // exactly length bytes during the synchronous callback. Neither escapes.
        let ssl = unsafe { SslRef::from_ptr(pointer) };
        if ssl.is_server() {
            return;
        }
        // SAFETY: native callback guarantees length initialized message bytes.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), length) };
        let event = if content_type == 21 && length == 2 {
            ClientAuthenticationEvent::PeerAlert(bytes[1])
        } else if content_type == 22 && bytes[0] == 11 {
            let Some(chain) = parse_certificate_message(bytes, version == 0x0304) else {
                return;
            };
            ClientAuthenticationEvent::CertificateQueued(chain)
        } else {
            return;
        };
        let observer = observer_ssl_index()
            .ok()
            .and_then(|index| ssl.ex_data(index))
            .cloned()
            .or_else(|| {
                observer_context_index()
                    .ok()
                    .and_then(|index| ssl.ssl_context().ex_data(index))
                    .cloned()
            });
        if let Some(observer) = observer {
            observer(event);
        }
    }));
}
fn length24(bytes: &mut &[u8]) -> Option<usize> {
    let count = bytes.get(..3)?;
    let value =
        (usize::from(count[0]) << 16) | (usize::from(count[1]) << 8) | usize::from(count[2]);
    *bytes = &bytes[3..];
    Some(value)
}
fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Option<&'a [u8]> {
    let value = bytes.get(..length)?;
    *bytes = &bytes[length..];
    Some(value)
}
fn parse_certificate_message(message: &[u8], tls13: bool) -> Option<Vec<Vec<u8>>> {
    let mut bytes = message.get(1..)?;
    if length24(&mut bytes)? != bytes.len() {
        return None;
    }
    if tls13 {
        let context_length = usize::from(*take(&mut bytes, 1)?.first()?);
        take(&mut bytes, context_length)?;
    }
    if length24(&mut bytes)? != bytes.len() {
        return None;
    }
    let mut chain = Vec::new();
    while !bytes.is_empty() {
        if chain.len() >= 32 {
            return None;
        }
        let certificate_length = length24(&mut bytes)?;
        if certificate_length == 0 || certificate_length > 1024 * 1024 {
            return None;
        }
        chain.push(take(&mut bytes, certificate_length)?.to_vec());
        if tls13 {
            let length = take(&mut bytes, 2)?;
            let extension_length = (usize::from(length[0]) << 8) | usize::from(length[1]);
            take(&mut bytes, extension_length)?;
        }
    }
    Some(chain)
}

type KeyMethod = Arc<dyn PrivateKeyMethod>;
static KEY_INDEX: OnceLock<Result<Index<Ssl, KeyMethod>, ErrorStack>> = OnceLock::new();
fn key_index() -> Result<Index<Ssl, KeyMethod>, ErrorStack> {
    KEY_INDEX
        .get_or_init(Ssl::new_ex_index::<KeyMethod>)
        .clone()
}
static KEY_METHOD: boring_sys::SSL_PRIVATE_KEY_METHOD = boring_sys::SSL_PRIVATE_KEY_METHOD {
    sign: Some(sign),
    decrypt: Some(decrypt),
    complete: Some(complete),
};

/// Installs a per-SSL signing provider without exporting a native private key.
/// # Errors
/// Returns a native ex-data allocation error.
pub fn set_private_key_method(ssl: &mut SslRef, method: KeyMethod) -> Result<(), ErrorStack> {
    ssl.set_ex_data(key_index()?, method);
    // SAFETY: the static callback table outlives SSL. SSL-owned typed ex-data
    // holds the signing provider, including through suspended operations.
    unsafe { boring_sys::SSL_set_private_key_method(ssl.as_ptr(), &raw const KEY_METHOD) };
    Ok(())
}

/// Restricts certificate signing to algorithms supported by the selected key.
/// # Errors
/// Returns a native error for an empty or invalid algorithm list.
pub fn set_signing_algorithms(ssl: &mut SslRef, algorithms: &[u16]) -> Result<(), ErrorStack> {
    if algorithms.is_empty() || algorithms.len() > 32 {
        return Err(ErrorStack::get());
    }
    // SAFETY: SSL is exclusively borrowed and algorithms contains the declared
    // number of initialized u16 values. BoringSSL copies them synchronously.
    if unsafe {
        boring_sys::SSL_set_signing_algorithm_prefs(
            ssl.as_ptr(),
            algorithms.as_ptr(),
            algorithms.len(),
        )
    } == 1
    {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

unsafe fn key_operation(
    pointer: *mut boring_sys::SSL,
    output: *mut u8,
    length: *mut usize,
    capacity: usize,
    operation: impl FnOnce(
        &dyn PrivateKeyMethod,
        &mut SslRef,
        &mut [u8],
    ) -> Result<usize, PrivateKeyMethodError>,
) -> boring_sys::ssl_private_key_result_t {
    const FAILURE: boring_sys::ssl_private_key_result_t =
        boring_sys::ssl_private_key_result_t::ssl_private_key_failure;
    const RETRY: boring_sys::ssl_private_key_result_t =
        boring_sys::ssl_private_key_result_t::ssl_private_key_retry;
    const SUCCESS: boring_sys::ssl_private_key_result_t =
        boring_sys::ssl_private_key_result_t::ssl_private_key_success;
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        if capacity == 0 || capacity > 65_536 || output.is_null() || length.is_null() {
            return FAILURE;
        }
        // SAFETY: BoringSSL provides a live SSL and writable output/capacity and
        // length buffers for the duration of this synchronous callback.
        let ssl = unsafe { SslRef::from_ptr_mut(pointer) };
        let Some(method) = key_index()
            .ok()
            .and_then(|index| ssl.ex_data(index))
            .cloned()
        else {
            return FAILURE;
        };
        // SAFETY: native callback contract guarantees capacity writable bytes,
        // but BoringSSL may reserve uninitialized signature storage. Initialize
        // every byte before exposing it as an initialized Rust slice.
        unsafe { std::ptr::write_bytes(output, 0, capacity) };
        // SAFETY: the output now contains capacity initialized, writable bytes.
        let buffer = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
        match operation(method.as_ref(), ssl, buffer) {
            Ok(written) if written <= capacity => {
                // SAFETY: length points to the native callback output count.
                unsafe { *length = written };
                SUCCESS
            }
            Err(error) if error == PrivateKeyMethodError::RETRY => RETRY,
            _ => FAILURE,
        }
    }))
    .unwrap_or(FAILURE)
}
unsafe extern "C" fn sign(
    ssl: *mut boring_sys::SSL,
    output: *mut u8,
    length: *mut usize,
    capacity: usize,
    algorithm: u16,
    input: *const u8,
    input_length: usize,
) -> boring_sys::ssl_private_key_result_t {
    if input.is_null() || input_length > 65_536 {
        return boring_sys::ssl_private_key_result_t::ssl_private_key_failure;
    }
    // SAFETY: the native callback guarantees input_length readable bytes.
    let input = unsafe { std::slice::from_raw_parts(input, input_length) };
    // SAFETY: forwarding the live SSL and native callback output buffers.
    unsafe {
        key_operation(ssl, output, length, capacity, |method, ssl, buffer| {
            method.sign(ssl, input, algorithm.into(), buffer)
        })
    }
}
unsafe extern "C" fn decrypt(
    ssl: *mut boring_sys::SSL,
    output: *mut u8,
    length: *mut usize,
    capacity: usize,
    input: *const u8,
    input_length: usize,
) -> boring_sys::ssl_private_key_result_t {
    if input.is_null() || input_length > 65_536 {
        return boring_sys::ssl_private_key_result_t::ssl_private_key_failure;
    }
    // SAFETY: the native callback guarantees input_length readable bytes.
    let input = unsafe { std::slice::from_raw_parts(input, input_length) };
    // SAFETY: forwarding the live SSL and native callback output buffers.
    unsafe {
        key_operation(ssl, output, length, capacity, |method, ssl, buffer| {
            method.decrypt(ssl, input, buffer)
        })
    }
}
unsafe extern "C" fn complete(
    ssl: *mut boring_sys::SSL,
    output: *mut u8,
    length: *mut usize,
    capacity: usize,
) -> boring_sys::ssl_private_key_result_t {
    // SAFETY: forwarding the live SSL and native callback output buffers.
    unsafe {
        key_operation(ssl, output, length, capacity, |method, ssl, buffer| {
            method.complete(ssl, buffer)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boring::ssl::{SslMethod, SslSignatureAlgorithm};
    use foreign_types::ForeignType;
    use std::mem::MaybeUninit;

    struct InitializedBufferMethod;
    fn signature(output: &mut [u8]) -> usize {
        assert!(output.iter().all(|byte| *byte == 0));
        output[..2].copy_from_slice(&[7, 9]);
        2
    }
    impl PrivateKeyMethod for InitializedBufferMethod {
        fn sign(
            &self,
            _: &mut SslRef,
            _: &[u8],
            _: SslSignatureAlgorithm,
            output: &mut [u8],
        ) -> Result<usize, PrivateKeyMethodError> {
            Ok(signature(output))
        }
        fn decrypt(
            &self,
            _: &mut SslRef,
            _: &[u8],
            output: &mut [u8],
        ) -> Result<usize, PrivateKeyMethodError> {
            Ok(signature(output))
        }
        fn complete(
            &self,
            _: &mut SslRef,
            output: &mut [u8],
        ) -> Result<usize, PrivateKeyMethodError> {
            Ok(signature(output))
        }
    }

    #[test]
    fn every_key_callback_initializes_reserved_native_output() {
        let context = SslContext::builder(SslMethod::tls()).unwrap().build();
        let mut ssl = Ssl::new(&context).unwrap();
        set_private_key_method(&mut ssl, Arc::new(InitializedBufferMethod)).unwrap();
        for operation in 0..3 {
            let mut output = [MaybeUninit::<u8>::uninit(); 8];
            let mut length = usize::MAX;
            let pointer = output.as_mut_ptr().cast::<u8>();
            // SAFETY: the live SSL and owned output/count buffers remain exclusive
            // throughout the call. Input is initialized and readable. Output is
            // deliberately uninitialized, as allowed by the native callback API.
            let result = unsafe {
                match operation {
                    0 => sign(
                        ssl.as_ptr(),
                        pointer,
                        &raw mut length,
                        8,
                        0x0403,
                        b"a".as_ptr(),
                        1,
                    ),
                    1 => decrypt(ssl.as_ptr(), pointer, &raw mut length, 8, b"a".as_ptr(), 1),
                    _ => complete(ssl.as_ptr(), pointer, &raw mut length, 8),
                }
            };
            assert_eq!(
                result,
                boring_sys::ssl_private_key_result_t::ssl_private_key_success
            );
            assert_eq!(length, 2);
            // SAFETY: success plus the provider's assertion proves the adapter
            // initialized all eight bytes before exposing them to safe Rust.
            let initialized = output.map(|byte| unsafe { byte.assume_init() });
            assert_eq!(initialized, [7, 9, 0, 0, 0, 0, 0, 0]);
        }
    }
}

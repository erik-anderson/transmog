//! Single certificate path builder: Chromium's `BoringSSL` PKI implementation.
use crate::trust_anchors::CertificatePolicy;
use boring::{error::ErrorStack, ssl::SslRef, x509::X509};
use foreign_types::ForeignTypeRef;
use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

unsafe extern "C" {
    fn transmog_tls_peer_chain_len(ssl: *const boring_sys::SSL) -> usize;
    fn transmog_tls_peer_certificate(
        ssl: *const boring_sys::SSL,
        index: usize,
        len: *mut usize,
    ) -> *const u8;
    fn transmog_pki_certificate_parseable(bytes: *const u8, len: usize) -> bool;
    fn transmog_pki_policy_new() -> *mut c_void;
    fn transmog_pki_policy_free(policy: *mut c_void);
    fn transmog_pki_policy_add(
        policy: *mut c_void,
        bytes: *const u8,
        len: usize,
        kind: i32,
        expiry: bool,
        constraints: bool,
    ) -> i32;
    fn transmog_pki_verify(
        policy: *mut c_void,
        certificates: *const *const u8,
        lengths: *const usize,
        count: usize,
        time: i64,
        authority: bool,
    ) -> *mut c_void;
    fn transmog_pki_result_free(result: *mut c_void);
    fn transmog_pki_result_code(result: *const c_void) -> i32;
    fn transmog_pki_result_depth(result: *const c_void) -> i32;
    fn transmog_pki_result_valid(result: *const c_void) -> bool;
    fn transmog_pki_result_parsed(result: *const c_void) -> bool;
    fn transmog_pki_result_path_len(result: *const c_void) -> usize;
    fn transmog_pki_result_names_len(result: *const c_void, ip: bool) -> usize;
    fn transmog_pki_result_name(
        result: *const c_void,
        ip: bool,
        index: usize,
        len: *mut usize,
    ) -> *const u8;
    fn transmog_pki_result_bytes(result: *const c_void, index: usize, len: *mut usize)
    -> *const u8;
}

/// Whether a certificate can be represented by the Chromium PKI parser.
/// This validates import syntax only and confers no certificate trust.
pub fn certificate_is_parseable(bytes: &[u8]) -> bool {
    if bytes.len() > 1024 * 1024 {
        return false;
    }
    // SAFETY: input is borrowed only during parsing and native parsing owns its
    // temporary buffer; no input pointer is retained or returned.
    unsafe { transmog_pki_certificate_parseable(bytes.as_ptr(), bytes.len()) }
}

/// Result of a single PKI path-building operation, including failed paths.
#[derive(Clone, Debug)]
pub struct Verification {
    /// Whether the requested certificate use passed verification.
    pub accepted: bool,
    /// Stable legacy display code for compatibility with saved capture evidence.
    pub error_code: i32,
    /// `BoringSSL` PKI status code, rather than a legacy X.509 verifier result.
    pub pki_status: i32,
    /// Certificate index for a specific error; absent for a path-wide error.
    pub depth: Option<u32>,
    /// Bounded human-readable PKI explanation.
    pub diagnostic: String,
    /// PKI's chosen path, leaf first, as owned public DER.
    pub path: Vec<Vec<u8>>,
    /// Trust role on a successfully verified path.
    pub trust_role: Option<&'static str>,
    pub(crate) names: Option<crate::hostname::Names>,
}
impl Verification {
    /// Creates a rejection when verification cannot start. Capture observers
    /// receive results by shared reference and cannot authorize certificates.
    pub fn failure(message: &str) -> Self {
        Self {
            accepted: false,
            error_code: 28,
            pki_status: 16,
            depth: None,
            diagnostic: message.into(),
            path: Vec::new(),
            trust_role: None,
            names: None,
        }
    }
}

/// Immutable native parsed trust inputs. Verification creates only local state.
pub(crate) struct CompiledPolicy(NonNull<c_void>);
// SAFETY: policy construction finishes before publication. PKI stores and
// issuer sources perform read-only lookups; each invocation owns its builder,
// delegate, errors and peer issuer source. No native pointer is exposed.
unsafe impl Send for CompiledPolicy {}
unsafe impl Sync for CompiledPolicy {}
impl Drop for CompiledPolicy {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and the last Arc guarantees no readers.
        unsafe { transmog_pki_policy_free(self.0.as_ptr()) };
    }
}
type CacheEntry = (Weak<CertificatePolicy>, Arc<CompiledPolicy>);

pub(crate) fn compile(policy: &Arc<CertificatePolicy>) -> Result<Arc<CompiledPolicy>, ErrorStack> {
    static CACHE: OnceLock<Mutex<Vec<CacheEntry>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|(input, _)| input.strong_count() != 0);
    if let Some((_, compiled)) = cache.iter().find(|(input, _)| {
        input
            .upgrade()
            .is_some_and(|input| Arc::ptr_eq(&input, policy))
    }) {
        return Ok(compiled.clone());
    }
    // SAFETY: constructor returns a newly owned native handle or null.
    let compiled = CompiledPolicy(
        NonNull::new(unsafe { transmog_pki_policy_new() }).ok_or_else(ErrorStack::get)?,
    );
    let add = |bytes: &[u8], kind, expiry, constraints| {
        // SAFETY: unpublished exclusively constructed policy; bytes are copied.
        if unsafe {
            transmog_pki_policy_add(
                compiled.0.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                kind,
                expiry,
                constraints,
            )
        } == 1
        {
            Ok(())
        } else {
            Err(ErrorStack::get())
        }
    };
    for issuer in policy.issuers.iter() {
        add(&issuer.to_der()?, 0, false, false)?;
    }
    // Public trust wins duplicate local ROOT/TrustedPeople trust, as it does in
    // Chromium's TrustStoreCollection. Exact Disallowed always wins everything.
    for leaf in policy.trusted_leaves.iter() {
        let bytes = leaf.to_der()?;
        if !policy.anchors.contains(&bytes) {
            add(&bytes, 3, true, true)?;
        }
    }
    for anchor in policy.anchors.iter() {
        add(
            anchor,
            if policy.anchor_only.contains(anchor) {
                1
            } else {
                2
            },
            !policy.ignore_anchor_expiry.contains(anchor),
            policy.constrained_anchors.contains(anchor),
        )?;
    }
    for blocked in policy.disallowed.iter() {
        add(blocked, 4, false, false)?;
    }
    let compiled = Arc::new(compiled);
    if cache.len() == 16 {
        cache.remove(0);
    }
    cache.push((Arc::downgrade(policy), compiled.clone()));
    Ok(compiled)
}

struct NativeResult(NonNull<c_void>);
impl Drop for NativeResult {
    fn drop(&mut self) {
        // SAFETY: result is exclusively owned; borrowed output was already copied.
        unsafe { transmog_pki_result_free(self.0.as_ptr()) };
    }
}
impl NativeResult {
    fn names(&self, ip: bool) -> Vec<Vec<u8>> {
        // SAFETY: immutable native result owns these parsed public SAN values.
        let count = unsafe { transmog_pki_result_names_len(self.0.as_ptr(), ip) };
        (0..count)
            .map(|index| {
                let mut len = 0;
                // SAFETY: index is bounded by the result's count; its bytes remain live.
                let bytes =
                    unsafe { transmog_pki_result_name(self.0.as_ptr(), ip, index, &raw mut len) };
                if len == 0 {
                    Vec::new()
                } else {
                    // SAFETY: native accessor returns exactly len readable bytes.
                    unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()
                }
            })
            .collect()
    }
    fn bytes(&self, index: usize) -> Vec<u8> {
        let mut len = 0;
        // SAFETY: native result is live and owns the returned bytes until Drop.
        let bytes = unsafe { transmog_pki_result_bytes(self.0.as_ptr(), index, &raw mut len) };
        if len == 0 {
            return Vec::new();
        }
        // SAFETY: the native accessor returned exactly len readable bytes.
        unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()
    }
}

/// Copies the peer's raw public TLS certificate buffers without legacy parsing.
/// Malformed entries remain available for diagnostics; they never confer trust.
pub fn peer_certificate_chain(ssl: &SslRef) -> Vec<Vec<u8>> {
    // SAFETY: raw buffers belong to the live borrowed handshake. This API does
    // not parse them through the legacy X.509 compatibility layer.
    let count = unsafe { transmog_tls_peer_chain_len(ssl.as_ptr()) };
    if count > 1024 {
        return Vec::new();
    }
    (0..count)
        .map(|index| {
            let mut len = 0;
            // SAFETY: index is bounded by the stack's count and SSL is borrowed.
            let bytes = unsafe { transmog_tls_peer_certificate(ssl.as_ptr(), index, &raw mut len) };
            if len == 0 || len > 1024 * 1024 {
                Vec::new()
            } else {
                // SAFETY: SSL owns the complete returned buffer during this copy.
                unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()
            }
        })
        .collect()
}

pub(crate) fn verify(
    compiled: &CompiledPolicy,
    policy: &CertificatePolicy,
    certificates: &[Vec<u8>],
    authority: bool,
) -> Verification {
    if policy.unavailable {
        return Verification::failure("Current certificate trust is unavailable");
    }
    if certificates.len() > 1024 || certificates.iter().any(|cert| cert.len() > 1024 * 1024) {
        return Verification::failure("Peer certificate chain exceeds supported bounds");
    }
    let Some(time) = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| i64::try_from(time.as_secs()).ok())
    else {
        return Verification::failure("Cannot determine certificate verification time");
    };
    let pointers: Vec<_> = certificates.iter().map(Vec::as_ptr).collect();
    let lengths: Vec<_> = certificates.iter().map(Vec::len).collect();
    // SAFETY: immutable compiled inputs and borrowed peer buffers live through
    // this synchronous call. Native output is a newly owned independent result.
    let result = unsafe {
        transmog_pki_verify(
            compiled.0.as_ptr(),
            pointers.as_ptr(),
            lengths.as_ptr(),
            certificates.len(),
            time,
            authority,
        )
    };
    let Some(result) = NonNull::new(result).map(NativeResult) else {
        return Verification::failure("Native PKI verification could not complete");
    };
    // SAFETY: getters read the live result owned exclusively by this scope.
    let (accepted, status, depth, count) = unsafe {
        (
            transmog_pki_result_valid(result.0.as_ptr()),
            transmog_pki_result_code(result.0.as_ptr()),
            transmog_pki_result_depth(result.0.as_ptr()),
            transmog_pki_result_path_len(result.0.as_ptr()),
        )
    };
    let path: Vec<_> = (0..count).map(|index| result.bytes(index)).collect();
    let mut diagnostic = String::from_utf8_lossy(&result.bytes(count)).into_owned();
    let mut end = diagnostic.len().min(2048);
    while !diagnostic.is_char_boundary(end) {
        end -= 1;
    }
    diagnostic.truncate(end);
    let trust_role = path.last().filter(|_| accepted).map(|anchor| {
        if path.len() == 1 && !policy.anchor_only.contains(anchor) {
            "Exact locally trusted server certificate"
        } else if policy.local_anchors.contains(anchor) {
            "Anchor present in Windows ROOT"
        } else if policy.anchor_only.contains(anchor) {
            "Chromium public anchor"
        } else {
            "Configured trust anchor"
        }
    });
    let error_code = match status {
        0 => 0,
        1 => 7,
        4 => 23,
        7 => 10,
        8 => 9,
        9 => 26,
        11 => 20,
        _ => 28,
    };
    Verification {
        accepted,
        error_code,
        pki_status: status,
        depth: u32::try_from(depth).ok(),
        diagnostic,
        path,
        trust_role,
        // SAFETY: getter reads the live immutable result's target parse status.
        names: unsafe { transmog_pki_result_parsed(result.0.as_ptr()) }.then(|| {
            crate::hostname::Names {
                dns: result.names(false),
                ips: result.names(true),
            }
        }),
    }
}

/// Validates a prospective TLS issuer using the same path builder and trust roles.
/// This confers no new trust and cannot authorize `TrustedPeople` as an issuer.
pub fn verify_certificate_authority(policy: &Arc<CertificatePolicy>, certificate: &X509) -> bool {
    let Ok(compiled) = compile(policy) else {
        return false;
    };
    let Ok(der) = certificate.to_der() else {
        return false;
    };
    verify(&compiled, policy, &[der], true).accepted
}

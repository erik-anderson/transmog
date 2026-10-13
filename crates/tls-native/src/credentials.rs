//! Safe ownership boundary for `BoringSSL` X.509 credentials and ML-DSA key types.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    pkey::{HasPublic, Id, PKey, PKeyRef, Private},
    ssl::{SslContextBuilder, SslCredential, SslCredentialRef, SslRef, SslSignatureAlgorithm},
    x509::X509,
};
use foreign_types::{ForeignType, ForeignTypeRef};
use std::{ptr::NonNull, sync::OnceLock};

static CHAIN_INDEX: OnceLock<Result<Index<SslCredential, Vec<X509>>, ErrorStack>> = OnceLock::new();
fn chain_index() -> Result<Index<SslCredential, Vec<X509>>, ErrorStack> {
    CHAIN_INDEX
        .get_or_init(SslCredential::new_ex_index::<Vec<X509>>)
        .clone()
}

/// Copies the selected credential's public chain while handshake callbacks run.
/// The legacy certificate getter describes the configured fallback instead.
pub fn selected_credential_chain(ssl: &SslRef) -> Option<Vec<X509>> {
    let index = chain_index().ok()?;
    // SAFETY: the getter's borrowed credential remains alive throughout this
    // synchronous SSL borrow. Only independently owned public references escape.
    let pointer = unsafe { boring_sys::SSL_get0_selected_credential(ssl.as_ptr()) };
    if pointer.is_null() {
        return None;
    }
    let credential = unsafe { SslCredentialRef::from_ptr(pointer.cast_mut()) };
    credential.ex_data(index).cloned()
}

/// ML-DSA parameter sets advertised by Chromium for TLS authentication.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MlDsaVariant {
    /// ML-DSA-44.
    MlDsa44,
    /// ML-DSA-65.
    MlDsa65,
    /// ML-DSA-87.
    MlDsa87,
}
impl MlDsaVariant {
    fn key_id(self) -> Id {
        Id::from_raw(match self {
            Self::MlDsa44 => boring_sys::EVP_PKEY_ML_DSA_44,
            Self::MlDsa65 => boring_sys::EVP_PKEY_ML_DSA_65,
            Self::MlDsa87 => boring_sys::EVP_PKEY_ML_DSA_87,
        })
    }
    /// Returns the exact TLS `SignatureScheme` for this key type.
    pub fn signature_algorithm(self) -> SslSignatureAlgorithm {
        SslSignatureAlgorithm::from(self.signature_scheme())
    }
    /// Returns the registered numeric TLS signature scheme.
    pub fn signature_scheme(self) -> u16 {
        match self {
            Self::MlDsa44 => 0x0904,
            Self::MlDsa65 => 0x0905,
            Self::MlDsa87 => 0x0906,
        }
    }
    /// Generates an in-memory key using the workspace's one `BoringSSL` instance.
    /// # Errors
    /// Returns the native key-generation failure.
    pub fn generate_key(self) -> Result<PKey<Private>, ErrorStack> {
        PKey::generate(self.key_id())
    }
    /// Identifies an ML-DSA public key without inspecting its private components.
    pub fn for_key<T: HasPublic>(key: &PKeyRef<T>) -> Option<Self> {
        [Self::MlDsa44, Self::MlDsa65, Self::MlDsa87]
            .into_iter()
            .find(|variant| key.id() == variant.key_id())
    }
}

struct PublicBuffer(NonNull<boring_sys::CRYPTO_BUFFER>);
impl PublicBuffer {
    fn new(der: &[u8]) -> Result<Self, ErrorStack> {
        // SAFETY: CRYPTO_BUFFER_new copies the borrowed public DER. This owner
        // frees its reference on every exit path; no borrowed bytes escape.
        let pointer =
            unsafe { boring_sys::CRYPTO_BUFFER_new(der.as_ptr(), der.len(), std::ptr::null_mut()) };
        NonNull::new(pointer).map(Self).ok_or_else(ErrorStack::get)
    }
}
impl Drop for PublicBuffer {
    fn drop(&mut self) {
        // SAFETY: this owner holds exactly one live reference from new().
        unsafe {
            boring_sys::CRYPTO_BUFFER_free(self.0.as_ptr());
        }
    }
}

/// Appends a complete X.509 credential in decreasing server preference order.
/// The context retains its own certificate/key references. Mismatched keys,
/// incomplete chains and native allocation failures are rejected.
/// # Errors
/// Returns the native credential construction or installation failure.
pub fn add_server_credential(
    context: &mut SslContextBuilder,
    chain: &[X509],
    key: &PKeyRef<Private>,
) -> Result<(), ErrorStack> {
    // SAFETY: new_x509 returns an owned allocation. SslCredential's Drop uses
    // the corresponding free function, including all early error returns.
    let pointer = unsafe { boring_sys::SSL_CREDENTIAL_new_x509() };
    if pointer.is_null() {
        return Err(ErrorStack::get());
    }
    let credential = unsafe { SslCredential::from_ptr(pointer) };
    let index = chain_index()?;
    let public_chain = Box::into_raw(Box::new(chain.to_vec()));
    // SAFETY: the typed ex-data index owns this Box through its registered
    // destructor once set_ex_data succeeds. A failed installation stores nothing.
    if unsafe {
        boring_sys::SSL_CREDENTIAL_set_ex_data(
            credential.as_ptr(),
            index.as_raw(),
            public_chain.cast(),
        )
    } != 1
    {
        unsafe {
            drop(Box::from_raw(public_chain));
        }
        return Err(ErrorStack::get());
    }
    let buffers = chain
        .iter()
        .map(|certificate| PublicBuffer::new(&certificate.to_der()?))
        .collect::<Result<Vec<_>, ErrorStack>>()?;
    let pointers: Vec<_> = buffers.iter().map(|buffer| buffer.0.as_ptr()).collect();
    // SAFETY: all arrays and objects are owned and live throughout these
    // synchronous calls. The set1/add1 APIs retain references. The context is
    // exclusively borrowed, and this credential is never mutated after add1.
    let accepted = unsafe {
        boring_sys::SSL_CREDENTIAL_set1_cert_chain(
            credential.as_ptr(),
            pointers.as_ptr(),
            pointers.len(),
        ) == 1
            && boring_sys::SSL_CREDENTIAL_set1_private_key(credential.as_ptr(), key.as_ptr()) == 1
            && boring_sys::SSL_CTX_add1_credential(context.as_ptr(), credential.as_ptr()) == 1
    };
    if accepted {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boring::{
        asn1::Asn1Time,
        bn::BigNum,
        ec::{EcGroup, EcKey},
        hash::MessageDigest,
        nid::Nid,
        ssl::{
            Ssl, SslAcceptor, SslConnector, SslMethod, SslSessionCacheMode, SslStream, SslVersion,
        },
        x509::{
            X509NameBuilder,
            extension::{BasicConstraints, SubjectAlternativeName},
        },
    };
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        time::Duration,
    };

    fn certificate() -> (X509, PKey<Private>) {
        let key = PKey::from_ec_key(
            EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
        )
        .unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        cert.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
            .unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        cert.append_extension(&BasicConstraints::new().ca().build().unwrap())
            .unwrap();
        let san = SubjectAlternativeName::new()
            .dns("localhost")
            .build(&cert.x509v3_context(None, None))
            .unwrap();
        cert.append_extension(&san).unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        (cert.build(), key)
    }

    #[test]
    fn resumed_tls13_reports_current_group_and_original_authentication() {
        let (cert, key) = certificate();
        let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        server.set_certificate(&cert).unwrap();
        server.set_private_key(&key).unwrap();
        server
            .set_min_proto_version(Some(SslVersion::TLS1_3))
            .unwrap();
        server
            .set_curves_list("X25519MLKEM768:X25519:P-256:P-384")
            .unwrap();
        server.set_session_id_context(b"pqc-resumption").unwrap();
        let server = server.build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            for (resumed, group) in [(false, 0x11ec), (true, 0x11ec), (true, 29)] {
                let (socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut tls = server.accept(socket).unwrap();
                assert_eq!(tls.ssl().session_reused(), resumed);
                assert_eq!(tls.ssl().curve(), Some(group));
                tls.write_all(&[7]).unwrap();
                let mut byte = [0];
                tls.read_exact(&mut byte).unwrap();
            }
        });
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let cache = sessions.clone();
        let mut context = SslConnector::builder(SslMethod::tls()).unwrap();
        context.cert_store_mut().add_cert(cert).unwrap();
        context
            .set_min_proto_version(Some(SslVersion::TLS1_3))
            .unwrap();
        context
            .set_curves_list("X25519MLKEM768:X25519:P-256:P-384")
            .unwrap();
        context.set_session_cache_mode(
            SslSessionCacheMode::CLIENT | SslSessionCacheMode::NO_INTERNAL_STORE,
        );
        context.set_new_session_callback(move |_, session| {
            cache.lock().unwrap().push(session);
        });
        let context = context.build();
        for (resumed, group) in [(false, 0x11ec), (true, 0x11ec), (true, 29)] {
            let mut ssl: Ssl = context.configure().unwrap().into_ssl("localhost").unwrap();
            if resumed {
                let sessions = sessions.lock().unwrap();
                let session = sessions
                    .last()
                    .expect("the first connection yielded a ticket");
                // SAFETY: this session came from the same connector context,
                // verified hostname, issuer trust and policy immediately above.
                unsafe {
                    ssl.set_session(session).unwrap();
                }
            }
            if group == 29 {
                ssl.set_curves_list("X25519").unwrap();
            }
            let socket = TcpStream::connect(address).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut tls = SslStream::new(ssl, socket).unwrap();
            tls.connect().unwrap();
            let mut byte = [0];
            tls.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [7]);
            assert_eq!(tls.ssl().session_reused(), resumed);
            assert_eq!(tls.ssl().curve(), Some(group));
            // This getter deliberately retains the original session's
            // CertificateVerify scheme, including on abbreviated handshakes.
            assert_eq!(
                tls.ssl().peer_signature_algorithm(),
                Some(SslSignatureAlgorithm::ECDSA_SECP256R1_SHA256)
            );
            tls.write_all(&byte).unwrap();
        }
        worker.join().unwrap();
    }
}

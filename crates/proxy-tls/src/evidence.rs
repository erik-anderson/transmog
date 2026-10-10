//! Certificate and verification collection never reads private keys or TLS secrets.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    hash::MessageDigest,
    ssl::{Ssl, SslContext, SslContextBuilder, SslRef},
    x509::{X509Ref, X509StoreContext, X509StoreContextRef},
};
use std::sync::{Arc, Mutex, OnceLock};
use transmog_core::{PublicCertificate, TlsEvidence, VerificationResult};

static SSL_INDEX: OnceLock<Result<Index<Ssl, TlsCapture>, ErrorStack>> = OnceLock::new();
static CONTEXT_INDEX: OnceLock<Result<Index<SslContext, TlsCapture>, ErrorStack>> = OnceLock::new();
fn ssl_index() -> Result<Index<Ssl, TlsCapture>, ErrorStack> {
    SSL_INDEX
        .get_or_init(Ssl::new_ex_index::<TlsCapture>)
        .clone()
}
fn context_index() -> Result<Index<SslContext, TlsCapture>, ErrorStack> {
    CONTEXT_INDEX
        .get_or_init(SslContext::new_ex_index::<TlsCapture>)
        .clone()
}

/// Cloneable public evidence for one physical TLS connection, including failures.
#[derive(Clone, Debug)]
pub struct TlsCapture(Arc<Mutex<TlsEvidence>>);
impl TlsCapture {
    /// Starts collection with the local connection policy and trust provenance.
    pub fn new(evidence: TlsEvidence) -> Self {
        Self(Arc::new(Mutex::new(evidence)))
    }
    fn data(&self) -> std::sync::MutexGuard<'_, TlsEvidence> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Installs collection on an individual Hyper or downstream SSL object.
    /// # Errors
    /// Returns failure to allocate a native callback-data index.
    pub fn attach_ssl(&self, ssl: &mut SslRef) -> Result<(), ErrorStack> {
        self.data().offered_ciphers = ssl
            .ciphers()
            .iter()
            .map(|cipher| cipher.name().to_owned())
            .collect();
        ssl.set_ex_data(ssl_index()?, self.clone());
        Ok(())
    }
    /// Installs collection on a connection-specific quiche context.
    /// # Errors
    /// Returns failure to allocate a native callback-data index.
    pub fn attach_context(&self, context: &mut SslContextBuilder) -> Result<(), ErrorStack> {
        self.data().offered_ciphers = context.ciphers().map_or_else(Vec::new, |ciphers| {
            ciphers
                .iter()
                .map(|cipher| cipher.name().to_owned())
                .collect()
        });
        context.set_ex_data(context_index()?, self.clone());
        Ok(())
    }
    /// Returns immutable recorded evidence; DER remains shared and public-only.
    #[must_use]
    pub fn snapshot(&self) -> TlsEvidence {
        self.data().clone()
    }
    /// Adds a known public issuer path without changing the handshake's wire chain.
    pub fn server_issuers<'a>(&self, chain: impl IntoIterator<Item = &'a X509Ref>) {
        let mut data = self.data();
        for certificate in chain {
            collect_certificate(&mut data.server_chain, certificate);
        }
    }
    /// Marks a completed QUIC verification; the callback retains its full path.
    pub fn verified(&self) {
        self.data().verification = Some(VerificationResult::Verified);
    }
    /// Records an operator-facing negotiation failure without changing verification.
    pub fn failure_chain(&self, mut error: &(dyn std::error::Error + 'static)) {
        for _ in 0..16 {
            self.failure(&error.to_string());
            let Some(cause) = error.source() else { break };
            error = cause;
        }
    }

    /// Records an operator-facing negotiation failure without changing verification.
    pub fn failure(&self, message: &str) {
        let mut data = self.data();
        let end = message
            .char_indices()
            .map(|(offset, _)| offset)
            .take_while(|offset| *offset <= 2048)
            .last()
            .unwrap_or(0);
        let message = if message.len() <= 2048 {
            message.to_owned()
        } else {
            message[..end].to_owned()
        };
        if data.errors.len() < 16 && !data.errors.contains(&message) {
            data.errors.push(message);
        }
    }
    /// Completes evidence from a successful handshake, including resumed sessions.
    /// `server` is true for the browser-facing proxy acceptor.
    pub fn completed(&self, ssl: &SslRef, server: bool) {
        let mut data = self.data();
        let mut local = Vec::new();
        if let Some(certificate) = ssl.certificate() {
            collect_certificate(&mut local, certificate);
        }
        // Boring's extra_chain_certs() asserts when an empty context returns a
        // null stack. Configured issuer paths are supplied explicitly by their
        // owner; peer chains below use the nullable handshake API.
        let mut peer = Vec::new();
        if let Some(certificate) = ssl.peer_certificate() {
            collect_certificate(&mut peer, &certificate);
        }
        if let Some(chain) = ssl.peer_cert_chain() {
            for certificate in chain {
                collect_certificate(&mut peer, certificate);
            }
        }
        if server {
            if data.server_chain.is_empty() {
                data.server_chain = local;
                data.chain_source = Some("configured".into());
            }
            data.client_chain = peer;
        } else {
            if data.server_chain.is_empty() {
                data.server_chain = peer;
                data.chain_source = Some("peer-presented".into());
            }
            data.client_chain = local;
            data.verification = Some(VerificationResult::Verified);
        }
    }
}

pub(crate) fn verification(context: &X509StoreContextRef, accepted: bool) {
    let Ok(index) = X509StoreContext::ssl_idx() else {
        return;
    };
    let Some(ssl) = context.ex_data(index) else {
        return;
    };
    let capture = ssl_index()
        .ok()
        .and_then(|index| ssl.ex_data(index))
        .or_else(|| {
            context_index()
                .ok()
                .and_then(|index| ssl.ssl_context().ex_data(index))
        })
        .cloned();
    let Some(capture) = capture else {
        return;
    };
    let mut data = capture.data();
    if let Some(chain) = context.chain() {
        let mut certificates = Vec::new();
        for certificate in chain {
            collect_certificate(&mut certificates, certificate);
        }
        if !certificates.is_empty() {
            data.server_chain = certificates;
            data.chain_source = Some("verification-path".into());
        }
    }
    if let Some(cipher) = ssl.current_cipher() {
        data.details
            .insert("Negotiated TLS version".into(), ssl.version_str().into());
        data.details
            .insert("Negotiated cipher".into(), cipher.name().into());
    }
    if !accepted && let Err(error) = context.verify_result() {
        data.verification = Some(VerificationResult::Rejected {
            error_code: error.as_raw(),
            depth: Some(context.error_depth()),
        });
        let message = format!(
            "Certificate verification: {} (code {}, depth {})",
            error.error_string(),
            error.as_raw(),
            context.error_depth()
        );
        if data.errors.len() < 16 && !data.errors.contains(&message) {
            data.errors.push(message);
        }
    }
}

fn collect_certificate(chain: &mut Vec<PublicCertificate>, certificate: &X509Ref) {
    let (Ok(der), Ok(digest)) = (
        certificate.to_der(),
        certificate.digest(MessageDigest::sha256()),
    ) else {
        return;
    };
    let sha256 = digest.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    if !chain.iter().any(|certificate| certificate.sha256 == sha256) {
        chain.push(PublicCertificate {
            sha256,
            der: Arc::from(der),
        });
    }
}

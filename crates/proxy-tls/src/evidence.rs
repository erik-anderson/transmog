//! Certificate and verification collection never reads private keys or TLS secrets.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    hash::MessageDigest,
    ssl::{Ssl, SslContext, SslContextBuilder, SslInfoCallbackMode, SslRef, SslVersion},
    x509::X509Ref,
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
pub struct TlsCapture(
    Arc<Mutex<TlsEvidence>>,
    Arc<Mutex<Vec<u8>>>,
    Arc<Mutex<Option<Vec<u8>>>>,
);
impl TlsCapture {
    /// Starts collection with the local connection policy and trust provenance.
    pub fn new(evidence: TlsEvidence) -> Self {
        Self(
            Arc::new(Mutex::new(evidence)),
            Arc::new(Mutex::new(Vec::new())),
            Arc::default(),
        )
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
        if let Some(name) = self.data().server_name.clone() {
            transmog_tls_native::configure_ssl_peer_name(ssl, name)?;
        }
        let capture = self.clone();
        transmog_tls_native::observe_ssl_verification(ssl, move |ssl, result| {
            capture.verification(ssl, result);
        })?;
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
        if let Some(name) = self.data().server_name.clone() {
            transmog_tls_native::configure_context_peer_name(context, name)?;
        }
        let capture = self.clone();
        transmog_tls_native::observe_context_verification(context, move |ssl, result| {
            capture.verification(ssl, result);
        })?;
        configure_completion(context);
        Ok(())
    }
    /// Returns immutable recorded evidence; DER remains shared and public-only.
    #[must_use]
    pub fn snapshot(&self) -> TlsEvidence {
        self.data().clone()
    }
    pub(crate) fn available_trust_anchors(&self) -> Vec<u8> {
        self.1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub(crate) fn ech_retry(&self) -> Option<Vec<u8>> {
        self.2
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// Whether the failed handshake authenticated an ECH retry or rollback.
    pub fn ech_retry_available(&self) -> bool {
        self.ech_retry().is_some()
    }
    /// Records a bounded public client authentication state or policy explanation.
    pub fn authentication_detail(&self, name: &str, value: &str) {
        insert_detail(&mut self.data(), name, value);
    }
    pub(crate) fn client_authentication_event(
        &self,
        event: transmog_tls_native::ClientAuthenticationEvent,
    ) {
        use transmog_tls_native::ClientAuthenticationEvent;
        let mut data = self.data();
        match event {
            ClientAuthenticationEvent::CertificateQueued(chain) => {
                data.client_chain.clear();
                for der in chain {
                    if let Ok(certificate) = boring::x509::X509::from_der(&der) {
                        collect_certificate(&mut data.client_chain, &certificate);
                    }
                }
                let status = if data.client_chain.is_empty() {
                    "Empty certificate message queued to TLS transport"
                } else {
                    "Certificate queued to TLS transport"
                };
                insert_detail(&mut data, "Client certificate", status);
            }
            ClientAuthenticationEvent::PeerAlert(description) => {
                insert_detail(&mut data, "Peer TLS alert", &description.to_string());
                if description == 116 {
                    insert_detail(
                        &mut data,
                        "Client certificate requirement",
                        "Server requires a client certificate",
                    );
                }
            }
        }
    }
    /// Adds a known public issuer path without changing the handshake's wire chain.
    pub fn server_issuers<'a>(&self, chain: impl IntoIterator<Item = &'a X509Ref>) {
        let mut data = self.data();
        for certificate in chain {
            collect_certificate(&mut data.server_chain, certificate);
        }
    }
    /// Retains the public issuer path belonging to the credential actually selected.
    /// Alternative credentials must never append their unrelated issuer paths.
    pub fn server_credential_issuers(&self, leaves: &[crate::IssuedLeaf]) {
        let mut data = self.data();
        if let Some(selected) = data.server_chain.first()
            && let Some(leaf) = leaves.iter().find(|leaf| {
                leaf.certificate
                    .to_der()
                    .is_ok_and(|der| der.as_slice() == selected.der.as_ref())
            })
        {
            for certificate in &leaf.issuer_chain {
                collect_certificate(&mut data.server_chain, certificate);
            }
        }
    }
    /// Adds a known public client authentication path, without reading its key.
    /// Context owners supply issuer certificates because Boring's chain getter
    /// panics for an empty chain. Completion keeps the local leaf first.
    pub fn client_issuers<'a>(&self, chain: impl IntoIterator<Item = &'a X509Ref>) {
        let mut data = self.data();
        for certificate in chain {
            collect_certificate(&mut data.client_chain, certificate);
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
        self.completed_with_state(ssl, server, false);
    }
    #[allow(clippy::too_many_lines)] // Atomically records one handshake's public evidence.
    fn completed_with_state(&self, ssl: &SslRef, server: bool, handshake_done: bool) {
        let mut data = self.data();
        let finished = handshake_done || ssl.is_init_finished();
        if !finished && transmog_tls_native::in_false_start(ssl) {
            insert_detail(&mut data, "TLS False Start", "Used");
            insert_detail(
                &mut data,
                "Handshake completion",
                "Waiting for server Finished",
            );
        } else if finished {
            insert_detail(&mut data, "Handshake completion", "Complete");
        }
        if finished {
            insert_detail(
                &mut data,
                "Encrypted Client Hello",
                if ssl.ech_accepted() {
                    "Accepted"
                } else if transmog_tls_native::ech_configured(ssl) {
                    "Configured; not accepted"
                } else {
                    "Not negotiated"
                },
            );
            collect_key_exchange(&mut data, ssl);
            // The local signature is available only at HANDSHAKE_DONE. A later
            // completed() call must preserve the value collected by the callback.
            for (name, algorithm) in [
                ("Local handshake signature", ssl.signature_algorithm_used()),
                ("Peer handshake signature", ssl.peer_signature_algorithm()),
            ] {
                if let Some(algorithm) = algorithm {
                    if name == "Peer handshake signature" && ssl.session_reused() {
                        // BoringSSL's getter reads SSL_SESSION, so this is the
                        // original authentication signature, not a new wire message.
                        insert_detail(
                            &mut data,
                            "Original peer handshake signature",
                            &algorithm.to_string(),
                        );
                        insert_detail(&mut data, name, "Not sent (session resumed)");
                    } else {
                        insert_detail(&mut data, name, &algorithm.to_string());
                    }
                }
            }
            collect_resumption(&mut data, ssl, server);
            if !server
                && ssl.session_reused()
                && data.details.contains_key("Requested Trust Anchor IDs")
            {
                insert_detail(
                    &mut data,
                    "Trust Anchor IDs",
                    "Offered; session resumed without a new certificate",
                );
            }
            insert_detail(
                &mut data,
                "HelloRetryRequest",
                if ssl.used_hello_retry_request() {
                    "Yes"
                } else {
                    "No"
                },
            );
            insert_detail(&mut data, "Negotiated TLS version", ssl.version_str());
            collect_alps(&mut data, ssl);
            if let Some(cipher) = ssl.current_cipher() {
                insert_detail(&mut data, "Negotiated cipher", cipher.name());
            }
        }
        let mut local = Vec::new();
        if let Some(chain) = transmog_tls_native::selected_credential_chain(ssl) {
            for certificate in &chain {
                collect_certificate(&mut local, certificate);
            }
        } else if ssl.ssl_context().has_x509_support() {
            if let Some(certificate) = ssl.certificate() {
                collect_certificate(&mut local, certificate);
            }
        } else {
            for der in transmog_tls_native::local_buffer_chain(ssl) {
                if let Some(certificate) = certificate_der_evidence(&der) {
                    local.push(certificate);
                }
            }
        }
        // Boring's extra_chain_certs() asserts when an empty context returns a
        // null stack. Configured issuer paths are supplied explicitly by their
        // owner; peer chains below use the nullable handshake API.
        let mut peer = Vec::new();
        for bytes in transmog_tls_native::peer_certificate_chain(ssl)
            .iter()
            .take(32)
        {
            if let Some(certificate) = certificate_der_evidence(bytes) {
                peer.push(certificate);
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
                data.chain_source = Some(
                    if ssl.session_reused() {
                        "resumed-session"
                    } else {
                        "peer-presented"
                    }
                    .into(),
                );
            }
            if !local.is_empty() {
                for certificate in &data.client_chain {
                    if !local
                        .iter()
                        .any(|existing| existing.sha256 == certificate.sha256)
                    {
                        local.push(certificate.clone());
                    }
                }
            }
            // Buffered credentials can be discarded after HANDSHAKE_DONE. Keep
            // the chain recorded by that callback or the actual wire observer.
            if !local.is_empty() || data.client_chain.is_empty() {
                data.client_chain = local;
            }
            data.verification = Some(VerificationResult::Verified);
        }
    }
}

fn collect_alps(data: &mut TlsEvidence, ssl: &SslRef) {
    if ssl.selected_alpn_protocol() == Some(b"h3") {
        let peer = transmog_tls_native::peer_application_settings(ssl);
        let alps = transmog_core::Http3AlpsEvidence {
            negotiated: peer.is_some(),
            peer_settings: peer.unwrap_or_default().into(),
        };
        insert_detail(data, "HTTP/3 ALPS", &alps.summary());
        data.http3_alps = Some(alps);
    }
    if ssl.selected_alpn_protocol() == Some(b"h2") {
        let alps = transmog_tls_native::peer_application_settings(ssl);
        let alps = transmog_core::Http2AlpsEvidence {
            negotiated: alps.is_some(),
            peer_settings: alps.unwrap_or_default().into(),
        };
        insert_detail(data, "HTTP/2 ALPS", &alps.summary());
        data.http2_alps = Some(alps);
    }
}

fn collect_resumption(data: &mut TlsEvidence, ssl: &SslRef, server: bool) {
    insert_detail(
        data,
        "Session resumed",
        if ssl.session_reused() { "Yes" } else { "No" },
    );
    if !server
        && ssl.session_reused()
        && data
            .details
            .get("Certificate Transparency")
            .is_none_or(|status| status == "Not reached")
    {
        for label in ["Certificate Transparency", "CRLSet result", "Stapled OCSP"] {
            insert_detail(data, label, "Not rechecked: resumed validated session");
        }
    }
}

// Both Hyper's per-SSL data and quiche's per-connection context use this hook.
// Reading public facts here also preserves signatures before BoringSSL releases
// handshake-only state, and never installs a verification override.
pub(crate) fn configure_completion(context: &mut SslContextBuilder) {
    context.set_info_callback(|ssl, mode, _| {
        if mode == SslInfoCallbackMode::HANDSHAKE_START {
            crate::client_hello::handshake_started(ssl);
        }
        if mode == SslInfoCallbackMode::CONNECT_EXIT {
            transmog_tls_native::record_ech_exit(ssl);
            if let Some(config) = transmog_tls_native::authenticated_ech_retry(ssl)
                && let Some(capture) = capture_for(ssl)
            {
                *capture
                    .2
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
                capture.authentication_detail(
                    "Encrypted Client Hello",
                    "Rejected; authenticated retry available",
                );
                capture.authentication_detail(
                    "Handshake completion",
                    "ECH rejected; connection closed",
                );
            }
        }
        if mode == SslInfoCallbackMode::HANDSHAKE_DONE
            && let Some(capture) = capture_for(ssl)
        {
            // BoringSSL emits HANDSHAKE_DONE before clearing handshake state;
            // is_init_finished() can still be false during this callback.
            capture.completed_with_state(ssl, ssl.is_server(), true);
        }
    });
}

pub(crate) fn capture_for(ssl: &SslRef) -> Option<TlsCapture> {
    ssl_index()
        .ok()
        .and_then(|index| ssl.ex_data(index))
        .or_else(|| {
            context_index()
                .ok()
                .and_then(|index| ssl.ssl_context().ex_data(index))
        })
        .cloned()
}

impl TlsCapture {
    fn verification(&self, ssl: &SslRef, result: &transmog_tls_native::Verification) {
        let capture = self;
        let mut data = capture.data();
        if ssl.get_ech_name_override().is_some() {
            data.client_chain.clear();
            insert_detail(
                &mut data,
                "ECH client identity",
                "Not sent to rejection endpoint",
            );
            if let Some(name) = ssl
                .get_ech_name_override()
                .and_then(|name| std::str::from_utf8(name).ok())
            {
                insert_detail(&mut data, "ECH rejection public name", name);
            }
        }
        collect_issuer_source(&mut data, result);
        collect_verification(&mut data, &result.security);
        insert_detail(
            &mut data,
            "Public trust program",
            if result.known_public_root {
                "Selected path ends at a known public root"
            } else {
                "Selected path does not end at a known public root"
            },
        );
        insert_detail(
            &mut data,
            "PKI verification status",
            &result.pki_status.to_string(),
        );
        if let Some(role) = result.trust_role {
            insert_detail(&mut data, "Verified trust role", role);
        }
        if data.details.contains_key("Requested Trust Anchor IDs") {
            let (matched, available) = transmog_tls_native::peer_trust_anchors(ssl);
            insert_detail(
                &mut data,
                "Trust Anchor IDs",
                if matched {
                    "Server reported a matching certificate chain"
                } else {
                    "Offered; server did not report a match"
                },
            );
            if !available.is_empty() {
                let value = crate::trust_anchors::hex(&available);
                insert_detail(
                    &mut data,
                    "Server available Trust Anchor IDs",
                    if value.len() <= 2048 {
                        &value
                    } else {
                        "List exceeds display limit; retained for bounded connection recovery"
                    },
                );
                *capture
                    .1
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = available;
            }
        }
        {
            let peer_chain;
            let (chain, source) = if result.path.is_empty() {
                peer_chain = transmog_tls_native::peer_certificate_chain(ssl);
                (&peer_chain, "peer-presented")
            } else if ssl.session_reused() {
                (&result.path, "resumed-session-verification-path")
            } else {
                (&result.path, "verification-path")
            };
            let certificates: Vec<_> = chain
                .iter()
                .filter_map(|bytes| certificate_der_evidence(bytes))
                .collect();
            if !certificates.is_empty() {
                data.server_chain = certificates;
                data.chain_source = Some(source.into());
            }
        }
        if let Some(cipher) = ssl.current_cipher() {
            insert_detail(&mut data, "Negotiated TLS version", ssl.version_str());
            insert_detail(&mut data, "Negotiated cipher", cipher.name());
        }
        if !result.accepted {
            data.verification = Some(VerificationResult::Rejected {
                error_code: result.error_code,
                depth: result.depth,
            });
            let message = format!(
                "Certificate verification: {} (PKI status {}, depth {:?})",
                result.diagnostic, result.pki_status, result.depth
            );
            let message = bounded_text(&message, 2048).to_owned();
            if data.errors.len() < 16 && !data.errors.contains(&message) {
                data.errors.push(message);
            }
        }
    }
}

fn collect_issuer_source(data: &mut TlsEvidence, result: &transmog_tls_native::Verification) {
    insert_detail(data, "Certificate verifier", "Chromium BoringSSL PKI");
    insert_detail(
        data,
        "AIA issuer retrieval attempts",
        &result.issuer_fetch_attempts.to_string(),
    );
    insert_detail(
        data,
        "Intermediate cache",
        if result.cached_issuer_used {
            "Used an untrusted cached issuer candidate"
        } else {
            "No cached issuer used"
        },
    );
}
fn collect_verification(
    data: &mut TlsEvidence,
    result: &transmog_tls_native::SecurityVerification,
) {
    insert_detail(data, "Certificate Transparency", result.ct);
    insert_detail(data, "Verified SCTs", &result.valid_scts.to_string());
    insert_detail(
        data,
        "Invalid or unknown SCTs",
        &result.invalid_scts.to_string(),
    );
    insert_detail(data, "CRLSet result", result.crl);
    insert_detail(data, "Stapled OCSP", result.ocsp);
    if result.known_interception {
        insert_detail(
            data,
            "Known interception key",
            "Present in selected verification path",
        );
    }
}

fn collect_key_exchange(data: &mut TlsEvidence, ssl: &SslRef) {
    // TLS 1.2's abbreviated handshake restores the original session group;
    // it has no new ServerKeyExchange or ClientKeyExchange. TLS 1.3 resumption
    // in our policy performs a fresh (EC)DHE/KEM exchange instead.
    let resumed_tls12 = ssl.session_reused() && ssl.version2() == Some(SslVersion::TLS1_2);
    if resumed_tls12 {
        insert_detail(
            data,
            "Key exchange",
            "Not performed (TLS 1.2 session resumed)",
        );
    }
    if let Some(group) = ssl.curve() {
        let (name, id_name) = if resumed_tls12 {
            ("Original session group", "Original session group ID")
        } else {
            ("Negotiated group", "Negotiated group ID")
        };
        insert_detail(data, name, ssl.curve_name().unwrap_or("Unknown"));
        insert_detail(data, id_name, &format!("0x{group:04x}"));
        let exchange = match group {
            0x11ec => "Hybrid post-quantum key exchange",
            23..=25 | 29 => "Classical key exchange",
            _ => "Unclassified key exchange",
        };
        if !resumed_tls12 {
            insert_detail(data, "Key exchange", exchange);
        }
    }
}

fn insert_detail(data: &mut TlsEvidence, name: &str, value: &str) {
    let name = bounded_text(name, 128);
    if data.details.len() < 64 || data.details.contains_key(name) {
        data.details
            .insert(name.to_owned(), bounded_text(value, 2048).to_owned());
    }
}

fn bounded_text(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn collect_certificate(chain: &mut Vec<PublicCertificate>, certificate: &X509Ref) {
    if let Some(certificate) = certificate_evidence(certificate)
        && !chain
            .iter()
            .any(|existing| existing.sha256 == certificate.sha256)
    {
        chain.push(certificate);
    }
}

fn certificate_evidence(certificate: &X509Ref) -> Option<PublicCertificate> {
    certificate_der_evidence(&certificate.to_der().ok()?)
}

fn certificate_der_evidence(der: &[u8]) -> Option<PublicCertificate> {
    let digest = boring::hash::hash(MessageDigest::sha256(), der).ok()?;
    let sha256 = digest.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    Some(PublicCertificate {
        sha256,
        der: Arc::from(der),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pki_rejections_keep_unicode_diagnostics_within_capture_bounds() {
        let capture = TlsCapture::new(TlsEvidence::default());
        let context = SslContext::builder(boring::ssl::SslMethod::tls())
            .unwrap()
            .build();
        let ssl = Ssl::new(&context).unwrap();
        let result = transmog_tls_native::Verification::failure(&"🦀".repeat(2000));
        capture.verification(&ssl, &result);
        assert!(capture.snapshot().valid());
        assert!(capture.snapshot().errors[0].contains("🦀"));
    }

    #[test]
    fn authentication_details_bound_utf8_bytes_and_entry_count() {
        let capture = TlsCapture::new(TlsEvidence::default());
        let name = format!("{}🦀", "a".repeat(127));
        let value = format!("{}🦀", "b".repeat(2047));
        capture.authentication_detail(&name, &value);
        capture.authentication_detail("🦀", &"🦀".repeat(1400));
        for index in 0..100 {
            capture.authentication_detail(&format!("entry {index}"), "value");
        }
        // Existing status entries can still update when the map is full.
        capture.authentication_detail("🦀", "updated");
        let evidence = capture.snapshot();
        assert!(evidence.valid());
        assert_eq!(evidence.details.len(), 64);
        assert_eq!(evidence.details[&"a".repeat(127)], "b".repeat(2047));
        assert_eq!(evidence.details["🦀"], "updated");
        capture.client_authentication_event(
            transmog_tls_native::ClientAuthenticationEvent::PeerAlert(116),
        );
        assert!(capture.snapshot().valid());
    }
}

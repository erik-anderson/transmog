use std::{fmt, net::IpAddr, sync::Arc, time::SystemTime};

use boring::{
    ssl::{
        NameType, SslConnector, SslConnectorBuilder, SslContext, SslContextBuilder, SslMethod,
        SslVerifyMode, SslVersion,
    },
    x509::{X509, X509StoreContext, X509VerifyError, store::X509StoreBuilder},
};
use thiserror::Error;

/// Raw trust material and source metadata before `BoringSSL` parsing.
#[derive(Clone, Debug)]
pub struct LoadedTrust {
    /// DER-encoded root certificates.
    pub certificates_der: Vec<Vec<u8>>,
    /// Human-readable source description.
    pub source_description: String,
    /// Source-specific version for later constrained trust sources.
    pub source_version: Option<String>,
    /// Non-fatal enumeration diagnostics.
    pub diagnostics: Vec<String>,
}

/// Supplies certificate bytes; it never performs path validation.
pub trait TrustSource: Send + Sync {
    /// Captures a fresh, immutable view of trust material.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] when the source cannot enumerate its trust data.
    fn load(&self) -> Result<LoadedTrust, TrustError>;
}

/// Enumerates the operating system's current root-certificate bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTrustSource;

impl TrustSource for SystemTrustSource {
    fn load(&self) -> Result<LoadedTrust, TrustError> {
        let native = rustls_native_certs::load_native_certs();
        Ok(LoadedTrust {
            certificates_der: native
                .certs
                .into_iter()
                .map(|certificate| certificate.as_ref().to_vec())
                .collect(),
            source_description: "operating-system certificate enumeration".to_owned(),
            source_version: None,
            diagnostics: native
                .errors
                .into_iter()
                .map(|error| error.to_string())
                .collect(),
        })
    }
}

/// One immutable `BoringSSL` trust-store generation.
#[derive(Clone)]
pub struct TrustSnapshot {
    generation: u64,
    loaded_at: SystemTime,
    source_description: String,
    source_version: Option<String>,
    root_count: usize,
    store: Arc<boring::x509::store::X509Store>,
    diagnostics: Arc<[String]>,
}

impl fmt::Debug for TrustSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustSnapshot")
            .field("generation", &self.generation)
            .field("loaded_at", &self.loaded_at)
            .field("source_description", &self.source_description)
            .field("source_version", &self.source_version)
            .field("root_count", &self.root_count)
            .field("diagnostics", &self.diagnostics)
            .finish_non_exhaustive()
    }
}

impl TrustSnapshot {
    /// Parses source bytes with `BoringSSL` and creates a new immutable store.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] if the source cannot load, no valid roots remain,
    /// or the native trust-store builder fails.
    pub fn load(source: &dyn TrustSource, generation: u64) -> Result<Self, TrustError> {
        let loaded = source.load()?;
        let mut builder = X509StoreBuilder::new()?;
        let mut valid = 0_usize;
        let mut diagnostics = loaded.diagnostics;
        for (index, der) in loaded.certificates_der.into_iter().enumerate() {
            match X509::from_der(&der) {
                Ok(certificate) => match builder.add_cert(&certificate) {
                    Ok(()) => valid += 1,
                    Err(error) => diagnostics.push(format!(
                        "root {index} was rejected by the BoringSSL store: {error}"
                    )),
                },
                Err(error) => diagnostics.push(format!(
                    "root {index} is not valid DER according to BoringSSL: {error}"
                )),
            }
        }
        if valid == 0 {
            return Err(TrustError::EmptyStore {
                diagnostics,
                source_description: loaded.source_description,
            });
        }
        Ok(Self {
            generation,
            loaded_at: SystemTime::now(),
            source_description: loaded.source_description,
            source_version: loaded.source_version,
            root_count: valid,
            store: Arc::new(builder.build()),
            diagnostics: diagnostics.into(),
        })
    }

    /// Monotonic generation supplied by the owner during reload.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Capture time.
    pub fn loaded_at(&self) -> SystemTime {
        self.loaded_at
    }

    /// Source description suitable for operator evidence.
    pub fn source_description(&self) -> &str {
        &self.source_description
    }

    /// Source-specific version metadata, when available.
    pub fn source_version(&self) -> Option<&str> {
        self.source_version.as_deref()
    }

    /// Number of roots accepted into `BoringSSL`'s store.
    pub fn root_count(&self) -> usize {
        self.root_count
    }

    /// Enumeration and parsing diagnostics.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

/// Shared TLS policy used to construct H1/H2 and H3 contexts.
#[derive(Clone, Debug)]
pub struct UpstreamTlsPolicy {
    /// Minimum TLS version. TLS 1.2 is the conservative default.
    pub minimum_version: SslVersion,
    /// Maximum TLS version. TLS 1.3 is the default.
    pub maximum_version: SslVersion,
    /// TLS 1.2 cipher list; TLS 1.3 uses `BoringSSL`'s secure defaults.
    pub cipher_list: String,
}

impl Default for UpstreamTlsPolicy {
    fn default() -> Self {
        Self {
            minimum_version: SslVersion::TLS1_2,
            maximum_version: SslVersion::TLS1_3,
            cipher_list: "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384"
                .to_owned(),
        }
    }
}

/// Produces every upstream `BoringSSL` context from one snapshot and policy.
#[derive(Clone, Debug)]
pub struct UpstreamTlsContextFactory {
    snapshot: Arc<TrustSnapshot>,
    policy: UpstreamTlsPolicy,
}

impl UpstreamTlsContextFactory {
    /// Creates a factory. Replacing trust requires constructing a new factory and pools.
    pub fn new(snapshot: Arc<TrustSnapshot>, policy: UpstreamTlsPolicy) -> Self {
        Self { snapshot, policy }
    }

    /// Snapshot used by contexts from this factory.
    pub fn snapshot(&self) -> &Arc<TrustSnapshot> {
        &self.snapshot
    }

    /// Creates the connector used by Hyper's HTTP/1.1 and HTTP/2 adapter.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] when the configured TLS policy cannot be applied.
    pub fn hyper_connector(&self) -> Result<SslConnector, TrustError> {
        Ok(self.hyper_connector_builder(b"\x02h2\x08http/1.1")?.build())
    }

    /// Creates a connector builder for `hyper-boring` with explicit ALPN.
    ///
    /// `alpn_wire_format` is the TLS length-prefixed protocol list. Supplying
    /// it here keeps protocol selection and certificate policy on the same
    /// context before the adapter takes ownership of the builder.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] when the TLS policy or ALPN list cannot be
    /// applied.
    pub fn hyper_connector_builder(
        &self,
        alpn_wire_format: &[u8],
    ) -> Result<SslConnectorBuilder, TrustError> {
        let mut builder = SslConnector::builder(SslMethod::tls())?;
        self.configure(&mut builder)?;
        builder.set_alpn_protos(alpn_wire_format)?;
        Ok(builder)
    }

    /// Creates the context builder passed to quiche.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] when the configured TLS policy cannot be applied.
    pub fn quiche_context_builder(&self) -> Result<SslContextBuilder, TrustError> {
        let mut builder = SslContext::builder(SslMethod::tls())?;
        self.configure(&mut builder)?;
        Ok(builder)
    }

    fn configure(&self, builder: &mut SslContextBuilder) -> Result<(), TrustError> {
        builder.set_cert_store((*self.snapshot.store).clone());
        builder.set_verify_callback(SslVerifyMode::PEER, verify_peer_identity);
        builder.set_min_proto_version(Some(self.policy.minimum_version))?;
        builder.set_max_proto_version(Some(self.policy.maximum_version))?;
        builder.set_cipher_list(&self.policy.cipher_list)?;
        Ok(())
    }
}

fn verify_peer_identity(
    preverify_ok: bool,
    context: &mut boring::x509::X509StoreContextRef,
) -> bool {
    if preverify_ok {
        return true;
    }
    if context.verify_result() != Err(X509VerifyError::HOSTNAME_MISMATCH)
        || context.error_depth() != 0
    {
        return false;
    }
    let Ok(ssl_index) = X509StoreContext::ssl_idx() else {
        return false;
    };
    let Some(ssl) = context.ex_data(ssl_index) else {
        return false;
    };
    let Some(peer_name) = ssl.servername(NameType::HOST_NAME) else {
        return false;
    };
    if peer_name.parse::<IpAddr>().is_err() {
        return false;
    }
    context
        .current_cert()
        .is_some_and(|certificate| certificate.check_ip_asc(peer_name).unwrap_or(false))
}

/// Trust loading or `BoringSSL` context failure.
#[derive(Debug, Error)]
pub enum TrustError {
    /// `BoringSSL` rejected certificate material or policy.
    #[error("BoringSSL trust operation failed: {0}")]
    Boring(#[from] boring::error::ErrorStack),
    /// No valid roots were available, which is always a startup failure.
    #[error(
        "trust source {source_description:?} yielded no valid roots; diagnostics: {diagnostics:?}"
    )]
    EmptyStore {
        /// Diagnostic messages collected while loading.
        diagnostics: Vec<String>,
        /// Source that was empty.
        source_description: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Empty;

    impl TrustSource for Empty {
        fn load(&self) -> Result<LoadedTrust, TrustError> {
            Ok(LoadedTrust {
                certificates_der: Vec::new(),
                source_description: "empty-test".to_owned(),
                source_version: Some("1".to_owned()),
                diagnostics: Vec::new(),
            })
        }
    }

    #[test]
    fn empty_trust_is_a_hard_failure() {
        assert!(matches!(
            TrustSnapshot::load(&Empty, 1),
            Err(TrustError::EmptyStore { .. })
        ));
    }
}

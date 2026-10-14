use std::{
    fmt,
    net::IpAddr,
    sync::{Arc, OnceLock},
    time::SystemTime,
};

use boring::{
    pkey::{PKey, Private},
    ssl::{
        NameType, SslConnector, SslConnectorBuilder, SslContext, SslContextBuilder, SslMethod,
        SslVerifyMode, SslVersion,
    },
    x509::{X509, X509StoreContext, X509VerifyError, store::X509StoreBuilder},
};
use thiserror::Error;

type ContextTrustIndex = boring::ex_data::Index<SslContext, Arc<TrustSnapshot>>;
fn context_trust_index() -> Result<ContextTrustIndex, boring::error::ErrorStack> {
    static INDEX: OnceLock<Result<ContextTrustIndex, boring::error::ErrorStack>> = OnceLock::new();
    INDEX
        .get_or_init(SslContext::new_ex_index::<Arc<TrustSnapshot>>)
        .clone()
}

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

/// Immutable trust roots parsed from one operator-supplied PEM bundle.
#[derive(Clone, Debug)]
pub struct PemTrustSource {
    certificates_der: Arc<[Vec<u8>]>,
    source_description: Arc<str>,
}

impl PemTrustSource {
    /// Parses every certificate in a PEM bundle without changing system trust.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] when the PEM is malformed or contains no
    /// certificates.
    pub fn from_pem(pem: &[u8], source_description: impl Into<String>) -> Result<Self, TrustError> {
        if pem.iter().all(u8::is_ascii_whitespace) {
            return Err(TrustError::EmptyPemBundle);
        }
        let certificates = X509::stack_from_pem(pem)?;
        if certificates.is_empty() {
            return Err(TrustError::EmptyPemBundle);
        }
        let certificates_der = certificates
            .into_iter()
            .map(|certificate| certificate.to_der())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            certificates_der: certificates_der.into(),
            source_description: source_description.into().into(),
        })
    }
}

impl TrustSource for PemTrustSource {
    fn load(&self) -> Result<LoadedTrust, TrustError> {
        Ok(LoadedTrust {
            certificates_der: self.certificates_der.to_vec(),
            source_description: self.source_description.to_string(),
            source_version: None,
            diagnostics: Vec::new(),
        })
    }
}

/// Joins explicit trust sources into one immutable snapshot input.
#[derive(Clone)]
pub struct CompositeTrustSource {
    sources: Arc<[Arc<dyn TrustSource>]>,
    source_description: Arc<str>,
}

impl CompositeTrustSource {
    /// Creates a composite. An empty source list is allowed and will fail
    /// closed when loaded into a [`TrustSnapshot`].
    pub fn new(source_description: impl Into<String>, sources: Vec<Arc<dyn TrustSource>>) -> Self {
        Self {
            sources: sources.into(),
            source_description: source_description.into().into(),
        }
    }
}

impl TrustSource for CompositeTrustSource {
    fn load(&self) -> Result<LoadedTrust, TrustError> {
        let mut certificates_der = Vec::new();
        let mut diagnostics = Vec::new();
        let mut versions = Vec::new();
        for source in self.sources.iter() {
            let loaded = source.load()?;
            certificates_der.extend(loaded.certificates_der);
            diagnostics.extend(
                loaded
                    .diagnostics
                    .into_iter()
                    .map(|diagnostic| format!("{}: {diagnostic}", loaded.source_description)),
            );
            if let Some(version) = loaded.source_version {
                versions.push(format!("{}={version}", loaded.source_description));
            }
        }
        Ok(LoadedTrust {
            certificates_der,
            source_description: self.source_description.to_string(),
            source_version: (!versions.is_empty()).then(|| versions.join(",")),
            diagnostics,
        })
    }
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
    pub(crate) store: Arc<boring::x509::store::X509Store>,
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
    pub(crate) fn from_public_store(
        store: boring::x509::store::X509Store,
        generation: u64,
        source: String,
        version: i64,
        root_count: usize,
        excluded: usize,
    ) -> Self {
        Self {
            generation, loaded_at: SystemTime::now(), source_description: source,
            source_version: Some(version.to_string()), root_count, store: Arc::new(store),
            diagnostics: if excluded == 0 { Vec::new() } else {
                vec![format!("{excluded} anchors excluded: their Chromium constraints require verification policy unavailable in Transmog")]
            }.into(),
        }
    }
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
            // Match Chromium's default offer, including its CBC and static-RSA
            // compatibility suites. TLS 1.3 keeps BoringSSL's default order.
            cipher_list: "ALL:!aPSK:!ECDSA+SHA1:!3DES".to_owned(),
        }
    }
}

/// Produces every upstream `BoringSSL` context from one snapshot and policy.
#[derive(Clone, Debug)]
pub struct UpstreamTlsContextFactory {
    snapshot: Arc<TrustSnapshot>,
    policy: UpstreamTlsPolicy,
    profile: Option<transmog_core::network_profile::NetworkProfileSnapshot>,
    metadata: Arc<crate::trust_anchors::PkiMetadataProvider>,
    metadata_override: bool,
    connection_metadata: Option<Arc<crate::trust_anchors::PkiMetadata>>,
    baseline_metadata: Option<Arc<crate::trust_anchors::PkiMetadata>>,
    recovery_ids: Option<Vec<u8>>,
    client_authentication: Option<Arc<ClientAuthentication>>,
    pub(crate) authentication: Option<Arc<crate::client_connections::AuthenticationScope>>,
}

struct ClientAuthentication {
    certificate: X509,
    private_key: PKey<Private>,
    issuers: Vec<X509>,
}
impl fmt::Debug for ClientAuthentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientAuthentication")
            .field("certificate", &self.certificate)
            .field("issuer_count", &self.issuers.len())
            .finish_non_exhaustive()
    }
}

impl UpstreamTlsContextFactory {
    /// Creates a factory. Replacing trust requires constructing a new factory and pools.
    pub fn new(snapshot: Arc<TrustSnapshot>, policy: UpstreamTlsPolicy) -> Self {
        Self {
            snapshot,
            policy,
            profile: None,
            metadata: Arc::new(crate::trust_anchors::PkiMetadataProvider::new(
                crate::trust_anchors::PkiMetadata::bundled(),
                None,
            )),
            metadata_override: false,
            connection_metadata: None,
            baseline_metadata: None,
            recovery_ids: None,
            client_authentication: None,
            authentication: None,
        }
    }

    /// Resolves an immutable recipe for fresh TCP and QUIC contexts.
    /// # Errors
    /// Returns an incompatible protocol/version choice before constructing any transport.
    pub fn with_network_profile(
        mut self,
        profile: transmog_core::network_profile::NetworkProfileSnapshot,
    ) -> Result<Self, TrustError> {
        use transmog_core::network_profile::TlsVersions;
        profile
            .settings
            .validate()
            .map_err(TrustError::NetworkProfile)?;
        if !self.metadata_override {
            use transmog_core::network_profile::RootStore;
            self.metadata = match profile.settings.root_store {
                RootStore::Webview2Installed => {
                    crate::trust_anchors::PkiMetadataProvider::webview2()
                }
                RootStore::ChromeInstalled => {
                    crate::trust_anchors::PkiMetadataProvider::automatic()
                }
                RootStore::Os | RootStore::ChromiumBundled => {
                    Arc::new(crate::trust_anchors::PkiMetadataProvider::new(
                        crate::trust_anchors::PkiMetadata::bundled(),
                        None,
                    ))
                }
            };
        }
        self.baseline_metadata = if profile.settings.trust_anchor_ids
            || profile.settings.root_store != transmog_core::network_profile::RootStore::Os
        {
            profile
                .settings
                .pki_metadata_file
                .as_ref()
                .map(|path| {
                    crate::trust_anchors::PkiMetadata::from_file(std::path::Path::new(path))
                        .map(Arc::new)
                })
                .transpose()?
        } else {
            None
        };
        self.connection_metadata = None;
        self.recovery_ids = None;
        self.policy.minimum_version = match profile.settings.tls_versions {
            TlsVersions::Tls13Only => SslVersion::TLS1_3,
            _ => SslVersion::TLS1_2,
        };
        self.policy.maximum_version = match profile.settings.tls_versions {
            TlsVersions::Tls12Only => SslVersion::TLS1_2,
            _ => SslVersion::TLS1_3,
        };
        // Freeze the measured TLS 1.2 offer rather than expanding ALL when the
        // native library adds suites. TLS 1.3 keeps the measured native defaults.
        self.policy.cipher_list = if profile.settings.compatibility_ciphers {
            "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-RSA-AES128-SHA:ECDHE-RSA-AES256-SHA:AES128-GCM-SHA256:AES256-GCM-SHA384:AES128-SHA:AES256-SHA"
        } else {
            "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305"
        }.into();
        self.profile = Some(profile);
        Ok(self)
    }

    /// Recipe actually bound to contexts from this factory, if managed by profiles.
    pub fn network_profile(
        &self,
    ) -> Option<&transmog_core::network_profile::NetworkProfileSnapshot> {
        self.profile.as_ref()
    }

    /// Uses an explicitly loaded catalog and optional automatic discovery source.
    /// The network profile chooses whether this supplies trust or only ID metadata.
    #[must_use]
    pub fn with_pki_metadata(
        mut self,
        provider: Arc<crate::trust_anchors::PkiMetadataProvider>,
    ) -> Self {
        self.metadata = provider;
        self.metadata_override = true;
        self.connection_metadata = None;
        self.recovery_ids = None;
        self
    }

    /// Freezes current PKI metadata for one physical connection.
    #[must_use]
    pub fn for_connection(&self) -> Self {
        let mut factory = self.clone();
        if factory.connection_metadata.is_none() {
            factory.connection_metadata = Some(self.metadata());
        }
        factory
    }

    fn effective_trust(&self) -> Arc<TrustSnapshot> {
        if self
            .profile
            .as_ref()
            .is_some_and(|p| p.settings.root_store != transmog_core::network_profile::RootStore::Os)
        {
            self.metadata().trust()
        } else {
            self.snapshot.clone()
        }
    }

    fn metadata(&self) -> Arc<crate::trust_anchors::PkiMetadata> {
        self.connection_metadata.clone().unwrap_or_else(|| {
            let discovered = self.metadata.current();
            self.baseline_metadata
                .as_ref()
                .filter(|base| {
                    base.version >= discovered.version
                        || Arc::ptr_eq(&discovered, &crate::trust_anchors::PkiMetadata::bundled())
                })
                .cloned()
                .unwrap_or(discovered)
        })
    }

    /// Applies the frozen offer and untrusted helpers to fresh TCP SSL objects.
    /// # Errors
    /// Returns native configuration errors.
    pub fn configure_ssl_metadata(
        &self,
        ssl: &mut boring::ssl::SslRef,
    ) -> Result<(), boring::error::ErrorStack> {
        let trust = self.effective_trust();
        if ssl
            .ssl_context()
            .ex_data(context_trust_index()?)
            .is_some_and(|original| !Arc::ptr_eq(original, &trust))
        {
            // Hyper applies its cached session before this callback. A session
            // validated against an older root store must not bypass new trust.
            crate::client_connections::clear_cached_session(ssl)?;
        }
        ssl.set_verify_cert_store((*trust.store).clone())?;
        if self.trust_anchor_ids_enabled() {
            let metadata = self.metadata();
            transmog_tls_native::configure_ssl_trust_anchors(
                ssl,
                self.recovery_ids
                    .as_deref()
                    .unwrap_or_else(|| metadata.requested_ids()),
                metadata.issuers(),
            )?;
        }
        Ok(())
    }

    fn trust_anchor_ids_enabled(&self) -> bool {
        self.profile
            .as_ref()
            .is_none_or(|p| p.settings.trust_anchor_ids)
    }

    /// Allows one certificate-selection recovery before any HTTP bytes are sent.
    /// Only advertised server IDs that chain to configured trust are requested.
    pub fn trust_anchor_recovery(&self, capture: &crate::TlsCapture) -> Option<Self> {
        if !self.trust_anchor_ids_enabled() || self.recovery_ids.is_some() {
            return None;
        }
        let evidence = capture.snapshot();
        if !matches!(
            evidence.verification,
            Some(transmog_core::VerificationResult::Rejected { .. })
        ) {
            return None;
        }
        let metadata = self.metadata();
        let ids =
            metadata.recovery_ids(&capture.available_trust_anchors(), &self.effective_trust());
        if ids.is_empty() || ids == metadata.requested_ids() {
            return None;
        }
        let mut factory = self.for_connection();
        factory.recovery_ids = Some(ids);
        Some(factory)
    }

    /// Configures a client certificate and its ordered public issuer path for
    /// both Hyper and QUIC. Context construction validates the certificate/key
    /// pair; capture retains public certificates only.
    #[must_use]
    pub fn with_client_authentication(
        mut self,
        certificate: X509,
        private_key: PKey<Private>,
        issuers: Vec<X509>,
    ) -> Self {
        self.client_authentication = Some(Arc::new(ClientAuthentication {
            certificate,
            private_key,
            issuers,
        }));
        self
    }

    /// Configured OS/embedding snapshot, retained for the OS profile choice.
    /// Public-store profiles freeze their selected metadata trust per connection;
    /// capture evidence records that effective source and generation.
    pub fn snapshot(&self) -> &Arc<TrustSnapshot> {
        &self.snapshot
    }

    /// Creates connection-specific public evidence before attempting a handshake.
    pub fn capture(&self, server_name: Option<String>, alpn: Vec<String>) -> crate::TlsCapture {
        let trust = self.effective_trust();
        let label = |version| {
            if version == SslVersion::TLS1_2 {
                "TLS1.2".into()
            } else if version == SslVersion::TLS1_3 {
                "TLS1.3".into()
            } else {
                format!("{version:?}")
            }
        };
        let mut versions = vec![label(self.policy.minimum_version)];
        if self.policy.maximum_version != self.policy.minimum_version {
            versions.push(label(self.policy.maximum_version));
        }
        let capture = crate::TlsCapture::new(transmog_core::TlsEvidence {
            server_name,
            offered_versions: versions,
            offered_alpn: alpn,
            trust_generation: Some(trust.generation()),
            trust_source: Some(trust.source_description().to_owned()),
            network_profile: self.profile.clone(),
            ..transmog_core::TlsEvidence::default()
        });
        if self
            .profile
            .as_ref()
            .is_some_and(|p| p.settings.root_store != transmog_core::network_profile::RootStore::Os)
        {
            let metadata = self.metadata();
            capture.authentication_detail("Root store version", &metadata.version.to_string());
            capture.authentication_detail("Root store SHA-256", &metadata.sha256);
            capture.authentication_detail("Root store policy", "BoringSSL certificate verification; unsupported Chromium constraints exclude affected anchors");
            if let Some(diagnostic) = trust.diagnostics().first() {
                capture.authentication_detail("Root store exclusions", diagnostic);
            }
        }
        if self.trust_anchor_ids_enabled() {
            let metadata = self.metadata();
            capture.authentication_detail(
                "Trust Anchor IDs",
                "Offer configured; server negotiation not reached",
            );
            capture.authentication_detail("PKI metadata source", &metadata.source);
            capture.authentication_detail("PKI metadata version", &metadata.version.to_string());
            capture.authentication_detail("PKI metadata SHA-256", &metadata.sha256);
            let ids = self
                .recovery_ids
                .as_deref()
                .unwrap_or_else(|| metadata.requested_ids());
            let wire = crate::trust_anchors::hex(ids);
            capture.authentication_detail(
                "Requested Trust Anchor IDs",
                if wire.len() <= 2048 {
                    &wire
                } else {
                    "Wire list exceeds display limit; the complete list was supplied to TLS"
                },
            );
            if self.recovery_ids.is_some() {
                capture.authentication_detail(
                    "Trust Anchor ID recovery",
                    "One retry with IDs validated against configured trust; no HTTP data replayed",
                );
            }
        } else {
            capture.authentication_detail("Trust Anchor IDs", "Disabled by network profile");
        }
        if let Some(authentication) = &self.client_authentication {
            capture.client_issuers(
                std::iter::once(authentication.certificate.as_ref())
                    .chain(authentication.issuers.iter().map(std::ops::Deref::deref)),
            );
        }
        capture
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
        self.for_connection().configure_frozen(builder)
    }

    fn configure_frozen(&self, builder: &mut SslContextBuilder) -> Result<(), TrustError> {
        crate::evidence::configure_completion(builder);
        let settings = self.profile.as_ref().map_or_else(
            transmog_core::network_profile::NetworkProfileSettings::default,
            |profile| profile.settings.clone(),
        );
        if settings.ech_grease {
            crate::client_hello::enable_ech_grease(builder)?;
        }
        let trust = self.effective_trust();
        builder.set_cert_store((*trust.store).clone());
        builder.set_ex_data(context_trust_index()?, trust);
        builder.set_verify_callback(SslVerifyMode::PEER, |accepted, context| {
            let accepted = verify_peer_identity(accepted, context);
            crate::evidence::verification(context, accepted);
            accepted
        });
        if settings.trust_anchor_ids {
            let metadata = self.metadata();
            transmog_tls_native::configure_context_trust_anchors(
                builder,
                self.recovery_ids
                    .as_deref()
                    .unwrap_or_else(|| metadata.requested_ids()),
                metadata.issuers(),
            )?;
        }
        builder.set_min_proto_version(Some(self.policy.minimum_version))?;
        builder.set_max_proto_version(Some(self.policy.maximum_version))?;
        if settings.hybrid_key_exchange {
            crate::algorithms::configure_key_exchange(builder)?;
        } else {
            builder.set_curves_list("X25519:P-256:P-384")?;
        }
        crate::algorithms::configure_authentication(builder)?;
        builder.set_grease_enabled(
            settings.grease != transmog_core::network_profile::TlsGrease::Disabled,
        );
        builder.set_permute_extensions(settings.randomize_extensions);
        builder.enable_ocsp_stapling();
        builder.enable_signed_cert_timestamps();
        if settings.certificate_compression {
            builder.add_certificate_compression_algorithm(
                crate::certificate_compression::BrotliCertificateDecoder,
            )?;
        }
        transmog_tls_native::set_signature_algorithm_grease(
            builder,
            settings.grease == transmog_core::network_profile::TlsGrease::Chromium,
        );
        builder.set_cipher_list(&self.policy.cipher_list)?;
        if let Some(authentication) = &self.client_authentication {
            builder.set_certificate(&authentication.certificate)?;
            builder.set_private_key(&authentication.private_key)?;
            builder.check_private_key()?;
            for issuer in &authentication.issuers {
                builder.add_extra_chain_cert(issuer.clone())?;
            }
        }
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
    /// An explicitly selected PKI metadata file could not be loaded.
    #[error("{0}")]
    Metadata(#[from] crate::trust_anchors::MetadataError),
    /// Diagnostic profile combinations rejected before transport changes.
    #[error("{0}")]
    NetworkProfile(&'static str),
    /// `BoringSSL` rejected certificate material or policy.
    #[error("BoringSSL trust operation failed: {0}")]
    Boring(#[from] boring::error::ErrorStack),
    /// A syntactically valid PEM input contained no certificate blocks.
    #[error("PEM trust bundle contains no certificates")]
    EmptyPemBundle,
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

    #[test]
    fn pem_and_composite_sources_preserve_all_roots() {
        let first = crate::ProxyCa::generate("first trust root", 1).unwrap();
        let second = crate::ProxyCa::generate("second trust root", 1).unwrap();
        let first =
            Arc::new(PemTrustSource::from_pem(&first.certificate_pem().unwrap(), "first").unwrap())
                as Arc<dyn TrustSource>;
        let second = Arc::new(
            PemTrustSource::from_pem(&second.certificate_pem().unwrap(), "second").unwrap(),
        ) as Arc<dyn TrustSource>;
        let composite = CompositeTrustSource::new("test composite", vec![first, second]);

        let loaded = composite.load().unwrap();
        assert_eq!(loaded.certificates_der.len(), 2);
        assert_eq!(loaded.source_description, "test composite");
        assert!(loaded.diagnostics.is_empty());
        assert_eq!(TrustSnapshot::load(&composite, 7).unwrap().root_count(), 2);
    }

    #[test]
    fn pem_source_rejects_an_empty_bundle() {
        assert!(matches!(
            PemTrustSource::from_pem(b"", "empty"),
            Err(TrustError::EmptyPemBundle)
        ));
        assert!(matches!(
            PemTrustSource::from_pem(b" \r\n\t", "whitespace"),
            Err(TrustError::EmptyPemBundle)
        ));
    }
}

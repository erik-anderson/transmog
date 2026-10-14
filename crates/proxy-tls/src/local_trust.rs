//! Public Chromium trust with distinct, monitored Windows local certificate roles.
use crate::{TrustError, TrustSnapshot, trust_anchors::PkiMetadata};
use boring::{
    hash::{MessageDigest, hash},
    x509::{X509, store::X509StoreBuilder, verify::X509VerifyFlags},
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use transmog_tls_native::CertificatePolicy;
pub use transmog_windows_trust::{LocalCertificates, LocalTrustError, LocalTrustMonitor};

/// Pool admission identity. Public components and local notifications are independent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct TrustEpoch {
    /// Configured embedding trust generation.
    pub configured: u64,
    /// Public component major version (zero for OS trust).
    pub public: i64,
    /// Local notification generation (zero when the overlay is disabled).
    pub local: u64,
}
#[derive(Debug)]
pub(crate) struct EffectiveLocalTrust {
    pub trust: Arc<TrustSnapshot>,
    pub policy: Arc<CertificatePolicy>,
    pub inventory: Arc<LocalCertificates>,
    pub monitored: bool,
}
#[derive(Debug)]
struct CacheEntry {
    public_sha256: String,
    public_source: String,
    local_generation: u64,
    local_sha256: String,
    snapshot: Arc<EffectiveLocalTrust>,
}
type Cache = Option<CacheEntry>;
#[derive(Debug)]
pub(crate) struct LocalTrust {
    monitor: Result<LocalTrustMonitor, LocalTrustError>,
    cache: Mutex<Cache>,
}
impl LocalTrust {
    pub fn automatic() -> Arc<Self> {
        static VALUE: OnceLock<Arc<LocalTrust>> = OnceLock::new();
        VALUE
            .get_or_init(|| Arc::new(Self::new(LocalTrustMonitor::start())))
            .clone()
    }
    pub fn new(monitor: Result<LocalTrustMonitor, LocalTrustError>) -> Self {
        Self {
            monitor,
            cache: Mutex::new(None),
        }
    }
    pub fn generation(&self) -> u64 {
        self.monitor
            .as_ref()
            .map_or(1, LocalTrustMonitor::generation)
    }
    pub async fn ready(&self) -> Result<(), TrustError> {
        let monitor = self.monitor.clone()?;
        match monitor.current() {
            Ok(_) => Ok(()),
            Err(LocalTrustError::Refreshing) => {
                tokio::task::spawn_blocking(move || monitor.wait_ready(Duration::from_secs(5)))
                    .await
                    .map_err(|_| LocalTrustError::Worker)??;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
    pub fn current(&self, metadata: &PkiMetadata) -> Result<Arc<EffectiveLocalTrust>, TrustError> {
        let inventory = self.monitor.as_ref().map_err(Clone::clone)?.current()?;
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = &*cache
            && entry.public_sha256 == metadata.sha256
            && entry.public_source == metadata.source
            && entry.local_generation == inventory.generation
            && entry.local_sha256 == inventory.sha256
        {
            return Ok(entry.snapshot.clone());
        }
        let mut result = build(metadata, &inventory)?;
        // Enumeration may become dirty while certificates are parsed into a
        // native store. Do not publish that build as current after a notification.
        let current = self.monitor.as_ref().map_err(Clone::clone)?.current()?;
        if current.generation != inventory.generation || current.sha256 != inventory.sha256 {
            return Err(LocalTrustError::Refreshing.into());
        }
        result.monitored = self
            .monitor
            .as_ref()
            .is_ok_and(LocalTrustMonitor::is_monitored);
        let result = Arc::new(result);
        *cache = Some(CacheEntry {
            public_sha256: metadata.sha256.clone(),
            public_source: metadata.source.clone(),
            local_generation: inventory.generation,
            local_sha256: inventory.sha256.clone(),
            snapshot: result.clone(),
        });
        Ok(result)
    }
}
fn self_signed(cert: &X509) -> bool {
    cert.subject_name()
        .to_der()
        .ok()
        .zip(cert.issuer_name().to_der().ok())
        .is_some_and(|(subject, issuer)| subject == issuer)
        && cert
            .public_key()
            .is_ok_and(|key| cert.verify(&key).unwrap_or(false))
}
fn build(
    metadata: &PkiMetadata,
    inventory: &Arc<LocalCertificates>,
) -> Result<EffectiveLocalTrust, TrustError> {
    let blocked: HashSet<Vec<u8>> = inventory.disallowed.iter().cloned().collect();
    let mut anchors = Vec::new();
    let mut leaves = Vec::new();
    let mut issuers = Vec::new();
    let mut builder = X509StoreBuilder::new()?;
    builder.set_flags(X509VerifyFlags::PARTIAL_CHAIN);
    let mut seen = HashSet::new();
    for cert in metadata.trusted_anchors().iter() {
        let der = cert.to_der()?;
        if !blocked.contains(&der) && seen.insert(der) {
            builder.add_cert(cert)?;
            anchors.push(cert.clone());
        }
    }
    for der in &inventory.anchors {
        if blocked.contains(der) {
            continue;
        }
        let Ok(cert) = X509::from_der(der) else {
            continue;
        };
        if transmog_tls_native::certificate_is_ca(&cert) {
            if seen.insert(der.clone()) {
                builder.add_cert(&cert)?;
                anchors.push(cert);
            }
        } else if self_signed(&cert) {
            leaves.push(cert);
        }
    }
    for der in &inventory.trusted_leaves {
        if blocked.contains(der) {
            continue;
        }
        if let Ok(cert) = X509::from_der(der)
            && self_signed(&cert)
        {
            leaves.push(cert);
        }
    }
    // Exact self-signed leaf trust needs ordinary store lookup (BoringSSL's
    // PARTIAL_CHAIN exact-match path). The verification callback rejects these
    // certificates at issuer depths unless they also have a normal ROOT role.
    for cert in &leaves {
        if seen.insert(cert.to_der()?) {
            builder.add_cert(cert)?;
        }
    }
    for cert in metadata.issuers().iter() {
        if !blocked.contains(&cert.to_der()?) {
            issuers.push(cert.clone());
        }
    }
    for der in &inventory.intermediates {
        if !blocked.contains(der)
            && let Ok(cert) = X509::from_der(der)
        {
            issuers.push(cert);
        }
    }
    let digest = hash(
        MessageDigest::sha256(),
        format!(
            "{}:{}:{}",
            metadata.sha256, inventory.generation, inventory.sha256
        )
        .as_bytes(),
    )?;
    let mut generation_bytes = [0; 8];
    generation_bytes.copy_from_slice(&digest[..8]);
    let generation = u64::from_be_bytes(generation_bytes) & 0x001f_ffff_ffff_ffff;
    let root_count = anchors.len();
    let mut trust = TrustSnapshot::from_public_store(
        builder.build(),
        generation,
        source_description(&metadata.source),
        metadata.version,
        root_count,
        0,
    );
    trust.diagnostics = metadata.trust().diagnostics.clone();
    Ok(EffectiveLocalTrust {
        trust: Arc::new(trust),
        inventory: inventory.clone(),
        monitored: false,
        policy: Arc::new(CertificatePolicy {
            unavailable: false,
            issuers: Arc::new(issuers),
            anchors: Arc::new(
                anchors
                    .iter()
                    .map(|cert| cert.to_der())
                    .collect::<Result<_, _>>()?,
            ),
            local_anchors: Arc::new(inventory.anchors.iter().cloned().collect()),
            trusted_leaves: Arc::new(leaves),
            disallowed: Arc::new(blocked),
        }),
    })
}

fn source_description(source: &str) -> String {
    const SUFFIX: &str = " + Windows locally installed certificates";
    if source.len() + SUFFIX.len() <= 1024 {
        return format!("{source}{SUFFIX}");
    }
    let mut end = 1024 - SUFFIX.len() - 3;
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...{SUFFIX}", &source[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_public_bytes_keep_distinct_sources_and_bound_utf8_capture_labels() {
        let metadata = PkiMetadata::bundled();
        let local = LocalTrust::new(Ok(LocalTrustMonitor::fixed(LocalCertificates::default())));
        let first = local.current(&metadata).unwrap();
        let mut other = (*metadata).clone();
        other.source = "Installed Chrome root-store fixture".into();
        let second = local.current(&other).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(second.trust.source_description().starts_with(&other.source));
        other.source = "界".repeat(341) + "x";
        let long = local.current(&other).unwrap();
        assert!(long.trust.source_description().len() <= 1024);
        assert!(
            long.trust
                .source_description()
                .ends_with("Windows locally installed certificates")
        );
        assert_eq!(
            first.trust.source_description(),
            source_description(&metadata.source)
        );
    }
}

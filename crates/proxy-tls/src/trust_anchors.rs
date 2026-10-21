//! Certificate-based Chrome Trust Anchor IDs and selectable public root stores.
use boring::{hash::MessageDigest, x509::X509};
use prost::Message;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
    time::Duration,
};
use thiserror::Error;

const BUNDLED: &[u8] = include_bytes!("../../../third_party/chromium-root-store/crs.pb");
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CERTIFICATES: usize = 1024;

// TLS projection of the checked-in Chromium schema. Preflight bounds repeated
// fields and rejects unknown policy; known non-TLS fields are ignored by prost.
// Unsupported Chrome CT constraints exclude anchors from our trusted store.
#[derive(Clone, PartialEq, Message)]
struct RootStore {
    #[prost(message, repeated, tag = "1")]
    trust_anchors: Vec<Anchor>,
    #[prost(int64, tag = "2")]
    version_major: i64,
    #[prost(message, repeated, tag = "3")]
    additional_certs: Vec<Anchor>,
}
#[derive(Clone, PartialEq, Message)]
#[allow(clippy::struct_field_names)] // Mirrors the external Chromium protobuf schema.
struct Anchor {
    #[prost(oneof = "Certificate", tags = "1, 2")]
    certificate: Option<Certificate>,
    #[prost(message, repeated, tag = "4")]
    constraints: Vec<ConstraintSet>,
    #[prost(bool, optional, tag = "8")]
    enforce_anchor_expiry: Option<bool>,
    #[prost(bool, optional, tag = "9")]
    enforce_anchor_constraints: Option<bool>,
    #[prost(bool, optional, tag = "10")]
    tls_trust_anchor: Option<bool>,
    #[prost(bytes, optional, tag = "11")]
    trust_anchor_id: Option<Vec<u8>>,
}
#[derive(Clone, PartialEq, Message)]
struct ConstraintSet {
    #[prost(int64, optional, tag = "1")]
    sct_not_after_sec: Option<i64>,
    #[prost(int64, optional, tag = "2")]
    sct_all_after_sec: Option<i64>,
    #[prost(string, optional, tag = "3")]
    min_version: Option<String>,
    #[prost(string, optional, tag = "4")]
    max_version_exclusive: Option<String>,
    #[prost(string, repeated, tag = "5")]
    permitted_dns_names: Vec<String>,
    #[prost(uint64, optional, tag = "8")]
    serial_not_after: Option<u64>,
    #[prost(uint64, optional, tag = "9")]
    serial_after: Option<u64>,
    #[prost(int64, optional, tag = "10")]
    validity_starts_not_after_sec: Option<i64>,
    #[prost(int64, optional, tag = "11")]
    validity_starts_after_sec: Option<i64>,
}
impl ConstraintSet {
    fn to_native(&self) -> Result<transmog_tls_native::RootConstraint, MetadataError> {
        if [&self.min_version, &self.max_version_exclusive]
            .iter()
            .any(|value| {
                value
                    .as_ref()
                    .is_some_and(|value| component_version(value).is_none())
            })
        {
            return Err(MetadataError::Invalid("constraint Chromium version"));
        }
        Ok(transmog_tls_native::RootConstraint {
            version_allowed: self.version_allowed_for_reference(),
            validity_not_after: self.validity_starts_not_after_sec,
            validity_after: self.validity_starts_after_sec,
            sct_not_after: self.sct_not_after_sec,
            sct_all_after: self.sct_all_after_sec,
            permitted_dns_names: self.permitted_dns_names.clone(),
        })
    }
    fn version_allowed_for_reference(&self) -> bool {
        let Some(reference) =
            component_version(transmog_core::network_profile::CHROMIUM_REFERENCE_VERSION)
        else {
            return false;
        };
        let version_allowed = |value: &Option<String>, minimum: bool| {
            value.as_ref().is_none_or(|v| {
                component_version(v).is_some_and(|v| {
                    if minimum {
                        reference >= v
                    } else {
                        reference < v
                    }
                })
            })
        };
        version_allowed(&self.min_version, true)
            && version_allowed(&self.max_version_exclusive, false)
    }
}
#[derive(Clone, PartialEq, prost::Oneof)]
enum Certificate {
    #[prost(bytes, tag = "1")]
    Der(Vec<u8>),
    #[prost(string, tag = "2")]
    Sha256(String),
}

/// A parsed, immutable public certificate catalog and its provenance.
#[derive(Clone, Debug)]
pub struct PkiMetadata {
    /// Chrome Root Store major version, used for freshness (not CRX version/mtime).
    pub version: i64,
    /// SHA-256 of the exact input protobuf.
    pub sha256: String,
    /// Public file location or bundled-source description.
    pub source: String,
    ids: Vec<(Vec<u8>, X509)>,
    issuers: Arc<Vec<X509>>,
    wire: Vec<u8>,
    trust: Arc<crate::TrustSnapshot>,
    trusted_anchors: Arc<Vec<X509>>,
    policy: Arc<transmog_tls_native::CertificatePolicy>,
}

/// Invalid or inaccessible metadata. Automatic discovery retains the last good copy.
#[derive(Debug, Error)]
pub enum MetadataError {
    /// File access failed.
    #[error("PKI metadata file: {0}")]
    Io(#[from] std::io::Error),
    /// The protobuf or its certificate catalog was not usable.
    #[error("Invalid PKI metadata: {0}")]
    Invalid(&'static str),
}

impl PkiMetadata {
    /// Loads a bounded `PKIMetadata` `crs.pb`, including a separate public trust store.
    /// # Errors
    /// Rejects malformed protobufs, missing DER, invalid CA certificates or bounds.
    pub fn from_file(path: &Path) -> Result<Self, MetadataError> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        Self::decode(&bytes, path.to_string_lossy().into_owned())
    }

    /// Decodes the public `RootStore` protobuf described by Chromium's schema.
    /// # Errors
    /// Rejects malformed or oversized inputs and invalid certificate/ID material.
    pub fn decode(bytes: &[u8], source: String) -> Result<Self, MetadataError> {
        let store = decode_store(bytes, &source)?;
        let mut ids = Vec::new();
        let mut issuers = Vec::new();
        let mut trusted_anchors = Vec::new();
        let mut anchor_only = std::collections::HashSet::new();
        let mut constrained_anchors = std::collections::HashSet::new();
        let mut ignore_anchor_expiry = std::collections::HashSet::new();
        let mut root_constraints = std::collections::HashMap::new();
        let mut declared = std::collections::HashMap::new();
        for (anchor, tls) in store.trust_anchors.iter().map(|a| (a, true)).chain(
            store
                .additional_certs
                .iter()
                .map(|a| (a, a.tls_trust_anchor == Some(true))),
        ) {
            let cert = parse_ca(anchor)?;
            let groups = anchor
                .constraints
                .iter()
                .map(ConstraintSet::to_native)
                .collect::<Result<Vec<_>, _>>()?;
            if tls {
                trusted_anchors.push(cert.clone());
                let der = cert
                    .to_der()
                    .map_err(|_| MetadataError::Invalid("anchor DER"))?;
                let flags = (
                    anchor.enforce_anchor_expiry == Some(true),
                    anchor.enforce_anchor_constraints == Some(true),
                    groups.clone(),
                );
                if declared
                    .insert(der.clone(), flags.clone())
                    .is_some_and(|prior| prior != flags)
                {
                    return Err(MetadataError::Invalid(
                        "conflicting duplicate anchor policies",
                    ));
                }
                anchor_only.insert(der.clone());
                if anchor.enforce_anchor_constraints == Some(true) {
                    constrained_anchors.insert(der.clone());
                }
                if anchor.enforce_anchor_expiry != Some(true) {
                    ignore_anchor_expiry.insert(der.clone());
                }
                if !anchor.constraints.is_empty() {
                    root_constraints.insert(der, groups);
                }
                issuers.push(cert.clone());
                if let Some(id) = &anchor.trust_anchor_id {
                    if id.is_empty() || id.len() > 32 || !valid_relative_oid(id) {
                        return Err(MetadataError::Invalid("Trust Anchor ID"));
                    }
                    ids.push((id.clone(), cert));
                }
            }
        }
        if issuers.is_empty() {
            return Err(MetadataError::Invalid("no TLS CA certificates"));
        }
        // Chromium sorts raw identifiers and preserves duplicate entries.
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        let wire = encode_ids(ids.iter().map(|(id, _)| id.as_slice()));
        if wire.len() > 65_533 {
            return Err(MetadataError::Invalid("TLS vector size"));
        }
        let digest = boring::hash::hash(MessageDigest::sha256(), bytes)
            .map_err(|_| MetadataError::Invalid("digest"))?;
        let sha256 = hex(&digest);
        let generation = u64::from_be_bytes(
            digest
                .get(..8)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(MetadataError::Invalid("digest size"))?,
        ) & 0x001f_ffff_ffff_ffff;
        let trust = Arc::new(crate::trust::TrustSnapshot::from_public_roots(
            generation,
            source.clone(),
            store.version_major,
            0,
            Arc::new(trusted_anchors.clone()),
        ));
        let policy = Arc::new(make_policy(
            &issuers,
            anchor_only,
            constrained_anchors,
            ignore_anchor_expiry,
            root_constraints,
        )?);
        Ok(Self {
            version: store.version_major,
            source,
            sha256,
            ids,
            issuers: Arc::new(issuers),
            wire,
            trust,
            trusted_anchors: Arc::new(trusted_anchors),
            policy,
        })
    }

    /// Reproducible public Chromium source snapshot matching the measured offer.
    /// # Panics
    /// Panics if the compiled-in, qualified snapshot cannot be decoded.
    #[must_use]
    pub fn bundled() -> Arc<Self> {
        static VALUE: OnceLock<Arc<PkiMetadata>> = OnceLock::new();
        VALUE
            .get_or_init(|| {
                Arc::new(
                    Self::decode(
                        BUNDLED,
                        "Bundled Chromium root-store source (version 41)".into(),
                    )
                    .expect("qualified bundled Chrome metadata"),
                )
            })
            .clone()
    }

    /// TLS list without the outer u16 length (added by `BoringSSL`).
    pub fn requested_ids(&self) -> &[u8] {
        &self.wire
    }
    /// Number of certificate-based identifiers in the Chrome offer.
    pub fn id_count(&self) -> usize {
        self.ids.len()
    }

    pub(crate) fn issuers(&self) -> Arc<Vec<X509>> {
        self.issuers.clone()
    }
    pub(crate) fn trust(&self) -> Arc<crate::TrustSnapshot> {
        self.trust.clone()
    }
    pub(crate) fn trusted_anchors(&self) -> Arc<Vec<X509>> {
        self.trusted_anchors.clone()
    }

    pub(crate) fn certificate_policy(&self) -> Arc<transmog_tls_native::CertificatePolicy> {
        self.policy.clone()
    }

    /// Recovery requests only IDs whose associated CA chains to configured trust.
    /// The peer's decreasing preference order is retained, with no invented IDs.
    pub(crate) fn recovery_ids(
        &self,
        available: &[u8],
        policy: &Arc<transmog_tls_native::CertificatePolicy>,
    ) -> Vec<u8> {
        let mut selected = Vec::new();
        let mut remaining = available;
        while let Some((&length, tail)) = remaining.split_first() {
            let length = usize::from(length);
            if length == 0 || length > tail.len() {
                return Vec::new();
            }
            let id = &tail[..length];
            remaining = &tail[length..];
            let Some((_, certificate)) = self.ids.iter().find(|(known, _)| known == id) else {
                continue;
            };
            if transmog_tls_native::verify_certificate_authority(policy, certificate) {
                selected.extend_from_slice(&tail[..length]);
                let start = selected.len() - length;
                selected.insert(start, u8::try_from(length).expect("validated identifier"));
            }
        }
        selected
    }
}

fn make_policy(
    issuers: &[X509],
    anchor_only: std::collections::HashSet<Vec<u8>>,
    constrained_anchors: std::collections::HashSet<Vec<u8>>,
    ignore_anchor_expiry: std::collections::HashSet<Vec<u8>>,
    root_constraints: std::collections::HashMap<Vec<u8>, Vec<transmog_tls_native::RootConstraint>>,
) -> Result<transmog_tls_native::CertificatePolicy, MetadataError> {
    Ok(transmog_tls_native::CertificatePolicy {
        root_constraints: Arc::new(root_constraints),
        known_public_roots: Arc::new(
            issuers
                .iter()
                .map(|cert| cert.to_der())
                .collect::<Result<_, _>>()
                .map_err(|_| MetadataError::Invalid("public root identity"))?,
        ),
        anchors: Arc::new(anchor_only.clone()),
        anchor_only: Arc::new(anchor_only),
        constrained_anchors: Arc::new(constrained_anchors),
        ignore_anchor_expiry: Arc::new(ignore_anchor_expiry),
        issuers: Arc::new(issuers.to_vec()),
        ..Default::default()
    })
}

fn decode_store(bytes: &[u8], source: &str) -> Result<RootStore, MetadataError> {
    if bytes.len() as u64 > MAX_BYTES || source.len() > 1024 {
        return Err(MetadataError::Invalid("size limit"));
    }
    preflight_store(bytes)?;
    let store = RootStore::decode(bytes).map_err(|_| MetadataError::Invalid("protobuf"))?;
    if store.version_major <= 0
        || store.trust_anchors.len() + store.additional_certs.len() == 0
        || store.trust_anchors.len() + store.additional_certs.len() > MAX_CERTIFICATES
    {
        return Err(MetadataError::Invalid("version or certificate count"));
    }
    if store
        .trust_anchors
        .iter()
        .any(|anchor| anchor.tls_trust_anchor.is_some())
    {
        return Err(MetadataError::Invalid(
            "trust_anchors must not override the TLS role",
        ));
    }
    Ok(store)
}

fn parse_ca(anchor: &Anchor) -> Result<X509, MetadataError> {
    let Some(Certificate::Der(der)) = &anchor.certificate else {
        return Err(MetadataError::Invalid(
            "DER required; textproto hashes are not certificates",
        ));
    };
    if der.is_empty() || der.len() > 65_536 {
        return Err(MetadataError::Invalid("certificate size"));
    }
    if !transmog_tls_native::certificate_is_parseable(der) {
        return Err(MetadataError::Invalid("PKI certificate DER"));
    }
    let cert = X509::from_der(der).map_err(|_| MetadataError::Invalid("certificate DER"))?;
    if !transmog_tls_native::certificate_is_ca(&cert) {
        return Err(MetadataError::Invalid("non-CA certificate"));
    }
    Ok(cert)
}

fn preflight_store(bytes: &[u8]) -> Result<(), MetadataError> {
    // Bound repeated-message allocation before prost materializes the catalog.
    let mut remaining = bytes;
    let mut certificates = 0;
    let mut fields = 0;
    while !remaining.is_empty() {
        let (tag, wire) = prost::encoding::decode_key(&mut remaining)
            .map_err(|_| MetadataError::Invalid("protobuf key"))?;
        if !matches!(tag, 1..=3) {
            return Err(MetadataError::Invalid("unsupported root-store fields"));
        }
        fields += 1;
        if matches!(tag, 1 | 3) {
            certificates += 1;
            if wire != prost::encoding::WireType::LengthDelimited {
                return Err(MetadataError::Invalid("certificate message wire type"));
            }
            let mut message = remaining;
            let length = usize::try_from(
                prost::encoding::decode_varint(&mut message)
                    .map_err(|_| MetadataError::Invalid("certificate message length"))?,
            )
            .map_err(|_| MetadataError::Invalid("certificate message length"))?;
            preflight_anchor(
                message
                    .get(..length)
                    .ok_or(MetadataError::Invalid("certificate message"))?,
            )?;
        }
        if certificates > MAX_CERTIFICATES || fields > 8192 {
            return Err(MetadataError::Invalid("protobuf field count"));
        }
        prost::encoding::skip_field(
            wire,
            tag,
            &mut remaining,
            prost::encoding::DecodeContext::default(),
        )
        .map_err(|_| MetadataError::Invalid("protobuf field"))?;
    }
    Ok(())
}

fn preflight_anchor(mut bytes: &[u8]) -> Result<(), MetadataError> {
    let mut fields = 0;
    let mut constraints = 0;
    while !bytes.is_empty() {
        let (tag, wire) = prost::encoding::decode_key(&mut bytes)
            .map_err(|_| MetadataError::Invalid("anchor field"))?;
        fields += 1;
        if fields > 512 || !matches!(tag, 1..=6 | 8..=12) {
            return Err(MetadataError::Invalid("unsupported anchor fields"));
        }
        if tag == 4 {
            constraints += 1;
            if constraints > 64 || wire != prost::encoding::WireType::LengthDelimited {
                return Err(MetadataError::Invalid("constraint count or wire type"));
            }
            let mut message = bytes;
            let length = usize::try_from(
                prost::encoding::decode_varint(&mut message)
                    .map_err(|_| MetadataError::Invalid("constraint length"))?,
            )
            .map_err(|_| MetadataError::Invalid("constraint length"))?;
            let mut constraint = message
                .get(..length)
                .ok_or(MetadataError::Invalid("constraint"))?;
            let mut count = 0;
            while !constraint.is_empty() {
                let (tag, wire) = prost::encoding::decode_key(&mut constraint)
                    .map_err(|_| MetadataError::Invalid("constraint field"))?;
                count += 1;
                if count > 256 || !matches!(tag, 1..=5 | 8..=11) {
                    return Err(MetadataError::Invalid("unsupported constraint fields"));
                }
                prost::encoding::skip_field(
                    wire,
                    tag,
                    &mut constraint,
                    prost::encoding::DecodeContext::default(),
                )
                .map_err(|_| MetadataError::Invalid("constraint field"))?;
            }
        }
        prost::encoding::skip_field(
            wire,
            tag,
            &mut bytes,
            prost::encoding::DecodeContext::default(),
        )
        .map_err(|_| MetadataError::Invalid("anchor field"))?;
    }
    Ok(())
}

fn valid_relative_oid(id: &[u8]) -> bool {
    let mut first = true;
    for &byte in id {
        if first && byte == 0x80 {
            return false;
        }
        first = byte & 0x80 == 0;
    }
    first
}
fn encode_ids<'a>(ids: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    ids.flat_map(|id| {
        std::iter::once(u8::try_from(id.len()).expect("bounded ID")).chain(id.iter().copied())
    })
    .collect()
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Shared last-good metadata, refreshed off transport threads once per minute.
/// Established TLS/QUIC connections keep their immutable original snapshot.
#[derive(Debug)]
pub struct PkiMetadataProvider {
    directory: Option<PathBuf>,
    current: RwLock<Arc<PkiMetadata>>,
}
impl PkiMetadataProvider {
    /// Creates a resolver with an explicit fallback and optional component directory.
    #[must_use]
    pub fn new(fallback: Arc<PkiMetadata>, directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            current: RwLock::new(fallback),
        }
    }
    /// Last validated catalog; this operation does no filesystem IO.
    pub fn current(&self) -> Arc<PkiMetadata> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// Scans numeric Chrome component directories, selecting a strictly newer
    /// `RootStore` version. Corrupt, partial, old, or missing files retain last-good.
    /// Invoke outside latency-sensitive async transport work.
    pub fn refresh(&self) {
        let Some(directory) = &self.directory else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut candidates = Vec::new();
        for entry in entries.take(4096).flatten() {
            let name = entry.file_name();
            let Some(version) = name.to_str().and_then(component_version) else {
                continue;
            };
            candidates.push((version, entry.path().join("crs.pb")));
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        let mut best = self.current();
        for (_, path) in candidates.into_iter().take(128) {
            if let Ok(value) = PkiMetadata::from_file(&path)
                && value.version > best.version
            {
                best = Arc::new(value);
            }
        }
        let mut current = self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if best.version > current.version {
            *current = best;
        }
    }
    /// Windows Chrome discovery with a bundled fallback. No updater downloads.
    pub fn automatic() -> Arc<Self> {
        static VALUE: OnceLock<Arc<PkiMetadataProvider>> = OnceLock::new();
        VALUE
            .get_or_init(|| {
                Self::discover("Google/Chrome/User Data/PKIMetadata", "chrome-pki-metadata")
            })
            .clone()
    }
    /// Installed `WebView2` discovery with the same public Chromium fallback.
    pub fn webview2() -> Arc<Self> {
        static VALUE: OnceLock<Arc<PkiMetadataProvider>> = OnceLock::new();
        VALUE
            .get_or_init(|| {
                Self::discover(
                    "Microsoft/Windows/SharedWebView/EBWebView/PKIMetadata",
                    "webview2-pki-metadata",
                )
            })
            .clone()
    }
    fn discover(relative_path: &str, worker_name: &str) -> Arc<Self> {
        let directory = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join(relative_path))
        } else {
            None
        };
        let provider = Arc::new(Self::new(PkiMetadata::bundled(), directory));
        let owner = Arc::downgrade(&provider);
        // First discovery runs immediately in this worker, without blocking UI
        // or making filesystem reads in SSL callbacks. Failure to spawn leaves
        // the bundled snapshot usable.
        let _ = std::thread::Builder::new()
            .name(worker_name.into())
            .spawn(move || {
                loop {
                    let Some(provider) = owner.upgrade() else {
                        break;
                    };
                    provider.refresh();
                    drop(provider);
                    std::thread::sleep(Duration::from_secs(60));
                }
            });
        provider
    }
}
fn component_version(name: &str) -> Option<Vec<u32>> {
    if name.len() > 64 {
        return None;
    }
    let version = name
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        })
        .collect::<Option<Vec<_>>>()?;
    (version.len() <= 4).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_respects_local_anchor_path_length_for_the_candidate_ca() {
        use crate::local_trust::{LocalCertificates, LocalTrust, LocalTrustMonitor};
        use boring::{pkey::PKey, x509::X509NameBuilder};
        let template = crate::ProxyCa::generate("limited recovery root", 2).unwrap();
        let root_key =
            PKey::private_key_from_pem(&template.private_key_pem_pkcs8().unwrap()).unwrap();
        for limit in [0, 1] {
            let root = recovery_ca(
                template.certificate().subject_name(),
                &root_key,
                template.certificate().subject_name(),
                &root_key,
                400,
                Some(limit),
            );
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", "recovery intermediate")
                .unwrap();
            let issuer_key = PKey::private_key_from_pem(
                &crate::ProxyCa::generate("issuer key", 2)
                    .unwrap()
                    .private_key_pem_pkcs8()
                    .unwrap(),
            )
            .unwrap();
            let issuer = recovery_ca(
                &name.build(),
                &issuer_key,
                root.subject_name(),
                &root_key,
                401,
                None,
            );
            let mut store = RootStore::decode(encoded(42).as_slice()).unwrap();
            store.trust_anchors.push(Anchor {
                certificate: Some(Certificate::Der(issuer.to_der().unwrap())),
                constraints: vec![ConstraintSet {
                    sct_not_after_sec: Some(1),
                    ..Default::default()
                }],
                tls_trust_anchor: None,
                trust_anchor_id: Some(vec![3]),
                enforce_anchor_expiry: None,
                enforce_anchor_constraints: None,
            });
            let metadata = PkiMetadata::decode(
                &store.encode_to_vec(),
                "recovery path length fixture".into(),
            )
            .unwrap();
            let local = LocalTrust::new(Ok(LocalTrustMonitor::fixed(LocalCertificates {
                anchors: vec![root.to_der().unwrap()],
                ..Default::default()
            })));
            let effective = local.current(&metadata, true).unwrap();
            let selected = metadata.recovery_ids(&[1, 3], &effective.policy);
            assert_eq!(selected, if limit == 0 { vec![] } else { vec![1, 3] });
        }
    }
    fn recovery_ca(
        subject: &boring::x509::X509NameRef,
        key: &boring::pkey::PKey<boring::pkey::Private>,
        issuer: &boring::x509::X509NameRef,
        signer: &boring::pkey::PKey<boring::pkey::Private>,
        serial: u32,
        path_length: Option<u32>,
    ) -> X509 {
        use boring::{
            asn1::Asn1Time,
            bn::BigNum,
            hash::MessageDigest,
            x509::extension::{BasicConstraints, KeyUsage},
        };
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        cert.set_serial_number(&BigNum::from_u32(serial).unwrap().to_asn1_integer().unwrap())
            .unwrap();
        cert.set_subject_name(subject).unwrap();
        cert.set_issuer_name(issuer).unwrap();
        cert.set_pubkey(key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        let mut constraints = BasicConstraints::new();
        constraints.critical().ca();
        if let Some(limit) = path_length {
            constraints.pathlen(limit);
        }
        cert.append_extension(&constraints.build().unwrap())
            .unwrap();
        cert.append_extension(&KeyUsage::new().critical().key_cert_sign().build().unwrap())
            .unwrap();
        cert.sign(signer, MessageDigest::sha256()).unwrap();
        cert.build()
    }
    #[test]
    fn recovery_does_not_bypass_root_version_limits_via_locally_trusted_leaves() {
        use crate::local_trust::{LocalCertificates, LocalTrust, LocalTrustMonitor};
        let mut store = RootStore::decode(encoded(42).as_slice()).unwrap();
        let restricted = match &store.trust_anchors[0].certificate {
            Some(Certificate::Der(der)) => der.clone(),
            _ => panic!("DER fixture"),
        };
        store.trust_anchors[0].constraints.push(ConstraintSet {
            max_version_exclusive: Some("153.0.8010.0".into()),
            ..Default::default()
        });
        let usable_ca = crate::ProxyCa::generate("Usable public recovery root", 2).unwrap();
        let usable = Anchor {
            certificate: Some(Certificate::Der(usable_ca.certificate().to_der().unwrap())),
            constraints: vec![],
            tls_trust_anchor: None,
            trust_anchor_id: Some(vec![3]),
            enforce_anchor_expiry: None,
            enforce_anchor_constraints: None,
        };
        store.trust_anchors.push(usable);
        let metadata =
            PkiMetadata::decode(&store.encode_to_vec(), "recovery role fixture".into()).unwrap();
        let local = LocalTrust::new(Ok(LocalTrustMonitor::fixed(LocalCertificates {
            trusted_leaves: vec![restricted],
            ..Default::default()
        })));
        let effective = local.current(&metadata, true).unwrap();
        assert!(
            metadata
                .recovery_ids(&[2, 1, 2], &effective.policy)
                .is_empty()
        );
    }
    fn encoded(version: i64) -> Vec<u8> {
        let ca = crate::ProxyCa::generate("metadata-only CA", 2).unwrap();
        RootStore {
            version_major: version,
            trust_anchors: vec![Anchor {
                certificate: Some(Certificate::Der(ca.certificate().to_der().unwrap())),
                tls_trust_anchor: None,
                trust_anchor_id: Some(vec![1, 2]),
                constraints: vec![],
                enforce_anchor_expiry: None,
                enforce_anchor_constraints: None,
            }],
            additional_certs: vec![],
        }
        .encode_to_vec()
    }
    #[test]
    fn bundle_has_the_exact_measured_chromium_certificate_based_offer() {
        let metadata = PkiMetadata::bundled();
        assert_eq!(metadata.version, 41);
        assert_eq!(metadata.id_count(), 28);
        assert_eq!(metadata.requested_ids().len(), 184);
        // Independent original Chrome ClientHello extension bytes (without
        // outer u16 length). This checks sorting, role filtering and exact IDs.
        let expected = include_str!("../../../third_party/chromium-root-store/snapshot.json");
        assert!(expected.contains(&metadata.sha256));
        let chrome = "0582df1302010582df1302060582df13020d0582df13020e0582df13020f0582df1302120582df1302130582df13021408839a648c9b2d010708839a648c9b2d010808839a648c9b2d010908839a648c9b2d010a08839a648c9b2d010b08839a648c9b2d010c08839a648c9b2d010d08839a648c9b2d011208839a648c9b2d011304d679090104d679090404d679090504d679090604d679090704d679090804d679090a04d679090b04d679090c04d679090d04d679090f";
        assert_eq!(hex(metadata.requested_ids()), chrome);
    }
    #[test]
    fn discovery_uses_root_version_and_keeps_last_good_after_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let provider =
            PkiMetadataProvider::new(PkiMetadata::bundled(), Some(directory.path().into()));
        let put = |name: &str, data: &[u8]| {
            let path = directory.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("crs.pb"), data).unwrap();
        };
        put("9999", &encoded(40));
        put("100.2", &encoded(42));
        put("100.10", &encoded(43));
        put("100.11", b"incomplete");
        put("ignore-me", &encoded(99));
        provider.refresh();
        let old = provider.current();
        assert_eq!(old.version, 43);
        assert!(old.source.contains("100.10"));
        put("101", &encoded(44));
        provider.refresh();
        assert_eq!(provider.current().version, 44);
        assert_eq!(old.version, 43); // Existing connections keep their Arc.
        put("101", b"truncated");
        provider.refresh();
        assert_eq!(provider.current().version, 44);
    }
    #[test]
    fn loader_rejects_bad_catalog_and_bounds_before_allocation() {
        for bytes in [
            vec![],
            vec![0x0a, 255],
            [0x0a, 0].repeat(1025),
            vec![0; 8 * 1024 * 1024 + 1],
        ] {
            assert!(PkiMetadata::decode(&bytes, "test".into()).is_err());
        }
        let mut store = RootStore::decode(encoded(42).as_slice()).unwrap();
        store.trust_anchors[0].certificate = Some(Certificate::Sha256("00".repeat(32)));
        assert!(PkiMetadata::decode(&store.encode_to_vec(), "test".into()).is_err());
        let ca = crate::ProxyCa::generate("nonCA", 2).unwrap();
        let leaf = ca
            .issue(crate::EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        store.trust_anchors[0].certificate =
            Some(Certificate::Der(leaf.certificate.to_der().unwrap()));
        assert!(PkiMetadata::decode(&store.encode_to_vec(), "test".into()).is_err());
        for id in [vec![], vec![0x80, 1], vec![0x81], vec![1; 33]] {
            store.trust_anchors[0].certificate =
                Some(Certificate::Der(ca.certificate().to_der().unwrap()));
            store.trust_anchors[0].trust_anchor_id = Some(id);
            assert!(PkiMetadata::decode(&store.encode_to_vec(), "test".into()).is_err());
        }
    }
    #[test]
    fn roles_sorting_and_duplicates_follow_chromium() {
        let mut store = RootStore::decode(encoded(42).as_slice()).unwrap();
        let mut extra = store.trust_anchors[0].clone();
        extra.trust_anchor_id = Some(vec![1]);
        extra.tls_trust_anchor = Some(false);
        store.additional_certs.push(extra.clone());
        extra.tls_trust_anchor = Some(true);
        store.additional_certs.push(extra.clone());
        extra.tls_trust_anchor = None;
        store.trust_anchors.push(extra);
        let metadata = PkiMetadata::decode(&store.encode_to_vec(), "test".into()).unwrap();
        assert_eq!(metadata.requested_ids(), &[1, 1, 1, 1, 2, 1, 2]);
        store.trust_anchors.clear();
        let additional_only = PkiMetadata::decode(&store.encode_to_vec(), "test".into()).unwrap();
        assert_eq!(additional_only.requested_ids(), &[1, 1]);
    }

    #[test]
    fn recovery_defers_leaf_constraints_but_checks_versions_and_unknown_policy() {
        let verifies = |store: &RootStore| {
            let metadata = PkiMetadata::decode(&store.encode_to_vec(), "policy".into()).unwrap();
            transmog_tls_native::verify_certificate_authority(
                &metadata.policy,
                &metadata.trusted_anchors[0],
            )
        };
        let mut store = RootStore::decode(encoded(42).as_slice()).unwrap();
        store.trust_anchors[0].constraints.push(ConstraintSet {
            sct_not_after_sec: Some(1),
            ..Default::default()
        });
        assert!(verifies(&store));
        // All groups must disallow this Chromium version to exclude recovery.
        store.trust_anchors[0].constraints.clear();
        store.trust_anchors[0].constraints.push(ConstraintSet {
            max_version_exclusive: Some("153.0.8010.0".into()),
            ..Default::default()
        });
        assert!(!verifies(&store));
        store.trust_anchors[0].constraints.push(ConstraintSet {
            min_version: Some("154.0.0.0".into()),
            ..Default::default()
        });
        assert!(verifies(&store));
        assert_eq!(
            PkiMetadata::decode(&store.encode_to_vec(), "policy".into())
                .unwrap()
                .trust
                .root_count(),
            1
        );
        // A future unknown constraint must not turn into an empty, unrestricted set.
        assert!(preflight_anchor(&[0x22, 3, 0x60, 1, 0]).is_err());
        assert!(preflight_anchor(&[0x22, 2, 0x60, 1]).is_err());
        assert!(preflight_anchor(&[0x22, 0].repeat(65)).is_err());
        assert!(
            PkiMetadata::decode(
                &[encoded(42), vec![0x20, 1]].concat(),
                "future root policy".into()
            )
            .is_err()
        );
    }

    #[test]
    fn duplicate_identifiers_do_not_allow_conflicting_anchor_policy() {
        let mut store = RootStore::decode(encoded(42).as_slice()).unwrap();
        let mut duplicate = store.trust_anchors[0].clone();
        duplicate.trust_anchor_id = Some(vec![7]);
        store.trust_anchors.push(duplicate);
        assert!(PkiMetadata::decode(&store.encode_to_vec(), "duplicate IDs".into()).is_ok());
        store.trust_anchors[1].enforce_anchor_constraints = Some(true);
        assert!(PkiMetadata::decode(&store.encode_to_vec(), "conflicting flags".into()).is_err());
        store.trust_anchors[1].enforce_anchor_constraints = None;
        store.trust_anchors[1].constraints.push(ConstraintSet {
            min_version: Some("broken".into()),
            ..Default::default()
        });
        assert!(PkiMetadata::decode(&store.encode_to_vec(), "malformed version".into()).is_err());
    }
}

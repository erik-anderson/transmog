use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    time::{Duration, SystemTime},
};

use boring::{
    asn1::Asn1Time,
    bn::{BigNum, MsbOption},
    ec::{EcGroup, EcKey},
    error::ErrorStack,
    hash::MessageDigest,
    nid::Nid,
    pkey::{PKey, Private},
    x509::{
        X509, X509NameBuilder,
        extension::{
            AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage,
            SubjectAlternativeName, SubjectKeyIdentifier,
        },
    },
};
use thiserror::Error;

/// Certificate identity selected from CONNECT authority and downstream SNI.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EndpointIdentity {
    /// Lowercase ASCII DNS name without a trailing dot.
    Dns(String),
    /// IP literal represented canonically.
    Ip(IpAddr),
}

impl EndpointIdentity {
    /// Parses and normalizes a CONNECT host without a port.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Malformed`] when `host` is not a canonical DNS
    /// name or IP literal supported by the proxy.
    pub fn parse(host: &str) -> Result<Self, IdentityError> {
        if host.is_empty() || host.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(IdentityError::Malformed);
        }
        let unbracketed = host
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or(host);
        if let Ok(ip) = unbracketed.parse::<IpAddr>() {
            return Ok(Self::Ip(ip));
        }
        if !unbracketed.is_ascii()
            || unbracketed.len() > 253
            || unbracketed.ends_with('.')
            || unbracketed.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return Err(IdentityError::Malformed);
        }
        Ok(Self::Dns(unbracketed.to_ascii_lowercase()))
    }

    /// Stable text used in SAN and cache keys.
    pub fn as_text(&self) -> String {
        match self {
            Self::Dns(name) => name.clone(),
            Self::Ip(ip) => ip.to_string(),
        }
    }
}

/// Normalizes CONNECT/SNI inputs and rejects target confusion.
///
/// # Errors
///
/// Returns [`IdentityError`] when either identity is malformed or when SNI
/// names a different target than the CONNECT authority.
pub fn normalize_connect_identity(
    connect_host: &str,
    sni: Option<&str>,
) -> Result<EndpointIdentity, IdentityError> {
    let connect = EndpointIdentity::parse(connect_host)?;
    match (&connect, sni) {
        (EndpointIdentity::Dns(expected), Some(actual)) => {
            let actual = EndpointIdentity::parse(actual)?;
            if actual == EndpointIdentity::Dns(expected.clone()) {
                Ok(connect)
            } else {
                Err(IdentityError::SniMismatch)
            }
        }
        (EndpointIdentity::Dns(_) | EndpointIdentity::Ip(_), None) => Ok(connect),
        (EndpointIdentity::Ip(expected), Some(actual)) => {
            let actual = EndpointIdentity::parse(actual)?;
            if actual == EndpointIdentity::Ip(*expected) {
                Ok(connect)
            } else {
                Err(IdentityError::SniMismatch)
            }
        }
    }
}

/// CONNECT/SNI identity validation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum IdentityError {
    /// Host is not a supported canonical DNS name or IP literal.
    #[error("malformed CONNECT/SNI identity")]
    Malformed,
    /// CONNECT authority and SNI select different targets.
    #[error("CONNECT authority and TLS SNI do not agree")]
    SniMismatch,
}

/// Proxy signing authority. It is never used as upstream trust.
#[derive(Clone)]
pub struct ProxyCa {
    certificate: X509,
    private_key: PKey<Private>,
}

impl ProxyCa {
    /// Generates an ECDSA P-256 CA with a unique operator-supplied common name.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyCaError`] for an empty subject, zero validity, or a
    /// certificate/key generation failure.
    pub fn generate(common_name: &str, validity_days: u32) -> Result<Self, ProxyCaError> {
        if common_name.trim().is_empty() || validity_days == 0 {
            return Err(ProxyCaError::InvalidConfiguration);
        }
        let private_key = generate_p256_key()?;
        let mut name = X509NameBuilder::new()?;
        name.append_entry_by_text("O", "Transmog local interception")?;
        name.append_entry_by_text("CN", common_name)?;
        let name = name.build();

        let mut builder = X509::builder()?;
        builder.set_version(2)?;
        let serial = random_serial()?;
        builder.set_serial_number(&serial)?;
        builder.set_subject_name(&name)?;
        builder.set_issuer_name(&name)?;
        builder.set_pubkey(&private_key)?;
        let not_before = Asn1Time::days_from_now(0)?;
        let not_after = Asn1Time::days_from_now(validity_days)?;
        builder.set_not_before(&not_before)?;
        builder.set_not_after(&not_after)?;
        builder.append_extension(
            BasicConstraints::new()
                .critical()
                .ca()
                .pathlen(0)
                .build()?
                .as_ref(),
        )?;
        builder.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()?
                .as_ref(),
        )?;
        let subject_key_id =
            SubjectKeyIdentifier::new().build(&builder.x509v3_context(None, None))?;
        builder.append_extension(&subject_key_id)?;
        builder.sign(&private_key, MessageDigest::sha256())?;

        Ok(Self {
            certificate: builder.build(),
            private_key,
        })
    }

    /// Restores a CA from explicit PEM values supplied by the operator.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyCaError`] when either PEM value is invalid or the public
    /// certificate does not match the private key.
    pub fn from_pem(certificate_pem: &[u8], private_key_pem: &[u8]) -> Result<Self, ProxyCaError> {
        let certificate = X509::from_pem(certificate_pem)?;
        let private_key = PKey::private_key_from_pem(private_key_pem)?;
        if !certificate.public_key()?.public_eq(&private_key) {
            return Err(ProxyCaError::KeyMismatch);
        }
        Ok(Self {
            certificate,
            private_key,
        })
    }

    /// CA certificate.
    pub fn certificate(&self) -> &X509 {
        &self.certificate
    }

    /// Exports only the public certificate for explicit trust installation.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyCaError`] if certificate serialization fails.
    pub fn certificate_pem(&self) -> Result<Vec<u8>, ProxyCaError> {
        Ok(self.certificate.to_pem()?)
    }

    /// Exports PKCS#8 private-key PEM for explicit secure persistence by an operator layer.
    ///
    /// Callers must write this using user-only permissions and must never log it.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyCaError`] if private-key serialization fails.
    pub fn private_key_pem_pkcs8(&self) -> Result<Vec<u8>, ProxyCaError> {
        Ok(self.private_key.private_key_to_pem_pkcs8()?)
    }

    /// SHA-256 certificate thumbprint used to target exact install/uninstall operations.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyCaError`] if certificate digest calculation fails.
    pub fn sha256_thumbprint(&self) -> Result<String, ProxyCaError> {
        let digest = self.certificate.digest(MessageDigest::sha256())?;
        Ok(hex_upper(&digest))
    }

    /// Issues a short-lived ECDSA leaf for exactly one DNS name or IP address.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyCaError`] when validity falls outside `1..=30` days or a
    /// certificate/key operation fails.
    pub fn issue(
        &self,
        identity: EndpointIdentity,
        validity_days: u32,
    ) -> Result<IssuedLeaf, ProxyCaError> {
        if validity_days == 0 || validity_days > 30 {
            return Err(ProxyCaError::InvalidConfiguration);
        }
        let private_key = generate_p256_key()?;
        let mut name = X509NameBuilder::new()?;
        name.append_entry_by_text("O", "Transmog intercepted origin")?;
        name.append_entry_by_text("CN", &identity.as_text())?;
        let name = name.build();

        let mut builder = X509::builder()?;
        builder.set_version(2)?;
        let serial = random_serial()?;
        builder.set_serial_number(&serial)?;
        builder.set_subject_name(&name)?;
        builder.set_issuer_name(self.certificate.subject_name())?;
        builder.set_pubkey(&private_key)?;
        let not_before = Asn1Time::days_from_now(0)?;
        let not_after = Asn1Time::days_from_now(validity_days)?;
        builder.set_not_before(&not_before)?;
        builder.set_not_after(&not_after)?;
        builder.append_extension(BasicConstraints::new().critical().build()?.as_ref())?;
        builder.append_extension(
            KeyUsage::new()
                .critical()
                .digital_signature()
                .build()?
                .as_ref(),
        )?;
        builder.append_extension(ExtendedKeyUsage::new().server_auth().build()?.as_ref())?;

        let mut san = SubjectAlternativeName::new();
        match &identity {
            EndpointIdentity::Dns(name) => {
                san.dns(name);
            }
            EndpointIdentity::Ip(ip) => {
                san.ip(&ip.to_string());
            }
        }
        let san = san.build(&builder.x509v3_context(Some(&self.certificate), None))?;
        builder.append_extension(&san)?;
        let subject_key_id = SubjectKeyIdentifier::new()
            .build(&builder.x509v3_context(Some(&self.certificate), None))?;
        builder.append_extension(&subject_key_id)?;
        let authority_key_id = AuthorityKeyIdentifier::new()
            .keyid(false)
            .issuer(false)
            .build(&builder.x509v3_context(Some(&self.certificate), None))?;
        builder.append_extension(&authority_key_id)?;
        builder.sign(&self.private_key, MessageDigest::sha256())?;

        Ok(IssuedLeaf {
            identity,
            certificate: builder.build(),
            private_key,
            expires_at: SystemTime::now() + Duration::from_secs(u64::from(validity_days) * 86_400),
        })
    }
}

/// Leaf material selected for one downstream TLS context.
#[derive(Clone)]
pub struct IssuedLeaf {
    /// Exact SAN identity.
    pub identity: EndpointIdentity,
    /// Signed certificate.
    pub certificate: X509,
    /// Leaf private key.
    pub private_key: PKey<Private>,
    /// Conservative local cache expiry.
    pub expires_at: SystemTime,
}

/// Size- and expiry-bounded leaf cache.
pub struct LeafCache {
    ca: ProxyCa,
    capacity: usize,
    validity_days: u32,
    entries: HashMap<EndpointIdentity, IssuedLeaf>,
    order: VecDeque<EndpointIdentity>,
}

impl LeafCache {
    /// Creates a cache. Zero capacity and validity outside 1..=30 are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`LeafCacheError::InvalidConfiguration`] for a zero capacity or
    /// validity outside `1..=30` days.
    pub fn new(ca: ProxyCa, capacity: usize, validity_days: u32) -> Result<Self, LeafCacheError> {
        if capacity == 0 || validity_days == 0 || validity_days > 30 {
            return Err(LeafCacheError::InvalidConfiguration);
        }
        Ok(Self {
            ca,
            capacity,
            validity_days,
            entries: HashMap::new(),
            order: VecDeque::new(),
        })
    }

    /// Returns a cached leaf or issues one. Expired entries are never returned.
    ///
    /// # Errors
    ///
    /// Returns [`LeafCacheError`] when issuing a replacement certificate fails.
    pub fn get_or_issue(
        &mut self,
        identity: EndpointIdentity,
        now: SystemTime,
    ) -> Result<IssuedLeaf, LeafCacheError> {
        self.remove_expired(now);
        if let Some(existing) = self.entries.get(&identity) {
            return Ok(existing.clone());
        }
        while self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        let issued = self.ca.issue(identity.clone(), self.validity_days)?;
        self.entries.insert(identity.clone(), issued.clone());
        self.order.push_back(identity);
        Ok(issued)
    }

    /// Number of currently cached leaves.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn remove_expired(&mut self, now: SystemTime) {
        self.entries.retain(|_, leaf| leaf.expires_at > now);
        self.order
            .retain(|identity| self.entries.contains_key(identity));
    }
}

fn generate_p256_key() -> Result<PKey<Private>, ErrorStack> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
    PKey::from_ec_key(EcKey::generate(&group)?)
}

fn random_serial() -> Result<boring::asn1::Asn1Integer, ErrorStack> {
    let mut serial = BigNum::new()?;
    serial.rand(159, MsbOption::ONE, false)?;
    serial.to_asn1_integer()
}

fn hex_upper(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

/// CA generation, loading, or issuance failure.
#[derive(Debug, Error)]
pub enum ProxyCaError {
    /// `BoringSSL` operation failed.
    #[error("BoringSSL certificate operation failed: {0}")]
    Boring(#[from] ErrorStack),
    /// Validity or subject configuration was unsafe.
    #[error("invalid proxy CA or leaf configuration")]
    InvalidConfiguration,
    /// Supplied certificate and private key do not match.
    #[error("proxy CA certificate and private key do not match")]
    KeyMismatch,
}

/// Leaf cache configuration or issuance failure.
#[derive(Debug, Error)]
pub enum LeafCacheError {
    /// Cache capacity or leaf validity is invalid.
    #[error("invalid leaf cache configuration")]
    InvalidConfiguration,
    /// Leaf issuance failed.
    #[error(transparent)]
    Issuance(#[from] ProxyCaError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_normalization_rejects_target_confusion() {
        assert_eq!(
            normalize_connect_identity("Example.COM", Some("example.com")).unwrap(),
            EndpointIdentity::Dns("example.com".to_owned())
        );
        assert_eq!(
            normalize_connect_identity("example.com", Some("evil.test")),
            Err(IdentityError::SniMismatch)
        );
        assert!(EndpointIdentity::parse("bad..example").is_err());
        assert_eq!(
            EndpointIdentity::parse("[2001:db8::1]").unwrap(),
            EndpointIdentity::Ip("2001:db8::1".parse().unwrap())
        );
    }

    #[test]
    fn issued_leaf_has_exact_identity_and_cache_is_bounded() {
        let ca = ProxyCa::generate("Transmog test CA", 2).unwrap();
        assert_eq!(ca.sha256_thumbprint().unwrap().len(), 64);
        let mut cache = LeafCache::new(ca, 1, 1).unwrap();
        let first = EndpointIdentity::parse("one.example").unwrap();
        let first_leaf = cache
            .get_or_issue(first.clone(), SystemTime::now())
            .unwrap();
        assert_eq!(first_leaf.identity, first);
        assert_eq!(cache.len(), 1);
        cache
            .get_or_issue(
                EndpointIdentity::parse("two.example").unwrap(),
                SystemTime::now(),
            )
            .unwrap();
        assert_eq!(cache.len(), 1);
    }
}

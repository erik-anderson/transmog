//! Immutable certificate-transparency and revocation inputs for the PKI delegate.

/// A log operator's tenure, ordered newest first as in Chromium's component.
#[derive(Clone, Debug)]
pub struct LogOperator {
    /// Human-readable operator identity used for diversity.
    pub name: String,
    /// Tenure start in Unix milliseconds.
    pub start_ms: i64,
}

/// A known CT log. Verification derives the identifier from the public key.
#[derive(Clone, Debug)]
pub struct CtLog {
    /// SHA-256 log identifier.
    pub id: [u8; 32],
    /// `SubjectPublicKeyInfo` DER.
    pub key: Vec<u8>,
    /// Public log description for diagnostics.
    pub name: String,
    /// Unspecified=0, RFC6962=1, Static CT API=2.
    pub log_type: u32,
    /// Terminal retirement time in Unix milliseconds.
    pub retired_at_ms: Option<i64>,
    /// Operator history, newest first.
    pub operators: Vec<LogOperator>,
}

/// Frozen Chromium CT configuration.
#[derive(Clone, Debug)]
pub struct CtConfig {
    /// Emergency component kill switch.
    pub disabled: bool,
    /// Log-list timestamp, used for Chromium's 70-day timeliness policy.
    pub timestamp_ms: i64,
    /// Require at least one RFC6962 log in the accepted quorum.
    pub require_rfc6962: bool,
    /// Known keys and log policy inputs.
    pub logs: Vec<CtLog>,
}

/// A `CRLSet` subject restriction.
#[derive(Clone, Debug)]
pub struct SubjectLimit {
    /// SHA-256 of the original DER-encoded subject Name.
    pub subject_hash: [u8; 32],
    /// Permitted SHA-256 SPKI hashes for this subject.
    pub allowed_spkis: Vec<[u8; 32]>,
}

/// Serial revocation records for one issuer.
#[derive(Clone, Debug)]
pub struct IssuerRevocations {
    /// SHA-256 of the issuer's SPKI.
    pub issuer: [u8; 32],
    /// Unsigned, leading-zero-normalized serials.
    pub serials: Vec<Vec<u8>>,
}

/// Frozen `CRLSet`. Expiration does not discard its blocking records.
#[derive(Clone, Debug, Default)]
pub struct CrlSet {
    /// Embedded sequence number, independent of folder version.
    pub sequence: u32,
    /// Expiry in Unix seconds; zero means no expiration.
    pub not_after: u64,
    /// Dynamically blocked SHA-256 SPKI hashes; native compiled blocks also apply.
    pub blocked_spkis: Vec<[u8; 32]>,
    /// Known interception keys for evidence, independent of revocation.
    pub interception_spkis: Vec<[u8; 32]>,
    /// Subject-based restrictions.
    pub subjects: Vec<SubjectLimit>,
    /// Issuer-indexed serial revocations.
    pub issuers: Vec<IssuerRevocations>,
}

/// Immutable inputs shared by all paths in one verification operation.
#[derive(Clone, Debug)]
pub struct SecurityPolicy {
    /// Certificate Transparency configuration.
    pub ct: CtConfig,
    /// Certificate revocation set.
    pub crl: CrlSet,
}

/// Security checks performed on the selected path, including skipped checks.
#[derive(Clone, Debug)]
pub struct SecurityVerification {
    /// CT enforcement result or the reason enforcement was skipped.
    pub ct: &'static str,
    /// `CRLSet` coverage or revocation result; inconclusive is a soft success.
    pub crl: &'static str,
    /// Stapled OCSP result under Chromium's default soft-fail policy.
    pub ocsp: &'static str,
    /// Cryptographically valid SCTs from enabled delivery mechanisms.
    pub valid_scts: u32,
    /// Unknown-log, malformed, invalid-signature or invalid-time SCTs.
    pub invalid_scts: u32,
    /// A key in the path is on the known interception list.
    pub known_interception: bool,
}

impl Default for SecurityVerification {
    fn default() -> Self {
        Self {
            ct: "Not reached",
            crl: "Not reached",
            ocsp: "Not reached",
            valid_scts: 0,
            invalid_scts: 0,
            known_interception: false,
        }
    }
}

impl SecurityPolicy {
    pub(crate) fn encode(&self) -> Option<Vec<u8>> {
        fn blob(out: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
            out.extend(u32::try_from(bytes.len()).ok()?.to_be_bytes());
            out.extend(bytes);
            Some(())
        }
        fn hashes(out: &mut Vec<u8>, items: &[[u8; 32]]) -> Option<()> {
            out.extend(u32::try_from(items.len()).ok()?.to_be_bytes());
            for item in items {
                out.extend(item);
            }
            Some(())
        }
        let mut out = vec![u8::from(self.ct.disabled)];
        out.extend(self.ct.timestamp_ms.to_be_bytes());
        out.push(u8::from(self.ct.require_rfc6962));
        out.extend(u32::try_from(self.ct.logs.len()).ok()?.to_be_bytes());
        for log in &self.ct.logs {
            out.extend(log.id);
            blob(&mut out, &log.key)?;
            out.extend(log.log_type.to_be_bytes());
            out.push(u8::from(log.retired_at_ms.is_some()));
            if let Some(time) = log.retired_at_ms {
                out.extend(time.to_be_bytes());
            }
            out.extend(u32::try_from(log.operators.len()).ok()?.to_be_bytes());
            for operator in &log.operators {
                blob(&mut out, operator.name.as_bytes())?;
                out.extend(operator.start_ms.to_be_bytes());
            }
        }
        out.extend(self.crl.not_after.to_be_bytes());
        hashes(&mut out, &self.crl.blocked_spkis)?;
        hashes(&mut out, &self.crl.interception_spkis)?;
        out.extend(u32::try_from(self.crl.subjects.len()).ok()?.to_be_bytes());
        for subject in &self.crl.subjects {
            out.extend(subject.subject_hash);
            hashes(&mut out, &subject.allowed_spkis)?;
        }
        out.extend(u32::try_from(self.crl.issuers.len()).ok()?.to_be_bytes());
        for issuer in &self.crl.issuers {
            out.extend(issuer.issuer);
            out.extend(u32::try_from(issuer.serials.len()).ok()?.to_be_bytes());
            for serial in &issuer.serials {
                blob(&mut out, serial)?;
            }
        }
        (out.len() <= 32 * 1024 * 1024).then_some(out)
    }
}

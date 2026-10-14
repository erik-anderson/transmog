//! Chromium component wire formats and independent, last-good freshness.
use boring::base64::decode_block;
use prost::Message;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use transmog_tls_native::{
    CrlSet, CtConfig, CtLog, IssuerRevocations, LogOperator, SecurityPolicy, SubjectLimit,
};

const LIMIT: usize = 16 * 1024 * 1024;
const BASELINE: &[u8] =
    include_bytes!("../../../third_party/chromium-certificate-policy/ct_config.pb");

pub(crate) fn timestamp(milliseconds: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(milliseconds) * 1_000_000)
        .ok()
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| format!("{milliseconds} Unix milliseconds"))
}
pub(crate) fn expiry(seconds: u64) -> String {
    if seconds == 0 {
        return "No expiry provided".into();
    }
    i64::try_from(seconds)
        .ok()
        .and_then(|time| time::OffsetDateTime::from_unix_timestamp(time).ok())
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| format!("{seconds} Unix seconds"))
}

#[derive(Clone, PartialEq, Message)]
struct Timestamp {
    #[prost(int64, tag = "1")]
    seconds: i64,
    #[prost(int32, tag = "2")]
    nanos: i32,
}
impl Timestamp {
    fn ms(&self) -> Result<i64, String> {
        if !(0..1_000_000_000).contains(&self.nanos) {
            return Err("Invalid CT timestamp".into());
        }
        self.seconds
            .checked_mul(1000)
            .and_then(|x| x.checked_add(i64::from(self.nanos / 1_000_000)))
            .ok_or_else(|| "CT timestamp overflow".into())
    }
}
#[derive(Clone, PartialEq, Message)]
struct State {
    #[prost(int32, tag = "1")]
    state: i32,
    #[prost(message, optional, tag = "2")]
    start: Option<Timestamp>,
}
#[derive(Clone, PartialEq, Message)]
struct Operator {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(message, optional, tag = "2")]
    start: Option<Timestamp>,
}
#[derive(Clone, PartialEq, Message)]
struct Log {
    #[prost(string, tag = "1")]
    description: String,
    #[prost(string, tag = "2")]
    key: String,
    #[prost(string, tag = "3")]
    id: String,
    #[prost(message, repeated, tag = "8")]
    states: Vec<State>,
    #[prost(message, repeated, tag = "9")]
    operators: Vec<Operator>,
    #[prost(uint32, tag = "18")]
    kind: u32,
}
#[derive(Clone, PartialEq, Message)]
struct LogList {
    #[prost(message, optional, tag = "3")]
    timestamp: Option<Timestamp>,
    #[prost(uint64, tag = "4")]
    compatibility: u64,
    #[prost(message, repeated, tag = "6")]
    logs: Vec<Log>,
}
#[derive(Clone, PartialEq, Message)]
struct Config {
    #[prost(bool, tag = "1")]
    disabled: bool,
    #[prost(message, optional, tag = "2")]
    list: Option<LogList>,
}

fn hash(bytes: &[u8]) -> String {
    crate::trust_anchors::hex(&boring::sha::sha256(bytes))
}
fn decode_hash(text: &str) -> Result<[u8; 32], String> {
    decode_block(text)
        .map_err(|_| "Invalid component base64".to_owned())?
        .try_into()
        .map_err(|_| "Invalid SHA-256 width".into())
}
/// Decodes a bounded `CTConfig`. The emergency disable bit precedes compatibility checks.
/// # Errors
/// Rejects malformed or unsupported data without altering a published snapshot.
pub fn decode_ct(bytes: &[u8]) -> Result<CtConfig, String> {
    if bytes.is_empty() || bytes.len() > LIMIT {
        return Err("Invalid CTConfig size".into());
    }
    bound_protobuf(bytes, 0)?;
    let config = Config::decode(bytes).map_err(|e| e.to_string())?;
    if config.disabled {
        return Ok(CtConfig {
            disabled: true,
            timestamp_ms: 0,
            require_rfc6962: false,
            logs: vec![],
        });
    }
    let list = config.list.ok_or("Missing CT log list")?;
    if list.compatibility > 4 || list.logs.len() > 1024 {
        return Err("Unsupported CT log list".into());
    }
    let timestamp_ms = list.timestamp.ok_or("Missing CT timestamp")?.ms()?;
    let mut logs = Vec::new();
    for log in list.logs {
        if log.key.len() > 16_384 || log.description.len() > 1024 || log.operators.len() > 64 {
            return Err("CT log exceeds bounds".into());
        }
        let Ok(id) = decode_hash(&log.id) else {
            continue;
        };
        let Ok(key) = decode_block(&log.key) else {
            continue;
        };
        if boring::sha::sha256(&key) != id {
            continue;
        }
        let retired_at_ms = log
            .states
            .first()
            .filter(|s| s.state == 5)
            .map(|s| s.start.as_ref().ok_or("Missing retirement timestamp")?.ms())
            .transpose()?;
        let mut operators = Vec::new();
        for operator in log.operators {
            if operator.name.is_empty() || operator.name.len() > 1024 {
                return Err("Invalid CT operator".into());
            }
            operators.push(LogOperator {
                name: operator.name,
                start_ms: operator
                    .start
                    .ok_or("Missing CT operator timestamp")?
                    .ms()?,
            });
        }
        if operators
            .windows(2)
            .any(|pair| pair[0].start_ms < pair[1].start_ms)
        {
            return Err("Unordered CT operator history".into());
        }
        logs.push(CtLog {
            id,
            key,
            name: log.description,
            log_type: log.kind,
            retired_at_ms,
            operators,
        });
    }
    // kEnforceOneRfc6962CtPolicy is disabled by default in Chromium 154.
    Ok(CtConfig {
        disabled: false,
        timestamp_ms,
        require_rfc6962: false,
        logs,
    })
}

fn hashes(header: &serde_json::Value, name: &str) -> Result<Vec<[u8; 32]>, String> {
    let Some(value) = header.get(name) else {
        return Ok(vec![]);
    };
    if value.is_null() {
        return Ok(vec![]);
    }
    value
        .as_array()
        .ok_or("Invalid CRLSet hash array")?
        .iter()
        .map(|v| decode_hash(v.as_str().ok_or("Invalid CRLSet hash")?))
        .collect()
}
/// Decodes Chromium's `CRLSet` binary format, independent of component directory version.
/// # Errors
/// Rejects invalid headers, hashes or truncated serial blocks.
pub fn decode_crl(bytes: &[u8]) -> Result<CrlSet, String> {
    if bytes.len() < 2 || bytes.len() > LIMIT {
        return Err("Invalid CRLSet size".into());
    }
    let length = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
    let header: serde_json::Value =
        serde_json::from_slice(bytes.get(2..2 + length).ok_or("Truncated CRLSet header")?)
            .map_err(|e| e.to_string())?;
    if header["ContentType"] != "CRLSet" || header["Version"].as_u64() != Some(0) {
        return Err("Unsupported CRLSet header".into());
    }
    let sequence = u32::try_from(
        header["Sequence"]
            .as_u64()
            .ok_or("Invalid CRLSet sequence")?,
    )
    .map_err(|_| "Invalid CRLSet sequence")?;
    let not_after = if let Some(value) = header.get("NotAfter") {
        let n = value.as_f64().ok_or("Invalid CRLSet expiry")?;
        if !(0.0..18_446_744_073_709_551_616.0).contains(&n) {
            return Err("Invalid CRLSet expiry".into());
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            n as u64
        }
    } else {
        0
    };
    let interception_blocked = hashes(&header, "BlockedInterceptionSPKIs")?;
    let mut blocked_spkis = hashes(&header, "BlockedSPKIs")?;
    blocked_spkis.extend(&interception_blocked);
    let mut interception_spkis = hashes(&header, "KnownInterceptionSPKIs")?;
    interception_spkis.extend(interception_blocked);
    let mut subjects = Vec::new();
    if let Some(value) = header
        .get("LimitedSubjects")
        .filter(|value| !value.is_null())
    {
        for (subject, allowed) in value.as_object().ok_or("Invalid LimitedSubjects")? {
            let allowed_spkis = allowed
                .as_array()
                .ok_or("Invalid subject allowlist")?
                .iter()
                .map(|v| decode_hash(v.as_str().ok_or("Invalid allowed SPKI")?))
                .collect::<Result<_, _>>()?;
            subjects.push(SubjectLimit {
                subject_hash: decode_hash(subject)?,
                allowed_spkis,
            });
        }
    }
    let mut payload = &bytes[2 + length..];
    let mut issuers = std::collections::BTreeMap::new();
    let mut total_serials = 0_u64;
    while !payload.is_empty() {
        let issuer: [u8; 32] = payload
            .get(..32)
            .ok_or("Truncated issuer")?
            .try_into()
            .map_err(|_| "Invalid issuer width")?;
        let count = u32::from_le_bytes(
            payload
                .get(32..36)
                .ok_or("Truncated serial count")?
                .try_into()
                .map_err(|_| "Invalid serial count width")?,
        );
        payload = &payload[36..];
        total_serials += u64::from(count);
        if u64::from(count) > payload.len() as u64
            || total_serials > 1_000_000
            || issuers.len() >= 8192
        {
            return Err("Invalid serial count".into());
        }
        let mut serials = Vec::new();
        for _ in 0..count {
            let length = usize::from(*payload.first().ok_or("Missing serial length")?);
            serials.push(
                payload
                    .get(1..1 + length)
                    .ok_or("Truncated serial")?
                    .to_vec(),
            );
            payload = &payload[1 + length..];
        }
        issuers.insert(issuer, serials);
    }
    Ok(CrlSet {
        sequence,
        not_after,
        blocked_spkis,
        interception_spkis,
        subjects,
        issuers: issuers
            .into_iter()
            .map(|(issuer, serials)| IssuerRevocations { issuer, serials })
            .collect(),
    })
}

/// Frozen verification inputs and the public component provenance used by a connection.
#[derive(Clone, Debug)]
pub struct VerificationData {
    /// Monotonic publication generation, including kill switch changes.
    pub generation: u64,
    /// Inputs consumed by the sole native verifier.
    pub policy: Arc<SecurityPolicy>,
    /// CT source and digest; no certificate inventory is exposed.
    pub ct_source: String,
    /// SHA-256 of the selected `CTConfig`.
    pub ct_sha256: String,
    /// Revocation source.
    pub crl_source: String,
    /// SHA-256 of the selected `CRLSet`, absent for the compiled baseline.
    pub crl_sha256: String,
}
impl VerificationData {
    fn bundled() -> Self {
        Self {
            generation: 1,
            policy: Arc::new(SecurityPolicy {
                ct: decode_ct(BASELINE).expect("qualified bundled CT list"),
                crl: CrlSet::default(),
            }),
            ct_source: "Bundled Chromium CT log list".into(),
            ct_sha256: hash(BASELINE),
            crl_source: "Compiled Chromium CRLSet baseline".into(),
            crl_sha256: String::new(),
        }
    }
}
#[derive(Debug)]
struct StateData {
    current: Arc<VerificationData>,
    ct_component: Vec<u64>,
}
/// Both browser sources share parsing and freshness code, retaining last-good data.
#[derive(Debug)]
pub struct VerificationDataProvider {
    baseline_timestamp: i64,
    refresh_lock: Mutex<()>,
    pki_directory: Option<PathBuf>,
    revocation_directory: Option<PathBuf>,
    state: Mutex<StateData>,
}
impl VerificationDataProvider {
    /// Creates a provider with optional installed component roots and a bundled fallback.
    pub fn new(pki_directory: Option<PathBuf>, revocation_directory: Option<PathBuf>) -> Self {
        let bundled = VerificationData::bundled();
        Self {
            baseline_timestamp: bundled.policy.ct.timestamp_ms,
            refresh_lock: Mutex::new(()),
            pki_directory,
            revocation_directory,
            state: Mutex::new(StateData {
                current: Arc::new(bundled),
                ct_component: vec![],
            }),
        }
    }
    /// Shared browser resolver. Only the source directories differ.
    pub fn installed(webview2: bool) -> Arc<Self> {
        static CHROME: OnceLock<Arc<VerificationDataProvider>> = OnceLock::new();
        static WV2: OnceLock<Arc<VerificationDataProvider>> = OnceLock::new();
        let slot = if webview2 { &WV2 } else { &CHROME };
        slot.get_or_init(|| {
            let base = if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .map(|p| {
                        p.join(if webview2 {
                            "Microsoft/Windows/SharedWebView/EBWebView"
                        } else {
                            "Google/Chrome/User Data"
                        })
                    })
            } else {
                None
            };
            let provider = Arc::new(Self::new(
                base.as_ref().map(|p| p.join("PKIMetadata")),
                base.map(|p| p.join("CertificateRevocation")),
            ));
            let weak = Arc::downgrade(&provider);
            let _ = std::thread::Builder::new()
                .name("chromium-verification-data".into())
                .spawn(move || {
                    loop {
                        {
                            let Some(provider) = weak.upgrade() else {
                                break;
                            };
                            provider.refresh();
                        }
                        std::thread::sleep(Duration::from_secs(60));
                    }
                });
            provider
        })
        .clone()
    }
    /// Returns one frozen data generation.
    pub fn current(&self) -> Arc<VerificationData> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .current
            .clone()
    }
    /// Reads bounded public component files, publishing only supported newer data.
    pub fn refresh(&self) {
        let _refresh = self
            .refresh_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (current, mut ct_component) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (state.current.clone(), state.ct_component.clone())
        };
        let mut next = (*current).clone();
        let mut policy = (*next.policy).clone();
        if let Some(directory) = &self.pki_directory {
            for (version, path) in versions(directory) {
                if version < ct_component {
                    break;
                }
                let file = path.join("ct_config.pb");
                let Ok(bytes) = read(&file) else {
                    continue;
                };
                let Ok(ct) = decode_ct(&bytes) else {
                    continue;
                };
                let digest = hash(&bytes);
                if (ct.disabled || ct.timestamp_ms >= self.baseline_timestamp)
                    && digest != next.ct_sha256
                {
                    policy.ct = ct;
                    next.ct_source = file.display().to_string();
                    next.ct_sha256 = digest;
                }
                ct_component = version;
                break;
            }
        }
        if let Some(directory) = &self.revocation_directory {
            for (_, path) in versions(directory) {
                let file = path.join("crl-set");
                let Ok(bytes) = read(&file) else {
                    continue;
                };
                let Ok(crl) = decode_crl(&bytes) else {
                    continue;
                };
                if crl.sequence > policy.crl.sequence {
                    policy.crl = crl;
                    next.crl_source = file.display().to_string();
                    next.crl_sha256 = hash(&bytes);
                }
            }
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.ct_component = ct_component;
        if next.ct_sha256 != current.ct_sha256 || next.crl_sha256 != current.crl_sha256 {
            next.generation = next.generation.saturating_add(1);
            next.policy = Arc::new(policy);
            state.current = Arc::new(next);
        }
    }
}
fn read(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let mut data = Vec::new();
    File::open(path)?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut data)?;
    Ok(data)
}
fn versions(path: &Path) -> Vec<(Vec<u64>, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return vec![];
    };
    let mut results = Vec::new();
    for entry in entries.take(4096).flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(version) = name
            .split('.')
            .map(|part| {
                if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
                    part.parse::<u64>().ok()
                } else {
                    None
                }
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        results.push((version, entry.path()));
    }
    results.sort_by(|a, b| b.0.cmp(&a.0));
    results.truncate(128);
    results
}

// Preflight repeated message counts before prost can allocate their vectors.
fn bound_protobuf(mut bytes: &[u8], level: u8) -> Result<(), String> {
    fn varint(bytes: &mut &[u8]) -> Result<u64, String> {
        let mut result = 0;
        for shift in (0..70).step_by(7) {
            let byte = *bytes.first().ok_or("Truncated protobuf varint")?;
            *bytes = &bytes[1..];
            if shift == 63 && byte > 1 {
                return Err("Invalid protobuf varint".into());
            }
            result |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(result);
            }
        }
        Err("Invalid protobuf varint".into())
    }
    let mut repeated = [0_u32; 2];
    while !bytes.is_empty() {
        let tag = varint(&mut bytes)?;
        match tag & 7 {
            0 => {
                varint(&mut bytes)?;
            }
            1 | 5 => {
                let length = if tag & 7 == 1 { 8 } else { 4 };
                bytes = bytes.get(length..).ok_or("Truncated protobuf scalar")?;
            }
            2 => {
                let length =
                    usize::try_from(varint(&mut bytes)?).map_err(|_| "Invalid protobuf length")?;
                let value = bytes.get(..length).ok_or("Truncated protobuf field")?;
                bytes = &bytes[length..];
                let field = tag >> 3;
                if (level == 0 && field == 2) || (level == 1 && field == 6) {
                    repeated[0] += 1;
                    if repeated[0] > (if level == 0 { 1 } else { 1024 }) {
                        return Err("CT message count exceeds bounds".into());
                    }
                    bound_protobuf(value, level + 1)?;
                } else if level == 2 && (field == 8 || field == 9) {
                    let index = usize::from(field == 9);
                    repeated[index] += 1;
                    if repeated[index] > 64 {
                        return Err("CT history exceeds bounds".into());
                    }
                }
            }
            _ => return Err("Unsupported protobuf wire type".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "verification_data_tests.rs"]
mod tests;

//! Versioned, inert support-capture profiles and durable incident marker extensions.
//! Neither a profile nor a marker authorizes trust, routing, execution or uploads.
#![allow(missing_docs)]
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write,
    io,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_PROFILE_BYTES: usize = 64 * 1024;
pub const MAX_MARKERS: usize = 256;
pub const MAX_NOTE_BYTES: usize = 2048;
pub const MARKER_KIND: &str = "incident-marker";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportProfile {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub title: String,
    #[serde(default)]
    pub support_reference: Option<String>,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub relevant_hosts: Vec<RelevantHost>,
    #[serde(default)]
    pub recording: RecordingSuggestions,
    #[serde(default)]
    pub duration_seconds: Option<u64>,
    #[serde(default)]
    pub incident_prompts: Vec<String>,
    #[serde(default)]
    pub expected_application: Option<String>,
    #[serde(default)]
    pub suggest_persistent_root: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelevantHost {
    pub host: String,
    #[serde(default)]
    pub include_subdomains: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingSuggestions {
    pub redact_sensitive_headers: Option<bool>,
    pub request_body_limit: Option<BodyLimit>,
    pub circular_buffer: Option<String>,
    pub include_network_context: Option<bool>,
    #[serde(default)]
    pub encryption: EncryptionSuggestion,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BodyLimit {
    Limited { bytes: u64 },
    Unlimited,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EncryptionSuggestion {
    #[default]
    Unspecified,
    Recommended,
    Required,
}

impl SupportProfile {
    /// # Errors
    /// Returns invalid input, an unsupported schema, or unavailable OS randomness.
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(invalid("profile: exceeds 64 KiB"));
        }
        let mut profile: Self =
            serde_json::from_slice(bytes).map_err(|e| invalid(format!("profile: {e}")))?;
        profile.validate()?;
        for item in &mut profile.relevant_hosts {
            item.host = normalize_host(&item.host)?;
        }
        Ok(profile)
    }
    /// # Errors
    /// Returns invalid input, an unsupported schema, or unavailable OS randomness.
    pub fn validate(&self) -> io::Result<()> {
        if self.schema_version != 1 {
            return Err(invalid("schemaVersion: only version 1 is supported"));
        }
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(invalid(
                "id: use 1–64 ASCII letters, numbers, dot, dash or underscore",
            ));
        }
        if self.revision == 0 {
            return Err(invalid("revision: must be positive"));
        }
        text("title", &self.title, 160, true)?;
        text("instructions", &self.instructions, 8192, false)?;
        if let Some(value) = &self.support_reference {
            text("supportReference", value, 160, false)?;
        }
        if let Some(value) = &self.expected_application {
            text("expectedApplication", value, 256, false)?;
        }
        if self.steps.len() > 32
            || self.incident_prompts.len() > 8
            || self.relevant_hosts.len() > 64
        {
            return Err(invalid(
                "profile: at most 32 steps, 8 incident prompts and 64 relevant hosts",
            ));
        }
        for (index, value) in self.steps.iter().enumerate() {
            text(&format!("steps[{index}]"), value, 1024, true)?;
        }
        for (index, value) in self.incident_prompts.iter().enumerate() {
            text(&format!("incidentPrompts[{index}]"), value, 256, true)?;
        }
        for (index, item) in self.relevant_hosts.iter().enumerate() {
            normalize_host(&item.host)
                .map_err(|e| invalid(format!("relevantHosts[{index}].host: {e}")))?;
        }
        if self.duration_seconds.is_some_and(|v| v == 0 || v > 86400) {
            return Err(invalid(
                "durationSeconds: use 1–86400 or omit for manual stop",
            ));
        }
        if matches!(
            self.recording.request_body_limit,
            Some(BodyLimit::Limited { bytes: 0 })
        ) {
            return Err(invalid(
                "recording.requestBodyLimit.bytes: must be positive",
            ));
        }
        if let Some(value) = &self.recording.circular_buffer
            && !matches!(value.as_str(), "auto" | "unlimited")
        {
            buffer_bytes(value)?;
        }
        Ok(())
    }
    /// # Errors
    /// Returns invalid input, an unsupported schema, or unavailable OS randomness.
    pub fn digest(&self) -> io::Result<String> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
    pub fn relevant(&self, host: &str) -> bool {
        self.relevant_hosts.iter().any(|entry| entry.matches(host))
    }
}
impl RelevantHost {
    pub fn matches(&self, host: &str) -> bool {
        let host = host
            .trim_end_matches('.')
            .trim_matches(['[', ']'])
            .to_ascii_lowercase();
        let expected = self.host.trim_matches(['[', ']']);
        host == expected
            || (self.include_subdomains
                && host
                    .strip_suffix(expected)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    }
}
/// # Errors
/// Returns invalid input, an unsupported schema, or unavailable OS randomness.
pub fn normalize_host(value: &str) -> io::Result<String> {
    if value.is_empty()
        || value.len() > 253
        || value
            .chars()
            .any(|c| c.is_whitespace() || "/@?#\\".contains(c))
    {
        return Err(invalid(
            "expected an exact hostname, without URL, path or credentials",
        ));
    }
    if value.contains(':') && !(value.starts_with('[') && value.ends_with(']')) {
        return Err(invalid(
            "hostname cannot specify a port; bracket IPv6 addresses",
        ));
    }
    let parsed =
        url::Url::parse(&format!("http://{value}/")).map_err(|_| invalid("invalid hostname"))?;
    if parsed.port().is_some() || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid("hostnames cannot specify ports or credentials"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| invalid("hostname unavailable"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err(invalid("hostname unavailable"));
    }
    Ok(host)
}
/// # Errors
/// Returns invalid input, an unsupported schema, or unavailable OS randomness.
pub fn buffer_bytes(value: &str) -> io::Result<u64> {
    let split = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    let amount = value[..split]
        .parse::<u64>()
        .map_err(|_| invalid("recording.circularBuffer: use auto, unlimited or a byte size"))?;
    let unit = match value[split..].to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "mib" => 1_048_576,
        "gib" => 1_073_741_824,
        _ => {
            return Err(invalid(
                "recording.circularBuffer: use bytes, MB, GB, MiB or GiB",
            ));
        }
    };
    amount
        .checked_mul(unit)
        .filter(|v| *v >= 1_048_576)
        .ok_or_else(|| invalid("recording.circularBuffer: at least 1 MiB, within u64"))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncidentMarker {
    pub version: u32,
    pub id: String,
    pub run_id: String,
    pub unix_millis: u64,
    pub offset_millis: Option<u64>,
    pub origin: MarkerOrigin,
    pub creation_kind: CreationKind,
    pub note: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerOrigin {
    Cli,
    Desktop,
    Analyst,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CreationKind {
    UserMark,
    LaterAnnotation,
}
impl IncidentMarker {
    /// # Errors
    /// Returns invalid input, an unsupported schema, or unavailable OS randomness.
    pub fn new(
        run_id: String,
        offset_millis: Option<u64>,
        origin: MarkerOrigin,
        note: String,
    ) -> io::Result<Self> {
        let marker = Self {
            version: 1,
            id: new_id()?,
            run_id,
            unix_millis: unix_millis(),
            offset_millis,
            origin,
            creation_kind: CreationKind::UserMark,
            note,
        };
        marker.validate()?;
        Ok(marker)
    }
    /// # Errors
    /// Returns invalid input, an unsupported schema, or unavailable OS randomness.
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1 || !valid_id(&self.id) || !valid_id(&self.run_id) {
            return Err(invalid("incident marker: invalid version or identity"));
        }
        if self.unix_millis > 253_402_300_799_999 {
            return Err(invalid("incident marker: UTC instant out of range"));
        }
        text("incident marker note", &self.note, MAX_NOTE_BYTES, false)
    }
    /// # Errors
    /// Returns invalid input, an unsupported schema, or unavailable OS randomness.
    pub fn record(&self) -> io::Result<crate::CaptureRecord> {
        self.validate()?;
        Ok(crate::CaptureRecord {
            sequence: 0,
            exchange_id: 0,
            kind: crate::CaptureRecordKind::Unknown {
                kind: MARKER_KIND.into(),
                payload: serde_json::to_value(self).map_err(io::Error::other)?,
            },
        })
    }
}
/// # Errors
/// Returns invalid input, an unsupported schema, or unavailable OS randomness.
pub fn parse_marker(value: serde_json::Value) -> io::Result<IncidentMarker> {
    let marker: IncidentMarker =
        serde_json::from_value(value).map_err(|e| invalid(format!("incident marker: {e}")))?;
    marker.validate()?;
    Ok(marker)
}
/// # Errors
/// Returns invalid input, an unsupported schema, or unavailable OS randomness.
pub fn new_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(32), |mut result, b| {
            let _ = write!(result, "{b:02x}");
            result
        }))
}
pub fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
pub fn unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
/// # Errors
/// Returns invalid input, an unsupported schema, or unavailable OS randomness.
pub fn text(field: &str, value: &str, limit: usize, required: bool) -> io::Result<()> {
    if (required && value.trim().is_empty())
        || value.len() > limit
        || value.chars().any(|ch| {
            (ch.is_control() && !matches!(ch, '\n' | '\t'))
                || matches!(ch,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
        })
    {
        return Err(invalid(format!(
            "{field}: expected bounded plain text (at most {limit} UTF-8 bytes, no hidden controls)"
        )));
    }
    Ok(())
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Validate recognized support metadata while leaving unrelated original context intact.
/// # Errors
/// Returns invalid input, an unsupported schema, or unavailable OS randomness.
pub fn validate_context(context: &serde_json::Value) -> io::Result<()> {
    if let Some(run) = context.get("captureRunId")
        && run.as_str().is_none_or(|v| !valid_id(v))
    {
        return Err(invalid("captureRunId: invalid identity"));
    }
    if let Some(support) = context.get("supportCapture") {
        if support.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
            return Err(invalid("supportCapture.version: unsupported"));
        }
        let bytes = serde_json::to_vec(
            support
                .get("profile")
                .ok_or_else(|| invalid("supportCapture.profile: missing"))?,
        )
        .map_err(io::Error::other)?;
        let profile = SupportProfile::parse(&bytes)?;
        if support
            .get("profileDigest")
            .and_then(serde_json::Value::as_str)
            != Some(profile.digest()?.as_str())
        {
            return Err(invalid("supportCapture.profileDigest: changed profile"));
        }
    }
    if let Some(markers) = context.get("incidentMarkers") {
        let markers = markers
            .as_array()
            .ok_or_else(|| invalid("incidentMarkers: expected array"))?;
        if markers.len() > MAX_MARKERS {
            return Err(invalid("incidentMarkers: exceeds 256"));
        }
        let mut ids = std::collections::HashSet::new();
        for value in markers {
            let marker = parse_marker(value.clone())?;
            if context
                .get("captureRunId")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|run| run != marker.run_id)
            {
                return Err(invalid(
                    "incidentMarkers: run identity differs from captureRunId",
                ));
            }
            if !ids.insert(marker.id) {
                return Err(invalid("incidentMarkers: duplicate identity"));
            }
        }
    }
    validate_marker_review(context)
}

fn validate_marker_review(context: &serde_json::Value) -> io::Result<()> {
    if let Some(review) = context.get("markerReview") {
        let edited = review
            .get("editedMarkerIds")
            .and_then(serde_json::Value::as_array)
            .filter(|ids| ids.len() <= MAX_MARKERS)
            .ok_or_else(|| invalid("markerReview: invalid edited markers"))?;
        if review.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
            return Err(invalid("markerReview: unsupported version"));
        }
        let mut seen = std::collections::HashSet::new();
        for id in edited {
            let id = id
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or_else(|| invalid("markerReview: invalid marker identity"))?;
            if !seen.insert(id)
                || !context
                    .get("incidentMarkers")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|markers| markers.iter().any(|marker| marker["id"] == id))
            {
                return Err(invalid(
                    "markerReview: edited marker is missing or duplicated",
                ));
            }
        }
        if let Some(links) = review.get("links") {
            let links = links
                .as_array()
                .filter(|links| links.len() <= MAX_MARKERS)
                .ok_or_else(|| invalid("markerReview: invalid links"))?;
            let mut seen = std::collections::HashSet::new();
            for link in links {
                let id = link
                    .get("markerId")
                    .and_then(serde_json::Value::as_str)
                    .filter(|id| valid_id(id))
                    .ok_or_else(|| invalid("markerReview: invalid linked marker"))?;
                if !seen.insert(id)
                    || !context
                        .get("incidentMarkers")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|markers| markers.iter().any(|m| m["id"] == id))
                    || link
                        .get("anchorId")
                        .and_then(serde_json::Value::as_str)
                        .is_none_or(|id| !valid_id(id))
                    || !matches!(
                        link.get("boundary").and_then(serde_json::Value::as_str),
                        Some("request" | "response")
                    )
                    || !matches!(
                        link.get("timeBasis").and_then(serde_json::Value::as_str),
                        Some("request-start" | "response-headers")
                    )
                {
                    return Err(invalid("markerReview: invalid marker link"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> SupportProfile {
        SupportProfile::parse(br#"{"schemaVersion":1,"id":"fixture","revision":1,"title":"Customer issue","steps":["Open the page","Reproduce the issue"],"relevantHosts":[{"host":"EXAMPLE.test.","includeSubdomains":true}],"recording":{"redactSensitiveHeaders":true,"requestBodyLimit":{"kind":"limited","bytes":1024},"encryption":"required"}}"#).unwrap()
    }
    #[test]
    fn profiles_are_bounded_inert_and_normalize_only_hosts() {
        let p = profile();
        assert!(p.relevant("cdn.example.test"));
        assert!(!p.relevant("notexample.test"));
        assert_eq!(p.relevant_hosts[0].host, "example.test");
        assert_eq!(
            p.digest().unwrap(),
            SupportProfile::parse(&serde_json::to_vec(&p).unwrap())
                .unwrap()
                .digest()
                .unwrap()
        );
        for host in [
            "https://example.test/path",
            "user@example.test",
            "example.test:80",
            "example.test:8080",
            "x.test/../../",
            "x.test?secret",
        ] {
            assert!(normalize_host(host).is_err(), "{host}");
        }
        assert_eq!(
            normalize_host("BÜCHER.example").unwrap(),
            "xn--bcher-kva.example"
        );
    }
    #[test]
    fn unknown_commands_versions_and_hostile_notes_are_rejected() {
        let mut p = serde_json::to_value(profile()).unwrap();
        p["uploadEndpoint"] = "https://example.test/".into();
        assert!(SupportProfile::parse(&serde_json::to_vec(&p).unwrap()).is_err());
        p.as_object_mut().unwrap().remove("uploadEndpoint");
        p["schemaVersion"] = 2.into();
        assert!(SupportProfile::parse(&serde_json::to_vec(&p).unwrap()).is_err());
        assert!(SupportProfile::parse(&vec![b' '; MAX_PROFILE_BYTES + 1]).is_err());
        assert!(
            IncidentMarker::new(
                new_id().unwrap(),
                Some(0),
                MarkerOrigin::Cli,
                "secret\0control".into()
            )
            .is_err()
        );
        assert!(
            IncidentMarker::new(
                new_id().unwrap(),
                None,
                MarkerOrigin::Cli,
                "x".repeat(MAX_NOTE_BYTES + 1)
            )
            .is_err()
        );
    }
    #[test]
    fn circular_eviction_keeps_independent_markers_and_encryption() {
        for disk in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let password = crate::CapturePassword::new("fixture-password".into());
            let encoding = crate::CaptureEncoding {
                password: Some(password.clone()),
            };
            let mut ring = crate::CircularCapture::new(
                Some(32 * 1024),
                disk.then(|| root.path().to_path_buf()),
                encoding,
                crate::CaptureLimits::default(),
            )
            .unwrap();
            let marker = IncidentMarker::new(
                new_id().unwrap(),
                Some(0),
                MarkerOrigin::Cli,
                "Issue near an active stream".into(),
            )
            .unwrap();
            ring.append(&marker.record().unwrap()).unwrap();
            for id in 1..1000u128 {
                ring.append(&crate::CaptureRecord {
                    sequence: 1,
                    exchange_id: id,
                    kind: crate::CaptureRecordKind::ExchangeStarted {
                        client_addr: "127.0.0.1:1".into(),
                        client_identity: transmog_core::ClientIdentity::LocalUnknown,
                        listener_addr: "127.0.0.1:2".into(),
                        authority: "example.test:443".into(),
                        started_unix_nanos: id,
                    },
                })
                .unwrap();
                ring.append(&crate::CaptureRecord {
                    sequence: 2,
                    exchange_id: id,
                    kind: crate::CaptureRecordKind::Completed,
                })
                .unwrap();
            }
            assert!(ring.evicted_exchanges() > 0);
            let mut output = std::io::Cursor::new(Vec::new());
            ring.write(&mut output).unwrap();
            let bytes = output.into_inner();
            assert!(
                !bytes
                    .windows(marker.note.len())
                    .any(|w| w == marker.note.as_bytes())
            );
            let capture = crate::recover_with_password(
                std::io::Cursor::new(bytes),
                crate::CaptureLimits::default(),
                Some(&password),
            )
            .unwrap();
            assert!(capture.records.iter().any(|r|matches!(&r.kind,crate::CaptureRecordKind::Unknown{kind,payload}if kind==MARKER_KIND&&payload["id"]==marker.id)));
        }
    }
}

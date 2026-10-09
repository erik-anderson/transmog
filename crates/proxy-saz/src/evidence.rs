//! Bounded namespaced evidence accompanying normalized SAZ message members.
use serde::{Deserialize, Serialize};
use transmog_capture::CapturedHeader;
use transmog_core::performance::PerformanceEvidence;

/// Optional Transmog evidence for one conventional SAZ session.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionEvidence {
    /// Extension schema version, currently one.
    pub schema: u32,
    /// Native exchange identifier before archive renumbering.
    pub native_exchange_id: Option<String>,
    /// Local milestones, work, physical transports and actual boundary protocols.
    #[serde(default)]
    pub performance: PerformanceEvidence,
    /// Original entry/source association, retained as bounded untrusted data.
    pub provenance: Option<serde_json::Value>,
    /// Original source endpoint, including its port when known.
    pub client_addr: Option<String>,
    /// Original request and response fields, including redacted presence/sizes.
    pub request_headers: Option<Vec<CapturedHeader>>,
    /// Original response fields before normalized SAZ framing.
    pub response_headers: Option<Vec<CapturedHeader>>,
    /// Original request trailers.
    #[serde(default)]
    pub request_trailers: Vec<CapturedHeader>,
    /// Original response trailers.
    #[serde(default)]
    pub response_trailers: Vec<CapturedHeader>,
}
impl SessionEvidence {
    pub(crate) fn valid(&self) -> bool {
        self.schema == 1
            && self.performance.valid()
            && self
                .native_exchange_id
                .as_ref()
                .is_none_or(|id| id.len() <= 128)
            && self
                .client_addr
                .as_ref()
                .is_none_or(|addr| addr.parse::<std::net::SocketAddr>().is_ok())
            && [&self.request_trailers, &self.response_trailers]
                .into_iter()
                .chain(self.request_headers.iter())
                .chain(self.response_headers.iter())
                .all(|fields| {
                    fields.len() <= 16384
                        && fields.iter().all(|field| match &field.value {
                            Some(value) => transmog_core::HeaderField::try_new(
                                field.name.clone(),
                                value.clone(),
                            )
                            .is_ok(),
                            None => transmog_core::HeaderField::from_redacted(
                                field.name.clone(),
                                field.original_value_bytes,
                            )
                            .is_ok(),
                        })
                })
    }
}

pub(crate) fn write_sources<W: std::io::Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    capture: &transmog_capture::RecoveredCapture,
    options: zip::write::SimpleFileOptions,
    remaining: usize,
) -> Result<usize, crate::SazError> {
    let sources = capture
        .records
        .iter()
        .filter_map(|record| match &record.kind {
            transmog_capture::CaptureRecordKind::Unknown { kind, payload }
                if record.exchange_id == 0 && kind == "trace-source" =>
            {
                Some(payload)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if sources.is_empty() {
        return Ok(0);
    }
    if remaining == 0 {
        return Err(crate::SazError::EntryLimitExceeded);
    }
    let bytes = serde_json::to_vec(&sources)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(crate::SazError::MetadataLimitExceeded);
    }
    crate::write_member(writer, "transmog/trace-sources.json", &bytes, options)?;
    Ok(1)
}
pub(crate) fn write_session<W: std::io::Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    session: &mut crate::Session,
    native_id: u128,
    id: usize,
    options: zip::write::SimpleFileOptions,
    remaining: usize,
) -> Result<usize, crate::SazError> {
    if remaining == 0 {
        return Err(crate::SazError::EntryLimitExceeded);
    }
    session.evidence.schema = 1;
    session.evidence.native_exchange_id = Some(format!("{native_id:032x}"));
    session.evidence.request_headers = session
        .request
        .as_ref()
        .map(|request| request.headers.clone());
    session.evidence.response_headers = session
        .response
        .as_ref()
        .map(|response| response.headers.clone());
    if !session.evidence.valid() {
        return Err(crate::SazError::InvalidArchive);
    }
    let bytes = serde_json::to_vec(&session.evidence)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(crate::SazError::MetadataLimitExceeded);
    }
    crate::write_member(
        writer,
        &format!("transmog/session-{id}.json"),
        &bytes,
        options,
    )?;
    Ok(1)
}

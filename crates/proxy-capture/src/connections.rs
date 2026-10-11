//! Shared immutable transport facts; per-request measurements remain in snapshots.
use crate::{CaptureError, CaptureRecord, CaptureRecordKind, FrameCodec, StoredRecord};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use transmog_core::{
    TlsEvidence,
    performance::{
        ConnectionTiming, PerformanceEvidence, ProtocolObservation, QuicObservation,
        SocketIoObservation, TcpObservation, TimingPoint, TransportObservation, TransportOutcome,
        WorkTiming,
    },
};

pub(crate) const DEFINITION_KIND: &str = "connection-context";
const MAX_CONTEXT_BYTES: usize = 256 * 1024 * 1024;
pub(crate) type Definitions = HashMap<String, Arc<ConnectionContext>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConnectionContext {
    tunnel_destination: Option<String>,
    outcome: TransportOutcome,
    leg: String,
    connection_id: String,
    peer: Option<String>,
    local: Option<String>,
    tls_version: Option<String>,
    tls_resumed: Option<bool>,
    tls: Option<TlsEvidence>,
    cipher: Option<String>,
    alpn: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransportSample {
    context: String,
    sampled_offset_micros: i64,
    shared: bool,
    setup_timings: Vec<ConnectionTiming>,
    dns_micros: Option<u64>,
    tcp_micros: Option<u64>,
    tls_micros: Option<u64>,
    bytes_read: Option<u64>,
    bytes_written: Option<u64>,
    tcp: Option<TcpObservation>,
    tcp_sampled_offset_micros: Option<i64>,
    socket_io: Option<SocketIoObservation>,
    quic: Option<QuicObservation>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReferencedPerformance {
    points: Vec<TimingPoint>,
    transports: Vec<TransportSample>,
    protocols: Vec<ProtocolObservation>,
    work: Vec<WorkTiming>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    sha256: String,
    context: ConnectionContext,
}

fn canonical(context: &ConnectionContext) -> Result<Vec<u8>, CaptureError> {
    // Explicit sorting is stable even if serde_json's preserve_order feature is enabled.
    let mut value = serde_json::to_value(context)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}
fn identity(context: &ConnectionContext) -> Result<String, CaptureError> {
    Ok(format!("{:x}", Sha256::digest(canonical(context)?)))
}
fn charge(context: &ConnectionContext) -> Result<usize, CaptureError> {
    // Bound both JSON size and dictionary/object overhead, including tiny definitions.
    canonical(context)?
        .len()
        .checked_mul(2)
        .and_then(|n| n.checked_add(std::mem::size_of::<ConnectionContext>() + 256))
        .ok_or(CaptureError::QuotaExceeded)
}
fn validate_definition(record: &StoredRecord) -> Result<Definition, CaptureError> {
    if record.exchange_id != 0 || record.sequence != 0 || record.body_index.is_some() {
        return Err(CaptureError::MalformedRecord(
            "connection context is not global metadata",
        ));
    }
    let definition: Definition = serde_json::from_value(record.payload.clone())?;
    let evidence = PerformanceEvidence {
        transports: vec![definition.context.combine(TransportSample::default())],
        ..Default::default()
    };
    if !evidence.valid() || identity(&definition.context)? != definition.sha256 {
        return Err(CaptureError::MalformedRecord(
            "invalid connection context definition",
        ));
    }
    Ok(definition)
}

impl ConnectionContext {
    fn split(transport: &TransportObservation) -> Self {
        Self {
            tunnel_destination: transport.tunnel_destination.clone(),
            outcome: transport.outcome,
            leg: transport.leg.clone(),
            connection_id: transport.connection_id.clone(),
            peer: transport.peer.clone(),
            local: transport.local.clone(),
            tls_version: transport.tls_version.clone(),
            tls_resumed: transport.tls_resumed,
            tls: transport.tls.clone(),
            cipher: transport.cipher.clone(),
            alpn: transport.alpn.clone(),
        }
    }
    fn combine(&self, sample: TransportSample) -> TransportObservation {
        TransportObservation {
            tunnel_destination: self.tunnel_destination.clone(),
            outcome: self.outcome,
            leg: self.leg.clone(),
            connection_id: self.connection_id.clone(),
            peer: self.peer.clone(),
            local: self.local.clone(),
            tls_version: self.tls_version.clone(),
            tls_resumed: self.tls_resumed,
            tls: self.tls.clone(),
            cipher: self.cipher.clone(),
            alpn: self.alpn.clone(),
            sampled_offset_micros: sample.sampled_offset_micros,
            shared: sample.shared,
            setup_timings: sample.setup_timings,
            dns_micros: sample.dns_micros,
            tcp_micros: sample.tcp_micros,
            tls_micros: sample.tls_micros,
            bytes_read: sample.bytes_read,
            bytes_written: sample.bytes_written,
            tcp: sample.tcp,
            tcp_sampled_offset_micros: sample.tcp_sampled_offset_micros,
            socket_io: sample.socket_io,
            quic: sample.quic,
        }
    }
}

pub(crate) fn split(
    record: &CaptureRecord,
    evidence: &PerformanceEvidence,
) -> Result<(Vec<CaptureRecord>, CaptureRecord), CaptureError> {
    if !evidence.valid() {
        return Err(CaptureError::MalformedRecord(
            "invalid performance evidence",
        ));
    }
    let mut definitions = Vec::new();
    let mut transports = Vec::new();
    for transport in &evidence.transports {
        let context = ConnectionContext::split(transport);
        let sha256 = identity(&context)?;
        definitions.push(CaptureRecord {
            sequence: 0,
            exchange_id: 0,
            kind: CaptureRecordKind::Unknown {
                kind: DEFINITION_KIND.into(),
                payload: serde_json::to_value(Definition {
                    sha256: sha256.clone(),
                    context,
                })?,
            },
        });
        transports.push(TransportSample {
            context: sha256,
            sampled_offset_micros: transport.sampled_offset_micros,
            shared: transport.shared,
            setup_timings: transport.setup_timings.clone(),
            dns_micros: transport.dns_micros,
            tcp_micros: transport.tcp_micros,
            tls_micros: transport.tls_micros,
            bytes_read: transport.bytes_read,
            bytes_written: transport.bytes_written,
            tcp: transport.tcp.clone(),
            tcp_sampled_offset_micros: transport.tcp_sampled_offset_micros,
            socket_io: transport.socket_io.clone(),
            quic: transport.quic.clone(),
        });
    }
    let payload = ReferencedPerformance {
        points: evidence.points.clone(),
        transports,
        protocols: evidence.protocols.clone(),
        work: evidence.work.clone(),
    };
    Ok((
        definitions,
        CaptureRecord {
            sequence: record.sequence,
            exchange_id: record.exchange_id,
            kind: CaptureRecordKind::Unknown {
                kind: "performance".into(),
                payload: serde_json::to_value(payload)?,
            },
        },
    ))
}

#[derive(Default)]
pub(crate) struct WriterContexts {
    seen: HashSet<String>,
    bytes: usize,
}
impl WriterContexts {
    pub(crate) fn insert(&mut self, record: &CaptureRecord) -> Result<bool, CaptureError> {
        let definition = validate_definition(&crate::store(record)?)?;
        if self.seen.contains(&definition.sha256) {
            return Ok(false);
        }
        let bytes = self
            .bytes
            .checked_add(charge(&definition.context)?)
            .ok_or(CaptureError::QuotaExceeded)?;
        if bytes > MAX_CONTEXT_BYTES {
            return Err(CaptureError::QuotaExceeded);
        }
        self.seen.insert(definition.sha256);
        self.bytes = bytes;
        Ok(true)
    }
}

#[derive(Default)]
pub(crate) struct ReaderContexts {
    definitions: Definitions,
    bytes: usize,
}
impl ReaderContexts {
    pub(crate) fn prepare(
        &mut self,
        stored: &StoredRecord,
        base: &FrameCodec,
    ) -> Result<FrameCodec, CaptureError> {
        if stored.revision != crate::CAPTURE_FORMAT_REVISION {
            return Err(CaptureError::UnsupportedRevision(stored.revision));
        }
        if stored.kind == DEFINITION_KIND {
            let definition = validate_definition(stored)?;
            if !self.definitions.contains_key(&definition.sha256) {
                let bytes = self
                    .bytes
                    .checked_add(charge(&definition.context)?)
                    .ok_or(CaptureError::QuotaExceeded)?;
                if bytes > MAX_CONTEXT_BYTES {
                    return Err(CaptureError::QuotaExceeded);
                }
                self.definitions
                    .insert(definition.sha256, Arc::new(definition.context));
                self.bytes = bytes;
            }
        }
        let mut codec = base.clone();
        if stored.kind == "performance" {
            let referenced: ReferencedPerformance = serde_json::from_value(stored.payload.clone())?;
            if referenced.transports.len() > 16 {
                return Err(CaptureError::QuotaExceeded);
            }
            let mut selected = Definitions::new();
            for transport in referenced.transports {
                let context = self
                    .definitions
                    .get(&transport.context)
                    .ok_or(CaptureError::MalformedRecord("missing connection context"))?;
                selected.insert(transport.context, context.clone());
            }
            // Immutable, at most sixteen references. Lazy body frames retain no dictionary.
            codec.connections = Arc::new(selected);
        }
        Ok(codec)
    }
}

pub(crate) fn expand(
    stored: &StoredRecord,
    definitions: &Definitions,
) -> Result<Option<PerformanceEvidence>, CaptureError> {
    if stored.kind == DEFINITION_KIND {
        validate_definition(stored)?;
    } else if stored.kind == "performance" {
        let referenced: ReferencedPerformance = serde_json::from_value(stored.payload.clone())?;
        if referenced.transports.len() > 16 {
            return Err(CaptureError::QuotaExceeded);
        }
        let mut evidence = PerformanceEvidence {
            points: referenced.points,
            transports: Vec::new(),
            protocols: referenced.protocols,
            work: referenced.work,
        };
        for sample in referenced.transports {
            let context = definitions
                .get(&sample.context)
                .ok_or(CaptureError::MalformedRecord("missing connection context"))?;
            evidence.transports.push(context.combine(sample));
        }
        if !evidence.valid() {
            return Err(CaptureError::MalformedRecord(
                "invalid performance evidence",
            ));
        }
        return Ok(Some(evidence));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition() -> CaptureRecord {
        let evidence = PerformanceEvidence {
            transports: vec![TransportObservation {
                leg: "upstream".into(),
                connection_id: "bound-test".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        split(
            &CaptureRecord {
                sequence: 1,
                exchange_id: 1,
                kind: CaptureRecordKind::Performance(evidence.clone()),
            },
            &evidence,
        )
        .unwrap()
        .0
        .remove(0)
    }
    #[test]
    fn context_dictionary_bounds_and_duplicate_accounting() {
        let record = definition();
        let stored = crate::store(&record).unwrap();
        let (base, _) =
            FrameCodec::create(&mut Vec::new(), &crate::CaptureEncoding::default()).unwrap();
        let mut reader = ReaderContexts::default();
        reader.prepare(&stored, &base).unwrap();
        let charged = reader.bytes;
        reader.prepare(&stored, &base).unwrap();
        assert_eq!(reader.bytes, charged);
        assert_eq!(reader.definitions.len(), 1);
        let mut reader = ReaderContexts {
            bytes: MAX_CONTEXT_BYTES,
            ..Default::default()
        };
        assert!(matches!(
            reader.prepare(&stored, &base),
            Err(CaptureError::QuotaExceeded)
        ));
        assert!(reader.definitions.is_empty());
        let mut writer = WriterContexts {
            bytes: MAX_CONTEXT_BYTES,
            ..Default::default()
        };
        assert!(matches!(
            writer.insert(&record),
            Err(CaptureError::QuotaExceeded)
        ));
        assert!(writer.seen.is_empty());
        let mut writer = WriterContexts::default();
        assert!(writer.insert(&record).unwrap());
        assert!(!writer.insert(&record).unwrap());
        assert_eq!(writer.bytes, charged);
    }
    #[test]
    fn invalid_revision_and_body_descriptor_cannot_install_a_definition() {
        let (base, _) =
            FrameCodec::create(&mut Vec::new(), &crate::CaptureEncoding::default()).unwrap();
        let mut reader = ReaderContexts::default();
        let mut stored = crate::store(&definition()).unwrap();
        stored.revision += 1;
        assert!(reader.prepare(&stored, &base).is_err());
        assert!(reader.definitions.is_empty());
        stored.revision = crate::CAPTURE_FORMAT_REVISION;
        stored.body_index = Some(crate::StoredBodyIndex {
            frame_bytes: 21,
            bytes: 0,
            digest: [0; 32],
        });
        assert!(reader.prepare(&stored, &base).is_err());
        assert!(reader.definitions.is_empty());
    }
}

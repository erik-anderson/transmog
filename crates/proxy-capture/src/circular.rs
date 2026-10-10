//! Bounded circular capture retaining complete exchange groups in admission order.
use crate::{
    CaptureEncoding, CaptureError, CaptureLimits, CaptureRecord, CaptureRecordKind, CaptureWriter,
    FrameCodec, read_indexed_frame_with_codec,
};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

struct Frame {
    certificate: Option<String>,
    index: u64,
    bytes: u64,
    offset: u64,
    memory: Option<Vec<u8>>,
}
struct Entry {
    certificates: BTreeSet<String>,
    frames: Vec<Frame>,
    disk: Option<tempfile::NamedTempFile>,
    charged: u64,
    terminal: bool,
}

/// Independently compressed circular trace storage. Memory mode creates no files.
/// Disk mode stores encoded frames in owned temporary files; encrypted runs store ciphertext.
pub struct CircularCapture {
    certificate_references: HashMap<String, usize>,
    encoder: CaptureWriter<Vec<u8>>,
    encoding: CaptureEncoding,
    limits: CaptureLimits,
    maximum: Option<u64>,
    directory: Option<PathBuf>,
    entries: HashMap<u128, Entry>,
    order: VecDeque<u128>,
    retained: u64,
    evicted: u64,
}
impl CircularCapture {
    /// Creates a circular capture; directory None keeps all retained frames in memory.
    ///
    /// # Errors
    /// Rejects zero limits or failed encoding/disk initialization.
    pub fn new(
        maximum: Option<u64>,
        directory: Option<PathBuf>,
        encoding: CaptureEncoding,
        limits: CaptureLimits,
    ) -> Result<Self, CaptureError> {
        if maximum == Some(0) {
            return Err(CaptureError::InvalidLimits);
        }
        if let Some(root) = &directory {
            std::fs::create_dir_all(root)?;
        }
        let mut encoder = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits {
                max_file_bytes: u64::MAX,
                max_records: usize::MAX,
                ..limits
            },
            &encoding,
        )?;
        encoder.output.clear();
        Ok(Self {
            certificate_references: HashMap::new(),
            encoder,
            encoding,
            limits,
            maximum,
            directory,
            entries: HashMap::new(),
            order: VecDeque::new(),
            retained: 0,
            evicted: 0,
        })
    }
    /// Retains a record, evicting old terminal exchanges before active ones when necessary.
    /// Eviction affects retained evidence, never forwarding. Later fragments of evicted exchanges are omitted.
    ///
    /// # Errors
    /// Returns a record bound, metadata quota, compression or disk error.
    pub fn append(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        let id = record.exchange_id;
        if !self.entries.contains_key(&id)
            && id != 0
            && !matches!(record.kind, CaptureRecordKind::ExchangeStarted { .. })
        {
            return Ok(());
        }
        self.ensure_entry(id)?;
        let index_base = self.encoder.records_written;
        self.record_certificate_references(record)?;
        self.encoder.append(record)?;
        let encoded = std::mem::take(&mut self.encoder.output);
        let mut scanner = crate::CaptureReader {
            certificates: HashMap::new(),
            certificate_bytes: 0,
            input: std::io::Cursor::new(&encoded),
            limits: crate::CaptureLimits {
                max_file_bytes: u64::MAX,
                max_records: usize::MAX,
                ..self.limits
            },
            records: 0,
            valid_bytes: 0,
            truncated_tail: false,
            sealed: false,
            done: false,
            codec: self.encoder.codec.clone(),
            index_base,
        };
        while let Some(frame) = scanner.read_next_indexed()? {
            let start = usize::try_from(frame.offset).map_err(|_| CaptureError::QuotaExceeded)?;
            let end = usize::try_from(frame.offset + frame.frame_bytes)
                .map_err(|_| CaptureError::QuotaExceeded)?;
            let bytes = encoded.get(start..end).ok_or(CaptureError::InvalidMagic)?;
            let frame_id = frame.record.exchange_id;
            self.ensure_entry(frame_id)?;
            let entry = self
                .entries
                .get_mut(&frame_id)
                .ok_or(CaptureError::InvalidMagic)?;
            let (offset, memory) = if let Some(file) = &mut entry.disk {
                let offset = file.as_file_mut().seek(SeekFrom::End(0))?;
                file.write_all(bytes)?;
                (offset, None)
            } else {
                (0, Some(bytes.to_vec()))
            };
            let charge = frame.frame_bytes + std::mem::size_of::<Frame>() as u64;
            entry.frames.push(Frame {
                certificate: match &frame.record.kind {
                    CaptureRecordKind::Unknown { kind, payload }
                        if kind == "public-certificate" =>
                    {
                        payload["sha256"].as_str().map(str::to_owned)
                    }
                    _ => None,
                },
                index: frame.index,
                bytes: frame.frame_bytes,
                offset,
                memory,
            });
            entry.charged = entry.charged.saturating_add(charge);
            self.retained = self.retained.saturating_add(charge);
        }
        if matches!(
            record.kind,
            CaptureRecordKind::Completed | CaptureRecordKind::Failed { .. }
        ) {
            self.entries
                .get_mut(&id)
                .ok_or(CaptureError::InvalidMagic)?
                .terminal = true;
        }
        while self.maximum.is_some_and(|maximum| self.retained > maximum) {
            let candidate = self
                .order
                .iter()
                .copied()
                .find(|id| self.entries.get(id).is_some_and(|entry| entry.terminal))
                .or_else(|| self.order.front().copied());
            let Some(candidate) = candidate else {
                return Err(CaptureError::QuotaExceeded);
            };
            self.order.retain(|id| *id != candidate);
            if let Some(entry) = self.entries.remove(&candidate) {
                self.retained = self.retained.saturating_sub(entry.charged);
                self.evicted = self.evicted.saturating_add(1);
                self.release_certificates(entry.certificates)?;
            }
        }
        Ok(())
    }
    fn record_certificate_references(
        &mut self,
        record: &CaptureRecord,
    ) -> Result<(), CaptureError> {
        if let CaptureRecordKind::Performance(evidence) = &record.kind {
            let entry = self
                .entries
                .get_mut(&record.exchange_id)
                .ok_or(CaptureError::InvalidMagic)?;
            for certificate in evidence
                .transports
                .iter()
                .filter_map(|transport| transport.tls.as_ref())
                .flat_map(|tls| tls.server_chain.iter().chain(&tls.client_chain))
            {
                if entry.certificates.insert(certificate.sha256.clone()) {
                    *self
                        .certificate_references
                        .entry(certificate.sha256.clone())
                        .or_default() += 1;
                }
            }
        }
        Ok(())
    }

    fn ensure_entry(&mut self, id: u128) -> Result<(), CaptureError> {
        if !self.entries.contains_key(&id) {
            self.entries.insert(
                id,
                Entry {
                    certificates: BTreeSet::new(),
                    frames: Vec::new(),
                    disk: self
                        .directory
                        .as_ref()
                        .map(tempfile::NamedTempFile::new_in)
                        .transpose()?,
                    charged: 0,
                    terminal: false,
                },
            );
            if id != 0 {
                self.order.push_back(id);
            }
        }
        Ok(())
    }

    fn release_certificates(&mut self, certificates: BTreeSet<String>) -> Result<(), CaptureError> {
        let mut unused = BTreeSet::new();
        for digest in certificates {
            if let Some(count) = self.certificate_references.get_mut(&digest) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.certificate_references.remove(&digest);
                    unused.insert(digest);
                }
            }
        }
        self.prune_certificates(&unused)
    }

    fn prune_certificates(&mut self, unused: &BTreeSet<String>) -> Result<(), CaptureError> {
        if unused.is_empty() {
            return Ok(());
        }
        let Some(global) = self.entries.get_mut(&0) else {
            return Ok(());
        };
        let mut removed = 0;
        global.frames.retain(|frame| {
            if let Some(digest) = &frame.certificate
                && unused.contains(digest)
            {
                removed += frame.bytes + std::mem::size_of::<Frame>() as u64;
                self.encoder.certificates.remove(digest);
                false
            } else {
                true
            }
        });
        if removed == 0 {
            return Ok(());
        }
        global.charged = global.charged.saturating_sub(removed);
        self.retained = self.retained.saturating_sub(removed);
        if let Some(file) = &mut global.disk {
            let mut compact = tempfile::NamedTempFile::new_in(
                self.directory.as_ref().ok_or(CaptureError::InvalidMagic)?,
            )?;
            let mut offsets = Vec::with_capacity(global.frames.len());
            for frame in &global.frames {
                file.as_file_mut().seek(SeekFrom::Start(frame.offset))?;
                let offset = compact.as_file_mut().seek(SeekFrom::End(0))?;
                let copied =
                    std::io::copy(&mut file.as_file_mut().take(frame.bytes), &mut compact)?;
                if copied != frame.bytes {
                    return Err(CaptureError::TruncatedPreamble);
                }
                offsets.push(offset);
            }
            for (frame, offset) in global.frames.iter_mut().zip(offsets) {
                frame.offset = offset;
            }
            *file = compact;
        }
        Ok(())
    }

    /// Whether an exchange still has retained evidence.
    pub fn retains_exchange(&self, id: u128) -> bool {
        self.entries.contains_key(&id)
    }
    /// Encoded retained bytes plus in-memory frame descriptor sizes.
    pub fn retained_bytes(&self) -> u64 {
        self.retained
    }
    /// Number of exchange groups removed by the circular quota.
    pub fn evicted_exchanges(&self) -> u64 {
        self.evicted
    }
    /// Writes and seals just the retained trace, decoding at most one frame at a time.
    ///
    /// # Errors
    /// Returns authentication, bounded decode, quota or destination errors.
    pub fn write(&mut self, output: impl Write) -> Result<u64, CaptureError> {
        let mut writer = CaptureWriter::with_encoding(output, self.limits, &self.encoding)?;
        let codec = self.encoder.codec.clone();
        for id in std::iter::once(0).chain(self.order.iter().copied()) {
            if let Some(entry) = self.entries.get_mut(&id) {
                write_entry(entry, &codec, &mut writer, self.limits)?;
            }
        }
        writer.append(&CaptureRecord {exchange_id:0,sequence:0,kind:CaptureRecordKind::Unknown {kind:"circular-retention".into(),payload:serde_json::json!({"evictedExchanges":self.evicted,"retainedEncodedBytes":self.retained,"maximumBytes":self.maximum,"storage":if self.directory.is_some(){"disk"}else{"memory"}})}})?;
        writer.seal()?;
        Ok(writer.bytes_written())
    }
}
fn write_entry(
    entry: &mut Entry,
    codec: &FrameCodec,
    writer: &mut CaptureWriter<impl Write>,
    limits: CaptureLimits,
) -> Result<(), CaptureError> {
    for frame in &entry.frames {
        let record = if let Some(bytes) = &frame.memory {
            read_indexed_frame_with_codec(
                bytes.as_slice(),
                frame.bytes,
                limits,
                codec,
                frame.index,
            )?
        } else {
            let file = entry.disk.as_mut().ok_or(CaptureError::InvalidMagic)?;
            file.as_file_mut().seek(SeekFrom::Start(frame.offset))?;
            read_indexed_frame_with_codec(
                file.as_file_mut().take(frame.bytes),
                frame.bytes,
                limits,
                codec,
                frame.index,
            )?
        };
        writer.append(&record)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn started(id: u128) -> CaptureRecord {
        CaptureRecord {
            exchange_id: id,
            sequence: 1,
            kind: CaptureRecordKind::ExchangeStarted {
                client_addr: "127.0.0.1:1".into(),
                client_identity: transmog_core::ClientIdentity::LocalUnknown,
                listener_addr: "127.0.0.1:2".into(),
                authority: "example.test".into(),
                started_unix_nanos: 1,
            },
        }
    }
    fn terminal(id: u128) -> CaptureRecord {
        CaptureRecord {
            exchange_id: id,
            sequence: 2,
            kind: CaptureRecordKind::Completed,
        }
    }
    #[test]
    fn memory_circular_keeps_newest_groups_and_no_late_fragments() {
        let root = tempfile::tempdir().unwrap();
        let forbidden = root.path().join("no-files");
        let mut ring = CircularCapture::new(
            Some(750),
            None,
            CaptureEncoding::default(),
            CaptureLimits::default(),
        )
        .unwrap();
        for id in 1..=10 {
            ring.append(&started(id)).unwrap();
            ring.append(&terminal(id)).unwrap();
            assert!(ring.retained_bytes() <= 750);
        }
        ring.append(&terminal(1)).unwrap();
        let mut output = Vec::new();
        ring.write(&mut output).unwrap();
        let capture = crate::recover(output.as_slice(), CaptureLimits::default()).unwrap();
        let ids = capture
            .records
            .iter()
            .filter_map(|record| (record.exchange_id > 0).then_some(record.exchange_id))
            .collect::<std::collections::BTreeSet<_>>();
        assert!(ids.contains(&10));
        assert!(!ids.contains(&1));
        assert!(ring.evicted_exchanges() > 0);
        assert!(!forbidden.exists());
        assert!(capture.sealed);
    }
    #[test]
    fn circular_eviction_retains_shared_certificate_definitions_for_later_requests() {
        let certificate = boring::x509::X509::from_pem(include_bytes!(
            "../../app/tests/fixtures/certificates/root.pem"
        ))
        .unwrap();
        let digest = certificate
            .digest(boring::hash::MessageDigest::sha256())
            .unwrap();
        let sha256 = digest.iter().fold(String::new(), |mut output, byte| {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
            output
        });
        let public = transmog_core::PublicCertificate {
            sha256,
            der: certificate.to_der().unwrap().into(),
        };
        let evidence = transmog_core::performance::PerformanceEvidence {
            transports: vec![transmog_core::performance::TransportObservation {
                leg: "upstream".into(),
                connection_id: "shared-certificate".into(),
                tls: Some(transmog_core::TlsEvidence {
                    server_chain: vec![public.clone()],
                    ..transmog_core::TlsEvidence::default()
                }),
                ..transmog_core::performance::TransportObservation::default()
            }],
            ..transmog_core::performance::PerformanceEvidence::default()
        };
        let mut ring = CircularCapture::new(
            None,
            None,
            CaptureEncoding::default(),
            CaptureLimits::default(),
        )
        .unwrap();
        for id in 1..=5 {
            ring.append(&started(id)).unwrap();
            ring.append(&CaptureRecord {
                exchange_id: id,
                sequence: 2,
                kind: CaptureRecordKind::Performance(evidence.clone()),
            })
            .unwrap();
            ring.append(&terminal(id)).unwrap();
            if id == 1 {
                ring.maximum = Some(ring.retained_bytes() + 200);
            }
        }
        assert!(!ring.retains_exchange(1));
        assert!(ring.retains_exchange(5));
        let mut bytes = Vec::new();
        ring.write(&mut bytes).unwrap();
        let saved = crate::recover(bytes.as_slice(), CaptureLimits::default()).unwrap();
        assert_eq!(saved.records.iter().filter(|record|matches!(&record.kind,CaptureRecordKind::Unknown {kind,..} if kind=="public-certificate")).count(),1);
        for record in saved.records {
            if let CaptureRecordKind::Performance(evidence) = record.kind {
                assert_eq!(
                    evidence.transports[0].tls.as_ref().unwrap().server_chain[0].der,
                    public.der
                );
            }
        }
    }
    #[test]
    fn circular_eviction_prunes_unused_certificates_and_reclaims_disk_frames() {
        let root = tempfile::tempdir().unwrap();
        let certificates = [
            include_bytes!("../../app/tests/fixtures/certificates/root.pem").as_slice(),
            include_bytes!("../../app/tests/fixtures/certificates/leaf.pem").as_slice(),
        ]
        .map(|pem| {
            let parsed = boring::x509::X509::from_pem(pem).unwrap();
            let digest = parsed
                .digest(boring::hash::MessageDigest::sha256())
                .unwrap();
            let sha256 = digest.iter().fold(String::new(), |mut output, byte| {
                use std::fmt::Write as _;
                let _ = write!(output, "{byte:02x}");
                output
            });
            transmog_core::PublicCertificate {
                sha256,
                der: parsed.to_der().unwrap().into(),
            }
        });
        for directory in [None, Some(root.path().to_path_buf())] {
            let mut ring = CircularCapture::new(
                None,
                directory,
                CaptureEncoding::default(),
                CaptureLimits::default(),
            )
            .unwrap();
            for id in 1..=10 {
                let certificate = certificates[id as usize % 2].clone();
                let evidence = transmog_core::performance::PerformanceEvidence {
                    transports: vec![transmog_core::performance::TransportObservation {
                        leg: "upstream".into(),
                        connection_id: "changing-certificate".into(),
                        tls: Some(transmog_core::TlsEvidence {
                            server_chain: vec![certificate],
                            ..transmog_core::TlsEvidence::default()
                        }),
                        ..transmog_core::performance::TransportObservation::default()
                    }],
                    ..transmog_core::performance::PerformanceEvidence::default()
                };
                ring.append(&started(id)).unwrap();
                ring.append(&CaptureRecord {
                    exchange_id: id,
                    sequence: 2,
                    kind: CaptureRecordKind::Performance(evidence),
                })
                .unwrap();
                ring.append(&terminal(id)).unwrap();
                if id == 1 {
                    ring.maximum = Some(ring.retained_bytes() + 100);
                }
            }
            assert!(ring.retains_exchange(10));
            assert!(!ring.retains_exchange(1));
            assert_eq!(ring.encoder.certificates.len(), 1);
            let global = ring.entries.get(&0).unwrap();
            if let Some(file) = &global.disk {
                assert_eq!(
                    file.as_file().metadata().unwrap().len(),
                    global.frames.iter().map(|frame| frame.bytes).sum::<u64>()
                );
            }
            let mut bytes = Vec::new();
            ring.write(&mut bytes).unwrap();
            let saved = crate::recover(bytes.as_slice(), CaptureLimits::default()).unwrap();
            for record in saved.records {
                if let CaptureRecordKind::Performance(evidence) = record.kind {
                    assert!(
                        !evidence.transports[0].tls.as_ref().unwrap().server_chain[0]
                            .der
                            .is_empty()
                    );
                }
            }
        }
    }
    #[test]
    fn encrypted_disk_circular_rewrites_only_retained_frames() {
        let root = tempfile::tempdir().unwrap();
        let password = crate::CapturePassword::new("password".into());
        let mut ring = CircularCapture::new(
            Some(900),
            Some(root.path().into()),
            CaptureEncoding {
                password: Some(password.clone()),
            },
            CaptureLimits::default(),
        )
        .unwrap();
        for id in 1..=5 {
            ring.append(&started(id)).unwrap();
            ring.append(&terminal(id)).unwrap();
        }
        let mut bytes = Vec::new();
        ring.write(&mut bytes).unwrap();
        let result = crate::recover_with_password(
            bytes.as_slice(),
            CaptureLimits::default(),
            Some(&password),
        )
        .unwrap();
        assert!(result.sealed);
        assert!(result.records.iter().any(|record| record.exchange_id == 5));
        for file in std::fs::read_dir(root.path()).unwrap() {
            assert!(
                !std::fs::read(file.unwrap().path())
                    .unwrap()
                    .windows(12)
                    .any(|part| part == b"example.test")
            );
        }
        drop(ring);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

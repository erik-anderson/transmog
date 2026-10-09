//! Bounded circular capture retaining complete exchange groups in admission order.
use crate::{
    CaptureEncoding, CaptureError, CaptureLimits, CaptureRecord, CaptureRecordKind, CaptureWriter,
    FrameCodec, read_indexed_frame_with_codec,
};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

struct Frame {
    index: u64,
    bytes: u64,
    offset: u64,
    memory: Option<Vec<u8>>,
}
struct Entry {
    frames: Vec<Frame>,
    disk: Option<tempfile::NamedTempFile>,
    charged: u64,
    terminal: bool,
}

/// Independently compressed circular trace storage. Memory mode creates no files.
/// Disk mode stores encoded frames in owned temporary files; encrypted runs store ciphertext.
pub struct CircularCapture {
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
        if !self.entries.contains_key(&id) {
            if id != 0 && !matches!(record.kind, CaptureRecordKind::ExchangeStarted { .. }) {
                return Ok(());
            }
            let disk = self
                .directory
                .as_ref()
                .map(tempfile::NamedTempFile::new_in)
                .transpose()?;
            self.entries.insert(
                id,
                Entry {
                    frames: Vec::new(),
                    disk,
                    charged: 0,
                    terminal: false,
                },
            );
            if id != 0 {
                self.order.push_back(id);
            }
        }
        let mut index = self.encoder.records_written;
        self.encoder.append(record)?;
        let encoded = std::mem::take(&mut self.encoder.output);
        let mut position = 0;
        while position < encoded.len() {
            let length = u32::from_le_bytes(
                encoded[position..position + 4]
                    .try_into()
                    .map_err(|_| CaptureError::InvalidMagic)?,
            ) as usize
                + 8;
            let bytes = encoded
                .get(position..position + length)
                .ok_or(CaptureError::InvalidMagic)?;
            let entry = self
                .entries
                .get_mut(&id)
                .ok_or(CaptureError::InvalidMagic)?;
            let (offset, memory) = if let Some(file) = &mut entry.disk {
                let offset = file.as_file_mut().seek(SeekFrom::End(0))?;
                file.write_all(bytes)?;
                (offset, None)
            } else {
                (0, Some(bytes.to_vec()))
            };
            let charge = length as u64 + std::mem::size_of::<Frame>() as u64;
            entry.frames.push(Frame {
                index,
                bytes: length as u64,
                offset,
                memory,
            });
            entry.charged = entry.charged.saturating_add(charge);
            self.retained = self.retained.saturating_add(charge);
            position += length;
            index += 1;
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
            }
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

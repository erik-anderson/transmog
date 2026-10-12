//! Indexed fixed-block container. Record encoding and physical encryption are independent.
use super::{
    CaptureBodyIndex, CaptureEncoding, CaptureError, CaptureFrame, CaptureLimits, CapturePassword,
    CaptureRecord, CaptureRecordKind, CertificateDefinition, FrameCodec, MAGIC, StoredBodyIndex,
    StoredRecord, store,
};
use crate::compression::{
    BODY_CHUNK_BYTES, BodyCompression, ChunkCompressor, CompressedChunk, CompressionPool,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::{
    collections::{BTreeSet, HashMap},
    io::{self, Read, Seek, SeekFrom, Write},
};

pub(crate) const BLOCK_PLAIN: usize = 16 * 1024;
#[cfg(test)]
pub(crate) const BLOCK_BYTES: usize = BLOCK_PLAIN + 16;
#[cfg(test)]
const PAYLOAD: usize = BLOCK_PLAIN - 5;
#[path = "container_metadata.rs"]
mod metadata;
#[path = "container_packed.rs"]
mod packed;
pub(crate) use packed::RecordLocation;
use packed::{BodyRef, BodySharing};
const DATA: u8 = 0;
const LEAF: u8 = 1;
const BRANCH: u8 = 2;
const FOOTER: u8 = 3;
const FANOUT: usize = 32;
const INDEX_BATCH: usize = 128 * 1024;
const INDEX_LIMIT: usize = 16 * 1024 * 1024;
const LOCATOR_BYTES: usize = 256;
pub(crate) const LOCATORS: usize = 2 * LOCATOR_BYTES;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    offset: u64,
    bytes: u32,
    first: u64,
    count: u64,
    level: u8,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    index: u64,
    stored: StoredRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<BodyRef>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Commit {
    roots: Vec<Node>,
    count: u64,
    sealed: bool,
    block: u64,
}
#[derive(Default)]
struct SmallCapture {
    objects: Vec<(u64, Vec<u8>)>,
    object_bytes: usize,
    entries: Vec<Entry>,
    metadata_bytes: usize,
}
const SMALL_CAPTURE_LIMIT: usize = 256 * 1024;

struct PendingRecord {
    stored: StoredRecord,
    index: u64,
    body: Option<PendingBody>,
    raw: Option<Vec<u8>>,
}
enum PendingBody {
    Ready(CompressedChunk),
    Worker {
        result: std::sync::mpsc::Receiver<io::Result<CompressedChunk>>,
        key: (u128, String),
    },
}
type UpdateLocator<W> = fn(&mut W, u64, &[u8], u64) -> io::Result<()>;

/// Buffered native capture writer with independent body chunks and a persisted index.
pub struct CaptureWriter<W> {
    output: W,
    bootstrap: Vec<u8>,
    encoding: CaptureEncoding,
    small: Option<SmallCapture>,
    limits: CaptureLimits,
    codec: FrameCodec,
    sharing: BodySharing,
    positions: HashMap<(u128, String), u64>,
    compressor: ChunkCompressor,
    compression_workers: usize,
    pool: Option<CompressionPool>,
    jobs: VecDeque<PendingRecord>,
    bodies: HashMap<(u128, String), BodyCompression>,
    certificates: BTreeSet<String>,
    connections: crate::connections::WriterContexts,
    start: u64,
    block: u64,
    data: Vec<u8>,
    pending: Vec<u8>,
    entries: Vec<Entry>,
    entry_bytes: usize,
    levels: Vec<Vec<Node>>,
    records: u64,
    poisoned: bool,
    sealed: bool,
    dirty: bool,
    committed: u64,
    locator: Option<UpdateLocator<W>>,
    generation: u64,
}

impl<W: Write> CaptureWriter<W> {
    /// Creates an unencrypted capture.
    /// # Errors
    /// Reports invalid limits or initialization/I/O failures.
    pub fn new(output: W, limits: CaptureLimits) -> Result<Self, CaptureError> {
        Self::with_encoding(output, limits, &CaptureEncoding::default())
    }
    /// Creates a capture with optional password encryption.
    /// # Errors
    /// Reports invalid limits, encryption initialization or I/O failures.
    pub fn with_encoding(
        output: W,
        limits: CaptureLimits,
        options: &CaptureEncoding,
    ) -> Result<Self, CaptureError> {
        Self::with_block_size(output, limits, options, BLOCK_PLAIN)
    }
    fn with_block_size(
        output: W,
        limits: CaptureLimits,
        options: &CaptureEncoding,
        block_size: usize,
    ) -> Result<Self, CaptureError> {
        let limits = limits.validate()?;
        let mut bootstrap = Vec::new();
        let (mut codec, header) = FrameCodec::create_with_block_size(
            &mut bootstrap,
            options,
            u32::try_from(block_size).map_err(|_| CaptureError::QuotaExceeded)?,
        )?;
        bootstrap.extend_from_slice(&[0; LOCATORS]);
        let start = header + LOCATORS as u64;
        codec.set_container_start(start);
        if start + codec.block_bytes() as u64 > limits.max_file_bytes {
            return Err(CaptureError::QuotaExceeded);
        }
        Ok(Self {
            output,
            bootstrap,
            encoding: options.clone(),
            small: Some(SmallCapture::default()),
            limits,
            codec,
            sharing: BodySharing::default(),
            positions: HashMap::new(),
            compressor: ChunkCompressor::new(1)?,
            compression_workers: std::thread::available_parallelism()
                .map_or(1, |count| count.get().min(2)),
            pool: None,
            jobs: VecDeque::new(),
            bodies: HashMap::new(),
            certificates: BTreeSet::new(),
            connections: crate::connections::WriterContexts::default(),
            start,
            block: 0,
            data: Vec::with_capacity(block_size - 5),
            pending: Vec::with_capacity(1024 * 1024),
            entries: Vec::new(),
            entry_bytes: 0,
            levels: Vec::new(),
            records: 0,
            poisoned: false,
            sealed: false,
            dirty: true,
            committed: 0,
            locator: None,
            generation: 0,
        })
    }
    /// Enables constant-read checkpoint lookup on a seekable output.
    /// Data and index blocks remain append-only; only alternating locator slots change.
    pub fn enable_checkpoint_locators(&mut self)
    where
        W: Seek,
    {
        self.locator = Some(|output, position, bytes, end| {
            output.seek(SeekFrom::Start(position))?;
            output.write_all(bytes)?;
            output.seek(SeekFrom::Start(end))?;
            Ok(())
        });
    }
    /// Overrides compression concurrency before records are appended. One uses the
    /// writer thread; two enables a lazily started pool after a useful body probe.
    /// # Errors
    /// Rejects unsupported counts or configuration after writing has started.
    pub fn set_compression_workers(&mut self, workers: usize) -> Result<(), CaptureError> {
        if !(1..=4).contains(&workers) || self.records != 0 {
            return Err(CaptureError::InvalidLimits);
        }
        self.compression_workers = workers;
        Ok(())
    }
    fn healthy(&self) -> Result<(), CaptureError> {
        if self.poisoned || self.sealed {
            Err(CaptureError::AlreadySealed)
        } else {
            Ok(())
        }
    }
    /// Queues one record, splitting bodies into independently readable chunks.
    /// # Errors
    /// Reports invalid records, limits or write failures. Failed writes poison the writer.
    pub fn append(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        self.healthy()?;
        let result = self.append_inner(record);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn append_performance(
        &mut self,
        record: &CaptureRecord,
        evidence: &transmog_core::performance::PerformanceEvidence,
    ) -> Result<(), CaptureError> {
        for certificate in evidence
            .transports
            .iter()
            .filter_map(|transport| transport.tls.as_ref())
            .flat_map(|tls| tls.server_chain.iter().chain(&tls.client_chain))
        {
            if !certificate.der.is_empty() && !self.certificates.contains(&certificate.sha256) {
                let definition = CertificateDefinition {
                    sha256: certificate.sha256.clone(),
                    der: certificate.der.to_vec(),
                };
                definition.validate()?;
                self.append_inner(&CaptureRecord {
                    sequence: 0,
                    exchange_id: 0,
                    kind: CaptureRecordKind::Unknown {
                        kind: "public-certificate".into(),
                        payload: serde_json::to_value(&definition)?,
                    },
                })?;
            }
        }
        let (definitions, snapshot) = crate::connections::split(record, evidence)?;
        for definition in definitions {
            self.append_inner(&definition)?;
        }
        self.append_one(&snapshot)
    }
    fn append_inner(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        super::validate_appended_record(record)?;
        if let CaptureRecordKind::Performance(evidence) = &record.kind {
            return self.append_performance(record, evidence);
        }
        if matches!(&record.kind, CaptureRecordKind::Unknown { kind, .. } if kind == crate::connections::DEFINITION_KIND)
            && !self.connections.insert(record)?
        {
            return Ok(());
        }
        if let CaptureRecordKind::Unknown { kind, payload } = &record.kind
            && kind == "public-certificate"
        {
            if record.exchange_id != 0 {
                return Err(CaptureError::InvalidMagic);
            }
            let definition: CertificateDefinition = serde_json::from_value(payload.clone())?;
            definition.validate()?;
            if !self.certificates.insert(definition.sha256) {
                return Ok(());
            }
        }
        if matches!(
            record.kind,
            CaptureRecordKind::Completed
                | CaptureRecordKind::Failed { .. }
                | CaptureRecordKind::RequestHead { .. }
                | CaptureRecordKind::ResponseHead { .. }
        ) {
            self.drain_jobs()?;
        }
        if let CaptureRecordKind::RequestHead { boundary, .. }
        | CaptureRecordKind::ResponseHead { boundary, .. } = &record.kind
        {
            self.bodies
                .remove(&(record.exchange_id, super::boundary(*boundary).into()));
            self.positions
                .remove(&(record.exchange_id, super::boundary(*boundary).into()));
        }
        if let CaptureRecordKind::BodySegment {
            boundary,
            byte_count,
            bytes: Some(bytes),
            truncated,
        } = &record.kind
        {
            let maximum = BODY_CHUNK_BYTES.min(self.limits.max_record_bytes);
            if bytes.len() > maximum {
                let mut counted = 0;
                for part in bytes.chunks(maximum) {
                    counted += part.len();
                    self.append_one(&CaptureRecord {
                        sequence: record.sequence,
                        exchange_id: record.exchange_id,
                        kind: CaptureRecordKind::BodySegment {
                            boundary: *boundary,
                            byte_count: if counted == bytes.len() {
                                byte_count.saturating_sub(counted - part.len())
                            } else {
                                part.len()
                            },
                            bytes: Some(part.to_vec()),
                            truncated: *truncated,
                        },
                    })?;
                }
                return Ok(());
            }
        }
        self.append_one(record)?;
        if let CaptureRecordKind::Unknown { kind, payload } = &record.kind
            && kind == "body-completed"
            && let Some(boundary) = payload["boundary"].as_str()
        {
            self.drain_jobs()?;
            self.bodies
                .remove(&(record.exchange_id, boundary.to_owned()));
        }
        if matches!(
            record.kind,
            CaptureRecordKind::Completed | CaptureRecordKind::Failed { .. }
        ) {
            self.bodies.retain(|(id, _), _| *id != record.exchange_id);
            self.positions
                .retain(|(id, _), _| *id != record.exchange_id);
        }
        Ok(())
    }
    fn append_one(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        if self.records >= self.limits.max_records.saturating_sub(1) as u64
            && !matches!(record.kind, CaptureRecordKind::Seal { .. })
        {
            return Err(CaptureError::RecordCountExceeded);
        }
        while self.jobs.len() >= 8 {
            self.finish_one()?;
        }
        let mut body = None;
        let mut raw = None;
        let mut stored;
        if let CaptureRecordKind::BodySegment {
            boundary,
            byte_count,
            bytes: Some(bytes),
            truncated,
        } = &record.kind
        {
            raw = Some(bytes.clone());
            let key = (record.exchange_id, super::boundary(*boundary).into());
            let state = self.bodies.entry(key.clone()).or_default();
            body = Some(
                if self.compression_workers > 1
                    && bytes.len() >= 4096
                    && state.parallel_worthwhile()
                {
                    if self.pool.is_none() {
                        self.pool = Some(CompressionPool::new(self.compression_workers)?);
                    }
                    PendingBody::Worker {
                        result: self.pool.as_ref().unwrap().submit(bytes.clone())?,
                        key,
                    }
                } else {
                    PendingBody::Ready(self.compressor.body(bytes, state)?)
                },
            );
            stored = store(&CaptureRecord {
                sequence: record.sequence,
                exchange_id: record.exchange_id,
                kind: CaptureRecordKind::BodySegment {
                    boundary: *boundary,
                    byte_count: *byte_count,
                    bytes: None,
                    truncated: *truncated,
                },
            })?;
            stored.body_index = Some(StoredBodyIndex {
                frame_bytes: 0,
                bytes: bytes.len(),
                digest: Sha256::digest(bytes).into(),
            });
        } else {
            stored = store(record)?;
        }
        // Validate before retaining a metadata payload in the bounded job queue.
        metadata::check_record(&serde_json::to_vec(&stored)?, self.limits)?;
        let ready = !matches!(body, Some(PendingBody::Worker { .. }));
        self.jobs.push_back(PendingRecord {
            stored,
            index: self.records,
            body,
            raw,
        });
        self.records += 1;
        self.dirty = true;
        if ready && self.jobs.len() == 1 {
            self.finish_one()?;
        }
        Ok(())
    }
    // Circular storage already compressed these bytes. Authenticate its envelope,
    // then carry the independent zstd frame into the final container unchanged.
    pub(crate) fn append_compressed(
        &mut self,
        stored: StoredRecord,
        body: CompressedChunk,
    ) -> Result<(), CaptureError> {
        self.healthy()?;
        let result = (|| {
            if self.records >= self.limits.max_records.saturating_sub(1) as u64 {
                return Err(CaptureError::RecordCountExceeded);
            }
            self.drain_jobs()?;
            self.jobs.push_back(PendingRecord {
                stored,
                index: self.records,
                body: Some(PendingBody::Ready(body)),
                raw: None,
            });
            self.records += 1;
            self.dirty = true;
            self.finish_one()
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn drain_jobs(&mut self) -> Result<(), CaptureError> {
        while !self.jobs.is_empty() {
            self.finish_one()?;
        }
        Ok(())
    }
    fn finish_one(&mut self) -> Result<(), CaptureError> {
        let Some(PendingRecord {
            mut stored,
            index,
            body,
            raw,
        }) = self.jobs.pop_front()
        else {
            return Ok(());
        };
        let body = match body {
            None => None,
            Some(PendingBody::Ready(chunk)) => Some(chunk),
            Some(PendingBody::Worker { result, key }) => {
                let chunk = result
                    .recv()
                    .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))??;
                self.bodies
                    .entry(key)
                    .or_default()
                    .observe(chunk.expanded, &chunk);
                Some(chunk)
            }
        };
        let reference = body
            .map(|chunk| self.store_body(&mut stored, chunk, raw))
            .transpose()?;
        let metadata = serde_json::to_vec(&stored)?;
        metadata::check_record(&metadata, self.limits)?;
        let entry = Entry {
            index,
            stored,
            body: reference,
        };
        let charge = serde_json::to_vec(&entry)?.len() + 1;
        if self
            .small
            .as_ref()
            .is_some_and(|small| small.metadata_bytes.saturating_add(charge) > SMALL_CAPTURE_LIMIT)
        {
            self.small = None;
        }
        if let Some(small) = &mut self.small {
            small.entries.push(entry.clone());
            small.metadata_bytes += charge;
        }
        self.entries.push(entry);
        self.entry_bytes = self.entry_bytes.saturating_add(charge);
        if self.entry_bytes >= INDEX_BATCH || self.entries.len() >= metadata::LEAF_ENTRIES {
            self.flush_index()?;
        }
        Ok(())
    }
    fn store_body(
        &mut self,
        stored: &mut StoredRecord,
        mut chunk: CompressedChunk,
        raw: Option<Vec<u8>>,
    ) -> Result<BodyRef, CaptureError> {
        let reference;
        let descriptor = stored.body_index.ok_or(CaptureError::InvalidMagic)?;
        let boundary = stored.payload["boundary"]
            .as_str()
            .ok_or(CaptureError::InvalidMagic)?;
        let position = self
            .positions
            .entry((stored.exchange_id, boundary.to_owned()))
            .or_default();
        let pair = match boundary {
            "client-request" => Some(((stored.exchange_id, true, *position), false)),
            "upstream-request" => Some(((stored.exchange_id, true, *position), true)),
            "upstream-response" => Some(((stored.exchange_id, false, *position), true)),
            "client-response" => Some(((stored.exchange_id, false, *position), false)),
            _ => return Err(CaptureError::InvalidMagic),
        };
        *position = position
            .checked_add(descriptor.bytes as u64)
            .ok_or(CaptureError::QuotaExceeded)?;
        if let Some((descriptor, shared)) = self.sharing.shared(&descriptor, pair) {
            stored.body_index = Some(descriptor);
            reference = shared;
        } else {
            let verify_raw = raw.is_none();
            let raw = match raw {
                Some(raw) => raw,
                None => match chunk.codec {
                    0 => chunk.bytes.clone(),
                    1 => zstd::bulk::decompress(&chunk.bytes, descriptor.bytes)?,
                    _ => return Err(CaptureError::InvalidMagic),
                },
            };
            if raw.len() != descriptor.bytes
                || (verify_raw && <[u8; 32]>::from(Sha256::digest(&raw)) != descriptor.digest)
            {
                return Err(CaptureError::AuthenticationFailed);
            }
            let mut dictionary = None;
            if let Some(base) =
                pair.and_then(|(key, upstream)| self.sharing.dictionary(key, upstream))
            {
                let compressed =
                    zstd::bulk::Compressor::with_dictionary(1, &base.bytes)?.compress(&raw)?;
                let saved = chunk.bytes.len().saturating_sub(compressed.len());
                if saved >= 256 && saved >= raw.len().div_ceil(50) {
                    chunk = CompressedChunk {
                        codec: 2,
                        expanded: raw.len(),
                        bytes: compressed,
                    };
                    dictionary = Some(base.reference);
                }
            }
            let bytes = packed::pack(DATA, &chunk)?;
            let offset = self.append_object_bytes(&bytes)?;
            let descriptor = StoredBodyIndex {
                frame_bytes: bytes.len() as u64,
                ..descriptor
            };
            let body = BodyRef { offset, dictionary };
            stored.body_index = Some(descriptor);
            self.sharing.insert(descriptor, &body, pair, raw);
            reference = body;
            if self.small.as_ref().is_some_and(|small| {
                small.object_bytes.saturating_add(bytes.len()) > SMALL_CAPTURE_LIMIT
            }) {
                self.small = None;
            }
            if let Some(small) = &mut self.small {
                small.object_bytes += bytes.len();
                small.objects.push((offset, bytes));
            }
        }
        Ok(reference)
    }
    fn append_object_bytes(&mut self, bytes: &[u8]) -> Result<u64, CaptureError> {
        if self.data.len() == self.codec.payload() {
            self.flush_data()?;
        }
        let offset =
            self.start + self.block * self.codec.block_bytes() as u64 + 5 + self.data.len() as u64;
        self.append_data(bytes)?;
        Ok(offset)
    }
    fn append_data(&mut self, mut bytes: &[u8]) -> Result<(), CaptureError> {
        while !bytes.is_empty() {
            if self.data.len() == self.codec.payload() {
                self.flush_data()?;
            }
            let take = bytes.len().min(self.codec.payload() - self.data.len());
            self.data.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        Ok(())
    }
    fn put_block(&mut self, kind: u8, bytes: &[u8]) -> Result<u64, CaptureError> {
        if bytes.len() > self.codec.payload() {
            return Err(CaptureError::QuotaExceeded);
        }
        let next = self
            .start
            .checked_add(
                (self.block + 1)
                    .checked_mul(self.codec.block_bytes() as u64)
                    .ok_or(CaptureError::QuotaExceeded)?,
            )
            .ok_or(CaptureError::QuotaExceeded)?;
        if next > self.limits.max_file_bytes {
            return Err(CaptureError::QuotaExceeded);
        }
        let mut plain = vec![0; self.codec.block_plain()];
        plain[0] = kind;
        plain[1..5].copy_from_slice(
            &u32::try_from(bytes.len())
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        );
        plain[5..5 + bytes.len()].copy_from_slice(bytes);
        let encoded = self.codec.encode_block(self.block, &plain)?;
        let block = self.block;
        self.block += 1;
        self.pending.extend_from_slice(&encoded);
        if self.pending.len() >= 1024 * 1024 {
            self.write_pending()?;
        }
        Ok(block)
    }
    fn write_pending(&mut self) -> Result<(), CaptureError> {
        if !self.bootstrap.is_empty() {
            self.output.write_all(&self.bootstrap)?;
            self.bootstrap.clear();
        }
        self.output.write_all(&self.pending)?;
        self.pending.clear();
        Ok(())
    }
    fn flush_data(&mut self) -> Result<(), CaptureError> {
        if !self.data.is_empty() {
            let data = std::mem::take(&mut self.data);
            self.put_block(DATA, &data)?;
            self.data = data;
            self.data.clear();
        }
        Ok(())
    }
    fn object(
        &mut self,
        kind: u8,
        bytes: &[u8],
        first: u64,
        count: u64,
        level: u8,
    ) -> Result<Node, CaptureError> {
        if bytes.len() > INDEX_LIMIT {
            return Err(CaptureError::RecordTooLarge {
                actual: bytes.len(),
                limit: INDEX_LIMIT,
            });
        }
        let chunk = self.compressor.metadata(bytes)?;
        let encoded = packed::pack(kind, &chunk)?;
        let offset = self.append_object_bytes(&encoded)?;
        Ok(Node {
            offset,
            bytes: u32::try_from(encoded.len()).map_err(|_| CaptureError::QuotaExceeded)?,
            first,
            count,
            level,
        })
    }
    fn flush_index(&mut self) -> Result<(), CaptureError> {
        if self.entries.is_empty() {
            return Ok(());
        }
        let entries = std::mem::take(&mut self.entries);
        let bytes = serde_json::to_vec(&entries)?;
        let node = self.object(LEAF, &bytes, entries[0].index, entries.len() as u64, 0)?;
        self.entry_bytes = 0;
        self.push_node(node)
    }
    fn push_node(&mut self, mut node: Node) -> Result<(), CaptureError> {
        loop {
            let level = node.level as usize;
            if level > 15 {
                return Err(CaptureError::QuotaExceeded);
            }
            while self.levels.len() <= level {
                self.levels.push(Vec::new());
            }
            self.levels[level].push(node);
            if self.levels[level].len() < FANOUT {
                return Ok(());
            }
            let children = std::mem::take(&mut self.levels[level]);
            let count = children.iter().map(|child| child.count).sum();
            node = self.object(
                BRANCH,
                &serde_json::to_vec(&children)?,
                children[0].first,
                count,
                u8::try_from(level + 1).map_err(|_| CaptureError::QuotaExceeded)?,
            )?;
        }
    }
    /// Publishes an independently readable checkpoint and writes application buffers to the OS.
    /// This does not synchronize disk caches. Call at least every five seconds during capture.
    /// # Errors
    /// Returns encoding or I/O errors, poisoning the writer on failure.
    pub fn checkpoint(&mut self) -> Result<(), CaptureError> {
        self.healthy()?;
        let result = self.commit(false);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn commit(&mut self, sealed: bool) -> Result<(), CaptureError> {
        if !self.dirty && !sealed {
            return Ok(());
        }
        self.drain_jobs()?;
        if sealed
            && !self.bootstrap.is_empty()
            && let Some(small) = &self.small
        {
            let bytes = compact(small, &self.encoding, self.limits)?;
            self.output.write_all(&bytes)?;
            self.output.flush()?;
            self.committed = bytes.len() as u64;
            self.bootstrap.clear();
            self.pending.clear();
            self.small = None;
            self.dirty = false;
            return Ok(());
        }
        self.flush_index()?;
        self.flush_data()?;
        let roots: Vec<Node> = self
            .levels
            .iter()
            .rev()
            .flat_map(|nodes| nodes.iter().cloned())
            .collect();
        let commit = Commit {
            roots,
            count: self.records,
            sealed,
            block: self.block,
        };
        let bytes = serde_json::to_vec(&commit)?;
        let chunk = self.compressor.metadata(&bytes)?;
        let mut packed = vec![chunk.codec];
        packed.extend_from_slice(
            &u32::try_from(chunk.expanded)
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        );
        packed.extend_from_slice(&chunk.bytes);
        let block = self.put_block(FOOTER, &packed)?;
        self.write_pending()?;
        self.output.flush()?;
        self.committed = self.start + self.block * self.codec.block_bytes() as u64;
        if let Some(update) = self.locator {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or(CaptureError::RecordCountExceeded)?;
            let slot = (self.generation % 2) as usize;
            let bytes = self.codec.encode_locator(self.generation, block, slot)?;
            update(
                &mut self.output,
                self.start - LOCATORS as u64 + (slot * LOCATOR_BYTES) as u64,
                &bytes,
                self.committed,
            )?;
            self.output.flush()?;
        }
        self.dirty = false;
        Ok(())
    }
    /// Drains the writer and publishes an authenticated final index.
    /// # Errors
    /// Returns serialization, quota or I/O errors.
    pub fn seal(&mut self) -> Result<(), CaptureError> {
        self.healthy()?;
        let result = (|| {
            self.append_one(&CaptureRecord {
                sequence: 0,
                exchange_id: 0,
                kind: CaptureRecordKind::Seal {
                    record_count: self.records,
                },
            })?;
            self.commit(true)
        })();
        if result.is_err() {
            self.poisoned = true;
        } else {
            self.sealed = true;
        }
        result
    }
    /// Returns a compact replacement for a small, already checkpointed capture.
    /// The same layout uses 1 KiB blocks and fresh cryptographic parameters.
    /// Publish it by atomic replacement, keeping the original until success.
    /// # Errors
    /// Rejects an unsealed writer or encoding/resource-limit failure.
    pub fn compact_sealed_bytes(&self) -> Result<Option<Vec<u8>>, CaptureError> {
        if !self.sealed || self.poisoned {
            return Err(CaptureError::AlreadySealed);
        }
        self.small
            .as_ref()
            .map(|small| compact(small, &self.encoding, self.limits))
            .transpose()
    }
    /// Bytes successfully published through the latest checkpoint.
    pub fn bytes_written(&self) -> u64 {
        self.committed
    }
    /// Writes a final unsealed checkpoint if necessary, then returns the destination.
    /// # Errors
    /// Returns any pending encoding or write failure.
    pub fn into_inner(mut self) -> Result<W, CaptureError> {
        if self.poisoned {
            return Err(CaptureError::AlreadySealed);
        }
        if !self.sealed {
            self.checkpoint()?;
        }
        Ok(self.output)
    }
}

fn compact(
    small: &SmallCapture,
    options: &CaptureEncoding,
    limits: CaptureLimits,
) -> Result<Vec<u8>, CaptureError> {
    // Re-encode into a new cryptographic domain. Never reuse a block nonce while
    // changing geometry, offsets, or contents.
    let mut writer = CaptureWriter::with_block_size(Vec::new(), limits, options, 1024)?;
    writer.small = None;
    let mut offsets = HashMap::new();
    for (old, bytes) in &small.objects {
        offsets.insert(*old, writer.append_object_bytes(bytes)?);
    }
    for original in &small.entries {
        let mut entry = original.clone();
        if let Some(body) = &mut entry.body {
            body.offset = *offsets
                .get(&body.offset)
                .ok_or(CaptureError::InvalidMagic)?;
            if let Some(dictionary) = &mut body.dictionary {
                dictionary.offset = *offsets
                    .get(&dictionary.offset)
                    .ok_or(CaptureError::InvalidMagic)?;
            }
        }
        writer.entry_bytes += serde_json::to_vec(&entry)?.len() + 1;
        writer.entries.push(entry);
        if writer.entry_bytes >= INDEX_BATCH || writer.entries.len() >= metadata::LEAF_ENTRIES {
            writer.flush_index()?;
        }
    }
    writer.records = small.entries.len() as u64;
    writer.commit(true)?;
    writer.sealed = true;
    writer.into_inner()
}

fn unpack(bytes: &[u8], maximum: usize) -> Result<Vec<u8>, CaptureError> {
    if bytes.len() < 5 {
        return Err(CaptureError::InvalidMagic);
    }
    let expanded = u32::from_le_bytes(bytes[1..5].try_into().unwrap()) as usize;
    if expanded > maximum {
        return Err(CaptureError::RecordTooLarge {
            actual: expanded,
            limit: maximum,
        });
    }
    let output = match bytes[0] {
        0 => bytes[5..].to_vec(),
        1 => zstd::bulk::decompress(&bytes[5..], expanded)?,
        _ => return Err(CaptureError::UnsupportedEncoding),
    };
    if output.len() != expanded {
        return Err(CaptureError::AuthenticationFailed);
    }
    Ok(output)
}
fn block<R: Read + Seek>(
    input: &mut R,
    codec: &FrameCodec,
    index: u64,
) -> Result<(u8, Vec<u8>), CaptureError> {
    let offset = index
        .checked_mul(codec.block_bytes() as u64)
        .and_then(|n| n.checked_add(codec.container_start()))
        .ok_or(CaptureError::QuotaExceeded)?;
    input.seek(SeekFrom::Start(offset))?;
    let mut encoded = vec![0; codec.block_bytes()];
    input.read_exact(&mut encoded)?;
    let plain = codec.decode_block(index, &encoded)?;
    let length = u32::from_le_bytes(plain[1..5].try_into().unwrap()) as usize;
    if length > codec.payload() {
        return Err(CaptureError::InvalidMagic);
    }
    Ok((plain[0], plain[5..5 + length].to_vec()))
}

/// Reader backed by the persisted metadata index. Bodies remain untouched until requested.
pub struct CaptureReader<R> {
    pub(crate) input: R,
    limits: CaptureLimits,
    codec: FrameCodec,
    stack: Vec<Node>,
    entries: VecDeque<metadata::RawEntry>,
    certificates: HashMap<String, std::sync::Arc<[u8]>>,
    connections: crate::connections::ReaderContexts,
    certificate_bytes: usize,
    page: Option<Node>,
    current_body: Option<(BodyRef, StoredBodyIndex)>,
    records: u64,
    count: u64,
    committed: u64,
    sealed: bool,
    truncated: bool,
}
impl<R: Read + Seek> CaptureReader<R> {
    /// Opens an unencrypted capture by reading its bootstrap and index root.
    /// # Errors
    /// Reports malformed, encrypted or corrupt inputs.
    pub fn new(input: R, limits: CaptureLimits) -> Result<Self, CaptureError> {
        Self::with_password(input, limits, None)
    }
    /// Opens the latest complete index without scanning or decompressing bodies.
    /// # Errors
    /// Reports password, framing, authentication or resource-limit failures.
    pub fn with_password(
        mut input: R,
        limits: CaptureLimits,
        password: Option<&CapturePassword>,
    ) -> Result<Self, CaptureError> {
        let limits = limits.validate()?;
        let mut magic = [0; 9];
        input
            .read_exact(&mut magic)
            .map_err(|_| CaptureError::TruncatedPreamble)?;
        if magic != MAGIC {
            return Err(CaptureError::InvalidMagic);
        }
        let (mut codec, header) = FrameCodec::read(&mut input, password)?;
        let mut locators = [0; LOCATORS];
        input.read_exact(&mut locators)?;
        let start = header + LOCATORS as u64;
        codec.set_container_start(start);
        let end = input.seek(SeekFrom::End(0))?;
        if end > limits.max_file_bytes || end < start {
            return Err(CaptureError::QuotaExceeded);
        }
        let blocks = (end - start) / codec.block_bytes() as u64;
        let mut candidates = Vec::new();
        if blocks > 0 && (end - start).is_multiple_of(codec.block_bytes() as u64) {
            candidates.push(blocks - 1);
        }
        let mut saved = (0..2)
            .filter_map(|slot| {
                codec
                    .decode_locator(
                        &locators[slot * LOCATOR_BYTES..(slot + 1) * LOCATOR_BYTES],
                        slot,
                    )
                    .ok()
            })
            .collect::<Vec<_>>();
        saved.sort_unstable_by_key(|item| std::cmp::Reverse(item.0));
        candidates.extend(saved.into_iter().map(|(_, block)| block));
        // Generic append-only sinks lack mutable locators. Their recovery search
        // is bounded to 16 MiB; product file writers always install locators.
        candidates.extend((blocks.saturating_sub(1024)..blocks).rev());
        let mut found = None;
        for index in candidates {
            if index >= blocks {
                continue;
            }
            let Ok((kind, bytes)) = block(&mut input, &codec, index) else {
                continue;
            };
            if kind != FOOTER {
                continue;
            }
            let commit: Commit = serde_json::from_slice(&unpack(&bytes, 64 * 1024)?)?;
            if commit.block != index
                || commit.count > limits.max_records as u64
                || (commit.sealed && commit.count == 0)
            {
                return Err(CaptureError::InvalidMagic);
            }
            validate_nodes(
                &commit.roots,
                0,
                commit.count,
                start + index * codec.block_bytes() as u64,
                &codec,
            )?;
            found = Some(commit);
            break;
        }
        let commit = found.ok_or(CaptureError::TruncatedPreamble)?;
        let committed = start + (commit.block + 1) * codec.block_bytes() as u64;
        if commit.sealed && committed != end {
            return Err(CaptureError::AuthenticationFailed);
        }
        let mut stack = commit.roots;
        stack.reverse();
        Ok(Self {
            input,
            limits,
            codec,
            stack,
            entries: VecDeque::new(),
            certificates: HashMap::new(),
            connections: crate::connections::ReaderContexts::default(),
            certificate_bytes: 0,
            page: None,
            current_body: None,
            records: 0,
            count: commit.count,
            committed,
            sealed: commit.sealed,
            truncated: committed != end,
        })
    }
    fn object(&mut self, node: &Node) -> Result<Vec<u8>, CaptureError> {
        packed::read_object(
            &mut self.input,
            &self.codec,
            node.offset,
            u64::from(node.bytes),
            if node.level == 0 { LEAF } else { BRANCH },
            INDEX_LIMIT,
            None,
        )
    }
    fn resolve_certificates(&mut self, record: &mut CaptureRecord) -> Result<(), CaptureError> {
        if let CaptureRecordKind::Unknown { kind, payload } = &record.kind
            && kind == "public-certificate"
        {
            let definition: CertificateDefinition = serde_json::from_value(payload.clone())?;
            definition.validate()?;
            if record.exchange_id != 0 {
                return Err(CaptureError::InvalidMagic);
            }
            if !self.certificates.contains_key(&definition.sha256) {
                self.certificate_bytes += definition.der.len();
                if self.certificate_bytes > 256 * 1024 * 1024 {
                    return Err(CaptureError::QuotaExceeded);
                }
                self.certificates
                    .insert(definition.sha256, definition.der.into());
            }
        }
        if let CaptureRecordKind::Performance(evidence) = &mut record.kind {
            for certificate in evidence
                .transports
                .iter_mut()
                .filter_map(|transport| transport.tls.as_mut())
                .flat_map(|tls| tls.server_chain.iter_mut().chain(&mut tls.client_chain))
            {
                if let Some(der) = self.certificates.get(&certificate.sha256) {
                    certificate.der = der.clone();
                }
            }
        }
        Ok(())
    }
    /// Reads one metadata record from the index without touching its body.
    /// # Errors
    /// Reports index corruption or exceeded resource bounds.
    pub fn read_next_indexed(&mut self) -> Result<Option<CaptureFrame>, CaptureError> {
        while self.entries.is_empty() {
            let Some(node) = self.stack.pop() else {
                if self.records != self.count {
                    return Err(CaptureError::AuthenticationFailed);
                }
                return Ok(None);
            };
            let bytes = self.object(&node)?;
            if node.level > 0 {
                let mut children: Vec<Node> = metadata::array(&bytes, FANOUT)?;
                if children.len() > FANOUT
                    || children
                        .iter()
                        .any(|child| child.level.checked_add(1) != Some(node.level))
                {
                    return Err(CaptureError::InvalidMagic);
                }
                validate_nodes(&children, node.first, node.count, node.offset, &self.codec)?;
                children.reverse();
                self.stack.extend(children);
            } else {
                let entries: Vec<metadata::RawEntry> = metadata::array(
                    &bytes,
                    usize::try_from(node.count).map_err(|_| CaptureError::QuotaExceeded)?,
                )?;
                if entries.len() as u64 != node.count {
                    return Err(CaptureError::InvalidMagic);
                }
                self.page = Some(node);
                self.entries = entries.into();
            }
        }
        let entry = self
            .entries
            .pop_front()
            .ok_or(CaptureError::InvalidMagic)?
            .decode(self.limits)?;
        if entry.index != self.records {
            return Err(CaptureError::InvalidMagic);
        }
        let page = self.page.clone().ok_or(CaptureError::InvalidMagic)?;
        validate_entry(&entry, &self.codec, page.offset, self.limits)?;
        let (offset, frame_bytes) = match (&entry.body, entry.stored.body_index) {
            (Some(body), Some(descriptor)) => (body.offset, descriptor.frame_bytes),
            (None, None) => (page.offset, u64::from(page.bytes)),
            _ => return Err(CaptureError::InvalidMagic),
        };
        let mut frame_codec = self.connections.prepare(&entry.stored, &self.codec)?;
        frame_codec.record = Some(RecordLocation {
            page,
            index: entry.index,
            offset,
            frame_bytes,
        });
        let (mut record, body) = entry.stored.load_container_metadata(&frame_codec)?;
        self.resolve_certificates(&mut record)?;
        if let CaptureRecordKind::Seal { record_count } = record.kind {
            if !self.sealed || record_count != entry.index || entry.index + 1 != self.count {
                return Err(CaptureError::AuthenticationFailed);
            }
        } else if self.sealed && entry.index + 1 == self.count {
            return Err(CaptureError::AuthenticationFailed);
        }
        self.current_body = entry.body.zip(body);
        self.records += 1;
        Ok(Some(CaptureFrame {
            offset,
            index: entry.index,
            frame_bytes,
            codec: frame_codec,
            record,
            body: body.map(|body| CaptureBodyIndex {
                bytes: body.bytes,
                digest: body.digest,
            }),
        }))
    }
    /// Reads and validates one complete record, including any body chunk.
    /// # Errors
    /// Reports metadata or body corruption and resource-limit failures.
    pub fn read_next(&mut self) -> Result<Option<CaptureFrame>, CaptureError> {
        let Some(mut frame) = self.read_next_indexed()? else {
            return Ok(None);
        };
        if let Some((reference, descriptor)) = self.current_body.take() {
            let before = self.page.as_ref().ok_or(CaptureError::InvalidMagic)?.offset;
            let bytes = packed::read_body(
                &mut self.input,
                &frame.codec,
                &reference,
                descriptor,
                before,
                self.limits.max_record_bytes,
            )?;
            let CaptureRecordKind::BodySegment { bytes: target, .. } = &mut frame.record.kind
            else {
                return Err(CaptureError::InvalidMagic);
            };
            *target = Some(bytes);
        }
        Ok(Some(frame))
    }
    /// Cryptographic context for independent lazy body readers.
    /// For random reads use the returned frame's codec: it pins the canonical
    /// metadata page, record ordinal, and any referenced connection definitions.
    pub fn codec(&self) -> FrameCodec {
        self.codec.clone()
    }
    /// Physical length of the latest committed prefix.
    pub fn valid_bytes(&self) -> u64 {
        self.committed
    }
    /// Whether bytes after the latest checkpoint were incomplete or uncommitted.
    pub fn truncated_tail(&self) -> bool {
        self.truncated
    }
    /// Whether an authenticated final index commits the complete file.
    pub fn sealed(&self) -> bool {
        self.sealed && !self.truncated
    }
}
fn validate_entry(
    entry: &Entry,
    codec: &FrameCodec,
    before: u64,
    limits: CaptureLimits,
) -> Result<(), CaptureError> {
    match (&entry.body, entry.stored.body_index) {
        (Some(body), Some(descriptor)) => {
            packed::validate_body(body, descriptor, codec, before, limits.max_record_bytes)
        }
        (None, None) => Ok(()),
        _ => Err(CaptureError::InvalidMagic),
    }
}
fn validate_nodes(
    nodes: &[Node],
    first: u64,
    count: u64,
    before: u64,
    codec: &FrameCodec,
) -> Result<(), CaptureError> {
    let mut next = first;
    let mut previous_end = codec.container_start();
    for node in nodes {
        let end = packed::span_end(codec, node.offset, u64::from(node.bytes))?;
        if node.first != next
            || node.count == 0
            || node.level > 15
            || (node.level == 0 && node.count > metadata::LEAF_ENTRIES as u64)
            || node.bytes as usize > INDEX_LIMIT + packed::OBJECT_HEADER
            || node.offset < previous_end
            || end > before
        {
            return Err(CaptureError::InvalidMagic);
        }
        previous_end = end;
        next = next
            .checked_add(node.count)
            .ok_or(CaptureError::QuotaExceeded)?;
    }
    if next
        != first
            .checked_add(count)
            .ok_or(CaptureError::QuotaExceeded)?
    {
        return Err(CaptureError::InvalidMagic);
    }
    Ok(())
}

/// Reads one indexed record from a source positioned at its indexed offset.
/// The complete original source and a seekable cursor are required for block authentication.
/// Pass the indexed frame's codec to resolve its shared connection references.
/// # Errors
/// Reports changed ciphertext, invalid references, or body corruption.
pub fn read_indexed_frame_with_codec(
    mut input: impl Read + Seek,
    frame_bytes: u64,
    limits: CaptureLimits,
    codec: &FrameCodec,
    index: u64,
) -> Result<CaptureRecord, CaptureError> {
    let limits = limits.validate()?;
    let location = codec.record.as_ref().ok_or(CaptureError::InvalidMagic)?;
    if input.stream_position()? != location.offset
        || frame_bytes != location.frame_bytes
        || index != location.index
    {
        return Err(CaptureError::AuthenticationFailed);
    }
    if input.seek(SeekFrom::End(0))? > limits.max_file_bytes {
        return Err(CaptureError::QuotaExceeded);
    }
    let page = &location.page;
    let bytes = packed::read_object(
        &mut input,
        codec,
        page.offset,
        u64::from(page.bytes),
        LEAF,
        INDEX_LIMIT,
        None,
    )?;
    let entries: Vec<metadata::RawEntry> = metadata::array(
        &bytes,
        usize::try_from(page.count).map_err(|_| CaptureError::QuotaExceeded)?,
    )?;
    if entries.len() as u64 != page.count {
        return Err(CaptureError::InvalidMagic);
    }
    let relative = index
        .checked_sub(page.first)
        .ok_or(CaptureError::InvalidMagic)?;
    let entry = entries
        .into_iter()
        .nth(usize::try_from(relative).map_err(|_| CaptureError::QuotaExceeded)?)
        .ok_or(CaptureError::InvalidMagic)?
        .decode(limits)?;
    if entry.index != index {
        return Err(CaptureError::InvalidMagic);
    }
    validate_entry(&entry, codec, page.offset, limits)?;
    let reference = entry.body.clone();
    let (mut record, descriptor) = entry.stored.load_container_metadata(codec)?;
    match (reference, descriptor, &mut record.kind) {
        (Some(reference), Some(descriptor), CaptureRecordKind::BodySegment { bytes, .. }) => {
            if reference.offset != location.offset || descriptor.frame_bytes != frame_bytes {
                return Err(CaptureError::AuthenticationFailed);
            }
            *bytes = Some(packed::read_body(
                &mut input,
                codec,
                &reference,
                descriptor,
                page.offset,
                limits.max_record_bytes,
            )?);
        }
        (None, None, _)
            if location.offset == page.offset && frame_bytes == u64::from(page.bytes) => {}
        _ => return Err(CaptureError::InvalidMagic),
    }
    Ok(record)
}

#[cfg(test)]
#[path = "container_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "review_tests.rs"]
mod review_tests;

#[cfg(test)]
#[path = "connection_tests.rs"]
mod connection_tests;

#[cfg(test)]
#[path = "packed_tests.rs"]
mod packed_tests;

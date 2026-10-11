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
pub(crate) const BLOCK_BYTES: usize = BLOCK_PLAIN + 16;
const PAYLOAD: usize = BLOCK_PLAIN - 5;
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
    block: u64,
    blocks: u32,
    bytes: u32,
    first: u64,
    count: u64,
    level: u8,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    offset: u64,
    frame_bytes: u64,
    index: u64,
    stored: StoredRecord,
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
    data: Vec<u8>,
    entries: Vec<Entry>,
    metadata_bytes: usize,
}
const SMALL_CAPTURE_LIMIT: usize = 256 * 1024;

struct PendingRecord {
    stored: StoredRecord,
    index: u64,
    body: Option<PendingBody>,
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
    plain: FrameCodec,
    compressor: ChunkCompressor,
    compression_workers: usize,
    pool: Option<CompressionPool>,
    jobs: VecDeque<PendingRecord>,
    bodies: HashMap<(u128, String), BodyCompression>,
    certificates: BTreeSet<String>,
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
        let limits = limits.validate()?;
        let mut bootstrap = Vec::new();
        let (mut codec, header) = FrameCodec::create(&mut bootstrap, options)?;
        bootstrap.extend_from_slice(&[0; LOCATORS]);
        let start = header + LOCATORS as u64;
        codec.set_container_start(start);
        let plain = codec.plain();
        if start + BLOCK_BYTES as u64 > limits.max_file_bytes {
            return Err(CaptureError::QuotaExceeded);
        }
        Ok(Self {
            output,
            bootstrap,
            encoding: options.clone(),
            small: Some(SmallCapture::default()),
            limits,
            codec,
            plain,
            compressor: ChunkCompressor::new(1)?,
            compression_workers: std::thread::available_parallelism()
                .map_or(1, |count| count.get().min(2)),
            pool: None,
            jobs: VecDeque::new(),
            bodies: HashMap::new(),
            certificates: BTreeSet::new(),
            start,
            block: 0,
            data: Vec::with_capacity(PAYLOAD),
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
    fn append_inner(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        super::validate_appended_record(record)?;
        if let CaptureRecordKind::Performance(evidence) = &record.kind {
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
        let mut stored;
        if let CaptureRecordKind::BodySegment {
            boundary,
            byte_count,
            bytes: Some(bytes),
            truncated,
        } = &record.kind
        {
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
        let metadata_bytes = serde_json::to_vec(&stored)?.len();
        if metadata_bytes > self.limits.max_record_bytes {
            return Err(CaptureError::RecordTooLarge {
                actual: metadata_bytes,
                limit: self.limits.max_record_bytes,
            });
        }
        let ready = !matches!(body, Some(PendingBody::Worker { .. }));
        self.jobs.push_back(PendingRecord {
            stored,
            index: self.records,
            body,
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
        let body = body
            .map(|chunk| {
                self.plain
                    .encode_chunk(&chunk, index, true)
                    .and_then(|encoded| frame(&encoded))
            })
            .transpose()?;
        if let Some(body) = &body {
            stored
                .body_index
                .as_mut()
                .ok_or(CaptureError::InvalidMagic)?
                .frame_bytes = body.len() as u64;
        }
        let metadata = serde_json::to_vec(&stored)?;
        if metadata.len() > self.limits.max_record_bytes {
            return Err(CaptureError::RecordTooLarge {
                actual: metadata.len(),
                limit: self.limits.max_record_bytes,
            });
        }
        let chunk = self.compressor.metadata(&metadata)?;
        let encoded = self.plain.encode_chunk(&chunk, index, false)?;
        let mut bytes = frame(&encoded)?;
        if let Some(body) = body {
            bytes.extend_from_slice(&body);
        }
        if self.data.len() == PAYLOAD {
            self.flush_data()?;
        }
        let offset = self.start + self.block * BLOCK_BYTES as u64 + 5 + self.data.len() as u64;
        self.append_data(&bytes)?;
        if self.small.as_ref().is_some_and(|small| {
            small.data.len().saturating_add(bytes.len()) > SMALL_CAPTURE_LIMIT
                || small.metadata_bytes.saturating_add(metadata.len() + 128) > SMALL_CAPTURE_LIMIT
        }) {
            self.small = None;
        }
        if let Some(small) = &mut self.small {
            small.entries.push(Entry {
                offset: small.data.len() as u64,
                frame_bytes: bytes.len() as u64,
                index,
                stored: stored.clone(),
            });
            small.metadata_bytes += metadata.len() + 128;
            small.data.extend_from_slice(&bytes);
        }
        self.entries.push(Entry {
            offset,
            frame_bytes: bytes.len() as u64,
            index,
            stored,
        });
        self.entry_bytes = self.entry_bytes.saturating_add(metadata.len() + 128);
        if self.entry_bytes >= INDEX_BATCH {
            self.flush_index()?;
        }
        Ok(())
    }
    fn append_data(&mut self, mut bytes: &[u8]) -> Result<(), CaptureError> {
        while !bytes.is_empty() {
            if self.data.len() == PAYLOAD {
                self.flush_data()?;
            }
            let take = bytes.len().min(PAYLOAD - self.data.len());
            self.data.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        Ok(())
    }
    fn put_block(&mut self, kind: u8, bytes: &[u8]) -> Result<u64, CaptureError> {
        if bytes.len() > PAYLOAD {
            return Err(CaptureError::QuotaExceeded);
        }
        let next = self
            .start
            .checked_add(
                (self.block + 1)
                    .checked_mul(BLOCK_BYTES as u64)
                    .ok_or(CaptureError::QuotaExceeded)?,
            )
            .ok_or(CaptureError::QuotaExceeded)?;
        if next > self.limits.max_file_bytes {
            return Err(CaptureError::QuotaExceeded);
        }
        let mut plain = vec![0; BLOCK_PLAIN];
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
        let mut encoded = Vec::with_capacity(chunk.bytes.len() + 5);
        encoded.push(chunk.codec);
        encoded.extend_from_slice(
            &u32::try_from(chunk.expanded)
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        );
        encoded.extend_from_slice(&chunk.bytes);
        if encoded.len() > INDEX_LIMIT {
            return Err(CaptureError::QuotaExceeded);
        }
        let block = self.block;
        for part in encoded.chunks(PAYLOAD) {
            self.put_block(kind, part)?;
        }
        Ok(Node {
            block,
            blocks: u32::try_from(self.block - block).map_err(|_| CaptureError::QuotaExceeded)?,
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
        self.flush_data()?;
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
        self.committed = self.start + self.block * BLOCK_BYTES as u64;
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
    let mut output = Vec::new();
    let (mut codec, header) = FrameCodec::create_with_block_size(&mut output, options, 1024)?;
    output.extend_from_slice(&[0; LOCATORS]);
    let start = header + LOCATORS as u64;
    codec.set_container_start(start);
    let payload = codec.payload();
    let mut ordinal = 0;
    let mut put = |kind: u8, bytes: &[u8]| -> Result<(), CaptureError> {
        let mut plain = vec![0; codec.block_plain()];
        plain[0] = kind;
        plain[1..5].copy_from_slice(
            &u32::try_from(bytes.len())
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        );
        plain[5..5 + bytes.len()].copy_from_slice(bytes);
        output.extend(codec.encode_block(ordinal, &plain)?);
        ordinal += 1;
        Ok(())
    };
    for part in small.data.chunks(payload) {
        put(DATA, part)?;
    }
    let mut entries = small.entries.clone();
    for entry in &mut entries {
        entry.offset = start
            + entry.offset / payload as u64 * codec.block_bytes() as u64
            + 5
            + entry.offset % payload as u64;
    }
    let mut compressor = ChunkCompressor::new(1)?;
    let mut pack = |bytes: &[u8]| -> Result<Vec<u8>, CaptureError> {
        let chunk = compressor.metadata(bytes)?;
        let mut packed = vec![chunk.codec];
        packed.extend_from_slice(
            &u32::try_from(chunk.expanded)
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        );
        packed.extend_from_slice(&chunk.bytes);
        Ok(packed)
    };
    let index = pack(&serde_json::to_vec(&entries)?)?;
    let first_index = small.data.len().div_ceil(payload) as u64;
    let blocks =
        u32::try_from(index.len().div_ceil(payload)).map_err(|_| CaptureError::QuotaExceeded)?;
    for part in index.chunks(payload) {
        put(LEAF, part)?;
    }
    let footer_block = first_index + u64::from(blocks);
    let roots = vec![Node {
        block: first_index,
        blocks,
        bytes: u32::try_from(index.len()).map_err(|_| CaptureError::QuotaExceeded)?,
        first: 0,
        count: entries.len() as u64,
        level: 0,
    }];
    let footer = pack(&serde_json::to_vec(&Commit {
        roots,
        count: entries.len() as u64,
        sealed: true,
        block: footer_block,
    })?)?;
    if footer.len() > payload {
        return Err(CaptureError::QuotaExceeded);
    }
    put(FOOTER, &footer)?;
    if output.len() as u64 > limits.max_file_bytes {
        return Err(CaptureError::QuotaExceeded);
    }
    Ok(output)
}

fn frame(bytes: &[u8]) -> Result<Vec<u8>, CaptureError> {
    let mut result = Vec::with_capacity(bytes.len() + 8);
    result.extend_from_slice(
        &u32::try_from(bytes.len())
            .map_err(|_| CaptureError::QuotaExceeded)?
            .to_le_bytes(),
    );
    result.extend_from_slice(&crc32fast::hash(bytes).to_le_bytes());
    result.extend_from_slice(bytes);
    Ok(result)
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
    entries: VecDeque<Entry>,
    certificates: HashMap<String, std::sync::Arc<[u8]>>,
    certificate_bytes: usize,
    data_end: u64,
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
            validate_nodes(&commit.roots, 0, commit.count, index)?;
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
            certificate_bytes: 0,
            data_end: start,
            records: 0,
            count: commit.count,
            committed,
            sealed: commit.sealed,
            truncated: committed != end,
        })
    }
    fn object(&mut self, node: &Node) -> Result<Vec<u8>, CaptureError> {
        if node.bytes as usize > INDEX_LIMIT
            || node.blocks == 0
            || u64::from(node.blocks) != u64::from(node.bytes).div_ceil(self.codec.payload() as u64)
        {
            return Err(CaptureError::QuotaExceeded);
        }
        let mut bytes = Vec::with_capacity(node.bytes as usize);
        for index in node.block..node.block + u64::from(node.blocks) {
            let (kind, part) = block(&mut self.input, &self.codec, index)?;
            if kind != if node.level == 0 { LEAF } else { BRANCH } {
                return Err(CaptureError::InvalidMagic);
            }
            bytes.extend_from_slice(&part);
            if bytes.len() > node.bytes as usize {
                return Err(CaptureError::InvalidMagic);
            }
        }
        if bytes.len() != node.bytes as usize {
            return Err(CaptureError::InvalidMagic);
        }
        unpack(&bytes, INDEX_LIMIT)
    }
    // Check physical references using index metadata only; body authentication stays lazy.
    fn validate_entry_ranges(
        &mut self,
        entries: &VecDeque<Entry>,
        before: u64,
    ) -> Result<(), CaptureError> {
        let stride = self.codec.block_bytes() as u64;
        let payload = self.codec.payload() as u64;
        let start = self.codec.container_start();
        let before = before
            .checked_mul(stride)
            .and_then(|n| n.checked_add(start))
            .ok_or(CaptureError::QuotaExceeded)?;
        let mut previous_end = self.data_end;
        for entry in entries {
            let relative = entry
                .offset
                .checked_sub(start)
                .ok_or(CaptureError::InvalidMagic)?;
            let within = (relative % stride)
                .checked_sub(5)
                .ok_or(CaptureError::InvalidMagic)?;
            // Eight framing bytes plus the thirteen-byte inner descriptor are mandatory.
            if within >= payload || entry.frame_bytes < 21 || entry.offset < previous_end {
                return Err(CaptureError::InvalidMagic);
            }
            let last = within
                .checked_add(entry.frame_bytes - 1)
                .ok_or(CaptureError::QuotaExceeded)?;
            let last_block = (relative / stride)
                .checked_add(last / payload)
                .ok_or(CaptureError::QuotaExceeded)?;
            let end = last_block
                .checked_mul(stride)
                .and_then(|n| n.checked_add(start))
                .and_then(|n| n.checked_add(6 + last % payload))
                .ok_or(CaptureError::QuotaExceeded)?;
            if end > before {
                return Err(CaptureError::InvalidMagic);
            }
            previous_end = end;
        }
        self.data_end = previous_end;
        Ok(())
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
                let mut children: Vec<Node> = serde_json::from_slice(&bytes)?;
                if children.len() > FANOUT
                    || children
                        .iter()
                        .any(|child| child.level.checked_add(1) != Some(node.level))
                {
                    return Err(CaptureError::InvalidMagic);
                }
                validate_nodes(&children, node.first, node.count, node.block)?;
                children.reverse();
                self.stack.extend(children);
            } else {
                let entries: VecDeque<Entry> = serde_json::from_slice(&bytes)?;
                if entries.len() as u64 != node.count
                    || entries
                        .front()
                        .is_none_or(|entry| entry.index != node.first)
                {
                    return Err(CaptureError::InvalidMagic);
                }
                self.validate_entry_ranges(&entries, node.block)?;
                self.entries = entries;
            }
        }
        let entry = self.entries.pop_front().ok_or(CaptureError::InvalidMagic)?;
        if entry.index != self.records
            || entry.offset < self.codec.container_start()
            || entry.offset >= self.committed
        {
            return Err(CaptureError::InvalidMagic);
        }
        let metadata_bytes = serde_json::to_vec(&entry.stored)?.len();
        if metadata_bytes > self.limits.max_record_bytes {
            return Err(CaptureError::RecordTooLarge {
                actual: metadata_bytes,
                limit: self.limits.max_record_bytes,
            });
        }
        if entry.frame_bytes > (self.limits.max_record_bytes as u64 + 64) * 2 {
            return Err(CaptureError::QuotaExceeded);
        }
        let (mut record, body) = entry.stored.load_metadata()?;
        if let Some(body) = body {
            body.validate(self.limits.max_record_bytes)?;
            if body.frame_bytes < 21 || body.frame_bytes > entry.frame_bytes.saturating_sub(21) {
                return Err(CaptureError::InvalidMagic);
            }
        }
        self.resolve_certificates(&mut record)?;
        if let CaptureRecordKind::Seal { record_count } = record.kind {
            if !self.sealed || record_count != entry.index || entry.index + 1 != self.count {
                return Err(CaptureError::AuthenticationFailed);
            }
        } else if self.sealed && entry.index + 1 == self.count {
            return Err(CaptureError::AuthenticationFailed);
        }
        self.records += 1;
        Ok(Some(CaptureFrame {
            offset: entry.offset,
            index: entry.index,
            frame_bytes: entry.frame_bytes,
            codec: self.codec.clone(),
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
        self.input.seek(SeekFrom::Start(frame.offset))?;
        let mut record = read_indexed_frame_with_codec(
            &mut self.input,
            frame.frame_bytes,
            self.limits,
            &self.codec,
            frame.index,
        )?;
        let body = match &mut record.kind {
            CaptureRecordKind::BodySegment { bytes, .. } => bytes.take(),
            _ => None,
        };
        if serde_json::to_vec(&store(&record)?)? != serde_json::to_vec(&store(&frame.record)?)? {
            return Err(CaptureError::AuthenticationFailed);
        }
        match (&frame.body, &body) {
            (Some(descriptor), Some(bytes))
                if descriptor.bytes == bytes.len()
                    && descriptor.digest == <[u8; 32]>::from(Sha256::digest(bytes)) => {}
            (None, None) => {}
            _ => return Err(CaptureError::AuthenticationFailed),
        }
        if let CaptureRecordKind::BodySegment { bytes, .. } = &mut frame.record.kind {
            *bytes = body;
        }
        Ok(Some(frame))
    }
    /// Cryptographic context for independent lazy body readers.
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
fn validate_nodes(nodes: &[Node], first: u64, count: u64, before: u64) -> Result<(), CaptureError> {
    let mut next = first;
    for node in nodes {
        if node.first != next
            || node.count == 0
            || node.level > 15
            || node
                .block
                .checked_add(u64::from(node.blocks))
                .is_none_or(|end| end > before)
        {
            return Err(CaptureError::InvalidMagic);
        }
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
    let offset = input.stream_position()?;
    let relative = offset
        .checked_sub(codec.container_start())
        .ok_or(CaptureError::InvalidMagic)?;
    let mut ordinal = relative / codec.block_bytes() as u64;
    let mut within = usize::try_from(
        (relative % codec.block_bytes() as u64)
            .checked_sub(5)
            .ok_or(CaptureError::InvalidMagic)?,
    )
    .map_err(|_| CaptureError::QuotaExceeded)?;
    let length = usize::try_from(frame_bytes).map_err(|_| CaptureError::QuotaExceeded)?;
    if length > limits.max_record_bytes.saturating_add(64).saturating_mul(2) {
        return Err(CaptureError::QuotaExceeded);
    }
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        let (kind, data) = block(&mut input, codec, ordinal)?;
        if kind != DATA || within > data.len() {
            return Err(CaptureError::InvalidMagic);
        }
        let take = (length - bytes.len()).min(data.len() - within);
        if take == 0 {
            return Err(CaptureError::InvalidMagic);
        }
        bytes.extend_from_slice(&data[within..within + take]);
        within = 0;
        ordinal += 1;
    }
    super::read_scratch_frame_with_codec(
        bytes.as_slice(),
        frame_bytes,
        limits,
        &codec.plain(),
        index,
    )
}

#[cfg(test)]
#[path = "container_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "review_tests.rs"]
mod review_tests;

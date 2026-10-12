//! Packed authenticated objects and bounded, single-hop body sharing.
use super::{
    BODY_CHUNK_BYTES, CaptureError, CompressedChunk, DATA, Deserialize, Digest, FrameCodec,
    HashMap, Node, Read, Seek, Serialize, Sha256, StoredBodyIndex, VecDeque, block,
};
use std::sync::Arc;

pub(super) const OBJECT_HEADER: usize = 10;
const SHARED_CHUNKS: usize = 65_536;
const DICTIONARY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BaseRef {
    pub offset: u64,
    pub descriptor: StoredBodyIndex,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BodyRef {
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dictionary: Option<BaseRef>,
}

#[derive(Clone)]
pub(crate) struct RecordLocation {
    pub(super) page: Node,
    pub(super) index: u64,
    pub(super) offset: u64,
    pub(super) frame_bytes: u64,
}

#[derive(Clone)]
pub(super) struct Dictionary {
    pub reference: BaseRef,
    pub bytes: Arc<[u8]>,
    pub upstream: bool,
}
pub(super) type PairKey = (u128, bool, u64);

#[derive(Default)]
pub(super) struct BodySharing {
    chunks: HashMap<PairKey, (StoredBodyIndex, BodyRef, bool)>,
    chunk_order: VecDeque<PairKey>,
    dictionaries: HashMap<PairKey, Dictionary>,
    dictionary_order: VecDeque<PairKey>,
    dictionary_bytes: usize,
}
impl BodySharing {
    pub fn shared(
        &self,
        descriptor: &StoredBodyIndex,
        pair: Option<(PairKey, bool)>,
    ) -> Option<(StoredBodyIndex, BodyRef)> {
        let (key, upstream) = pair?;
        self.chunks
            .get(&key)
            .and_then(|(saved, reference, saved_upstream)| {
                (saved.bytes == descriptor.bytes
                    && saved.digest == descriptor.digest
                    && *saved_upstream != upstream)
                    .then(|| (*saved, reference.clone()))
            })
    }
    pub fn dictionary(&self, key: PairKey, upstream: bool) -> Option<Dictionary> {
        self.dictionaries
            .get(&key)
            .filter(|base| base.upstream != upstream)
            .cloned()
    }
    pub fn insert(
        &mut self,
        descriptor: StoredBodyIndex,
        reference: &BodyRef,
        pair: Option<(PairKey, bool)>,
        raw: Vec<u8>,
    ) {
        let Some((key, upstream)) = pair else { return };
        if !self.chunks.contains_key(&key) {
            while self.chunks.len() >= SHARED_CHUNKS {
                if let Some(digest) = self.chunk_order.pop_front() {
                    self.chunks.remove(&digest);
                }
            }
            self.chunk_order.push_back(key);
            self.chunks
                .insert(key, (descriptor, reference.clone(), upstream));
        }
        // Dictionary-compressed chunks never become dictionaries: decoding has
        // exactly one dependency, even across arbitrarily many checkpoints.
        if reference.dictionary.is_some()
            || raw.len() < 1024
            || self.dictionaries.contains_key(&key)
        {
            return;
        }
        let charge = raw.len().saturating_add(256);
        while self.dictionary_bytes.saturating_add(charge) > DICTIONARY_BYTES {
            let Some(old) = self.dictionary_order.pop_front() else {
                return;
            };
            if let Some(old) = self.dictionaries.remove(&old) {
                self.dictionary_bytes -= old.bytes.len() + 256;
            }
        }
        self.dictionary_order.push_back(key);
        self.dictionary_bytes += charge;
        self.dictionaries.insert(
            key,
            Dictionary {
                reference: BaseRef {
                    offset: reference.offset,
                    descriptor,
                },
                bytes: raw.into(),
                upstream,
            },
        );
    }
}

pub(super) fn pack(kind: u8, chunk: &CompressedChunk) -> Result<Vec<u8>, CaptureError> {
    let mut bytes = Vec::with_capacity(OBJECT_HEADER + chunk.bytes.len());
    bytes.extend([kind, chunk.codec]);
    bytes.extend(
        u32::try_from(chunk.expanded)
            .map_err(|_| CaptureError::QuotaExceeded)?
            .to_le_bytes(),
    );
    bytes.extend(
        u32::try_from(chunk.bytes.len())
            .map_err(|_| CaptureError::QuotaExceeded)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&chunk.bytes);
    Ok(bytes)
}

pub(super) fn span_end(codec: &FrameCodec, offset: u64, bytes: u64) -> Result<u64, CaptureError> {
    let relative = offset
        .checked_sub(codec.container_start())
        .ok_or(CaptureError::InvalidMagic)?;
    let stride = codec.block_bytes() as u64;
    let within = (relative % stride)
        .checked_sub(5)
        .ok_or(CaptureError::InvalidMagic)?;
    let payload = codec.payload() as u64;
    if within >= payload || bytes < OBJECT_HEADER as u64 {
        return Err(CaptureError::InvalidMagic);
    }
    let last = within
        .checked_add(bytes - 1)
        .ok_or(CaptureError::QuotaExceeded)?;
    (relative / stride)
        .checked_add(last / payload)
        .and_then(|block| block.checked_mul(stride))
        .and_then(|value| value.checked_add(codec.container_start()))
        .and_then(|value| value.checked_add(6 + last % payload))
        .ok_or(CaptureError::QuotaExceeded)
}

pub(super) fn read_object(
    input: &mut (impl Read + Seek),
    codec: &FrameCodec,
    offset: u64,
    length: u64,
    kind: u8,
    maximum: usize,
    dictionary: Option<&[u8]>,
) -> Result<Vec<u8>, CaptureError> {
    span_end(codec, offset, length)?;
    let length = usize::try_from(length).map_err(|_| CaptureError::QuotaExceeded)?;
    if length > maximum.saturating_add(OBJECT_HEADER) {
        return Err(CaptureError::QuotaExceeded);
    }
    let relative = offset - codec.container_start();
    let mut ordinal = relative / codec.block_bytes() as u64;
    let mut within = usize::try_from(relative % codec.block_bytes() as u64 - 5)
        .map_err(|_| CaptureError::QuotaExceeded)?;
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        let (block_kind, data) = block(input, codec, ordinal)?;
        if block_kind != DATA || within >= data.len() {
            return Err(CaptureError::InvalidMagic);
        }
        let take = (length - bytes.len()).min(data.len() - within);
        bytes.extend_from_slice(&data[within..within + take]);
        if bytes.len() < length && data.len() != codec.payload() {
            return Err(CaptureError::InvalidMagic);
        }
        within = 0;
        ordinal = ordinal.checked_add(1).ok_or(CaptureError::QuotaExceeded)?;
    }
    if bytes[0] != kind {
        return Err(CaptureError::InvalidMagic);
    }
    let expanded = u32::from_le_bytes(bytes[2..6].try_into().unwrap()) as usize;
    let encoded = u32::from_le_bytes(bytes[6..10].try_into().unwrap()) as usize;
    if expanded > maximum {
        return Err(CaptureError::QuotaExceeded);
    }
    if encoded != bytes.len() - OBJECT_HEADER {
        return Err(CaptureError::InvalidMagic);
    }
    let decoded = match (bytes[1], dictionary) {
        (0, None) => bytes[OBJECT_HEADER..].to_vec(),
        (1, None) => zstd::bulk::decompress(&bytes[OBJECT_HEADER..], expanded)?,
        (2, Some(dictionary)) => zstd::bulk::Decompressor::with_dictionary(dictionary)?
            .decompress(&bytes[OBJECT_HEADER..], expanded)?,
        _ => return Err(CaptureError::InvalidMagic),
    };
    if decoded.len() != expanded {
        return Err(CaptureError::InvalidMagic);
    }
    Ok(decoded)
}

pub(super) fn validate_body(
    reference: &BodyRef,
    descriptor: StoredBodyIndex,
    codec: &FrameCodec,
    before: u64,
    maximum: usize,
) -> Result<(), CaptureError> {
    if descriptor.frame_bytes < OBJECT_HEADER as u64 {
        return Err(CaptureError::InvalidMagic);
    }
    descriptor.validate(maximum)?;
    if span_end(codec, reference.offset, descriptor.frame_bytes)? > before {
        return Err(CaptureError::InvalidMagic);
    }
    if let Some(base) = &reference.dictionary {
        base.descriptor.validate(maximum.min(BODY_CHUNK_BYTES))?;
        if base.descriptor.bytes == 0
            || span_end(codec, base.offset, base.descriptor.frame_bytes)? > reference.offset
        {
            return Err(CaptureError::InvalidMagic);
        }
    }
    Ok(())
}

pub(super) fn read_body(
    input: &mut (impl Read + Seek),
    codec: &FrameCodec,
    reference: &BodyRef,
    descriptor: StoredBodyIndex,
    before: u64,
    maximum: usize,
) -> Result<Vec<u8>, CaptureError> {
    validate_body(reference, descriptor, codec, before, maximum)?;
    let dictionary = reference
        .dictionary
        .as_ref()
        .map(|base| {
            let bytes = read_object(
                input,
                codec,
                base.offset,
                base.descriptor.frame_bytes,
                DATA,
                maximum.min(BODY_CHUNK_BYTES),
                None,
            )?;
            check_digest(&bytes, base.descriptor)?;
            Ok::<_, CaptureError>(bytes)
        })
        .transpose()?;
    let bytes = read_object(
        input,
        codec,
        reference.offset,
        descriptor.frame_bytes,
        DATA,
        maximum,
        dictionary.as_deref(),
    )?;
    check_digest(&bytes, descriptor)?;
    Ok(bytes)
}
fn check_digest(bytes: &[u8], descriptor: StoredBodyIndex) -> Result<(), CaptureError> {
    if bytes.len() != descriptor.bytes
        || <[u8; 32]>::from(Sha256::digest(bytes)) != descriptor.digest
    {
        return Err(CaptureError::AuthenticationFailed);
    }
    Ok(())
}

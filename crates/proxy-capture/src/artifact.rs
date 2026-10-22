//! Sealed, seekable portable ZIP payloads using `TMCap`'s existing block codec.
use crate::compression::{BODY_CHUNK_BYTES, BodyCompression, ChunkCompressor};
use crate::{CaptureEncoding, CaptureError, CapturePassword, FrameCodec, encoding::MAGIC};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom, Write};

const BLOCK: usize = 16 * 1024;
const PHYSICAL: u64 = (BLOCK + 16) as u64;
const MAX_INDEX: usize = 16 * 1024 * 1024;
const ENTRY: usize = 32;
const FOOTER: &[u8; 8] = b"TMARCH01";

/// Protected descriptor of the exact inner ZIP stream, independent of outer bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDescriptor {
    /// `application/vnd.transmog.case+zip` or `application/vnd.transmog.rules+zip`.
    pub media_type: String,
    /// Exact unsigned-64-bit logical length.
    pub bytes: u64,
    /// SHA-256 of the complete unwrapped ZIP, not publisher identity.
    pub sha256: String,
}
#[derive(Clone)]
struct Chunk {
    logical: u64,
    offset: u64,
    plain: u32,
    stored: u32,
    codec: u8,
}

fn invalid() -> CaptureError {
    CaptureError::MalformedRecord("Invalid portable archive container")
}
fn io_error(error: CaptureError) -> io::Error {
    io::Error::other(error)
}
fn media(value: &str) -> bool {
    matches!(
        value,
        "application/vnd.transmog.case+zip" | "application/vnd.transmog.rules+zip"
    )
}

/// Streaming writer. Member boundaries reset adaptive sampling without padding blocks.
pub struct ArtifactWriter<W> {
    output: W,
    codec: FrameCodec,
    block: u64,
    data: Vec<u8>,
    chunk: Vec<u8>,
    chunks: Vec<Chunk>,
    encoded: u64,
    logical: u64,
    hash: Sha256,
    compressor: ChunkCompressor,
    compression: BodyCompression,
    compress: bool,
    media: String,
    failed: bool,
}
impl<W: Write> ArtifactWriter<W> {
    /// Starts a new compressed or raw artifact; the caller owns transactional publication.
    /// # Errors
    /// Rejects media types, password initialization and output failures.
    pub fn new(
        mut output: W,
        media_type: &str,
        encoding: &CaptureEncoding,
        compress: bool,
    ) -> Result<Self, CaptureError> {
        if !media(media_type) {
            return Err(invalid());
        }
        let (codec, _) =
            FrameCodec::create_kind(&mut output, encoding, 16 * 1024, "portable-archive")?;
        Ok(Self {
            output,
            codec,
            block: 0,
            data: Vec::with_capacity(BLOCK),
            chunk: Vec::with_capacity(BODY_CHUNK_BYTES),
            chunks: vec![],
            encoded: 0,
            logical: 0,
            hash: Sha256::new(),
            compressor: ChunkCompressor::new(1)?,
            compression: BodyCompression::default(),
            compress,
            media: media_type.into(),
            failed: false,
        })
    }
    fn block(&mut self, plain: &[u8]) -> Result<(), CaptureError> {
        self.output
            .write_all(&self.codec.encode_block(self.block, plain)?)?;
        self.block = self.block.checked_add(1).ok_or_else(invalid)?;
        Ok(())
    }
    fn data(&mut self, mut bytes: &[u8]) -> Result<(), CaptureError> {
        while !bytes.is_empty() {
            let count = bytes.len().min(BLOCK - self.data.len());
            self.data.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.data.len() == BLOCK {
                let full = std::mem::replace(&mut self.data, Vec::with_capacity(BLOCK));
                self.block(&full)?;
            }
        }
        Ok(())
    }
    fn flush_data(&mut self) -> Result<(), CaptureError> {
        if !self.data.is_empty() {
            let mut last = std::mem::replace(&mut self.data, Vec::with_capacity(BLOCK));
            last.resize(BLOCK, 0);
            self.block(&last)?;
        }
        Ok(())
    }
    fn chunk(&mut self) -> Result<(), CaptureError> {
        if self.chunk.is_empty() {
            return Ok(());
        }
        if self.chunks.len() >= (MAX_INDEX - 4096) / ENTRY {
            return Err(CaptureError::QuotaExceeded);
        }
        let plain = std::mem::replace(&mut self.chunk, Vec::with_capacity(BODY_CHUNK_BYTES));
        let encoded = if self.compress {
            self.compressor.body(&plain, &mut self.compression)?
        } else {
            crate::compression::CompressedChunk {
                codec: 0,
                expanded: plain.len(),
                bytes: plain.clone(),
            }
        };
        self.data(&encoded.bytes)?;
        self.chunks.push(Chunk {
            logical: self.logical - plain.len() as u64,
            offset: self.encoded,
            plain: u32::try_from(plain.len()).map_err(|_| invalid())?,
            stored: u32::try_from(encoded.bytes.len()).map_err(|_| invalid())?,
            codec: encoded.codec,
        });
        self.encoded = self
            .encoded
            .checked_add(encoded.bytes.len() as u64)
            .ok_or_else(invalid)?;
        Ok(())
    }
    /// Ends a ZIP member's compression scope. Does not pad a physical block.
    /// # Errors
    /// Returns output, codec or finite-index failures.
    pub fn member_boundary(&mut self) -> Result<(), CaptureError> {
        if self.failed {
            return Err(invalid());
        }
        let result = self.chunk();
        if result.is_err() {
            self.failed = true;
        }
        self.compression = BodyCompression::default();
        result
    }
    /// Writes the protected descriptor/index and authenticated completion block.
    /// # Errors
    /// A failed stream cannot be sealed; output is incomplete until this succeeds.
    pub fn finish(mut self) -> Result<(W, ArtifactDescriptor), CaptureError> {
        if self.failed {
            return Err(invalid());
        }
        self.chunk()?;
        self.flush_data()?;
        let descriptor = ArtifactDescriptor {
            media_type: self.media.clone(),
            bytes: self.logical,
            sha256: format!("{:x}", self.hash.clone().finalize()),
        };
        let json = serde_json::to_vec(&descriptor)?;
        let mut index = Vec::with_capacity(4 + json.len() + ENTRY * self.chunks.len());
        index.extend_from_slice(
            &u32::try_from(json.len())
                .map_err(|_| invalid())?
                .to_le_bytes(),
        );
        index.extend_from_slice(&json);
        for entry in &self.chunks {
            index.extend_from_slice(&entry.logical.to_le_bytes());
            index.extend_from_slice(&entry.offset.to_le_bytes());
            index.extend_from_slice(&entry.plain.to_le_bytes());
            index.extend_from_slice(&entry.stored.to_le_bytes());
            index.push(entry.codec);
            index.extend_from_slice(&[0; 7]);
        }
        if index.len() > MAX_INDEX {
            return Err(CaptureError::QuotaExceeded);
        }
        self.data(&index)?;
        self.flush_data()?;
        let mut footer = vec![0; BLOCK];
        footer[..8].copy_from_slice(FOOTER);
        footer[8..16].copy_from_slice(&(index.len() as u64).to_le_bytes());
        footer[16..24].copy_from_slice(&self.encoded.to_le_bytes());
        self.block(&footer)?;
        self.output.flush()?;
        Ok((self.output, descriptor))
    }
}
impl<W: Write> Write for ArtifactWriter<W> {
    fn write(&mut self, mut input: &[u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io_error(invalid()));
        }
        let length = input.len();
        let result = (|| {
            while !input.is_empty() {
                let count = input.len().min(BODY_CHUNK_BYTES - self.chunk.len());
                self.logical = self.logical.checked_add(count as u64).ok_or_else(invalid)?;
                self.hash.update(&input[..count]);
                self.chunk.extend_from_slice(&input[..count]);
                input = &input[count..];
                if self.chunk.len() == BODY_CHUNK_BYTES {
                    self.chunk()?;
                }
            }
            Ok(length)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result.map_err(io_error)
    }
    fn flush(&mut self) -> io::Result<()> {
        let result = self.output.flush();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

/// Integrity-checked logical ZIP view. Never creates a decrypted temporary archive.
pub struct ArtifactReader<R> {
    input: R,
    codec: FrameCodec,
    start: u64,
    descriptor: ArtifactDescriptor,
    chunks: Vec<Chunk>,
    position: u64,
    cache: Option<(usize, Vec<u8>)>,
}
impl<R: Read + Seek> ArtifactReader<R> {
    /// Opens only a sealed portable artifact; validates bounded indexes before allocation.
    /// # Errors
    /// Rejects capture documents, unknown KDFs, wrong passwords and incomplete/tampered data.
    pub fn new(mut input: R, password: Option<&CapturePassword>) -> Result<Self, CaptureError> {
        input.seek(SeekFrom::Start(0))?;
        let mut magic = [0; 9];
        input.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(CaptureError::InvalidMagic);
        }
        let (codec, start) = FrameCodec::read_kind(&mut input, password, "portable-archive")?;
        let length = input.seek(SeekFrom::End(0))?;
        let physical = length.checked_sub(start).ok_or_else(invalid)?;
        if physical % PHYSICAL != 0 || physical < 2 * PHYSICAL {
            return Err(invalid());
        }
        let blocks = physical / PHYSICAL;
        let footer = block(&mut input, &codec, start, blocks - 1)?;
        if &footer[..8] != FOOTER || footer[24..].iter().any(|b| *b != 0) {
            return Err(invalid());
        }
        let index_len = u64::from_le_bytes(footer[8..16].try_into().map_err(|_| invalid())?);
        let encoded = u64::from_le_bytes(footer[16..24].try_into().map_err(|_| invalid())?);
        if !(4..=MAX_INDEX as u64).contains(&index_len) {
            return Err(invalid());
        }
        let data_blocks = encoded.div_ceil(BLOCK as u64);
        if data_blocks
            .checked_add(index_len.div_ceil(BLOCK as u64))
            .and_then(|n| n.checked_add(1))
            != Some(blocks)
        {
            return Err(invalid());
        }
        let index = range(
            &mut input,
            &codec,
            start,
            data_blocks * BLOCK as u64,
            usize::try_from(index_len).map_err(|_| invalid())?,
        )?;
        let json_len = u32::from_le_bytes(index[..4].try_into().map_err(|_| invalid())?) as usize;
        if json_len > 4096
            || json_len + 4 > index.len()
            || !(index.len() - 4 - json_len).is_multiple_of(ENTRY)
        {
            return Err(invalid());
        }
        let descriptor: ArtifactDescriptor = serde_json::from_slice(&index[4..4 + json_len])?;
        if !media(&descriptor.media_type)
            || descriptor.sha256.len() != 64
            || !descriptor
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid());
        }
        let mut chunks = vec![];
        let mut logical = 0_u64;
        let mut offset = 0_u64;
        for entry in index[4 + json_len..].as_chunks::<ENTRY>().0 {
            let chunk = Chunk {
                logical: u64::from_le_bytes(entry[..8].try_into().map_err(|_| invalid())?),
                offset: u64::from_le_bytes(entry[8..16].try_into().map_err(|_| invalid())?),
                plain: u32::from_le_bytes(entry[16..20].try_into().map_err(|_| invalid())?),
                stored: u32::from_le_bytes(entry[20..24].try_into().map_err(|_| invalid())?),
                codec: entry[24],
            };
            if chunk.logical != logical
                || chunk.offset != offset
                || chunk.plain == 0
                || chunk.plain as usize > BODY_CHUNK_BYTES
                || chunk.stored == 0
                || chunk.stored as usize > BODY_CHUNK_BYTES
                || chunk.codec > 1
                || (chunk.codec == 0 && chunk.stored != chunk.plain)
                || entry[25..].iter().any(|b| *b != 0)
            {
                return Err(invalid());
            }
            logical = logical
                .checked_add(u64::from(chunk.plain))
                .ok_or_else(invalid)?;
            offset = offset
                .checked_add(u64::from(chunk.stored))
                .ok_or_else(invalid)?;
            chunks.push(chunk);
        }
        if logical != descriptor.bytes || offset != encoded {
            return Err(invalid());
        }
        Ok(Self {
            input,
            codec,
            start,
            descriptor,
            chunks,
            position: 0,
            cache: None,
        })
    }
    /// Exact protected payload identity, without any plaintext descriptive sidecar.
    pub fn descriptor(&self) -> &ArtifactDescriptor {
        &self.descriptor
    }
    /// Whether the outer payload/descriptor/index are password encrypted.
    pub fn encrypted(&self) -> bool {
        self.codec.encrypted()
    }
    /// Verifies the entire logical stream before publication, with bounded buffers.
    /// # Errors
    /// Returns corruption, authentication, I/O or explicit cancellation.
    pub fn verify(&mut self, mut canceled: impl FnMut() -> bool) -> Result<(), CaptureError> {
        self.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; BODY_CHUNK_BYTES];
        loop {
            if canceled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Artifact verification canceled",
                )
                .into());
            }
            let count = self.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        if format!("{:x}", hash.finalize()) != self.descriptor.sha256 {
            return Err(invalid());
        }
        self.seek(SeekFrom::Start(0))?;
        Ok(())
    }
}
fn block(
    input: &mut (impl Read + Seek),
    codec: &FrameCodec,
    start: u64,
    index: u64,
) -> Result<Vec<u8>, CaptureError> {
    let at = index
        .checked_mul(PHYSICAL)
        .and_then(|n| n.checked_add(start))
        .ok_or_else(invalid)?;
    input.seek(SeekFrom::Start(at))?;
    let mut encoded = vec![0; BLOCK + 16];
    input.read_exact(&mut encoded)?;
    codec.decode_block(index, &encoded)
}
fn range(
    input: &mut (impl Read + Seek),
    codec: &FrameCodec,
    start: u64,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>, CaptureError> {
    let mut result = Vec::with_capacity(length);
    let mut at = offset;
    while result.len() < length {
        let plain = block(input, codec, start, at / BLOCK as u64)?;
        let skip = usize::try_from(at % BLOCK as u64).map_err(|_| invalid())?;
        let count = (length - result.len()).min(BLOCK - skip);
        result.extend_from_slice(&plain[skip..skip + count]);
        at += count as u64;
    }
    Ok(result)
}
impl<R: Read + Seek> Read for ArtifactReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.descriptor.bytes || output.is_empty() {
            return Ok(0);
        }
        let index = self.chunks.partition_point(|c| c.logical <= self.position) - 1;
        if self.cache.as_ref().map(|c| c.0) != Some(index) {
            let chunk = &self.chunks[index];
            let stored = range(
                &mut self.input,
                &self.codec,
                self.start,
                chunk.offset,
                chunk.stored as usize,
            )
            .map_err(io_error)?;
            let plain = if chunk.codec == 0 {
                stored
            } else {
                let plain = zstd::bulk::decompress(&stored, chunk.plain as usize)?;
                if plain.len() != chunk.plain as usize {
                    return Err(io_error(invalid()));
                }
                plain
            };
            self.cache = Some((index, plain));
        }
        let plain = &self.cache.as_ref().unwrap().1;
        let skip = usize::try_from(self.position - self.chunks[index].logical)
            .map_err(|_| io_error(invalid()))?;
        let count = output.len().min(plain.len() - skip);
        output[..count].copy_from_slice(&plain[skip..skip + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl<R: Read + Seek> Seek for ArtifactReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let at = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.descriptor.bytes) + i128::from(n),
        };
        self.position = u64::try_from(at)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid archive seek"))?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    struct FullDisk(usize);
    impl Write for FullDisk {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "Injected full disk",
                ));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn failed_output_cannot_be_retried_or_sealed() {
        let mut writer = ArtifactWriter::new(
            FullDisk(4096),
            "application/vnd.transmog.case+zip",
            &CaptureEncoding::default(),
            false,
        )
        .unwrap();
        assert!(writer.write_all(&vec![1; BODY_CHUNK_BYTES]).is_err());
        assert!(writer.write_all(b"retry").is_err());
        assert!(writer.member_boundary().is_err());
        assert!(writer.finish().is_err());
    }
    #[test]
    fn sealed_artifact_is_seekable_and_binds_every_chunk_and_completion() {
        let input = (0_u32..600_000)
            .map(|n| u8::try_from(n % 233).unwrap())
            .collect::<Vec<_>>();
        let mut writer = ArtifactWriter::new(
            Vec::new(),
            "application/vnd.transmog.case+zip",
            &CaptureEncoding::default(),
            true,
        )
        .unwrap();
        writer.write_all(&input[..12345]).unwrap();
        writer.member_boundary().unwrap();
        writer.write_all(&input[12345..]).unwrap();
        let (bytes, descriptor) = writer.finish().unwrap();
        assert_eq!(descriptor.bytes, input.len() as u64);
        let mut reader = ArtifactReader::new(Cursor::new(&bytes), None).unwrap();
        reader.verify(|| false).unwrap();
        for start in [0, 12300, 12500, 131_070, 599_900] {
            reader.seek(SeekFrom::Start(start)).unwrap();
            let mut actual = vec![0; 100];
            reader.read_exact(&mut actual).unwrap();
            assert_eq!(
                actual,
                input[usize::try_from(start).unwrap()..usize::try_from(start).unwrap() + 100]
            );
        }
        assert!(
            crate::CaptureReader::new(Cursor::new(&bytes), crate::CaptureLimits::default())
                .is_err()
        );
        for cut in [1, BLOCK + 16, bytes.len() / 2] {
            assert!(ArtifactReader::new(Cursor::new(&bytes[..bytes.len() - cut]), None).is_err());
        }
        let mut corrupt = bytes.clone();
        let offset = usize::try_from(reader.start).unwrap() + 50;
        corrupt[offset] ^= 1;
        let mut bad = ArtifactReader::new(Cursor::new(corrupt), None).unwrap();
        assert!(bad.verify(|| false).is_err());
        assert!(reader.verify(|| true).is_err());
    }
    #[test]
    fn encrypted_descriptor_and_payload_use_existing_password_codec() {
        let password = CapturePassword::new("fixture-password".into());
        let secret = b"secret-title-notes-context";
        let mut writer = ArtifactWriter::new(
            Vec::new(),
            "application/vnd.transmog.rules+zip",
            &CaptureEncoding {
                password: Some(password.clone()),
            },
            true,
        )
        .unwrap();
        writer.write_all(secret).unwrap();
        let (bytes, _) = writer.finish().unwrap();
        assert!(!bytes.windows(secret.len()).any(|w| w == secret));
        assert!(
            !bytes
                .windows(b"application/vnd.transmog.rules+zip".len())
                .any(|w| w == b"application/vnd.transmog.rules+zip")
        );
        assert!(matches!(
            ArtifactReader::new(Cursor::new(&bytes), None),
            Err(CaptureError::PasswordRequired)
        ));
        assert!(matches!(
            ArtifactReader::new(
                Cursor::new(&bytes),
                Some(&CapturePassword::new("wrong".into()))
            ),
            Err(CaptureError::InvalidPassword)
        ));
        let mut reader = ArtifactReader::new(Cursor::new(bytes), Some(&password)).unwrap();
        reader.verify(|| false).unwrap();
        let mut actual = vec![];
        reader.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, secret);
    }
}

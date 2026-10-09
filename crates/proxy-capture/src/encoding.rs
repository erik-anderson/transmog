//! Independently compressed frames with optional password-derived AES-256-GCM.
use crate::CaptureError;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{Read, Write},
    sync::Arc,
};
use zeroize::Zeroizing;

pub(crate) const MAGIC: [u8; 8] = *b"TMCAP05\0";
const MAX_HEADER: usize = 4096;
const FRAME_OVERHEAD: usize = 29;

/// A transient password which is never printed or serialized into preferences.
#[derive(Clone)]
pub struct CapturePassword(Arc<Zeroizing<String>>);
impl<'de> Deserialize<'de> for CapturePassword {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}
impl fmt::Debug for CapturePassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapturePassword([redacted])")
    }
}
impl CapturePassword {
    /// Own a password in memory, wiping its bytes when the last reference drops.
    pub fn new(password: String) -> Self {
        Self(Arc::new(Zeroizing::new(password)))
    }
    /// Borrow bytes only for a cryptographic operation.
    pub fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Encoding choices for a new native capture. Each frame is compressed independently.
#[derive(Clone, Debug, Default)]
pub struct CaptureEncoding {
    /// An optional password enables AES-256-GCM after compression.
    pub password: Option<CapturePassword>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    compression: String,
    cipher: String,
    kdf: String,
    salt: [u8; 16],
    nonce_prefix: [u8; 4],
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
}

/// Immutable decoding context shared by indexed payload readers.
/// Keys remain in memory and Debug never reveals them.
#[derive(Clone)]
pub struct FrameCodec {
    inner: Arc<CodecInner>,
}
struct CodecInner {
    header: Vec<u8>,
    key: Option<Zeroizing<[u8; 32]>>,
    prefix: [u8; 4],
}
impl fmt::Debug for FrameCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameCodec")
            .field("compressed", &true)
            .field("encrypted", &self.inner.key.is_some())
            .finish()
    }
}
impl FrameCodec {
    pub(crate) fn create(
        output: &mut impl Write,
        options: &CaptureEncoding,
    ) -> Result<(Self, u64), CaptureError> {
        let mut salt = [0; 16];
        let mut prefix = [0; 4];
        getrandom::fill(&mut salt)
            .map_err(|_| CaptureError::Crypto("System randomness is unavailable"))?;
        getrandom::fill(&mut prefix)
            .map_err(|_| CaptureError::Crypto("System randomness is unavailable"))?;
        let encrypted = options.password.is_some();
        let header = Header {
            version: 5,
            compression: "deflate".into(),
            cipher: if encrypted { "aes-256-gcm" } else { "none" }.into(),
            kdf: if encrypted { "argon2id" } else { "none" }.into(),
            salt,
            nonce_prefix: prefix,
            memory_kib: if encrypted { 65536 } else { 0 },
            iterations: if encrypted { 3 } else { 0 },
            lanes: u32::from(encrypted),
        };
        let encoded = serde_json::to_vec(&header)?;
        let codec = Self::from_header(encoded.clone(), &header, options.password.as_ref())?;
        let proof = codec.proof()?;
        output.write_all(&MAGIC)?;
        output.write_all(
            &u32::try_from(encoded.len())
                .map_err(|_| CaptureError::InvalidMagic)?
                .to_le_bytes(),
        )?;
        output.write_all(&encoded)?;
        output.write_all(&proof)?;
        Ok((codec, 12 + encoded.len() as u64 + proof.len() as u64))
    }
    pub(crate) fn read(
        input: &mut impl Read,
        password: Option<&CapturePassword>,
    ) -> Result<(Self, u64), CaptureError> {
        let mut length = [0; 4];
        input.read_exact(&mut length)?;
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 || length > MAX_HEADER {
            return Err(CaptureError::InvalidMagic);
        }
        let mut encoded = vec![0; length];
        input.read_exact(&mut encoded)?;
        let header: Header = serde_json::from_slice(&encoded)?;
        let codec = Self::from_header(encoded, &header, password)?;
        let mut proof = vec![0; codec.proof()?.len()];
        input.read_exact(&mut proof)?;
        if codec.inner.key.is_some() {
            codec
                .decrypt(0, &[], &proof)
                .map_err(|_| CaptureError::InvalidPassword)?;
        }
        Ok((codec, 12 + length as u64 + proof.len() as u64))
    }
    fn from_header(
        encoded: Vec<u8>,
        header: &Header,
        password: Option<&CapturePassword>,
    ) -> Result<Self, CaptureError> {
        if header.version != 5 || header.compression != "deflate" {
            return Err(CaptureError::UnsupportedEncoding);
        }
        let key = match (header.cipher.as_str(), header.kdf.as_str()) {
            ("none", "none")
                if header.memory_kib == 0 && header.iterations == 0 && header.lanes == 0 =>
            {
                None
            }
            ("aes-256-gcm", "argon2id")
                if header.memory_kib == 65536 && header.iterations == 3 && header.lanes == 1 =>
            {
                let password = password.ok_or(CaptureError::PasswordRequired)?;
                if password.bytes().is_empty() {
                    return Err(CaptureError::InvalidPassword);
                }
                let params =
                    Params::new(header.memory_kib, header.iterations, header.lanes, Some(32))
                        .map_err(|_| CaptureError::UnsupportedEncoding)?;
                let mut key = Zeroizing::new([0; 32]);
                Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
                    .hash_password_into(password.bytes(), &header.salt, &mut *key)
                    .map_err(|_| CaptureError::Crypto("Password derivation failed"))?;
                Some(key)
            }
            _ => return Err(CaptureError::UnsupportedEncoding),
        };
        Ok(Self {
            inner: Arc::new(CodecInner {
                header: encoded,
                key,
                prefix: header.nonce_prefix,
            }),
        })
    }
    fn nonce(&self, counter: u64) -> [u8; 12] {
        let mut nonce = [0; 12];
        nonce[..4].copy_from_slice(&self.inner.prefix);
        nonce[4..].copy_from_slice(&counter.to_be_bytes());
        nonce
    }
    fn aad(&self, counter: u64, frame_head: &[u8]) -> Vec<u8> {
        let mut aad = self.inner.header.clone();
        aad.extend_from_slice(&counter.to_le_bytes());
        aad.extend_from_slice(frame_head);
        aad
    }
    fn encrypt(&self, counter: u64, head: &[u8], bytes: &[u8]) -> Result<Vec<u8>, CaptureError> {
        let Some(key) = self.inner.key.as_ref() else {
            return Ok(bytes.to_vec());
        };
        let cipher = Aes256Gcm::new_from_slice(&**key)
            .map_err(|_| CaptureError::Crypto("Invalid encryption key"))?;
        cipher
            .encrypt(
                Nonce::from_slice(&self.nonce(counter)),
                Payload {
                    msg: bytes,
                    aad: &self.aad(counter, head),
                },
            )
            .map_err(|_| CaptureError::Crypto("Frame encryption failed"))
    }
    fn decrypt(&self, counter: u64, head: &[u8], bytes: &[u8]) -> Result<Vec<u8>, CaptureError> {
        let Some(key) = self.inner.key.as_ref() else {
            return Ok(bytes.to_vec());
        };
        let cipher = Aes256Gcm::new_from_slice(&**key)
            .map_err(|_| CaptureError::Crypto("Invalid decryption key"))?;
        cipher
            .decrypt(
                Nonce::from_slice(&self.nonce(counter)),
                Payload {
                    msg: bytes,
                    aad: &self.aad(counter, head),
                },
            )
            .map_err(|_| CaptureError::AuthenticationFailed)
    }
    fn proof(&self) -> Result<Vec<u8>, CaptureError> {
        if self.inner.key.is_some() {
            self.encrypt(0, &[], &[])
        } else {
            Ok(Vec::new())
        }
    }
    pub(crate) fn encoded_bound(maximum: usize) -> usize {
        maximum.saturating_add(FRAME_OVERHEAD)
    }
    fn counter(index: u64, body: bool) -> Result<u64, CaptureError> {
        index
            .checked_mul(2)
            .and_then(|value| value.checked_add(if body { 2 } else { 1 }))
            .ok_or(CaptureError::RecordCountExceeded)
    }
    pub(crate) fn encode_body(&self, bytes: &[u8], index: u64) -> Result<Vec<u8>, CaptureError> {
        self.encode_counter(bytes, Self::counter(index, true)?)
    }
    pub(crate) fn encode(&self, bytes: &[u8], index: u64) -> Result<Vec<u8>, CaptureError> {
        self.encode_counter(bytes, Self::counter(index, false)?)
    }
    fn encode_counter(&self, bytes: &[u8], counter: u64) -> Result<Vec<u8>, CaptureError> {
        let mut compressor =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        compressor.write_all(bytes)?;
        let compressed = compressor.finish()?;
        // DEFLATE can expand incompressible data. Store that frame raw, retaining authenticated flag and length.
        let (flag, encoded) = if compressed.len() < bytes.len() {
            (1_u8, compressed.as_slice())
        } else {
            (0, bytes)
        };
        let mut head = Vec::with_capacity(13);
        head.extend_from_slice(&counter.to_le_bytes());
        head.extend_from_slice(
            &u32::try_from(bytes.len())
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        );
        head.push(flag);
        let cipher = self.encrypt(counter, &head, encoded)?;
        head.extend_from_slice(&cipher);
        Ok(head)
    }
    pub(crate) fn decode(
        &self,
        bytes: &[u8],
        index: u64,
        maximum: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        self.decode_counter(bytes, Self::counter(index, false)?, maximum)
    }
    pub(crate) fn decode_body(
        &self,
        bytes: &[u8],
        index: u64,
        maximum: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        self.decode_counter(bytes, Self::counter(index, true)?, maximum)
    }
    fn decode_counter(
        &self,
        bytes: &[u8],
        expected_counter: u64,
        maximum: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        if bytes.len() < 13 {
            return Err(CaptureError::AuthenticationFailed);
        }
        let counter = u64::from_le_bytes(
            bytes[..8]
                .try_into()
                .map_err(|_| CaptureError::InvalidMagic)?,
        );
        if counter != expected_counter {
            return Err(CaptureError::AuthenticationFailed);
        }
        let expanded = u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| CaptureError::InvalidMagic)?,
        ) as usize;
        if expanded > maximum {
            return Err(CaptureError::RecordTooLarge {
                actual: expanded,
                limit: maximum,
            });
        }
        let decoded = self.decrypt(counter, &bytes[..13], &bytes[13..])?;
        let mut output = Vec::new();
        match bytes[12] {
            0 => output = decoded,
            1 => {
                flate2::read::DeflateDecoder::new(decoded.as_slice())
                    .take(expanded as u64 + 1)
                    .read_to_end(&mut output)?;
            }
            _ => return Err(CaptureError::UnsupportedEncoding),
        }
        if output.len() != expanded {
            return Err(CaptureError::AuthenticationFailed);
        }
        Ok(output)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CaptureLimits, CaptureReader, CaptureRecord, CaptureRecordKind, CaptureWriter,
        read_indexed_frame_with_codec, recover_with_password,
    };
    use std::io::Cursor;
    fn record() -> CaptureRecord {
        CaptureRecord {
            exchange_id: 1,
            sequence: 1,
            kind: CaptureRecordKind::BodySegment {
                boundary: transmog_core::observe::ExchangeBoundary::ClientRequest,
                byte_count: 200_000,
                bytes: Some(vec![b'x'; 200_000]),
                truncated: false,
            },
        }
    }
    struct CountedCursor {
        input: Cursor<Vec<u8>>,
        read_bytes: usize,
    }
    impl Read for CountedCursor {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = self.input.read(output)?;
            self.read_bytes += count;
            Ok(count)
        }
    }
    impl std::io::Seek for CountedCursor {
        fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
            self.input.seek(position)
        }
    }
    #[test]
    fn indexed_open_seeks_over_payload_and_authenticates_it_only_on_read() {
        for password in [None, Some(CapturePassword::new("lazy-password".into()))] {
            let mut random = 0x1234_5678_u32;
            let body = (0..1_048_576)
                .map(|_| {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    random.to_le_bytes()[0]
                })
                .collect::<Vec<_>>();
            let expected = CaptureRecord {
                sequence: 1,
                exchange_id: 1,
                kind: CaptureRecordKind::BodySegment {
                    boundary: transmog_core::observe::ExchangeBoundary::ClientRequest,
                    byte_count: body.len(),
                    bytes: Some(body),
                    truncated: false,
                },
            };
            let mut writer = CaptureWriter::with_encoding(
                Vec::new(),
                CaptureLimits::default(),
                &CaptureEncoding {
                    password: password.clone(),
                },
            )
            .unwrap();
            writer.append(&expected).unwrap();
            writer.seal().unwrap();
            let encoded = writer.into_inner();
            let mut reader = CaptureReader::with_password(
                CountedCursor {
                    input: Cursor::new(encoded.clone()),
                    read_bytes: 0,
                },
                CaptureLimits::default(),
                password.as_ref(),
            )
            .unwrap();
            let frame = reader.read_next_indexed().unwrap().unwrap();
            assert!(matches!(
                frame.record.kind,
                CaptureRecordKind::BodySegment { bytes: None, .. }
            ));
            assert_eq!(frame.body.unwrap().bytes, 1_048_576);
            assert!(reader.read_next_indexed().unwrap().is_some());
            assert!(reader.read_next_indexed().unwrap().is_none());
            assert!(
                reader.input.read_bytes < 4096,
                "Indexing read body payload bytes"
            );
            let mut cursor = Cursor::new(encoded.clone());
            cursor.set_position(frame.offset);
            assert_eq!(
                read_indexed_frame_with_codec(
                    cursor,
                    frame.frame_bytes,
                    CaptureLimits::default(),
                    &frame.codec,
                    frame.index
                )
                .unwrap(),
                expected
            );
            let mut changed = encoded;
            // The last byte of the body pair belongs to the body payload, not its metadata.
            changed[usize::try_from(frame.offset + frame.frame_bytes - 1).unwrap()] ^= 1;
            let mut lazy = CaptureReader::with_password(
                Cursor::new(&changed),
                CaptureLimits::default(),
                password.as_ref(),
            )
            .unwrap();
            assert!(lazy.read_next_indexed().unwrap().is_some());
            let mut cursor = Cursor::new(changed);
            cursor.set_position(frame.offset);
            assert!(
                read_indexed_frame_with_codec(
                    cursor,
                    frame.frame_bytes,
                    CaptureLimits::default(),
                    &frame.codec,
                    frame.index
                )
                .is_err()
            );
        }
    }
    #[test]
    fn compressed_frames_are_small_and_randomly_readable() {
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding::default(),
        )
        .unwrap();
        writer.append(&record()).unwrap();
        writer.seal().unwrap();
        let bytes = writer.into_inner();
        assert!(bytes.len() < 5000);
        let mut reader = CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        let frame = reader.read_next().unwrap().unwrap();
        assert_eq!(frame.record, record());
        let decoded = read_indexed_frame_with_codec(
            Cursor::new(&bytes[usize::try_from(frame.offset).unwrap()..]),
            frame.frame_bytes,
            CaptureLimits::default(),
            &frame.codec,
            frame.index,
        )
        .unwrap();
        assert_eq!(decoded, record());
        assert!(reader.read_next().unwrap().is_some());
        assert!(reader.sealed());
    }
    #[test]
    fn passwords_tampering_reordering_and_interrupted_tail_are_checked() {
        let password = CapturePassword::new("private password".into());
        assert!(!format!("{password:?}").contains("private password"));
        let options = CaptureEncoding {
            password: Some(password.clone()),
        };
        let mut writer =
            CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options).unwrap();
        writer.append(&record()).unwrap();
        writer.seal().unwrap();
        let bytes = writer.into_inner();
        assert!(
            !bytes
                .windows(20)
                .any(|part| part == b"xxxxxxxxxxxxxxxxxxxx")
        );
        assert!(matches!(
            CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()),
            Err(CaptureError::PasswordRequired)
        ));
        assert!(matches!(
            CaptureReader::with_password(
                Cursor::new(&bytes),
                CaptureLimits::default(),
                Some(&CapturePassword::new("wrong".into()))
            ),
            Err(CaptureError::InvalidPassword)
        ));
        let mut reader = CaptureReader::with_password(
            Cursor::new(&bytes),
            CaptureLimits::default(),
            Some(&password),
        )
        .unwrap();
        let frame = reader.read_next().unwrap().unwrap();
        assert_eq!(frame.record, record());
        let indexed = &bytes[usize::try_from(frame.offset).unwrap()
            ..usize::try_from(frame.offset + frame.frame_bytes).unwrap()];
        assert!(matches!(
            read_indexed_frame_with_codec(
                Cursor::new(indexed),
                frame.frame_bytes,
                CaptureLimits::default(),
                &frame.codec,
                frame.index + 1
            ),
            Err(CaptureError::AuthenticationFailed)
        ));
        let mut changed = indexed.to_vec();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        let body_start = 8 + u32::from_le_bytes(changed[..4].try_into().unwrap()) as usize;
        let checksum = crc32fast::hash(&changed[body_start + 8..]);
        changed[body_start + 4..body_start + 8].copy_from_slice(&checksum.to_le_bytes());
        assert!(matches!(
            read_indexed_frame_with_codec(
                Cursor::new(changed),
                frame.frame_bytes,
                CaptureLimits::default(),
                &frame.codec,
                frame.index
            ),
            Err(CaptureError::AuthenticationFailed)
        ));
        let recovered = recover_with_password(
            Cursor::new(&bytes[..bytes.len() - 1]),
            CaptureLimits::default(),
            Some(&password),
        )
        .unwrap();
        assert!(recovered.truncated_tail);
        assert_eq!(recovered.records, vec![record()]);
    }
    #[test]
    fn expanded_frame_bound_is_enforced_before_allocation() {
        let mut encoded = Vec::new();
        let (codec, _) = FrameCodec::create(&mut encoded, &CaptureEncoding::default()).unwrap();
        let bytes = codec.encode(&vec![0; 1_000_000], 0).unwrap();
        assert!(matches!(
            codec.decode(&bytes, 0, 1000),
            Err(CaptureError::RecordTooLarge { .. })
        ));
    }
}

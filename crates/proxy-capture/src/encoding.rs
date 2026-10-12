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

pub(crate) const MAGIC: [u8; 9] = *b"TMCAP001\0";
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
    layout: String,
    block_size: u32,
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
    // Only the immutable connection definitions needed by this indexed frame.
    pub(crate) connections: Arc<crate::connections::Definitions>,
    pub(crate) record: Option<crate::container::RecordLocation>,
    inner: Arc<CodecInner>,
    start: u64,
}
struct CodecInner {
    header: Vec<u8>,
    key: Option<Zeroizing<[u8; 32]>>,
    prefix: [u8; 4],
    block_size: usize,
}
impl fmt::Debug for FrameCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameCodec")
            .field("encrypted", &self.inner.key.is_some())
            .finish_non_exhaustive()
    }
}
impl FrameCodec {
    pub(crate) fn block_plain(&self) -> usize {
        self.inner.block_size
    }
    pub(crate) fn block_bytes(&self) -> usize {
        self.inner.block_size + 16
    }
    pub(crate) fn payload(&self) -> usize {
        self.inner.block_size - 5
    }
    pub(crate) fn set_container_start(&mut self, start: u64) {
        self.start = start;
    }
    pub(crate) fn container_start(&self) -> u64 {
        self.start
    }
    pub(crate) fn encode_chunk(
        &self,
        chunk: &crate::compression::CompressedChunk,
        index: u64,
        body: bool,
    ) -> Result<Vec<u8>, CaptureError> {
        let counter = Self::counter(index, body)?;
        let mut head = Vec::with_capacity(13 + chunk.bytes.len());
        head.extend_from_slice(&counter.to_le_bytes());
        head.extend_from_slice(
            &u32::try_from(chunk.expanded)
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        );
        head.push(chunk.codec);
        let encrypted = self.encrypt(counter, &head, &chunk.bytes)?;
        head.extend_from_slice(&encrypted);
        Ok(head)
    }
    fn subkey(&self, salt: &[u8], domain: &[u8]) -> Result<Zeroizing<[u8; 32]>, CaptureError> {
        let mut result = Zeroizing::new([0; 32]);
        let key = self
            .inner
            .key
            .as_ref()
            .ok_or(CaptureError::AuthenticationFailed)?;
        hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), &**key)
            .expand(domain, &mut *result)
            .map_err(|_| CaptureError::AuthenticationFailed)?;
        Ok(result)
    }
    fn block_key(&self, ordinal: u64) -> Result<Zeroizing<[u8; 32]>, CaptureError> {
        use sha2::Digest;
        let salt = sha2::Sha256::digest(&self.inner.header);
        let mut domain = b"TMCap block epoch\0".to_vec();
        // At most one GiB of plaintext per derived AES-GCM key.
        domain.extend_from_slice(&(ordinal / 65_536).to_le_bytes());
        self.subkey(&salt, &domain)
    }
    pub(crate) fn encode_block(&self, ordinal: u64, plain: &[u8]) -> Result<Vec<u8>, CaptureError> {
        use sha2::Digest;
        let aad = self.aad(ordinal, b"TMCap block");
        if self.inner.key.is_some() {
            let key = self.block_key(ordinal)?;
            let cipher =
                Aes256Gcm::new_from_slice(&*key).map_err(|_| CaptureError::AuthenticationFailed)?;
            cipher
                .encrypt(
                    Nonce::from_slice(&self.nonce(ordinal % 65_536)),
                    Payload {
                        msg: plain,
                        aad: &aad,
                    },
                )
                .map_err(|_| CaptureError::AuthenticationFailed)
        } else {
            let mut hasher = sha2::Sha256::new();
            hasher.update(&aad);
            hasher.update(plain);
            let mut encoded = plain.to_vec();
            encoded.extend_from_slice(&hasher.finalize()[..16]);
            Ok(encoded)
        }
    }
    pub(crate) fn decode_block(
        &self,
        ordinal: u64,
        encoded: &[u8],
    ) -> Result<Vec<u8>, CaptureError> {
        use sha2::Digest;
        if encoded.len() != self.block_bytes() {
            return Err(CaptureError::InvalidMagic);
        }
        let aad = self.aad(ordinal, b"TMCap block");
        if self.inner.key.is_some() {
            let key = self.block_key(ordinal)?;
            let cipher =
                Aes256Gcm::new_from_slice(&*key).map_err(|_| CaptureError::AuthenticationFailed)?;
            cipher
                .decrypt(
                    Nonce::from_slice(&self.nonce(ordinal % 65_536)),
                    Payload {
                        msg: encoded,
                        aad: &aad,
                    },
                )
                .map_err(|_| CaptureError::AuthenticationFailed)
        } else {
            let plain = &encoded[..self.block_plain()];
            let mut hasher = sha2::Sha256::new();
            hasher.update(&aad);
            hasher.update(plain);
            if hasher.finalize()[..16] != encoded[self.block_plain()..] {
                return Err(CaptureError::AuthenticationFailed);
            }
            Ok(plain.to_vec())
        }
    }
    pub(crate) fn encode_locator(
        &self,
        generation: u64,
        block: u64,
        slot: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        use sha2::Digest;
        let mut salt = [0; 16];
        getrandom::fill(&mut salt)
            .map_err(|_| CaptureError::Crypto("System randomness is unavailable"))?;
        let mut plain = [0; 224];
        plain[..8].copy_from_slice(&generation.to_le_bytes());
        plain[8..16].copy_from_slice(&block.to_le_bytes());
        let aad = self.aad(slot as u64, b"TMCap checkpoint locator");
        let mut result = salt.to_vec();
        if self.inner.key.is_some() {
            let key = self.subkey(&salt, b"TMCap checkpoint locator key")?;
            let cipher =
                Aes256Gcm::new_from_slice(&*key).map_err(|_| CaptureError::AuthenticationFailed)?;
            result.extend(
                cipher
                    .encrypt(
                        Nonce::from_slice(&[0; 12]),
                        Payload {
                            msg: &plain,
                            aad: &aad,
                        },
                    )
                    .map_err(|_| CaptureError::AuthenticationFailed)?,
            );
        } else {
            let mut hash = sha2::Sha256::new();
            hash.update(&aad);
            hash.update(salt);
            hash.update(plain);
            result.extend_from_slice(&plain);
            result.extend_from_slice(&hash.finalize()[..16]);
        }
        Ok(result)
    }
    pub(crate) fn decode_locator(
        &self,
        encoded: &[u8],
        slot: usize,
    ) -> Result<(u64, u64), CaptureError> {
        use sha2::Digest;
        if encoded.len() != 256 {
            return Err(CaptureError::InvalidMagic);
        }
        let aad = self.aad(slot as u64, b"TMCap checkpoint locator");
        let plain = if self.inner.key.is_some() {
            let key = self.subkey(&encoded[..16], b"TMCap checkpoint locator key")?;
            let cipher =
                Aes256Gcm::new_from_slice(&*key).map_err(|_| CaptureError::AuthenticationFailed)?;
            cipher
                .decrypt(
                    Nonce::from_slice(&[0; 12]),
                    Payload {
                        msg: &encoded[16..],
                        aad: &aad,
                    },
                )
                .map_err(|_| CaptureError::AuthenticationFailed)?
        } else {
            let mut hash = sha2::Sha256::new();
            hash.update(&aad);
            hash.update(&encoded[..240]);
            if hash.finalize()[..16] != encoded[240..] {
                return Err(CaptureError::AuthenticationFailed);
            }
            encoded[16..240].to_vec()
        };
        let generation = u64::from_le_bytes(plain[..8].try_into().unwrap());
        let block = u64::from_le_bytes(plain[8..16].try_into().unwrap());
        if generation == 0 {
            return Err(CaptureError::InvalidMagic);
        }
        Ok((generation, block))
    }

    pub(crate) fn create(
        output: &mut impl Write,
        options: &CaptureEncoding,
    ) -> Result<(Self, u64), CaptureError> {
        Self::create_with_block_size(output, options, 16 * 1024)
    }
    pub(crate) fn create_with_block_size(
        output: &mut impl Write,
        options: &CaptureEncoding,
        block_size: u32,
    ) -> Result<(Self, u64), CaptureError> {
        let mut salt = [0; 16];
        let mut prefix = [0; 4];
        getrandom::fill(&mut salt)
            .map_err(|_| CaptureError::Crypto("System randomness is unavailable"))?;
        getrandom::fill(&mut prefix)
            .map_err(|_| CaptureError::Crypto("System randomness is unavailable"))?;
        let encrypted = options.password.is_some();
        let header = Header {
            version: 1,
            layout: "indexed-blocks".into(),
            block_size: if encrypted { 0 } else { block_size },
            compression: "zstd".into(),
            cipher: if encrypted { "aes-256-gcm" } else { "none" }.into(),
            kdf: if encrypted { "argon2id" } else { "none" }.into(),
            salt,
            nonce_prefix: prefix,
            memory_kib: if encrypted { 65536 } else { 0 },
            iterations: if encrypted { 3 } else { 0 },
            lanes: u32::from(encrypted),
        };
        let encoded = serde_json::to_vec(&header)?;
        let mut codec = Self::from_header(encoded.clone(), &header, options.password.as_ref())?;
        Arc::get_mut(&mut codec.inner).unwrap().block_size = block_size as usize;
        let proof = codec.proof()?;
        output.write_all(&MAGIC)?;
        output.write_all(
            &u32::try_from(encoded.len())
                .map_err(|_| CaptureError::InvalidMagic)?
                .to_le_bytes(),
        )?;
        output.write_all(&encoded)?;
        output.write_all(&proof)?;
        Ok((
            codec,
            MAGIC.len() as u64 + 4 + encoded.len() as u64 + proof.len() as u64,
        ))
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
        let mut codec = Self::from_header(encoded, &header, password)?;
        let mut proof = vec![0; if codec.inner.key.is_some() { 20 } else { 4 }];
        input.read_exact(&mut proof)?;
        let geometry = codec
            .decrypt(0, &[], &proof)
            .map_err(|_| CaptureError::InvalidPassword)?;
        let block_size = u32::from_le_bytes(
            geometry
                .as_slice()
                .try_into()
                .map_err(|_| CaptureError::InvalidMagic)?,
        ) as usize;
        if ![1024, 16 * 1024].contains(&block_size)
            || (codec.inner.key.is_none() && block_size != header.block_size as usize)
        {
            return Err(CaptureError::UnsupportedEncoding);
        }
        Arc::get_mut(&mut codec.inner).unwrap().block_size = block_size;
        Ok((
            codec,
            MAGIC.len() as u64 + 4 + length as u64 + proof.len() as u64,
        ))
    }
    fn from_header(
        encoded: Vec<u8>,
        header: &Header,
        password: Option<&CapturePassword>,
    ) -> Result<Self, CaptureError> {
        if header.version != 1 || header.compression != "zstd" || header.layout != "indexed-blocks"
        {
            return Err(CaptureError::UnsupportedEncoding);
        }
        let key = match (header.cipher.as_str(), header.kdf.as_str()) {
            ("none", "none")
                if header.memory_kib == 0
                    && header.iterations == 0
                    && header.lanes == 0
                    && [1024, 16 * 1024].contains(&header.block_size) =>
            {
                None
            }
            ("aes-256-gcm", "argon2id")
                if header.memory_kib == 65536
                    && header.iterations == 3
                    && header.lanes == 1
                    && header.block_size == 0 =>
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
            start: 0,
            connections: Arc::default(),
            record: None,
            inner: Arc::new(CodecInner {
                header: encoded,
                key,
                prefix: header.nonce_prefix,
                block_size: if header.block_size == 0 {
                    16 * 1024
                } else {
                    header.block_size as usize
                },
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
        self.encrypt(
            0,
            &[],
            &u32::try_from(self.block_plain())
                .map_err(|_| CaptureError::QuotaExceeded)?
                .to_le_bytes(),
        )
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
    #[cfg(test)]
    pub(crate) fn encode(&self, bytes: &[u8], index: u64) -> Result<Vec<u8>, CaptureError> {
        self.encode_counter(bytes, Self::counter(index, false)?)
    }
    #[cfg(test)]
    fn encode_counter(&self, bytes: &[u8], counter: u64) -> Result<Vec<u8>, CaptureError> {
        let compressed = zstd::bulk::compress(bytes, 3)?;
        // Store incompressible frames raw, retaining the authenticated flag and length.
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
    pub(crate) fn decode_compressed(
        &self,
        bytes: &[u8],
        index: u64,
        body: bool,
        maximum: usize,
    ) -> Result<crate::compression::CompressedChunk, CaptureError> {
        self.decode_compressed_counter(bytes, Self::counter(index, body)?, maximum)
    }
    fn decode_compressed_counter(
        &self,
        bytes: &[u8],
        expected_counter: u64,
        maximum: usize,
    ) -> Result<crate::compression::CompressedChunk, CaptureError> {
        if bytes.len() < 13 {
            return Err(CaptureError::AuthenticationFailed);
        }
        let counter = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        if counter != expected_counter {
            return Err(CaptureError::AuthenticationFailed);
        }
        let expanded = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        if expanded > maximum {
            return Err(CaptureError::RecordTooLarge {
                actual: expanded,
                limit: maximum,
            });
        }
        let decoded = self.decrypt(counter, &bytes[..13], &bytes[13..])?;
        if bytes[12] > 1 {
            return Err(CaptureError::UnsupportedEncoding);
        }
        if bytes[12] == 0 && decoded.len() != expanded {
            return Err(CaptureError::AuthenticationFailed);
        }
        Ok(crate::compression::CompressedChunk {
            codec: bytes[12],
            expanded,
            bytes: decoded,
        })
    }
    fn decode_counter(
        &self,
        bytes: &[u8],
        expected_counter: u64,
        maximum: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        let chunk = self.decode_compressed_counter(bytes, expected_counter, maximum)?;
        let output = if chunk.codec == 0 {
            chunk.bytes
        } else {
            zstd::bulk::decompress(&chunk.bytes, chunk.expanded)?
        };
        if output.len() != chunk.expanded {
            return Err(CaptureError::AuthenticationFailed);
        }
        Ok(output)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CaptureLimits, CaptureRecord, CaptureRecordKind, ScratchReader, ScratchWriter,
        read_scratch_frame_with_codec,
    };
    use std::io::Cursor;
    #[test]
    fn bootstrap_tags_bind_geometry_and_exact_public_header() {
        let options = CaptureEncoding {
            password: Some(CapturePassword::new("geometry review".into())),
        };
        let mut bytes = Vec::new();
        let (codec, _) = FrameCodec::create(&mut bytes, &options).unwrap();
        let proof = codec.proof().unwrap();
        assert_eq!(
            codec.decrypt(0, &[], &proof).unwrap(),
            (16 * 1024_u32).to_le_bytes()
        );
        for position in 0..proof.len() {
            let mut changed = proof.clone();
            changed[position] ^= 1;
            assert!(codec.decrypt(0, &[], &changed).is_err());
        }
        let mut header = codec.inner.header.clone();
        header.push(b' '); // Equivalent JSON is still different authenticated bytes.
        let changed = FrameCodec {
            start: codec.start,
            connections: codec.connections.clone(),
            record: None,
            inner: Arc::new(CodecInner {
                header,
                key: codec.inner.key.clone(),
                prefix: codec.inner.prefix,
                block_size: codec.block_plain(),
            }),
        };
        assert!(changed.decrypt(0, &[], &proof).is_err());
    }

    #[test]
    fn kdf_parameters_are_rejected_before_password_work() {
        let mut bytes = Vec::new();
        FrameCodec::create(&mut bytes, &CaptureEncoding::default()).unwrap();
        let length = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
        let mut value: serde_json::Value = serde_json::from_slice(&bytes[13..13 + length]).unwrap();
        value["cipher"] = "aes-256-gcm".into();
        value["kdf"] = "argon2id".into();
        value["block_size"] = 0.into();
        value["memory_kib"] = 65536.into();
        value["iterations"] = 3.into();
        value["lanes"] = 1.into();
        for (field, number) in [
            ("memory_kib", u32::MAX),
            ("memory_kib", 0),
            ("iterations", u32::MAX),
            ("iterations", 0),
            ("lanes", u32::MAX),
            ("lanes", 0),
        ] {
            let mut changed = value.clone();
            changed[field] = number.into();
            let encoded = serde_json::to_vec(&changed).unwrap();
            let header: Header = serde_json::from_slice(&encoded).unwrap();
            assert!(matches!(
                FrameCodec::from_header(encoded, &header, None),
                Err(CaptureError::UnsupportedEncoding)
            ));
        }
    }

    #[test]
    fn block_ciphertext_matches_boringssl_across_epoch_boundary() {
        let mut bytes = Vec::new();
        let options = CaptureEncoding {
            password: Some(CapturePassword::new("independent cipher check".into())),
        };
        let (codec, _) = FrameCodec::create(&mut bytes, &options).unwrap();
        let plain = vec![42; 16 * 1024];
        for ordinal in [0_u64, 65_535, 65_536] {
            let key = codec.block_key(ordinal).unwrap();
            let mut tag = [0; 16];
            let mut reference = boring::symm::encrypt_aead(
                boring::symm::Cipher::aes_256_gcm(),
                &*key,
                Some(&codec.nonce(ordinal % 65_536)),
                &codec.aad(ordinal, b"TMCap block"),
                &plain,
                &mut tag,
            )
            .unwrap();
            reference.extend_from_slice(&tag);
            assert_eq!(codec.encode_block(ordinal, &plain).unwrap(), reference);
            assert_eq!(codec.decode_block(ordinal, &reference).unwrap(), plain);
        }
        assert_eq!(
            *codec.block_key(0).unwrap(),
            *codec.block_key(65_535).unwrap()
        );
        assert_ne!(
            *codec.block_key(0).unwrap(),
            *codec.block_key(65_536).unwrap()
        );
        for slot in 0..2 {
            let locator = codec.encode_locator(1, 3, slot).unwrap();
            for position in [0, 15, 16, 128, 239, 240, 255] {
                let mut changed = locator.clone();
                changed[position] ^= 1;
                assert!(codec.decode_locator(&changed, slot).is_err());
            }
        }
    }

    fn record() -> CaptureRecord {
        CaptureRecord {
            exchange_id: 1,
            sequence: 1,
            kind: CaptureRecordKind::BodySegment {
                boundary: transmog_core::observe::ExchangeBoundary::ClientRequest,
                byte_count: 100_000,
                bytes: Some(vec![b'x'; 100_000]),
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
            let body = (0..131_072)
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
            let mut writer = ScratchWriter::with_encoding(
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
            let mut reader = ScratchReader::with_password(
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
            assert_eq!(frame.body.unwrap().bytes, 131_072);
            assert!(reader.read_next_indexed().unwrap().is_some());
            assert!(reader.read_next_indexed().unwrap().is_none());
            assert!(
                reader.input.read_bytes < 4096,
                "Indexing read body payload bytes"
            );
            let mut cursor = Cursor::new(encoded.clone());
            cursor.set_position(frame.offset);
            assert_eq!(
                read_scratch_frame_with_codec(
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
            let mut lazy = ScratchReader::with_password(
                Cursor::new(&changed),
                CaptureLimits::default(),
                password.as_ref(),
            )
            .unwrap();
            assert!(lazy.read_next_indexed().unwrap().is_some());
            let mut cursor = Cursor::new(changed);
            cursor.set_position(frame.offset);
            assert!(
                read_scratch_frame_with_codec(
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
        let mut writer = ScratchWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding::default(),
        )
        .unwrap();
        writer.append(&record()).unwrap();
        writer.seal().unwrap();
        let bytes = writer.into_inner();
        assert!(bytes.len() < 5000);
        assert_eq!(&bytes[..9], b"TMCAP001\0");
        let header_bytes = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
        let header: serde_json::Value =
            serde_json::from_slice(&bytes[13..13 + header_bytes]).unwrap();
        assert_eq!(header["version"], 1);
        assert_eq!(header["compression"], "zstd");
        let mut reader = ScratchReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        assert_eq!(reader.valid_bytes(), (17 + header_bytes) as u64);
        let frame = reader.read_next().unwrap().unwrap();
        assert_eq!(frame.offset, (17 + header_bytes) as u64);
        assert_eq!(frame.record, record());
        let decoded = read_scratch_frame_with_codec(
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
            ScratchWriter::with_encoding(Vec::new(), CaptureLimits::default(), &options).unwrap();
        writer.append(&record()).unwrap();
        writer.seal().unwrap();
        let bytes = writer.into_inner();
        assert!(
            !bytes
                .windows(20)
                .any(|part| part == b"xxxxxxxxxxxxxxxxxxxx")
        );
        assert!(matches!(
            ScratchReader::new(Cursor::new(&bytes), CaptureLimits::default()),
            Err(CaptureError::PasswordRequired)
        ));
        assert!(matches!(
            ScratchReader::with_password(
                Cursor::new(&bytes),
                CaptureLimits::default(),
                Some(&CapturePassword::new("wrong".into()))
            ),
            Err(CaptureError::InvalidPassword)
        ));
        let mut reader = ScratchReader::with_password(
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
            read_scratch_frame_with_codec(
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
            read_scratch_frame_with_codec(
                Cursor::new(changed),
                frame.frame_bytes,
                CaptureLimits::default(),
                &frame.codec,
                frame.index
            ),
            Err(CaptureError::AuthenticationFailed)
        ));
        let mut recovered = ScratchReader::with_password(
            Cursor::new(&bytes[..bytes.len() - 1]),
            CaptureLimits::default(),
            Some(&password),
        )
        .unwrap();
        assert_eq!(recovered.read_next().unwrap().unwrap().record, record());
        assert!(recovered.read_next().unwrap().is_none());
        assert!(recovered.truncated_tail());
    }
    #[test]
    fn zstd_frames_validate_expanded_length_and_truncation() {
        let mut header = Vec::new();
        let (codec, _) = FrameCodec::create(&mut header, &CaptureEncoding::default()).unwrap();
        let original = vec![b'x'; 65_536];
        let encoded = codec.encode(&original, 0).unwrap();
        assert_eq!(encoded[12], 1);
        assert_eq!(&encoded[13..17], &[0x28, 0xb5, 0x2f, 0xfd]);
        assert_eq!(codec.decode(&encoded, 0, original.len()).unwrap(), original);

        let mut understated = encoded.clone();
        understated[8..12].copy_from_slice(&1024_u32.to_le_bytes());
        assert!(codec.decode(&understated, 0, original.len()).is_err());

        let mut overstated = encoded.clone();
        overstated[8..12].copy_from_slice(&65_537_u32.to_le_bytes());
        assert!(matches!(
            codec.decode(&overstated, 0, 65_537),
            Err(CaptureError::AuthenticationFailed)
        ));
        assert!(
            codec
                .decode(&encoded[..encoded.len() - 1], 0, original.len())
                .is_err()
        );

        let raw = codec.encode(b"short payload", 0).unwrap();
        assert_eq!(raw[12], 0);
        assert_eq!(codec.decode(&raw, 0, 100).unwrap(), b"short payload");
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

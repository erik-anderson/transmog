use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use thiserror::Error;

use crate::{Direction, PerMessageDeflate};

const SYNC_FLUSH_TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// Stateful raw-DEFLATE codec for one negotiated sender direction.
///
/// Separate instances must be used for client-to-server and server-to-client
/// traffic. Context takeover is retained or reset exactly as negotiated.
#[derive(Debug)]
pub struct PerMessageDeflateCodec {
    compressor: Compress,
    decompressor: Decompress,
    no_context_takeover: bool,
}

impl PerMessageDeflateCodec {
    /// Creates a directional codec.
    ///
    /// # Errors
    ///
    /// The current safe encoder supports the RFC default 15-bit window. A
    /// smaller explicitly negotiated window fails closed instead of emitting a
    /// non-conforming replacement message.
    pub fn new(
        negotiated: PerMessageDeflate,
        direction: Direction,
    ) -> Result<Self, CompressionError> {
        if negotiated
            .window_bits(direction)
            .is_some_and(|bits| bits != 15)
        {
            return Err(CompressionError::UnsupportedWindowBits);
        }
        Ok(Self {
            compressor: Compress::new(Compression::default(), false),
            decompressor: Decompress::new(false),
            no_context_takeover: negotiated.no_context_takeover(direction),
        })
    }

    /// Inflates one message with a strict decompressed-byte ceiling.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid raw DEFLATE data, stalled codec state, or
    /// output that would exceed `limit`.
    pub fn decompress(
        &mut self,
        compressed: &[u8],
        limit: usize,
    ) -> Result<Vec<u8>, CompressionError> {
        if limit == 0 {
            return Err(CompressionError::InvalidLimit);
        }
        let mut input = Vec::with_capacity(compressed.len().saturating_add(4));
        input.extend_from_slice(compressed);
        input.extend_from_slice(&SYNC_FLUSH_TAIL);
        let mut offset = 0_usize;
        let mut output = Vec::new();
        loop {
            if output.capacity() == output.len() {
                let remaining = limit.saturating_add(1).saturating_sub(output.len());
                if remaining == 0 {
                    return Err(CompressionError::DecompressedLimitExceeded);
                }
                output.reserve(remaining.min(8 * 1024));
            }
            let before_input = self.decompressor.total_in();
            let before_output = self.decompressor.total_out();
            let status = self
                .decompressor
                .decompress_vec(&input[offset..], &mut output, FlushDecompress::Sync)
                .map_err(|_| CompressionError::InvalidCompressedData)?;
            let consumed = usize::try_from(self.decompressor.total_in() - before_input)
                .map_err(|_| CompressionError::InvalidCompressedData)?;
            let produced = usize::try_from(self.decompressor.total_out() - before_output)
                .map_err(|_| CompressionError::InvalidCompressedData)?;
            offset = offset
                .checked_add(consumed)
                .ok_or(CompressionError::InvalidCompressedData)?;
            if output.len() > limit {
                return Err(CompressionError::DecompressedLimitExceeded);
            }
            if offset == input.len() && produced == 0 {
                break;
            }
            if consumed == 0 && produced == 0 {
                return Err(CompressionError::CodecStalled);
            }
            if status == Status::StreamEnd {
                break;
            }
        }
        if offset != input.len() {
            return Err(CompressionError::InvalidCompressedData);
        }
        if self.no_context_takeover {
            self.decompressor.reset(false);
        }
        Ok(output)
    }

    /// Deflates one message using RFC 7692 sync-flush framing.
    ///
    /// # Errors
    ///
    /// Returns an error if the codec stalls or does not emit the required
    /// four-byte sync-flush suffix.
    pub fn compress(&mut self, plain: &[u8]) -> Result<Vec<u8>, CompressionError> {
        let output_limit = plain
            .len()
            .checked_mul(2)
            .and_then(|value| value.checked_add(1024))
            .ok_or(CompressionError::CompressedLimitExceeded)?;
        let mut output = Vec::with_capacity(plain.len().saturating_add(64).min(output_limit));
        let mut offset = 0_usize;
        loop {
            if output.capacity() == output.len() {
                let remaining = output_limit.saturating_sub(output.len());
                if remaining == 0 {
                    return Err(CompressionError::CompressedLimitExceeded);
                }
                output.reserve(remaining.min(8 * 1024));
            }
            let before_input = self.compressor.total_in();
            let before_output = self.compressor.total_out();
            let status = self
                .compressor
                .compress_vec(&plain[offset..], &mut output, FlushCompress::Sync)
                .map_err(|_| CompressionError::CompressionFailed)?;
            let consumed = usize::try_from(self.compressor.total_in() - before_input)
                .map_err(|_| CompressionError::CompressionFailed)?;
            let produced = usize::try_from(self.compressor.total_out() - before_output)
                .map_err(|_| CompressionError::CompressionFailed)?;
            offset = offset
                .checked_add(consumed)
                .ok_or(CompressionError::CompressionFailed)?;
            if output.len() > output_limit {
                return Err(CompressionError::CompressedLimitExceeded);
            }
            if offset == plain.len() && output.ends_with(&SYNC_FLUSH_TAIL) {
                break;
            }
            if consumed == 0 && produced == 0 && status == Status::BufError {
                return Err(CompressionError::CodecStalled);
            }
        }
        output.truncate(output.len() - SYNC_FLUSH_TAIL.len());
        if self.no_context_takeover {
            self.compressor.reset();
        }
        Ok(output)
    }
}

/// `permessage-deflate` processing failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CompressionError {
    /// Explicitly negotiated window is not supported for safe re-encoding.
    #[error("negotiated permessage-deflate window is unsupported")]
    UnsupportedWindowBits,
    /// Decompressed-byte limit was zero.
    #[error("invalid decompressed message limit")]
    InvalidLimit,
    /// Raw DEFLATE stream was malformed.
    #[error("invalid permessage-deflate payload")]
    InvalidCompressedData,
    /// Inflated output exceeded the configured ceiling.
    #[error("permessage-deflate output limit exceeded")]
    DecompressedLimitExceeded,
    /// Compressed output exceeded a defensive encoder ceiling.
    #[error("permessage-deflate compressed output limit exceeded")]
    CompressedLimitExceeded,
    /// Compressor rejected the message.
    #[error("permessage-deflate compression failed")]
    CompressionFailed,
    /// Codec made no progress despite remaining work.
    #[error("permessage-deflate codec stalled")]
    CodecStalled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_round_trip_and_no_context_reset() {
        let negotiated = PerMessageDeflate {
            client_no_context_takeover: true,
            ..PerMessageDeflate::default()
        };
        let mut encoder =
            PerMessageDeflateCodec::new(negotiated, Direction::ClientToServer).unwrap();
        let mut decoder =
            PerMessageDeflateCodec::new(negotiated, Direction::ClientToServer).unwrap();
        for payload in [b"hello hello hello".as_slice(), b"second message"] {
            let compressed = encoder.compress(payload).unwrap();
            assert_eq!(decoder.decompress(&compressed, 1024).unwrap(), payload);
        }
    }

    #[test]
    fn context_takeover_round_trips_a_message_sequence() {
        let negotiated = PerMessageDeflate::default();
        let mut encoder =
            PerMessageDeflateCodec::new(negotiated, Direction::ServerToClient).unwrap();
        let mut decoder =
            PerMessageDeflateCodec::new(negotiated, Direction::ServerToClient).unwrap();
        for payload in [
            b"shared prefix and shared suffix".as_slice(),
            b"shared prefix and another shared suffix",
            b"shared prefix and shared suffix",
        ] {
            let compressed = encoder.compress(payload).unwrap();
            assert_eq!(decoder.decompress(&compressed, 1024).unwrap(), payload);
        }
    }

    #[test]
    fn decompression_bomb_is_bounded() {
        let negotiated = PerMessageDeflate::default();
        let mut encoder =
            PerMessageDeflateCodec::new(negotiated, Direction::ServerToClient).unwrap();
        let compressed = encoder.compress(&vec![b'a'; 64 * 1024]).unwrap();
        let mut decoder =
            PerMessageDeflateCodec::new(negotiated, Direction::ServerToClient).unwrap();
        assert_eq!(
            decoder.decompress(&compressed, 1024),
            Err(CompressionError::DecompressedLimitExceeded)
        );
    }

    #[test]
    fn unsupported_encoder_window_fails_closed() {
        assert_eq!(
            PerMessageDeflateCodec::new(
                PerMessageDeflate {
                    server_max_window_bits: Some(12),
                    ..PerMessageDeflate::default()
                },
                Direction::ServerToClient,
            )
            .unwrap_err(),
            CompressionError::UnsupportedWindowBits
        );
    }
}

//! Client-only RFC 8879 Brotli certificate decompression.
use boring::ssl::{CertificateCompressionAlgorithm, CertificateCompressor};
use std::io::{self, Write};

pub(crate) struct BrotliCertificateDecoder;

impl CertificateCompressor for BrotliCertificateDecoder {
    const ALGORITHM: CertificateCompressionAlgorithm = CertificateCompressionAlgorithm::BROTLI;
    const CAN_COMPRESS: bool = false;
    const CAN_DECOMPRESS: bool = true;

    fn decompress<W: Write>(&self, input: &[u8], output: &mut W) -> io::Result<()> {
        // BoringSSL limits the declared certificate size before invoking us.
        // Its writer is a fixed buffer of exactly that size: write_all rejects
        // expansion beyond it, and the callback rejects a short result.
        let mut state = brotli::BrotliState::new(
            brotli::HeapAlloc::<u8>::new(0),
            brotli::HeapAlloc::<u32>::new(0),
            brotli::HeapAlloc::<brotli::HuffmanCode>::new(brotli::HuffmanCode {
                bits: 2,
                value: 1,
            }),
        );
        let mut remaining = input.len();
        let mut input_offset = 0;
        let mut total_output = 0;
        let mut buffer = [0; 4096];
        loop {
            let mut available_output = buffer.len();
            let mut output_offset = 0;
            let result = brotli::BrotliDecompressStream(
                &mut remaining,
                &mut input_offset,
                input,
                &mut available_output,
                &mut output_offset,
                &mut buffer,
                &mut total_output,
                &mut state,
            );
            output.write_all(&buffer[..output_offset])?;
            match result {
                brotli::BrotliResult::NeedsMoreOutput => {}
                brotli::BrotliResult::ResultSuccess if remaining == 0 => return Ok(()),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid compressed TLS certificate",
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compressed(input: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        brotli::BrotliCompress(
            &mut io::Cursor::new(input),
            &mut result,
            &brotli::enc::BrotliEncoderParams::default(),
        )
        .unwrap();
        result
    }

    #[test]
    fn streaming_decoder_respects_the_native_output_bound() {
        let input = vec![42; 20000];
        let encoded = compressed(&input);
        let mut output = vec![0; input.len()];
        BrotliCertificateDecoder
            .decompress(&encoded, &mut io::Cursor::new(output.as_mut_slice()))
            .unwrap();
        assert_eq!(output, input);
        let mut short = [0; 32];
        assert!(
            BrotliCertificateDecoder
                .decompress(&encoded, &mut io::Cursor::new(short.as_mut_slice()))
                .is_err()
        );
    }

    #[test]
    fn decoder_rejects_malformed_truncated_and_trailing_input() {
        let encoded = compressed(b"a TLS certificate message");
        for length in 0..encoded.len() {
            assert!(
                BrotliCertificateDecoder
                    .decompress(&encoded[..length], &mut Vec::new())
                    .is_err()
            );
        }
        assert!(
            BrotliCertificateDecoder
                .decompress(&[0xff; 16], &mut Vec::new())
                .is_err()
        );
        let mut trailing = encoded;
        trailing.push(0);
        assert!(
            BrotliCertificateDecoder
                .decompress(&trailing, &mut Vec::new())
                .is_err()
        );
    }
}

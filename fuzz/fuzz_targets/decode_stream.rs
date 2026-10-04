#![no_main]

use std::num::NonZeroUsize;

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use rustymiddle_content::{ContentCoding, ContentDecoder, ContentLimits};
use rustymiddle_core::BodyFrame;

const CODINGS: [ContentCoding; 4] = [
    ContentCoding::Gzip,
    ContentCoding::Brotli,
    ContentCoding::Deflate,
    ContentCoding::Zstd,
];
const MAX_ENCODED: usize = 4 * 1024;
const MAX_DECODED: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let Some((&coding_selector, rest)) = data.split_first() else {
        return;
    };
    let Some((&chunk_selector, payload)) = rest.split_first() else {
        return;
    };
    let coding = CODINGS[usize::from(coding_selector) % CODINGS.len()];
    let chunk_size = usize::from(chunk_selector % 64).saturating_add(1);
    let payload = decode_payload(payload);
    let payload = &payload[..payload.len().min(MAX_ENCODED)];
    let limits = ContentLimits::new(
        NonZeroUsize::new(MAX_ENCODED).expect("encoded limit"),
        NonZeroUsize::new(MAX_DECODED).expect("decoded limit"),
        NonZeroUsize::new(MAX_DECODED).expect("output limit"),
        NonZeroUsize::new(16 * 1024 * 1024).expect("window limit"),
        NonZeroUsize::new(64).expect("ratio limit"),
        1024,
        NonZeroUsize::new(CODINGS.len()).expect("coding depth"),
    );
    let mut decoder = ContentDecoder::new(coding, limits).expect("valid decoder configuration");
    let mut released = 0_usize;

    for chunk in payload.chunks(chunk_size) {
        let Ok(frames) = decoder.on_frame(BodyFrame::Data(Bytes::copy_from_slice(chunk))) else {
            return;
        };
        released = count_output(released, frames);
    }
    if let Ok(frames) = decoder.finish() {
        count_output(released, frames);
    }
});

fn count_output(mut released: usize, frames: Vec<BodyFrame>) -> usize {
    for frame in frames {
        if let BodyFrame::Data(bytes) = frame {
            released = released
                .checked_add(bytes.len())
                .expect("bounded decoded count cannot overflow");
            assert!(released <= MAX_DECODED);
        }
    }
    released
}

fn decode_payload(payload: &[u8]) -> Vec<u8> {
    let Some(hex) = payload.strip_prefix(b"hex:") else {
        return payload[..payload.len().min(MAX_ENCODED)].to_vec();
    };
    hex.chunks_exact(2)
        .take(MAX_ENCODED)
        .map_while(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Some(high << 4 | low)
        })
        .collect()
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

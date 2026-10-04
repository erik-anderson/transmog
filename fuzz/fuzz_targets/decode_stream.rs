#![no_main]

use std::{num::NonZeroUsize, sync::OnceLock, time::Duration};

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use rustymiddle_content::{
    ContentCodecError, ContentCoding, ContentDecoder, ContentDecoderOptions, ContentLimits,
    ContentWorkLimits, DeflateCompatibility,
};
use rustymiddle_core::{BodyFrame, HeaderBlock, HeaderField};
use tokio::runtime::{Builder, Runtime};

const CODINGS: [ContentCoding; 4] = [
    ContentCoding::Gzip,
    ContentCoding::Brotli,
    ContentCoding::Deflate,
    ContentCoding::Zstd,
];
const MAX_ENCODED: usize = 4 * 1024;
const MAX_DECODED: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| runtime().block_on(fuzz(data)));

async fn fuzz(data: &[u8]) {
    let Some((&coding_selector, rest)) = data.split_first() else {
        return;
    };
    let Some((&chunk_selector, payload)) = rest.split_first() else {
        return;
    };
    let Some((&flags, payload)) = payload.split_first() else {
        return;
    };
    let Some((&limit_selector, payload)) = payload.split_first() else {
        return;
    };
    let flags = selector(flags);
    let coding = CODINGS[usize::from(selector(coding_selector)) % CODINGS.len()];
    let chunk_size = usize::from(selector(chunk_selector) % 64).saturating_add(1);
    let payload = decode_payload(payload);
    let payload = &payload[..payload.len().min(MAX_ENCODED)];
    let limits = limits(selector(limit_selector));
    let options = ContentDecoderOptions::new().with_deflate_compatibility(if flags & 1 == 0 {
        DeflateCompatibility::StrictZlib
    } else {
        DeflateCompatibility::AllowRaw
    });
    let mut decoder = ContentDecoder::with_options(coding, limits, options)
        .expect("all fuzzed decoder configurations are valid");
    let mut released = 0_usize;
    let mut consumed = 0_usize;
    let split = payload.len().div_ceil(2);
    let trailers = || {
        BodyFrame::Trailers(HeaderBlock::from_fields(vec![
            HeaderField::try_new("x-fuzz-trailer", "present").expect("static trailer is valid"),
        ]))
    };

    for chunk in payload.chunks(chunk_size) {
        if flags & 2 != 0 && consumed >= split {
            if !accept(
                &mut decoder,
                trailers(),
                limits.max_decoded_bytes().get(),
                &mut released,
            )
            .await
            {
                return;
            }
        }
        if flags & 8 != 0 && consumed >= split {
            let _ = decoder.finish().await;
            assert_terminal(&mut decoder).await;
            return;
        }
        if !accept(
            &mut decoder,
            BodyFrame::Data(Bytes::copy_from_slice(chunk)),
            limits.max_decoded_bytes().get(),
            &mut released,
        )
        .await
        {
            return;
        }
        consumed = consumed.saturating_add(chunk.len());
    }

    if flags & 16 != 0
        && !accept(
            &mut decoder,
            trailers(),
            limits.max_decoded_bytes().get(),
            &mut released,
        )
        .await
    {
        return;
    }
    if flags & 4 != 0 {
        if !accept(
            &mut decoder,
            trailers(),
            limits.max_decoded_bytes().get(),
            &mut released,
        )
        .await
        {
            return;
        }
        assert!(
            !accept(
                &mut decoder,
                trailers(),
                limits.max_decoded_bytes().get(),
                &mut released,
            )
            .await
        );
        return;
    }

    if let Ok(frames) = decoder.finish().await {
        count_output(released, frames, limits.max_decoded_bytes().get());
    }
    assert_terminal(&mut decoder).await;
}

async fn accept(
    decoder: &mut ContentDecoder,
    frame: BodyFrame,
    limit: usize,
    released: &mut usize,
) -> bool {
    match decoder.on_frame(frame).await {
        Ok(frames) => {
            *released = count_output(*released, frames, limit);
            true
        }
        Err(_) => {
            assert_terminal(decoder).await;
            false
        }
    }
}

async fn assert_terminal(decoder: &mut ContentDecoder) {
    assert!(matches!(
        decoder.finish().await,
        Err(ContentCodecError::AlreadyFinished { .. })
    ));
    assert!(matches!(
        decoder.on_frame(BodyFrame::Data(Bytes::new())).await,
        Err(ContentCodecError::AlreadyFinished { .. })
    ));
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("fuzz runtime must initialize")
    })
}

fn count_output(mut released: usize, frames: Vec<BodyFrame>, limit: usize) -> usize {
    let mut trailers = 0_usize;
    for frame in frames {
        match frame {
            BodyFrame::Data(bytes) => {
                released = released
                    .checked_add(bytes.len())
                    .expect("bounded decoded count cannot overflow");
                assert!(released <= limit);
            }
            BodyFrame::Trailers(_) => trailers = trailers.saturating_add(1),
        }
    }
    assert!(trailers <= 1);
    released
}

fn limits(selector: u8) -> ContentLimits {
    let (encoded, decoded, window, ratio, slack) = match selector % 4 {
        0 => (MAX_ENCODED, MAX_DECODED, 16 * 1024 * 1024, 64, 1_024),
        1 => (64, 512, 1_024, 2, 32),
        2 => (MAX_ENCODED, 1_024, 4_096, 4, 0),
        _ => (8, 16, 1_024, 1, 0),
    };
    ContentLimits::new(
        NonZeroUsize::new(encoded).expect("encoded limit"),
        NonZeroUsize::new(decoded).expect("decoded limit"),
        NonZeroUsize::new(decoded).expect("output limit"),
        NonZeroUsize::new(window).expect("window limit"),
        NonZeroUsize::new(ratio).expect("ratio limit"),
        slack,
        NonZeroUsize::new(CODINGS.len()).expect("coding depth"),
    )
    .with_work_limits(
        ContentWorkLimits::new(
            NonZeroUsize::new([64, 256, 1024, 4096][usize::from(selector % 4)])
                .expect("work quantum"),
            Duration::from_secs(10),
            Duration::from_secs(30),
        )
        .expect("valid work limits"),
    )
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

const fn selector(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => value,
    }
}

//! Fixed-input content-processing microbenchmark.
//!
//! Run with `cargo run --release -p rustymiddle-content --example content_benchmark`.

use std::{hint::black_box, sync::Arc, time::Instant};

use bytes::Bytes;
use rustymiddle_content::{
    ContentBodyPipeline, ContentCoding, ContentDecoder, ContentEncoder, ContentLimits,
    ContentPolicy,
};
use rustymiddle_core::{
    BodyFrame, ConnectionId, HeaderBlock, HeaderField, HttpLegVersion, RequestHead, SessionId,
    SessionMetadata, StreamId, Target,
    intercept::{
        BodyPipelineLimits, ExchangeMetadata, HookLimits, InterceptorChainFactory,
        InterceptorRegistration, InterceptorRequirement, NoopInterceptorFactory,
        RequestHeadOutcome,
    },
};

const CODINGS: [ContentCoding; 4] = [
    ContentCoding::Gzip,
    ContentCoding::Brotli,
    ContentCoding::Deflate,
    ContentCoding::Zstd,
];
const MEBIBYTE: u128 = 1024 * 1024;

fn target() -> Target {
    Target {
        scheme: "https".to_owned(),
        authority: "benchmark.test".to_owned(),
        host: "benchmark.test".to_owned(),
        port: 443,
        path: "/content".to_owned(),
        query: None,
    }
}

fn metadata(iteration: u64) -> ExchangeMetadata {
    ExchangeMetadata::from_session(
        &SessionMetadata {
            session_id: SessionId(u128::from(iteration) + 1),
            downstream_connection_id: ConnectionId(1),
            stream_id: StreamId(u128::from(iteration) + 1),
            client_addr: "127.0.0.1:1000".parse().expect("static address"),
            proxy_addr: "127.0.0.1:2000".parse().expect("static address"),
            ingress_version: HttpLegVersion::Http2,
            egress_version: None,
        },
        target(),
    )
}

fn request() -> RequestHead {
    RequestHead {
        method: "POST".to_owned(),
        target: target(),
        headers: HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-encoding", "gzip").expect("static header"),
        ]),
        source_version: HttpLegVersion::Http2,
    }
}

async fn pass_through_once(
    factory: &InterceptorChainFactory,
    iteration: u64,
    input: Bytes,
) -> Bytes {
    let mut chain = factory
        .create_exchange(metadata(iteration))
        .expect("no-op chain");
    let RequestHeadOutcome::Continue { head, .. } =
        chain.request_head(request()).await.expect("request head")
    else {
        panic!("no-op chain changed the request outcome");
    };
    let body = chain
        .request_body_pipeline(&head, BodyPipelineLimits::default())
        .await
        .expect("body pipeline");
    let mut pipeline =
        ContentBodyPipeline::from_policy(body, &head.headers, ContentPolicy::disabled())
            .expect("neutral coded pass-through");
    let mut frames = pipeline
        .process(BodyFrame::Data(input))
        .await
        .expect("pass-through frame");
    frames.extend(pipeline.finish().await.expect("pass-through finish"));
    collect_data(frames)
}

async fn encode(coding: ContentCoding, input: Bytes) -> Bytes {
    let mut encoder = ContentEncoder::new(coding, ContentLimits::default()).expect("encoder");
    let mut frames = encoder
        .on_frame(BodyFrame::Data(input))
        .await
        .expect("encode frame");
    frames.extend(encoder.finish().await.expect("encode finish"));
    collect_data(frames)
}

async fn decode(coding: ContentCoding, input: Bytes) -> Bytes {
    let mut decoder = ContentDecoder::new(coding, ContentLimits::default()).expect("decoder");
    let mut frames = decoder
        .on_frame(BodyFrame::Data(input))
        .await
        .expect("decode frame");
    frames.extend(decoder.finish().await.expect("decode finish"));
    collect_data(frames)
}

fn collect_data(frames: Vec<BodyFrame>) -> Bytes {
    let capacity = frames
        .iter()
        .map(|frame| match frame {
            BodyFrame::Data(bytes) => bytes.len(),
            BodyFrame::Trailers(_) => 0,
        })
        .sum();
    let mut output = Vec::with_capacity(capacity);
    for frame in frames {
        match frame {
            BodyFrame::Data(bytes) => output.extend_from_slice(&bytes),
            BodyFrame::Trailers(_) => panic!("benchmark unexpectedly produced trailers"),
        }
    }
    Bytes::from(output)
}

fn representative_input(size: usize) -> Bytes {
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    let block_len = size.min(4096);
    let mut block = Vec::with_capacity(block_len);
    for _ in 0..block_len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        block.push(state.to_le_bytes()[0]);
    }
    Bytes::from(block.iter().copied().cycle().take(size).collect::<Vec<_>>())
}

fn report(label: &str, input_bytes: usize, iterations: u64, started: Instant) {
    let elapsed = started.elapsed();
    let nanos = elapsed.as_nanos().max(1);
    let iterations = u128::from(iterations);
    let total_bytes = u128::try_from(input_bytes)
        .expect("usize fits u128")
        .saturating_mul(iterations);
    let bytes_per_second = total_bytes.saturating_mul(1_000_000_000) / nanos;
    let whole_mib = bytes_per_second / MEBIBYTE;
    let tenth_mib = bytes_per_second % MEBIBYTE * 10 / MEBIBYTE;
    let nanos_per_operation = nanos / iterations;
    println!("{label:<34} {nanos_per_operation:>10} ns/op  {whole_mib:>5}.{tenth_mib} MiB/s");
}

#[tokio::main]
async fn main() {
    let factory = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "noop",
            Arc::new(NoopInterceptorFactory),
            InterceptorRequirement::Required,
        )],
        HookLimits::default(),
    );

    for (size, iterations) in [(4 * 1024, 500_u64), (256 * 1024, 20_u64)] {
        let input = representative_input(size);
        let started = Instant::now();
        for iteration in 0..iterations {
            let output = pass_through_once(&factory, iteration, input.clone()).await;
            assert_eq!(black_box(output.len()), input.len());
        }
        report("coded neutral pass-through", size, iterations, started);

        for coding in CODINGS {
            let encoded = encode(coding, input.clone()).await;
            let started = Instant::now();
            for _ in 0..iterations {
                let decoded = decode(coding, encoded.clone()).await;
                assert_eq!(black_box(decoded.len()), input.len());
            }
            report(
                &format!("{coding:?} decode-only"),
                size,
                iterations,
                started,
            );

            let started = Instant::now();
            for _ in 0..iterations {
                let decoded = decode(coding, encoded.clone()).await;
                let reencoded = encode(coding, decoded).await;
                black_box(reencoded);
            }
            report(
                &format!("{coding:?} decode/re-encode"),
                size,
                iterations,
                started,
            );
        }
        println!();
    }
}

#![no_main]

use std::{
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
    time::Duration,
};

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use rustymiddle_content::{
    ContentBodyPipeline, ContentCoding, ContentCodingStack, ContentDecoder, ContentDecoderOptions,
    ContentEncoder, ContentLimits, ContentOutput, ContentPipelineError, ContentWorkLimits,
};
use rustymiddle_core::{
    BodyFrame, ConnectionId, HeaderBlock, HeaderField, HttpLegVersion, RequestHead, SessionId,
    SessionMetadata, StreamId, Target,
    intercept::{
        BodyFilter, BodyHookError, BodyPipeline, BodyPipelineLimits, BodyPlan, BoxBodyFuture,
        BoxHookFuture, ExchangeInterceptor, ExchangeMetadata, HookInitError, HookLimits,
        InterceptorChainFactory, InterceptorFactory, InterceptorRegistration,
        InterceptorRequirement, RequestBodyAction, RequestBodyEvent, RequestHeadOutcome,
    },
};
use tokio::runtime::{Builder, Runtime};

const CODINGS: [ContentCoding; 4] = [
    ContentCoding::Gzip,
    ContentCoding::Brotli,
    ContentCoding::Deflate,
    ContentCoding::Zstd,
];
const MAX_PIPELINE_BYTES: usize = 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() < 10 || data.len() > 8 * 1024 {
        return;
    }
    runtime().block_on(run_pipeline(data));
});

async fn run_pipeline(data: &[u8]) {
    let layer_count = usize::from(selector(data[0]) % 5);
    let selected = (0..layer_count)
        .map(|index| CODINGS[usize::from(selector(data[index + 1])) % CODINGS.len()])
        .collect::<Vec<_>>();
    let coding_header = selected
        .iter()
        .map(|coding| coding.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let source_headers = source_headers(&coding_header);
    let stack = ContentCodingStack::from_headers(
        &source_headers,
        NonZeroUsize::new(CODINGS.len()).expect("coding depth"),
    )
    .expect("generated coding stack is valid");
    assert_eq!(stack.encode_order().collect::<Vec<_>>(), selected);

    let output = if selector(data[5]) % 2 == 0 {
        ContentOutput::Identity
    } else {
        ContentOutput::PreserveOriginal
    };
    let source_chunk = usize::from(selector(data[6]) % 64) + 1;
    let codec_chunk = usize::from(selector(data[7]) % 64) + 1;
    let prefix = (selector(data[8]) % 2 != 0).then_some(0xa5);
    let source = make_source(selector(data[9]), &data[10..]);
    let limits = limits();

    let mut encoded = source.clone();
    for coding in stack.encode_order() {
        encoded = encode_layer(coding, &encoded, codec_chunk, limits).await;
    }

    let body = decoded_body_pipeline(source_headers.clone(), prefix).await;
    let mut pipeline = ContentBodyPipeline::new(
        body,
        &source_headers,
        output,
        limits,
        ContentDecoderOptions::default(),
    )
    .expect("valid generated content pipeline");
    assert!(pipeline.modifies_body());
    assert!(
        pipeline
            .hook_headers()
            .values("content-encoding")
            .next()
            .is_none()
    );
    assert!(
        pipeline
            .hook_headers()
            .values("content-length")
            .next()
            .is_none()
    );
    assert!(pipeline.hook_headers().values("etag").next().is_none());
    assert_eq!(
        pipeline
            .hook_headers()
            .values("x-preserved")
            .collect::<Vec<_>>(),
        [b"yes".as_slice()]
    );

    let expected_coding = match output {
        ContentOutput::Identity => None,
        ContentOutput::PreserveOriginal => stack.header_value(),
    };
    assert_eq!(
        pipeline.output_headers().values("content-encoding").next(),
        expected_coding.as_deref().map(str::as_bytes)
    );
    assert!(
        pipeline
            .output_headers()
            .values("content-length")
            .next()
            .is_none()
    );
    assert!(pipeline.output_headers().values("etag").next().is_none());

    let mut output_frames = Vec::new();
    for chunk in encoded.chunks(source_chunk) {
        output_frames.extend(
            pipeline
                .process(BodyFrame::Data(Bytes::copy_from_slice(chunk)))
                .await
                .expect("valid generated source frame"),
        );
    }
    output_frames.extend(
        pipeline
            .process(BodyFrame::Trailers(source_trailers()))
            .await
            .expect("valid generated trailers"),
    );
    output_frames.extend(
        pipeline
            .finish()
            .await
            .expect("valid generated pipeline must finish"),
    );
    assert!(matches!(
        pipeline.finish().await,
        Err(ContentPipelineError::AlreadyFinished)
    ));
    assert!(matches!(
        pipeline.process(BodyFrame::Data(Bytes::new())).await,
        Err(ContentPipelineError::AlreadyFinished)
    ));

    let (mut output_data, trailers) = collect_output(output_frames);
    if output == ContentOutput::PreserveOriginal {
        for coding in stack.decode_order() {
            output_data = decode_layer(coding, &output_data, codec_chunk, limits).await;
        }
    }
    let mut expected = source.clone();
    if let Some(prefix) = prefix
        && !expected.is_empty()
    {
        expected.insert(0, prefix);
    }
    assert_eq!(output_data, expected);
    let trailers = trailers.expect("source trailers must survive the pipeline");
    assert!(trailers.values("content-digest").next().is_none());
    assert_eq!(
        trailers.values("x-fuzz-trailer").collect::<Vec<_>>(),
        [b"kept".as_slice()]
    );
}

#[derive(Clone, Copy)]
struct FuzzFactory {
    prefix: Option<u8>,
}

impl InterceptorFactory for FuzzFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(FuzzInterceptor {
            prefix: self.prefix,
        }))
    }
}

struct FuzzInterceptor {
    prefix: Option<u8>,
}

impl ExchangeInterceptor for FuzzInterceptor {
    fn on_request_body(&self, _event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        let prefix = self.prefix;
        Box::pin(async move {
            RequestBodyAction::decoded(BodyPlan::Transform(Box::new(PrefixOnce {
                prefix,
                seen_data: false,
            })))
        })
    }
}

struct PrefixOnce {
    prefix: Option<u8>,
    seen_data: bool,
}

impl BodyFilter for PrefixOnce {
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        let output = match frame {
            BodyFrame::Data(bytes)
                if !bytes.is_empty() && !self.seen_data && self.prefix.is_some() =>
            {
                self.seen_data = true;
                let mut output = Vec::with_capacity(bytes.len() + 1);
                output.push(self.prefix.expect("guarded by is_some"));
                output.extend_from_slice(&bytes);
                BodyFrame::Data(Bytes::from(output))
            }
            BodyFrame::Data(bytes) => {
                self.seen_data |= !bytes.is_empty();
                BodyFrame::Data(bytes)
            }
            frame => frame,
        };
        Box::pin(async move { Ok(vec![output]) })
    }
}

async fn decoded_body_pipeline(headers: HeaderBlock, prefix: Option<u8>) -> BodyPipeline {
    let factory = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "content-fuzz",
            Arc::new(FuzzFactory { prefix }),
            InterceptorRequirement::Required,
        )],
        HookLimits::default(),
    );
    let mut chain = factory
        .create_exchange(metadata())
        .expect("static interceptor must initialize");
    let request = RequestHead {
        method: "POST".to_owned(),
        target: metadata().original_target.as_target().clone(),
        headers,
        source_version: HttpLegVersion::Http2,
    };
    let RequestHeadOutcome::Continue { head, .. } = chain
        .request_head(request)
        .await
        .expect("static request-head hook must succeed")
    else {
        panic!("static interceptor must continue");
    };
    chain
        .request_body_pipeline(&head, BodyPipelineLimits::default())
        .await
        .expect("decoded body plan must compose")
}

fn metadata() -> ExchangeMetadata {
    ExchangeMetadata::from_session(
        &SessionMetadata {
            session_id: SessionId(1),
            downstream_connection_id: ConnectionId(2),
            stream_id: StreamId(3),
            client_addr: "127.0.0.1:1000".parse().expect("static client address"),
            proxy_addr: "127.0.0.1:2000".parse().expect("static proxy address"),
            ingress_version: HttpLegVersion::Http2,
            egress_version: None,
        },
        Target {
            scheme: "https".to_owned(),
            authority: "example.test".to_owned(),
            host: "example.test".to_owned(),
            port: 443,
            path: "/".to_owned(),
            query: None,
        },
    )
}

fn source_headers(coding: &str) -> HeaderBlock {
    let mut fields = Vec::new();
    if !coding.is_empty() {
        fields.push(
            HeaderField::try_new("content-encoding", coding).expect("generated coding header"),
        );
    }
    fields.extend([
        HeaderField::try_new("content-length", "123").expect("static header"),
        HeaderField::try_new("etag", "\"stale\"").expect("static header"),
        HeaderField::try_new("x-preserved", "yes").expect("static header"),
    ]);
    HeaderBlock::from_fields(fields)
}

fn source_trailers() -> HeaderBlock {
    HeaderBlock::from_fields(vec![
        HeaderField::try_new("content-digest", "sha-256=:stale:").expect("static trailer"),
        HeaderField::try_new("x-fuzz-trailer", "kept").expect("static trailer"),
    ])
}

async fn encode_layer(
    coding: ContentCoding,
    input: &[u8],
    chunk_size: usize,
    limits: ContentLimits,
) -> Vec<u8> {
    let mut encoder = ContentEncoder::new(coding, limits).expect("valid encoder configuration");
    let mut output = Vec::new();
    for chunk in input.chunks(chunk_size) {
        append_data(
            &mut output,
            encoder
                .on_frame(BodyFrame::Data(Bytes::copy_from_slice(chunk)))
                .await
                .expect("generated input must encode"),
        );
    }
    append_data(
        &mut output,
        encoder.finish().await.expect("encoder must finish"),
    );
    output
}

async fn decode_layer(
    coding: ContentCoding,
    input: &[u8],
    chunk_size: usize,
    limits: ContentLimits,
) -> Vec<u8> {
    let mut decoder = ContentDecoder::new(coding, limits).expect("valid decoder configuration");
    let mut output = Vec::new();
    for chunk in input.chunks(chunk_size) {
        append_data(
            &mut output,
            decoder
                .on_frame(BodyFrame::Data(Bytes::copy_from_slice(chunk)))
                .await
                .expect("generated stream must decode"),
        );
    }
    append_data(
        &mut output,
        decoder.finish().await.expect("decoder must finish"),
    );
    output
}

fn append_data(output: &mut Vec<u8>, frames: Vec<BodyFrame>) {
    for frame in frames {
        match frame {
            BodyFrame::Data(bytes) => {
                output.extend_from_slice(&bytes);
                assert!(output.len() <= MAX_PIPELINE_BYTES);
            }
            BodyFrame::Trailers(_) => panic!("codec helper did not submit trailers"),
        }
    }
}

fn collect_output(frames: Vec<BodyFrame>) -> (Vec<u8>, Option<HeaderBlock>) {
    let mut data = Vec::new();
    let mut trailers = None;
    for frame in frames {
        match frame {
            BodyFrame::Data(bytes) => {
                assert!(trailers.is_none(), "data must not follow trailers");
                data.extend_from_slice(&bytes);
                assert!(data.len() <= MAX_PIPELINE_BYTES);
            }
            BodyFrame::Trailers(value) => {
                assert!(trailers.replace(value).is_none(), "trailers are unique");
            }
        }
    }
    (data, trailers)
}

fn make_source(size_selector: u8, pattern: &[u8]) -> Vec<u8> {
    let size = match size_selector % 8 {
        0 => 0,
        1 => 1,
        2 => 63,
        3 => 64,
        4 => 1_023,
        5 => 1_024,
        6 => 4_095,
        _ => 4_096,
    };
    let mut output = Vec::with_capacity(size);
    if pattern.is_empty() {
        output.resize(size, size_selector);
    } else {
        output.extend(pattern.iter().copied().cycle().take(size));
    }
    output
}

fn limits() -> ContentLimits {
    ContentLimits::new(
        NonZeroUsize::new(MAX_PIPELINE_BYTES).expect("encoded limit"),
        NonZeroUsize::new(MAX_PIPELINE_BYTES).expect("decoded limit"),
        NonZeroUsize::new(MAX_PIPELINE_BYTES).expect("output limit"),
        NonZeroUsize::new(16 * 1024 * 1024).expect("window limit"),
        NonZeroUsize::new(1_024).expect("ratio limit"),
        MAX_PIPELINE_BYTES,
        NonZeroUsize::new(CODINGS.len()).expect("coding depth"),
    )
    .with_work_limits(
        ContentWorkLimits::new(
            NonZeroUsize::new(1024).expect("work quantum"),
            Duration::from_secs(10),
            Duration::from_secs(30),
        )
        .expect("valid work limits"),
    )
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

const fn selector(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => value,
    }
}

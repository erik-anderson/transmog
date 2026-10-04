//! End-to-end content-pipeline performance and boundedness harness.
//!
//! Run through `scripts/benchmark-content.ps1` to capture the metrics plus a
//! sampled process peak-working-set measurement.

use std::{
    hint::black_box,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use rustymiddle_content::{
    ContentBodyPipeline, ContentCoding, ContentDecoder, ContentDecoderOptions, ContentEncoder,
    ContentLimits, ContentOutput, ContentWorkLimits,
};
use rustymiddle_core::{
    BodyFrame, ConnectionId, HeaderBlock, HeaderField, HttpLegVersion, RequestHead, SessionId,
    SessionMetadata, StreamId, Target,
    intercept::{
        BodyFilter, BodyHookError, BodyPipelineLimits, BodyPlan, BoxBodyFuture, BoxHookFuture,
        ExchangeInterceptor, ExchangeMetadata, HookInitError, HookLimits, InterceptorChainFactory,
        InterceptorFactory, InterceptorRegistration, InterceptorRequirement, RequestBodyAction,
        RequestBodyEvent, RequestHeadOutcome,
    },
};
use serde::Serialize;
use tokio::sync::{Barrier, Notify};

const CODINGS: [ContentCoding; 4] = [
    ContentCoding::Gzip,
    ContentCoding::Brotli,
    ContentCoding::Deflate,
    ContentCoding::Zstd,
];
const BODY_BYTES: usize = 1024 * 1024;
const CHUNK_BYTES: usize = 4 * 1024;
const THROUGHPUT_ITERATIONS: u64 = 8;
const CANCELLATION_BODY_BYTES: usize = 8 * 1024 * 1024;
const CANCELLATION_ITERATIONS: usize = 16;
const MEMORY_CONCURRENCY: usize = 16;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkReport {
    schema_version: u32,
    body_bytes: usize,
    chunk_bytes: usize,
    throughput_iterations: u64,
    work_limits: WorkLimitReport,
    throughput: Vec<ThroughputMetric>,
    first_byte: Vec<FirstByteMetric>,
    cancellation: CancellationMetric,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkLimitReport {
    bytes_per_yield: usize,
    operation_millis: u64,
    body_millis: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThroughputMetric {
    coding: &'static str,
    output: &'static str,
    nanos_per_operation: u64,
    bytes_per_second: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FirstByteMetric {
    coding: &'static str,
    output: &'static str,
    first_byte_nanos: u64,
    encoded_bytes_before_output: usize,
    encoded_body_bytes: usize,
    output_body_bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CancellationMetric {
    coding: &'static str,
    body_bytes: usize,
    iterations: usize,
    p50_nanos: u64,
    p95_nanos: u64,
    max_nanos: u64,
    deadline_nanos: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryProbeReport {
    schema_version: u32,
    concurrent_pipelines: usize,
    body_bytes_per_pipeline: usize,
    chunk_bytes: usize,
    completed_output_bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryBaselineReport {
    schema_version: u32,
    body_bytes: usize,
    encoded_fixture_bytes: usize,
}

struct DecodedIdentityFactory;

impl InterceptorFactory for DecodedIdentityFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(DecodedIdentityInterceptor))
    }
}

struct DecodedIdentityInterceptor;

impl ExchangeInterceptor for DecodedIdentityInterceptor {
    fn on_request_body(&self, _event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        Box::pin(async {
            RequestBodyAction::decoded(BodyPlan::Transform(Box::new(IdentityFilter)))
        })
    }
}

struct IdentityFilter;

impl BodyFilter for IdentityFilter {
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async move { Ok(vec![frame]) })
    }
}

fn factory() -> InterceptorChainFactory {
    InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "decoded-identity",
            Arc::new(DecodedIdentityFactory),
            InterceptorRequirement::Required,
        )],
        HookLimits::default(),
    )
}

fn target() -> Target {
    Target {
        scheme: "https".to_owned(),
        authority: "benchmark.test".to_owned(),
        host: "benchmark.test".to_owned(),
        port: 443,
        path: "/content-pipeline".to_owned(),
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

fn request(coding: ContentCoding) -> RequestHead {
    RequestHead {
        method: "POST".to_owned(),
        target: target(),
        headers: HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-encoding", coding.as_str()).expect("known coding"),
        ]),
        source_version: HttpLegVersion::Http2,
    }
}

async fn pipeline(
    factory: &InterceptorChainFactory,
    coding: ContentCoding,
    output: ContentOutput,
    limits: ContentLimits,
    iteration: u64,
) -> ContentBodyPipeline {
    let mut chain = factory
        .create_exchange(metadata(iteration))
        .expect("benchmark interceptor initialization");
    let RequestHeadOutcome::Continue { head, .. } = chain
        .request_head(request(coding))
        .await
        .expect("benchmark request head")
    else {
        panic!("benchmark interceptor changed the request outcome");
    };
    let body = chain
        .request_body_pipeline(&head, BodyPipelineLimits::default())
        .await
        .expect("benchmark body pipeline");
    ContentBodyPipeline::new(
        body,
        &head.headers,
        output,
        limits,
        ContentDecoderOptions::default(),
    )
    .expect("benchmark content pipeline")
}

async fn encode(coding: ContentCoding, input: Bytes) -> Bytes {
    let mut encoder = ContentEncoder::new(coding, ContentLimits::default()).expect("encoder");
    let mut output = Vec::new();
    for chunk in input.chunks(CHUNK_BYTES) {
        append_data(
            &mut output,
            encoder
                .on_frame(BodyFrame::Data(Bytes::copy_from_slice(chunk)))
                .await
                .expect("encode frame"),
        );
    }
    append_data(&mut output, encoder.finish().await.expect("encode finish"));
    Bytes::from(output)
}

async fn decode(coding: ContentCoding, input: Bytes) -> Bytes {
    let mut decoder = ContentDecoder::new(coding, ContentLimits::default()).expect("decoder");
    let mut output = Vec::new();
    append_data(
        &mut output,
        decoder
            .on_frame(BodyFrame::Data(input))
            .await
            .expect("decode frame"),
    );
    append_data(&mut output, decoder.finish().await.expect("decode finish"));
    Bytes::from(output)
}

async fn drive(pipeline: &mut ContentBodyPipeline, input: &Bytes, chunk_bytes: usize) -> Bytes {
    let mut output = Vec::new();
    for chunk in input.chunks(chunk_bytes) {
        append_data(
            &mut output,
            pipeline
                .process(BodyFrame::Data(Bytes::copy_from_slice(chunk)))
                .await
                .expect("content frame"),
        );
    }
    append_data(
        &mut output,
        pipeline.finish().await.expect("content finish"),
    );
    Bytes::from(output)
}

fn append_data(output: &mut Vec<u8>, frames: Vec<BodyFrame>) -> usize {
    let mut appended = 0_usize;
    for frame in frames {
        match frame {
            BodyFrame::Data(bytes) => {
                appended = appended.saturating_add(bytes.len());
                output.extend_from_slice(&bytes);
            }
            BodyFrame::Trailers(_) => panic!("benchmark unexpectedly produced trailers"),
        }
    }
    appended
}

fn representative_input(size: usize) -> Bytes {
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    let mut output = Vec::with_capacity(size);
    for _ in 0..size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        output.push(state.to_le_bytes()[0]);
    }
    Bytes::from(output)
}

async fn throughput_metrics(
    factory: &InterceptorChainFactory,
    source: &Bytes,
) -> Vec<ThroughputMetric> {
    let mut metrics = Vec::new();
    for coding in CODINGS {
        let encoded = encode(coding, source.clone()).await;
        for output in [ContentOutput::Identity, ContentOutput::PreserveOriginal] {
            let mut warmup = pipeline(factory, coding, output, ContentLimits::default(), 0).await;
            let warmup_output = drive(&mut warmup, &encoded, CHUNK_BYTES).await;
            validate_output(coding, output, source, warmup_output).await;

            let started = Instant::now();
            for iteration in 0..THROUGHPUT_ITERATIONS {
                let mut content = pipeline(
                    factory,
                    coding,
                    output,
                    ContentLimits::default(),
                    iteration + 1,
                )
                .await;
                let result = drive(&mut content, &encoded, CHUNK_BYTES).await;
                black_box(result);
            }
            let elapsed = started.elapsed();
            metrics.push(ThroughputMetric {
                coding: coding.as_str(),
                output: output_name(output),
                nanos_per_operation: duration_nanos(elapsed) / THROUGHPUT_ITERATIONS,
                bytes_per_second: bytes_per_second(source.len(), THROUGHPUT_ITERATIONS, elapsed),
            });
        }
    }
    metrics
}

async fn first_byte_metrics(
    factory: &InterceptorChainFactory,
    source: &Bytes,
) -> Vec<FirstByteMetric> {
    let mut metrics = Vec::new();
    for coding in CODINGS {
        let encoded = encode(coding, source.clone()).await;
        for output in [ContentOutput::Identity, ContentOutput::PreserveOriginal] {
            let mut content = pipeline(factory, coding, output, ContentLimits::default(), 0).await;
            let started = Instant::now();
            let mut first = None;
            let mut consumed = 0_usize;
            let mut result = Vec::new();
            for chunk in encoded.chunks(CHUNK_BYTES) {
                consumed = consumed.saturating_add(chunk.len());
                let frames = content
                    .process(BodyFrame::Data(Bytes::copy_from_slice(chunk)))
                    .await
                    .expect("first-byte content frame");
                let produced = append_data(&mut result, frames);
                if produced != 0 && first.is_none() {
                    first = Some((started.elapsed(), consumed));
                }
            }
            let produced = append_data(
                &mut result,
                content.finish().await.expect("first-byte content finish"),
            );
            if produced != 0 && first.is_none() {
                first = Some((started.elapsed(), consumed));
            }
            let (latency, consumed) = first.expect("non-empty body must produce output");
            let output_bytes = result.len();
            validate_output(coding, output, source, Bytes::from(result)).await;
            metrics.push(FirstByteMetric {
                coding: coding.as_str(),
                output: output_name(output),
                first_byte_nanos: duration_nanos(latency),
                encoded_bytes_before_output: consumed,
                encoded_body_bytes: encoded.len(),
                output_body_bytes: output_bytes,
            });
        }
    }
    metrics
}

async fn validate_output(
    coding: ContentCoding,
    output: ContentOutput,
    expected: &Bytes,
    actual: Bytes,
) {
    let actual = match output {
        ContentOutput::Identity => actual,
        ContentOutput::PreserveOriginal => decode(coding, actual).await,
    };
    assert_eq!(&actual, expected, "{coding:?} {output:?}");
}

async fn cancellation_metric(factory: &InterceptorChainFactory) -> CancellationMetric {
    const DEADLINE: Duration = Duration::from_secs(1);
    let source = representative_input(CANCELLATION_BODY_BYTES);
    let encoded = encode(ContentCoding::Brotli, source).await;
    let work = ContentWorkLimits::new(
        NonZeroUsize::new(1024).expect("work quantum"),
        Duration::from_secs(5),
        Duration::from_secs(30),
    )
    .expect("valid work limits");
    let limits = ContentLimits::default().with_work_limits(work);
    let mut samples = Vec::with_capacity(CANCELLATION_ITERATIONS);

    for iteration in 0..CANCELLATION_ITERATIONS {
        let mut content = pipeline(
            factory,
            ContentCoding::Brotli,
            ContentOutput::PreserveOriginal,
            limits,
            u64::try_from(iteration).expect("iteration fits u64"),
        )
        .await;
        let input = encoded.clone();
        let started = Arc::new(Notify::new());
        let task = {
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                started.notify_one();
                content.process(BodyFrame::Data(input)).await
            })
        };
        started.notified().await;
        tokio::task::yield_now().await;
        let cancelled = Instant::now();
        task.abort();
        let failure = tokio::time::timeout(DEADLINE, task)
            .await
            .expect("cancelled content operation exceeded the benchmark deadline")
            .expect_err("content operation completed before cancellation");
        assert!(failure.is_cancelled());
        samples.push(duration_nanos(cancelled.elapsed()));
    }

    samples.sort_unstable();
    CancellationMetric {
        coding: ContentCoding::Brotli.as_str(),
        body_bytes: CANCELLATION_BODY_BYTES,
        iterations: CANCELLATION_ITERATIONS,
        p50_nanos: percentile(&samples, 50),
        p95_nanos: percentile(&samples, 95),
        max_nanos: *samples.last().expect("cancellation samples"),
        deadline_nanos: duration_nanos(DEADLINE),
    }
}

async fn memory_probe(factory: &InterceptorChainFactory) -> MemoryProbeReport {
    let source = representative_input(BODY_BYTES);
    let mut encoded = Vec::new();
    for coding in CODINGS {
        encoded.push((coding, encode(coding, source.clone()).await));
    }
    let barrier = Arc::new(Barrier::new(MEMORY_CONCURRENCY + 1));
    let mut tasks = Vec::with_capacity(MEMORY_CONCURRENCY);
    for iteration in 0..MEMORY_CONCURRENCY {
        let (coding, input) = encoded[iteration % encoded.len()].clone();
        let mut content = pipeline(
            factory,
            coding,
            ContentOutput::PreserveOriginal,
            ContentLimits::default(),
            u64::try_from(iteration).expect("iteration fits u64"),
        )
        .await;
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            drive(&mut content, &input, CHUNK_BYTES).await.len()
        }));
    }
    barrier.wait().await;
    let mut output_bytes = 0_usize;
    for task in tasks {
        output_bytes = output_bytes
            .checked_add(task.await.expect("memory-probe task"))
            .expect("bounded output total");
    }
    assert!(output_bytes != 0);
    MemoryProbeReport {
        schema_version: 1,
        concurrent_pipelines: MEMORY_CONCURRENCY,
        body_bytes_per_pipeline: BODY_BYTES,
        chunk_bytes: CHUNK_BYTES,
        completed_output_bytes: output_bytes,
    }
}

async fn memory_baseline() -> MemoryBaselineReport {
    let source = representative_input(BODY_BYTES);
    let mut fixtures = Vec::new();
    for coding in CODINGS {
        fixtures.push(encode(coding, source.clone()).await);
    }
    let encoded_fixture_bytes = fixtures.iter().map(Bytes::len).sum();
    tokio::time::sleep(Duration::from_millis(100)).await;
    black_box(&fixtures);
    MemoryBaselineReport {
        schema_version: 1,
        body_bytes: BODY_BYTES,
        encoded_fixture_bytes,
    }
}

async fn benchmark_report() -> BenchmarkReport {
    let factory = factory();
    let source = representative_input(BODY_BYTES);
    let limits = ContentWorkLimits::default();
    BenchmarkReport {
        schema_version: 1,
        body_bytes: BODY_BYTES,
        chunk_bytes: CHUNK_BYTES,
        throughput_iterations: THROUGHPUT_ITERATIONS,
        work_limits: WorkLimitReport {
            bytes_per_yield: limits.max_bytes_per_yield().get(),
            operation_millis: duration_millis(limits.max_operation_duration()),
            body_millis: duration_millis(limits.max_body_duration()),
        },
        throughput: throughput_metrics(&factory, &source).await,
        first_byte: first_byte_metrics(&factory, &source).await,
        cancellation: cancellation_metric(&factory).await,
    }
}

fn output_name(output: ContentOutput) -> &'static str {
    match output {
        ContentOutput::Identity => "identity",
        ContentOutput::PreserveOriginal => "preserve-original",
    }
}

fn duration_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn bytes_per_second(bytes: usize, iterations: u64, elapsed: Duration) -> u64 {
    let total = u128::try_from(bytes)
        .expect("usize fits u128")
        .saturating_mul(u128::from(iterations));
    let rate = total.saturating_mul(1_000_000_000) / elapsed.as_nanos().max(1);
    u64::try_from(rate).unwrap_or(u64::MAX)
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let rank = sorted
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[rank]
}

fn print_human(report: &BenchmarkReport) {
    println!(
        "end-to-end content pipeline: {}-byte bodies, {}-byte frames",
        report.body_bytes, report.chunk_bytes
    );
    for metric in &report.throughput {
        let mib = metric.bytes_per_second / (1024 * 1024);
        let tenth = metric.bytes_per_second % (1024 * 1024) * 10 / (1024 * 1024);
        println!(
            "{:<8} {:<17} {:>10} ns/op  {:>5}.{} MiB/s",
            metric.coding, metric.output, metric.nanos_per_operation, mib, tenth
        );
    }
    println!();
    for metric in &report.first_byte {
        println!(
            "first byte {:<8} {:<17} {:>10} ns after {:>7}/{:>7} encoded bytes",
            metric.coding,
            metric.output,
            metric.first_byte_nanos,
            metric.encoded_bytes_before_output,
            metric.encoded_body_bytes
        );
    }
    println!();
    println!(
        "cancellation {:<8} p50 {:>8} ns  p95 {:>8} ns  max {:>8} ns",
        report.cancellation.coding,
        report.cancellation.p50_nanos,
        report.cancellation.p95_nanos,
        report.cancellation.max_nanos
    );
}

#[tokio::main]
async fn main() {
    let argument = std::env::args().nth(1);
    match argument.as_deref() {
        None => print_human(&benchmark_report().await),
        Some("--json") => println!(
            "{}",
            serde_json::to_string_pretty(&benchmark_report().await).expect("serialize report")
        ),
        Some("--memory-probe") => println!(
            "{}",
            serde_json::to_string(&memory_probe(&factory()).await).expect("serialize memory probe")
        ),
        Some("--memory-baseline") => println!(
            "{}",
            serde_json::to_string(&memory_baseline().await).expect("serialize memory baseline")
        ),
        Some(argument) => {
            eprintln!(
                "unknown argument {argument:?}; expected --json, --memory-baseline, or --memory-probe"
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_and_nearest_rank_percentiles_are_integer_and_deterministic() {
        assert_eq!(bytes_per_second(1024, 2, Duration::from_secs(2)), 1024);
        let samples = [10, 20, 30, 40];
        assert_eq!(percentile(&samples, 50), 20);
        assert_eq!(percentile(&samples, 95), 40);
        assert_eq!(percentile(&samples, 100), 40);
    }

    #[test]
    fn output_names_are_stable_report_values() {
        assert_eq!(output_name(ContentOutput::Identity), "identity");
        assert_eq!(
            output_name(ContentOutput::PreserveOriginal),
            "preserve-original"
        );
    }
}

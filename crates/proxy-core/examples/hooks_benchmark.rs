//! Small dependency-free Hooks v2 microbenchmark harness.
//!
//! Run with `cargo run --release -p transmog-core --example hooks_benchmark`.

use std::{num::NonZeroUsize, sync::Arc, time::Instant};

use bytes::Bytes;
use transmog_core::{
    BodyFrame, ConnectionId, HeaderBlock, HttpLegVersion, RequestHead, ResponseHead, SessionId,
    SessionMetadata, StreamId, Target,
    intercept::{
        BodyFilter, BodyHookError, BodyPipelineLimits, BodyPlan, BoxBodyFuture, BoxHookFuture,
        CompletedExchange, ExchangeInterceptor, ExchangeMetadata, HookInitError, HookLimits,
        InterceptorChainFactory, InterceptorFactory, InterceptorRegistration,
        InterceptorRequirement, NoopInterceptorFactory, RequestBodyAction, RequestBodyEvent,
    },
    observe::{
        BoxObserverFuture, Observer, ObserverConfig, ObserverEvent, ObserverEventKind, ObserverHub,
    },
};

const ITERATIONS: u32 = 2_000;

fn metadata(iteration: u32) -> ExchangeMetadata {
    ExchangeMetadata::from_session(
        &SessionMetadata {
            session_id: SessionId(u128::from(iteration) + 1),
            downstream_connection_id: ConnectionId(1),
            stream_id: StreamId(u128::from(iteration) + 1),
            client_addr: "127.0.0.1:1000".parse().expect("static address"),
            client_identity: transmog_core::ClientIdentity::default(),
            proxy_addr: "127.0.0.1:2000".parse().expect("static address"),
            ingress_version: HttpLegVersion::Http2,
            egress_version: None,
        },
        target(),
    )
}

fn target() -> Target {
    Target {
        scheme: "https".to_owned(),
        authority: "example.test".to_owned(),
        host: "example.test".to_owned(),
        port: 443,
        path: "/fixed".to_owned(),
        query: None,
    }
}

fn request() -> RequestHead {
    RequestHead {
        method: "GET".to_owned(),
        target: target(),
        headers: HeaderBlock::new(),
        source_version: HttpLegVersion::Http2,
    }
}

fn response() -> ResponseHead {
    ResponseHead {
        status: 200,
        headers: HeaderBlock::new(),
        source_version: HttpLegVersion::Http2,
    }
}

async fn measure(interceptors: usize) {
    let registrations = (0..interceptors)
        .map(|index| {
            InterceptorRegistration::new(
                format!("noop-{index}"),
                Arc::new(NoopInterceptorFactory),
                InterceptorRequirement::Required,
            )
        })
        .collect();
    let factory = InterceptorChainFactory::new(registrations, HookLimits::default());
    let started = Instant::now();
    for iteration in 0..ITERATIONS {
        let mut chain = factory
            .create_exchange(metadata(iteration))
            .expect("no-op initialization");
        let transmog_core::intercept::RequestHeadOutcome::Continue { head: request, .. } =
            chain.request_head(request()).await.expect("request hook")
        else {
            panic!("no-op interceptor changed request outcome");
        };
        let transmog_core::intercept::ResponseHeadOutcome::Continue { head: response, .. } = chain
            .response_head(&request, response(), None, false)
            .await
            .expect("response hook")
        else {
            panic!("no-op interceptor changed response outcome");
        };
        assert!(
            chain
                .completed(CompletedExchange {
                    metadata: Arc::clone(chain.context().metadata()),
                    request_head: request,
                    response_head: response,
                })
                .await
                .is_clean()
        );
    }
    let elapsed = started.elapsed();
    let nanos = elapsed.as_nanos() / u128::from(ITERATIONS);
    println!("{interceptors} no-op interceptor(s): {nanos} ns/exchange");
}

struct TransformFactory;

impl InterceptorFactory for TransformFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(TransformInterceptor))
    }
}

struct TransformInterceptor;

impl ExchangeInterceptor for TransformInterceptor {
    fn on_request_body(&self, _event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        Box::pin(async { RequestBodyAction::raw(BodyPlan::Transform(Box::new(PrefixFilter))) })
    }
}

struct PrefixFilter;

impl BodyFilter for PrefixFilter {
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async move {
            Ok(match frame {
                BodyFrame::Data(data) => {
                    let mut output = Vec::with_capacity(data.len() + 1);
                    output.push(b'x');
                    output.extend_from_slice(&data);
                    vec![BodyFrame::Data(Bytes::from(output))]
                }
                BodyFrame::Trailers(trailers) => vec![BodyFrame::Trailers(trailers)],
            })
        })
    }
}

async fn measure_transform() {
    let factory = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "transform",
            Arc::new(TransformFactory),
            InterceptorRequirement::Required,
        )],
        HookLimits::default(),
    );
    let input = Bytes::from(vec![b'a'; 4096]);
    let started = Instant::now();
    for iteration in 0..ITERATIONS {
        let mut chain = factory.create_exchange(metadata(iteration)).unwrap();
        let transmog_core::intercept::RequestHeadOutcome::Continue { head, .. } =
            chain.request_head(request()).await.unwrap()
        else {
            panic!("transform interceptor changed request outcome");
        };
        let mut pipeline = chain
            .request_body_pipeline(&head, BodyPipelineLimits::default())
            .await
            .unwrap();
        let output = pipeline
            .process(BodyFrame::Data(input.clone()))
            .await
            .unwrap();
        assert!(matches!(&output[0], BodyFrame::Data(data) if data.len() == 4097));
        assert!(pipeline.finish().await.unwrap().is_empty());
    }
    let nanos = started.elapsed().as_nanos() / u128::from(ITERATIONS);
    println!("one 4 KiB streaming transform: {nanos} ns/exchange");
}

struct NoopObserver;

impl Observer for NoopObserver {
    fn on_event(&self, _event: ObserverEvent) -> BoxObserverFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

async fn measure_observer() {
    let hub = ObserverHub::new(vec![(
        Arc::new(NoopObserver),
        ObserverConfig {
            queue_capacity: NonZeroUsize::new(4096).expect("nonzero"),
            ..ObserverConfig::default()
        },
    )]);
    let observer = hub.start_exchange(Arc::new(metadata(1)));
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        observer
            .emit(ObserverEventKind::RequestHeadFinalized(request()))
            .await;
    }
    hub.shutdown().await;
    let nanos = started.elapsed().as_nanos() / u128::from(ITERATIONS);
    println!("one bounded observer delivery: {nanos} ns/event");
}

#[tokio::main]
async fn main() {
    measure(0).await;
    measure(1).await;
    measure(4).await;
    measure_transform().await;
    measure_observer().await;
}

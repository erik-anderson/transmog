//! Embeds rustymiddle with application-owned hooks, observation, routing, and upstream service.

use std::{num::NonZeroUsize, sync::Arc};

use bytes::Bytes;
use rustymiddle_content::{ContentLimits, ContentPolicy};
use rustymiddle_core::{
    BodyFrame, BodyStream, HeaderBlock, HeaderField, HttpLegVersion, ResponseHead, RoutePolicy,
    StreamingRequest, StreamingResponse,
    intercept::{
        BodyFilter, BodyHookError, BodyPlan, BoxBodyFuture, BoxHookFuture, ExchangeInterceptor,
        ExchangeMetadata, HookInitError, InterceptorChainFactory, InterceptorFactory,
        InterceptorRegistration, InterceptorRequirement, RequestBodyAction, RequestBodyEvent,
        RequestHeadAction, RequestHeadEvent, ResponseBodyAction, ResponseBodyEvent,
    },
    observe::{
        BoxObserverFuture, Observer, ObserverConfig, ObserverError, ObserverEvent, ObserverHub,
    },
    route::{OriginalDestinationOnly, PolicyRouteSelector, UpstreamPlan},
    upstream::{BoxUpstreamFuture, UpstreamService},
};
use rustymiddle_runtime::{ProxyComponents, ProxyConfig, ProxyServer};
use rustymiddle_tls::{CachedMitmCertificateResolver, ProxyCa, SystemTrustSource, TrustSnapshot};

#[derive(Clone, Copy)]
struct AddApplicationHeader;

impl InterceptorFactory for AddApplicationHeader {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(*self))
    }
}

impl ExchangeInterceptor for AddApplicationHeader {
    fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        Box::pin(async move {
            let mut head = event.head;
            head.headers.replace_all(
                HeaderField::try_new("x-embedded-proxy", "rustymiddle")
                    .expect("static header is valid"),
            );
            RequestHeadAction::Replace(head)
        })
    }

    fn on_request_body(&self, _event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        Box::pin(async {
            RequestBodyAction::decoded(BodyPlan::Transform(Box::new(PrefixFirstData::default())))
        })
    }

    fn on_response_body(&self, _event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        Box::pin(async {
            ResponseBodyAction::decoded(BodyPlan::Transform(Box::new(PrefixFirstData::default())))
        })
    }
}

#[derive(Default)]
struct PrefixFirstData {
    prefixed: bool,
}

impl BodyFilter for PrefixFirstData {
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async move {
            match frame {
                BodyFrame::Data(data) if !data.is_empty() && !self.prefixed => {
                    self.prefixed = true;
                    let mut output = Vec::with_capacity(data.len().saturating_add(9));
                    output.extend_from_slice(b"embedded:");
                    output.extend_from_slice(&data);
                    Ok(vec![BodyFrame::Data(Bytes::from(output))])
                }
                frame => Ok(vec![frame]),
            }
        })
    }
}

struct MetadataObserver;

impl Observer for MetadataObserver {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        // Observer events are redacted by the core before this callback.
        println!("exchange={} event={:?}", event.exchange_id.0, event.kind);
        Box::pin(async { Ok::<(), ObserverError>(()) })
    }
}

struct ApplicationUpstream;

impl UpstreamService for ApplicationUpstream {
    fn execute(
        &self,
        mut request: StreamingRequest,
        _plan: UpstreamPlan,
        cancellation: rustymiddle_core::intercept::ExchangeCancellation,
    ) -> BoxUpstreamFuture<'_> {
        Box::pin(async move {
            while let Some(frame) = request.body.recv().await {
                if cancellation.is_cancelled() {
                    return Err(rustymiddle_core::upstream::UpstreamError::application(
                        "exchange cancelled",
                    ));
                }
                frame.map_err(|error| {
                    rustymiddle_core::upstream::UpstreamError::application(error.to_string())
                })?;
            }

            let (sender, body) = BodyStream::channel(NonZeroUsize::new(2).expect("nonzero"));
            tokio::spawn(async move {
                let _ = sender
                    .send(Ok(BodyFrame::Data(Bytes::from_static(
                        b"hello from the embedded upstream\n",
                    ))))
                    .await;
            });
            Ok(StreamingResponse {
                head: ResponseHead {
                    status: 200,
                    headers: HeaderBlock::new(),
                    source_version: HttpLegVersion::Http1,
                },
                body,
            })
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ProxyConfig {
        route_policy: RoutePolicy::Http1Only,
        ..ProxyConfig::default()
    };
    let trust_generation = 1;
    let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, trust_generation)?);
    let hooks = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "application-header",
            Arc::new(AddApplicationHeader),
            InterceptorRequirement::Required,
        )],
        config.limits.hooks,
    );
    let observers = ObserverHub::new(vec![(
        Arc::new(MetadataObserver),
        ObserverConfig::default(),
    )]);
    let selector = PolicyRouteSelector::new(
        Arc::new(OriginalDestinationOnly),
        RoutePolicy::Http1Only,
        trust_generation,
        "application",
        "in-process",
        "embedded",
    );
    let certificates = Arc::new(CachedMitmCertificateResolver::new(
        ProxyCa::generate("rustymiddle embedded example", 2)?,
        config.limits.leaf_cache_capacity,
        config.limits.leaf_validity_days,
    )?);
    let components = ProxyComponents::new(hooks, certificates)
        // Semantic hooks see identity bytes. Coded requests and responses are
        // restored to their original gzip/br/deflate/zstd representation.
        .with_content_policy(ContentPolicy::preserve_original_output(
            ContentLimits::default(),
        ))
        .with_observers(observers)
        .with_route_selector(Arc::new(selector))
        .with_upstream_service(Arc::new(ApplicationUpstream));
    let proxy = ProxyServer::bind_with_components(config, trust, components).await?;
    println!("embedded proxy listening on {}", proxy.local_addr()?);
    proxy
        .serve(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

use std::{
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    num::NonZeroUsize,
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use boring::{
    rand::rand_bytes,
    ssl::{SslContext, SslMethod},
};
use bytes::Bytes;
use http::{Request, Response, Version};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use quiche::h3::NameValue;
use rustymiddle_content::{
    ContentCoding, ContentDecoder, ContentEncoder, ContentLimits, ContentPolicy,
};
use rustymiddle_core::{
    BodyFrame, BodyStream, HeaderBlock, HeaderField, HttpLegVersion, ResponseHead, RoutePolicy,
    StreamingRequest, StreamingResponse,
    intercept::{
        BodyFilter, BodyHookError, BodyPlan, BoxBodyFuture, BoxHookFuture, ExchangeInterceptor,
        ExchangeMetadata, HookInitError, InterceptorChainFactory, InterceptorFactory,
        InterceptorRegistration, InterceptorRequirement, RequestBodyAction, RequestBodyEvent,
        ResponseBodyAction, ResponseBodyEvent,
    },
    route::UpstreamPlan,
    upstream::{BoxUpstreamFuture, UpstreamError, UpstreamService},
};
use rustymiddle_h3::{H3TransportLimits, Origin};
use rustymiddle_tls::{
    CachedMitmCertificateResolver, DownstreamTlsContextFactory, DownstreamTlsPolicy,
    EndpointIdentity, LoadedTrust, ProxyCa, TrustError, TrustSnapshot, TrustSource,
    UpstreamTlsContextFactory, UpstreamTlsPolicy,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Notify, oneshot},
    task::JoinHandle,
    time::timeout,
};

use super::{ProxyComponents, ProxyConfig, ProxyServer};

const CODING_HEADER: &str = "gzip, br, deflate, zstd";
const CODINGS: [ContentCoding; 4] = [
    ContentCoding::Gzip,
    ContentCoding::Brotli,
    ContentCoding::Deflate,
    ContentCoding::Zstd,
];

struct TestOrigin {
    // H3 fixtures must retain the UDP binding until the client has consumed
    // their final packets. TCP fixtures do not need an extra socket owner.
    socket: Option<Arc<UdpSocket>>,
    task: JoinHandle<()>,
}

impl TestOrigin {
    async fn finish(self) {
        let Self { socket, task } = self;
        let result = task.await;
        drop(socket);
        result.unwrap();
    }
}

struct StaticTrust(Vec<Vec<u8>>);

impl TrustSource for StaticTrust {
    fn load(&self) -> Result<LoadedTrust, TrustError> {
        Ok(LoadedTrust {
            certificates_der: self.0.clone(),
            source_description: "content matrix root".to_owned(),
            source_version: Some("test".to_owned()),
            diagnostics: Vec::new(),
        })
    }
}

#[derive(Clone, Copy)]
struct DecodedPrefixFactory;

impl InterceptorFactory for DecodedPrefixFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(*self))
    }
}

impl ExchangeInterceptor for DecodedPrefixFactory {
    fn on_request_body(&self, _event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        Box::pin(async {
            RequestBodyAction::decoded(BodyPlan::Transform(Box::new(PrefixOnce::default())))
        })
    }

    fn on_response_body(&self, _event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        Box::pin(async {
            ResponseBodyAction::decoded(BodyPlan::Transform(Box::new(PrefixOnce::default())))
        })
    }
}

#[derive(Default)]
struct PrefixOnce(bool);

impl BodyFilter for PrefixOnce {
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async move {
            match frame {
                BodyFrame::Data(data) if !data.is_empty() && !self.0 => {
                    self.0 = true;
                    let mut output = Vec::with_capacity(data.len().saturating_add(1));
                    output.push(b'x');
                    output.extend_from_slice(&data);
                    Ok(vec![BodyFrame::Data(Bytes::from(output))])
                }
                frame => Ok(vec![frame]),
            }
        })
    }
}

struct StreamingCompressedUpstream {
    first: Bytes,
    second: Bytes,
    first_sent: Arc<Notify>,
    release: Arc<Notify>,
}

impl UpstreamService for StreamingCompressedUpstream {
    fn execute(
        &self,
        mut request: StreamingRequest,
        _plan: UpstreamPlan,
        _cancellation: rustymiddle_core::intercept::ExchangeCancellation,
    ) -> BoxUpstreamFuture<'_> {
        let first = self.first.clone();
        let second = self.second.clone();
        let first_sent = Arc::clone(&self.first_sent);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            while let Some(frame) = request.body.recv().await {
                frame.map_err(|error| UpstreamError::application(error.to_string()))?;
            }
            let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
            tokio::spawn(async move {
                sender.send(Ok(BodyFrame::Data(first))).await.unwrap();
                first_sent.notify_one();
                release.notified().await;
                sender.send(Ok(BodyFrame::Data(second))).await.unwrap();
            });
            Ok(StreamingResponse {
                head: ResponseHead {
                    status: 200,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("content-encoding", "gzip").unwrap(),
                    ]),
                    source_version: HttpLegVersion::Http1,
                },
                body,
            })
        })
    }
}

struct MultiplexedCompressedUpstream {
    slow_body: Bytes,
    fast_body: Bytes,
    slow_entered: Arc<Notify>,
    release_slow: Arc<Notify>,
}

impl UpstreamService for MultiplexedCompressedUpstream {
    fn execute(
        &self,
        mut request: StreamingRequest,
        _plan: UpstreamPlan,
        _cancellation: rustymiddle_core::intercept::ExchangeCancellation,
    ) -> BoxUpstreamFuture<'_> {
        let slow = request.head.target.path == "/slow";
        let body_bytes = if slow {
            self.slow_body.clone()
        } else {
            self.fast_body.clone()
        };
        let slow_entered = Arc::clone(&self.slow_entered);
        let release_slow = Arc::clone(&self.release_slow);
        Box::pin(async move {
            while let Some(frame) = request.body.recv().await {
                frame.map_err(|error| UpstreamError::application(error.to_string()))?;
            }
            let (sender, body) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
            tokio::spawn(async move {
                if slow {
                    slow_entered.notify_one();
                    release_slow.notified().await;
                }
                sender.send(Ok(BodyFrame::Data(body_bytes))).await.unwrap();
            });
            Ok(StreamingResponse {
                head: ResponseHead {
                    status: 200,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("content-encoding", "gzip").unwrap(),
                    ]),
                    source_version: HttpLegVersion::Http1,
                },
                body,
            })
        })
    }
}

struct BodyFailureUpstream {
    body_failed: Arc<AtomicBool>,
}

impl UpstreamService for BodyFailureUpstream {
    fn execute(
        &self,
        mut request: StreamingRequest,
        _plan: UpstreamPlan,
        _cancellation: rustymiddle_core::intercept::ExchangeCancellation,
    ) -> BoxUpstreamFuture<'_> {
        let body_failed = Arc::clone(&self.body_failed);
        Box::pin(async move {
            while let Some(frame) = request.body.recv().await {
                if frame.is_err() {
                    body_failed.store(true, Ordering::SeqCst);
                    return Err(UpstreamError::application("request body failed"));
                }
            }
            Err(UpstreamError::application("request unexpectedly completed"))
        })
    }
}

#[tokio::test]
async fn every_content_coding_crosses_the_complete_protocol_matrix() {
    let encoded_request = encode_stack(b"request").await;
    let encoded_response = encode_stack(b"response").await;
    let matrix = [
        (HttpLegVersion::Http1, HttpLegVersion::Http1),
        (HttpLegVersion::Http1, HttpLegVersion::Http2),
        (HttpLegVersion::Http2, HttpLegVersion::Http1),
        (HttpLegVersion::Http2, HttpLegVersion::Http2),
        (HttpLegVersion::Http1, HttpLegVersion::Http3),
        (HttpLegVersion::Http2, HttpLegVersion::Http3),
    ];

    for (index, (ingress, egress)) in matrix.into_iter().enumerate() {
        run_matrix_case(
            ingress,
            egress,
            100_u64.saturating_add(index as u64),
            encoded_request.clone(),
            encoded_response.clone(),
        )
        .await;
    }
}

#[tokio::test]
async fn auto_fallback_replays_the_processed_coded_body_before_response_start() {
    let encoded_request = encode_stack(b"request").await;
    let encoded_response = encode_stack(b"response").await;
    let origin_ca = ProxyCa::generate("rustymiddle coded fallback origin", 2).unwrap();
    let origin_leaf = origin_ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let (origin_addr, origin_task) =
        spawn_content_tls_origin(HttpLegVersion::Http2, origin_leaf, encoded_response).await;
    let trust_generation = 300;
    let origin_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
            trust_generation,
        )
        .unwrap(),
    );
    let proxy_ca = ProxyCa::generate("rustymiddle coded fallback proxy", 2).unwrap();
    let browser_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
            1,
        )
        .unwrap(),
    );
    let config = ProxyConfig {
        route_policy: RoutePolicy::Auto,
        h3: H3TransportLimits {
            idle_timeout: Duration::from_millis(100),
            ..H3TransportLimits::default()
        },
        ..ProxyConfig::default()
    };
    let hooks = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "decoded-prefix",
            Arc::new(DecodedPrefixFactory),
            InterceptorRequirement::Required,
        )],
        config.limits.hooks,
    );
    let certificates = Arc::new(
        CachedMitmCertificateResolver::new(
            proxy_ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )
        .unwrap(),
    );
    let components = ProxyComponents::new(hooks, certificates).with_content_policy(
        ContentPolicy::preserve_original_output(ContentLimits::default()),
    );
    let proxy = ProxyServer::bind_with_components(config, origin_trust, components)
        .await
        .unwrap();
    proxy
        .state
        .alt_svc
        .lock()
        .await
        .observe(
            Origin {
                host: "localhost".to_owned(),
                port: origin_addr.port(),
            },
            &format!("h3=\":{}\"; ma=60", origin_addr.port()),
            Instant::now(),
        )
        .unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    let mut evidence = proxy.subscribe_evidence();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let proxy_task = tokio::spawn(proxy.serve(async move {
        let _ = shutdown_rx.await;
    }));

    let transport = connect_tunnel(proxy_addr, "localhost", origin_addr.port()).await;
    let response = send_h1_browser_request(
        transport,
        origin_addr.port(),
        browser_trust,
        encoded_request,
        "GET",
    )
    .await;
    assert_eq!(decode_stack(&response).await, b"xresponse");
    let proof = evidence.recv().await.unwrap();
    assert_eq!(proof.egress_version, HttpLegVersion::Http2);
    assert_eq!(proof.route_attempts.len(), 2);
    assert_eq!(proof.route_attempts[0].protocol, HttpLegVersion::Http3);
    assert_eq!(proof.route_attempts[0].outcome, "failed-before-response");
    assert_eq!(proof.route_attempts[1].protocol, HttpLegVersion::Http2);
    assert_eq!(proof.route_attempts[1].outcome, "success");

    shutdown_tx.send(()).unwrap();
    proxy_task.await.unwrap().unwrap();
    origin_task.finish().await;
}

#[tokio::test]
async fn decoded_response_streams_before_the_next_encoded_member_arrives() {
    let first_sent = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = Arc::new(StreamingCompressedUpstream {
        first: encode_one(ContentCoding::Gzip, b"first").await,
        second: encode_one(ContentCoding::Gzip, b"second").await,
        first_sent: Arc::clone(&first_sent),
        release: Arc::clone(&release),
    });
    let (proxy, _) = bind_application_proxy(
        ProxyConfig::default(),
        ContentPolicy::inspect_to_identity(ContentLimits::default()),
        service,
    )
    .await;
    let proxy_addr = proxy.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let proxy_task = tokio::spawn(proxy.serve(async move {
        let _ = shutdown_rx.await;
    }));

    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    client
        .write_all(
            b"GET http://application.test/stream HTTP/1.1\r\nHost: application.test\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    first_sent.notified().await;
    let mut response = read_until_contains(&mut client, b"xfirst").await;
    assert!(!response.windows(6).any(|window| window == b"second"));
    release.notify_one();
    client.read_to_end(&mut response).await.unwrap();
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8_lossy(&response[..head_end]).to_ascii_lowercase();
    assert!(!head.contains("content-encoding:"));
    assert_eq!(http1_response_body(&response), b"xfirstsecond");

    shutdown_tx.send(()).unwrap();
    proxy_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn paused_codec_stream_does_not_block_an_unrelated_h2_stream() {
    let slow_entered = Arc::new(Notify::new());
    let release_slow = Arc::new(Notify::new());
    let service = Arc::new(MultiplexedCompressedUpstream {
        slow_body: encode_one(ContentCoding::Gzip, b"slow").await,
        fast_body: encode_one(ContentCoding::Gzip, b"fast").await,
        slow_entered: Arc::clone(&slow_entered),
        release_slow: Arc::clone(&release_slow),
    });
    let (proxy, browser_trust) = bind_application_proxy(
        ProxyConfig::default(),
        ContentPolicy::inspect_to_identity(ContentLimits::default()),
        service,
    )
    .await;
    let proxy_addr = proxy.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let proxy_task = tokio::spawn(proxy.serve(async move {
        let _ = shutdown_rx.await;
    }));
    let transport = connect_tunnel(proxy_addr, "application.test", 443).await;
    let browser_tls = UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
        .hyper_connector_builder(b"\x02h2")
        .unwrap()
        .build();
    let browser = tokio_boring::connect(
        browser_tls.configure().unwrap(),
        "application.test",
        transport,
    )
    .await
    .unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(browser))
            .await
            .unwrap();
    let connection_task = tokio::spawn(connection);

    let slow = sender
        .send_request(
            Request::builder()
                .uri("https://application.test/slow")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    slow_entered.notified().await;
    let fast = sender
        .send_request(
            Request::builder()
                .uri("https://application.test/fast")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    let fast_body = timeout(Duration::from_secs(1), fast.into_body().collect())
        .await
        .expect("fast decoded stream was blocked by the slow stream")
        .unwrap()
        .to_bytes();
    assert_eq!(fast_body, b"xfast".as_slice());

    release_slow.notify_one();
    let slow_body = slow.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(slow_body, b"xslow".as_slice());

    drop(sender);
    connection_task.abort();
    shutdown_tx.send(()).unwrap();
    proxy_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn paused_h3_codec_stream_does_not_block_an_unrelated_stream() {
    let origin_ca = ProxyCa::generate("rustymiddle multiplexed content h3 origin", 2).unwrap();
    let origin_leaf = origin_ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let bind_ip = tokio::net::lookup_host(("localhost", 0))
        .await
        .unwrap()
        .next()
        .unwrap()
        .ip();
    let release_slow = Arc::new(AtomicBool::new(false));
    let (origin_addr, origin_task) = spawn_multiplexed_content_h3_origin(
        origin_leaf,
        bind_ip,
        encode_one(ContentCoding::Gzip, b"slow").await,
        encode_one(ContentCoding::Gzip, b"fast").await,
        Arc::clone(&release_slow),
    )
    .await;
    let (proxy, browser_trust) = bind_h3_content_proxy(
        origin_ca.certificate().to_der().unwrap(),
        "rustymiddle multiplexed content h3 proxy",
    )
    .await;
    let proxy_addr = proxy.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let proxy_task = tokio::spawn(proxy.serve(async move {
        let _ = shutdown_rx.await;
    }));
    let transport = connect_tunnel(proxy_addr, "localhost", origin_addr.port()).await;
    let browser_tls = UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
        .hyper_connector_builder(b"\x02h2")
        .unwrap()
        .build();
    let browser = tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
        .await
        .unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(browser))
            .await
            .unwrap();
    let connection_task = tokio::spawn(connection);

    let slow = sender
        .send_request(
            Request::builder()
                .uri(format!("https://localhost:{}/slow", origin_addr.port()))
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    let fast = sender
        .send_request(
            Request::builder()
                .uri(format!("https://localhost:{}/fast", origin_addr.port()))
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    let fast_body = timeout(Duration::from_secs(1), fast.into_body().collect())
        .await
        .expect("fast H3 codec stream was blocked by the slow stream")
        .unwrap()
        .to_bytes();
    assert_eq!(fast_body, b"xfast".as_slice());

    release_slow.store(true, Ordering::SeqCst);
    let slow_body = timeout(Duration::from_secs(1), slow.into_body().collect())
        .await
        .expect("released H3 codec stream did not complete")
        .unwrap()
        .to_bytes();
    assert_eq!(slow_body, b"xslow".as_slice());

    drop(sender);
    connection_task.abort();
    shutdown_tx.send(()).unwrap();
    proxy_task.await.unwrap().unwrap();
    origin_task.finish().await;
}

#[tokio::test]
async fn decoded_limit_failure_terminates_the_stream_and_reaches_the_upstream() {
    let body_failed = Arc::new(AtomicBool::new(false));
    let limits = ContentLimits::new(
        NonZeroUsize::new(1024).unwrap(),
        NonZeroUsize::new(32).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
        NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
        NonZeroUsize::new(1000).unwrap(),
        4096,
        NonZeroUsize::new(4).unwrap(),
    );
    let service = Arc::new(BodyFailureUpstream {
        body_failed: Arc::clone(&body_failed),
    });
    let (proxy, _) = bind_application_proxy(
        ProxyConfig::default(),
        ContentPolicy::inspect_to_identity(limits),
        service,
    )
    .await;
    let proxy_addr = proxy.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let proxy_task = tokio::spawn(proxy.serve(async move {
        let _ = shutdown_rx.await;
    }));
    let encoded = encode_one(ContentCoding::Gzip, &[b'a'; 4096]).await;
    let mut request = format!(
        "POST http://application.test/limit HTTP/1.1\r\nHost: application.test\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        encoded.len()
    )
    .into_bytes();
    request.extend_from_slice(&encoded);
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    client.write_all(&request).await.unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 502"));
    assert!(body_failed.load(Ordering::SeqCst));

    shutdown_tx.send(()).unwrap();
    proxy_task.await.unwrap().unwrap();
}

async fn bind_application_proxy(
    config: ProxyConfig,
    policy: ContentPolicy,
    service: Arc<dyn UpstreamService>,
) -> (ProxyServer, Arc<TrustSnapshot>) {
    let trust_ca = ProxyCa::generate("rustymiddle application content trust", 2).unwrap();
    let trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![trust_ca.certificate().to_der().unwrap()]),
            200,
        )
        .unwrap(),
    );
    let proxy_ca = ProxyCa::generate("rustymiddle application content proxy", 2).unwrap();
    let browser_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
            1,
        )
        .unwrap(),
    );
    let hooks = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "decoded-prefix",
            Arc::new(DecodedPrefixFactory),
            InterceptorRequirement::Required,
        )],
        config.limits.hooks,
    );
    let certificates = Arc::new(
        CachedMitmCertificateResolver::new(
            proxy_ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )
        .unwrap(),
    );
    let components = ProxyComponents::new(hooks, certificates)
        .with_content_policy(policy)
        .with_upstream_service(service);
    let proxy = ProxyServer::bind_with_components(config, trust, components)
        .await
        .unwrap();
    (proxy, browser_trust)
}

async fn bind_h3_content_proxy(
    origin_root_der: Vec<u8>,
    proxy_ca_name: &str,
) -> (ProxyServer, Arc<TrustSnapshot>) {
    let origin_trust =
        Arc::new(TrustSnapshot::load(&StaticTrust(vec![origin_root_der]), 400).unwrap());
    let proxy_ca = ProxyCa::generate(proxy_ca_name, 2).unwrap();
    let browser_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
            1,
        )
        .unwrap(),
    );
    let config = ProxyConfig {
        route_policy: RoutePolicy::Http3Only,
        h3: H3TransportLimits {
            idle_timeout: Duration::from_secs(3),
            ..H3TransportLimits::default()
        },
        ..ProxyConfig::default()
    };
    let hooks = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "decoded-prefix",
            Arc::new(DecodedPrefixFactory),
            InterceptorRequirement::Required,
        )],
        config.limits.hooks,
    );
    let certificates = Arc::new(
        CachedMitmCertificateResolver::new(
            proxy_ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )
        .unwrap(),
    );
    let components = ProxyComponents::new(hooks, certificates)
        .with_content_policy(ContentPolicy::inspect_to_identity(ContentLimits::default()));
    let proxy = ProxyServer::bind_with_components(config, origin_trust, components)
        .await
        .unwrap();
    (proxy, browser_trust)
}

async fn connect_tunnel(proxy_addr: SocketAddr, target_host: &str, target_port: u16) -> TcpStream {
    let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
    transport
        .write_all(
            format!(
                "CONNECT {target_host}:{target_port} HTTP/1.1\r\nHost: {target_host}:{target_port}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert!(
        read_http_head(&mut transport)
            .await
            .starts_with(b"HTTP/1.1 200")
    );
    transport
}

async fn read_until_contains<S>(stream: &mut S, pattern: &[u8]) -> Vec<u8>
where
    S: AsyncRead + Unpin,
{
    timeout(Duration::from_secs(2), async {
        let mut result = Vec::new();
        let mut byte = [0_u8; 1];
        while !result
            .windows(pattern.len())
            .any(|window| window == pattern)
        {
            let read = stream.read(&mut byte).await.unwrap();
            assert_ne!(read, 0, "connection ended before expected body bytes");
            result.push(byte[0]);
        }
        result
    })
    .await
    .expect("streaming response did not expose its first decoded member")
}

async fn run_matrix_case(
    ingress: HttpLegVersion,
    egress: HttpLegVersion,
    trust_generation: u64,
    encoded_request: Bytes,
    encoded_response: Bytes,
) {
    let origin_ca = ProxyCa::generate("rustymiddle content matrix origin", 2).unwrap();
    let origin_leaf = origin_ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let bind_ip = if egress == HttpLegVersion::Http3 {
        tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip()
    } else {
        IpAddr::from([127, 0, 0, 1])
    };
    let (origin_addr, origin_task) =
        spawn_content_origin(egress, origin_leaf, bind_ip, encoded_response).await;
    let origin_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
            trust_generation,
        )
        .unwrap(),
    );

    let proxy_ca = ProxyCa::generate("rustymiddle content matrix proxy", 2).unwrap();
    let browser_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![proxy_ca.certificate().to_der().unwrap()]),
            1,
        )
        .unwrap(),
    );
    let config = ProxyConfig {
        route_policy: match egress {
            HttpLegVersion::Http1 => RoutePolicy::Http1Only,
            HttpLegVersion::Http2 => RoutePolicy::Http2Only,
            HttpLegVersion::Http3 => RoutePolicy::Http3Only,
        },
        h3: H3TransportLimits {
            idle_timeout: Duration::from_secs(3),
            ..H3TransportLimits::default()
        },
        ..ProxyConfig::default()
    };
    let hooks = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "decoded-prefix",
            Arc::new(DecodedPrefixFactory),
            InterceptorRequirement::Required,
        )],
        config.limits.hooks,
    );
    let certificates = Arc::new(
        CachedMitmCertificateResolver::new(
            proxy_ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )
        .unwrap(),
    );
    let components = ProxyComponents::new(hooks, certificates).with_content_policy(
        ContentPolicy::preserve_original_output(ContentLimits::default()),
    );
    let proxy = ProxyServer::bind_with_components(config, origin_trust, components)
        .await
        .unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    let mut evidence = proxy.subscribe_evidence();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let proxy_task = tokio::spawn(proxy.serve(async move {
        let _ = shutdown_rx.await;
    }));

    let response = send_browser_request(
        ingress,
        proxy_addr,
        origin_addr.port(),
        browser_trust,
        encoded_request,
    )
    .await;
    assert_eq!(
        decode_stack(&response).await,
        b"xresponse",
        "{ingress:?}/{egress:?}"
    );

    let proof = evidence.recv().await.unwrap();
    assert_eq!(proof.ingress_version, ingress);
    assert_eq!(proof.egress_version, egress);
    assert_eq!(proof.trust_generation, trust_generation);

    shutdown_tx.send(()).unwrap();
    proxy_task.await.unwrap().unwrap();
    origin_task.finish().await;
}

async fn spawn_content_origin(
    version: HttpLegVersion,
    leaf: rustymiddle_tls::IssuedLeaf,
    bind_ip: IpAddr,
    response: Bytes,
) -> (SocketAddr, TestOrigin) {
    match version {
        HttpLegVersion::Http1 | HttpLegVersion::Http2 => {
            spawn_content_tls_origin(version, leaf, response).await
        }
        HttpLegVersion::Http3 => spawn_content_h3_origin(leaf, bind_ip, response).await,
    }
}

async fn spawn_content_tls_origin(
    version: HttpLegVersion,
    leaf: rustymiddle_tls::IssuedLeaf,
    response_body: Bytes,
) -> (SocketAddr, TestOrigin) {
    let acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
        .acceptor(&leaf)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let stream = tokio_boring::accept(&acceptor, stream).await.unwrap();
        let service = service_fn(move |request: Request<Incoming>| {
            let response_body = response_body.clone();
            async move {
                assert_eq!(request.version(), http_version(version));
                assert_coded_headers(request.headers());
                let request_body = request.into_body().collect().await.unwrap().to_bytes();
                assert_eq!(decode_stack(&request_body).await, b"xrequest");
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(200)
                        .header("content-encoding", CODING_HEADER)
                        .header("content-length", response_body.len())
                        .header("etag", "\"stale\"")
                        .body(Full::new(response_body))
                        .unwrap(),
                )
            }
        });
        match version {
            HttpLegVersion::Http1 => hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap(),
            HttpLegVersion::Http2 => hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap(),
            HttpLegVersion::Http3 => unreachable!("HTTP/3 uses the QUIC fixture"),
        }
    });
    (address, TestOrigin { socket: None, task })
}

fn http_version(version: HttpLegVersion) -> Version {
    match version {
        HttpLegVersion::Http1 => Version::HTTP_11,
        HttpLegVersion::Http2 => Version::HTTP_2,
        HttpLegVersion::Http3 => Version::HTTP_3,
    }
}

fn assert_coded_headers(headers: &http::HeaderMap) {
    assert_eq!(headers["content-encoding"], CODING_HEADER);
    assert!(!headers.contains_key("content-length"));
    assert!(!headers.contains_key("etag"));
}

async fn send_browser_request(
    ingress: HttpLegVersion,
    proxy_addr: SocketAddr,
    origin_port: u16,
    browser_trust: Arc<TrustSnapshot>,
    encoded_request: Bytes,
) -> Bytes {
    let mut transport = TcpStream::connect(proxy_addr).await.unwrap();
    transport
        .write_all(
            format!(
                "CONNECT localhost:{origin_port} HTTP/1.1\r\nHost: localhost:{origin_port}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert!(
        read_http_head(&mut transport)
            .await
            .starts_with(b"HTTP/1.1 200")
    );

    match ingress {
        HttpLegVersion::Http1 => {
            send_h1_browser_request(
                transport,
                origin_port,
                browser_trust,
                encoded_request,
                "POST",
            )
            .await
        }
        HttpLegVersion::Http2 => {
            send_h2_browser_request(transport, origin_port, browser_trust, encoded_request).await
        }
        HttpLegVersion::Http3 => unreachable!("native downstream HTTP/3 is not supported"),
    }
}

async fn send_h1_browser_request(
    transport: TcpStream,
    origin_port: u16,
    browser_trust: Arc<TrustSnapshot>,
    encoded_request: Bytes,
    method: &str,
) -> Bytes {
    let browser_tls = UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
        .hyper_connector_builder(b"\x08http/1.1")
        .unwrap()
        .build();
    let mut browser =
        tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
            .await
            .unwrap();
    let mut request = format!(
        "{method} /content HTTP/1.1\r\nHost: localhost:{origin_port}\r\nContent-Encoding: {CODING_HEADER}\r\nContent-Length: {}\r\nETag: \"stale\"\r\nConnection: close\r\n\r\n",
        encoded_request.len()
    )
    .into_bytes();
    request.extend_from_slice(&encoded_request);
    browser.write_all(&request).await.unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), browser.read_to_end(&mut response))
        .await
        .expect("HTTP/1 content-matrix response timed out")
        .unwrap();
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8_lossy(&response[..head_end]).to_ascii_lowercase();
    assert!(head.contains("content-encoding: gzip, br, deflate, zstd"));
    assert!(!head.contains("etag:"));
    assert!(!head.contains("content-length:"));
    Bytes::from(http1_response_body(&response))
}

async fn send_h2_browser_request(
    transport: TcpStream,
    origin_port: u16,
    browser_trust: Arc<TrustSnapshot>,
    encoded_request: Bytes,
) -> Bytes {
    let browser_tls = UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
        .hyper_connector_builder(b"\x02h2")
        .unwrap()
        .build();
    let browser = tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", transport)
        .await
        .unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(browser))
            .await
            .unwrap();
    let connection_task = tokio::spawn(connection);
    let response = sender
        .send_request(
            Request::builder()
                .method("POST")
                .uri(format!("https://localhost:{origin_port}/content"))
                .header("content-encoding", CODING_HEADER)
                .header("content-length", encoded_request.len())
                .header("etag", "\"stale\"")
                .body(Full::new(encoded_request))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-encoding"], CODING_HEADER);
    assert!(!response.headers().contains_key("content-length"));
    assert!(!response.headers().contains_key("etag"));
    let body = response.into_body().collect().await.unwrap().to_bytes();
    drop(sender);
    connection_task.abort();
    body
}

async fn encode_stack(input: &[u8]) -> Bytes {
    let mut output = Bytes::copy_from_slice(input);
    for coding in CODINGS {
        output = encode_one(coding, &output).await;
    }
    output
}

async fn encode_one(coding: ContentCoding, input: &[u8]) -> Bytes {
    let mut encoder = ContentEncoder::new(coding, ContentLimits::default()).unwrap();
    let mut frames = encoder
        .on_frame(BodyFrame::Data(Bytes::copy_from_slice(input)))
        .await
        .unwrap();
    frames.extend(encoder.finish().await.unwrap());
    Bytes::from(frame_data(frames))
}

async fn decode_stack(input: &[u8]) -> Vec<u8> {
    let mut output = Bytes::copy_from_slice(input);
    for coding in CODINGS.into_iter().rev() {
        let mut decoder = ContentDecoder::new(coding, ContentLimits::default()).unwrap();
        let mut frames = decoder.on_frame(BodyFrame::Data(output)).await.unwrap();
        frames.extend(decoder.finish().await.unwrap());
        output = Bytes::from(frame_data(frames));
    }
    output.to_vec()
}

fn frame_data(frames: Vec<BodyFrame>) -> Vec<u8> {
    frames
        .into_iter()
        .filter_map(|frame| match frame {
            BodyFrame::Data(data) => Some(data),
            BodyFrame::Trailers(_) => None,
        })
        .flatten()
        .collect()
}

async fn read_http_head<S>(stream: &mut S) -> Vec<u8>
where
    S: AsyncRead + Unpin,
{
    let mut result = Vec::new();
    let mut byte = [0_u8; 1];
    while !result.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await.unwrap();
        assert_ne!(read, 0, "connection ended before HTTP head completed");
        result.push(byte[0]);
    }
    result
}

fn http1_response_body(response: &[u8]) -> Vec<u8> {
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let body = &response[head_end + 4..];
    let head = String::from_utf8_lossy(&response[..head_end]).to_ascii_lowercase();
    if !head.contains("transfer-encoding: chunked") {
        return body.to_vec();
    }

    let mut decoded = Vec::new();
    let mut cursor = 0;
    loop {
        let size_end = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| cursor + offset)
            .unwrap();
        let size_text = std::str::from_utf8(&body[cursor..size_end]).unwrap();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap(), 16).unwrap();
        cursor = size_end + 2;
        if size == 0 {
            break;
        }
        decoded.extend_from_slice(&body[cursor..cursor + size]);
        cursor += size + 2;
    }
    decoded
}

async fn spawn_content_h3_origin(
    leaf: rustymiddle_tls::IssuedLeaf,
    bind_ip: IpAddr,
    response: Bytes,
) -> (SocketAddr, TestOrigin) {
    let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
    tls.set_certificate(&leaf.certificate).unwrap();
    tls.set_private_key(&leaf.private_key).unwrap();
    tls.check_private_key().unwrap();
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(3_000);
    config.set_max_recv_udp_payload_size(1_350);
    config.set_max_send_udp_payload_size(1_350);
    config.set_initial_max_data(1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(256 * 1024);
    config.set_initial_max_stream_data_bidi_remote(256 * 1024);
    config.set_initial_max_stream_data_uni(256 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    config.set_disable_active_migration(true);
    let socket = Arc::new(UdpSocket::bind((bind_ip, 0)).await.unwrap());
    let local = socket.local_addr().unwrap();
    let task = tokio::spawn(run_content_h3_origin(
        Arc::clone(&socket),
        local,
        config,
        response,
    ));
    (
        local,
        TestOrigin {
            socket: Some(socket),
            task,
        },
    )
}

async fn spawn_multiplexed_content_h3_origin(
    leaf: rustymiddle_tls::IssuedLeaf,
    bind_ip: IpAddr,
    slow_body: Bytes,
    fast_body: Bytes,
    release_slow: Arc<AtomicBool>,
) -> (SocketAddr, TestOrigin) {
    let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
    tls.set_certificate(&leaf.certificate).unwrap();
    tls.set_private_key(&leaf.private_key).unwrap();
    tls.check_private_key().unwrap();
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(3_000);
    config.set_max_recv_udp_payload_size(1_350);
    config.set_max_send_udp_payload_size(1_350);
    config.set_initial_max_data(1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(256 * 1024);
    config.set_initial_max_stream_data_bidi_remote(256 * 1024);
    config.set_initial_max_stream_data_uni(256 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    config.set_disable_active_migration(true);
    let socket = Arc::new(UdpSocket::bind((bind_ip, 0)).await.unwrap());
    let local = socket.local_addr().unwrap();
    let task = tokio::spawn(run_multiplexed_content_h3_origin(
        Arc::clone(&socket),
        local,
        config,
        slow_body,
        fast_body,
        release_slow,
    ));
    (
        local,
        TestOrigin {
            socket: Some(socket),
            task,
        },
    )
}

async fn run_multiplexed_content_h3_origin(
    socket: Arc<UdpSocket>,
    local: SocketAddr,
    mut config: quiche::Config,
    slow_body: Bytes,
    fast_body: Bytes,
    release_slow: Arc<AtomicBool>,
) {
    let mut connection = None;
    let h3_config = quiche::h3::Config::new().unwrap();
    let mut http3: Option<quiche::h3::Connection> = None;
    let mut slow_stream: Option<u64> = None;
    let mut fast_sent = false;
    let mut slow_sent = false;
    let mut received = vec![0_u8; 65_535];
    let mut outgoing = vec![0_u8; 1_350];
    loop {
        if fast_sent && !slow_sent && release_slow.load(Ordering::SeqCst) {
            let connection = connection.as_mut().unwrap();
            let http3 = http3.as_mut().unwrap();
            let stream_id = slow_stream.expect("slow H3 request was not received");
            http3
                .send_body(connection, stream_id, &slow_body, true)
                .unwrap();
            slow_sent = true;
        }

        if let Some(connection) = connection.as_mut() {
            loop {
                match connection.send(&mut outgoing) {
                    Ok((written, send_info)) => {
                        socket
                            .send_to(&outgoing[..written], send_info.to)
                            .await
                            .unwrap();
                    }
                    Err(quiche::Error::Done) => break,
                    Err(error) => panic!("multiplexed H3 content fixture send failed: {error}"),
                }
            }
        }
        if slow_sent {
            return;
        }

        let packet = timeout(Duration::from_millis(25), socket.recv_from(&mut received)).await;
        let Ok(result) = packet else {
            if let Some(connection) = connection.as_mut()
                && connection
                    .timeout()
                    .is_some_and(|remaining| remaining.is_zero())
            {
                connection.on_timeout();
            }
            continue;
        };
        let (read, from) = result.unwrap();
        if connection.is_none() {
            let header =
                quiche::Header::from_slice(&mut received[..read], quiche::MAX_CONN_ID_LEN).unwrap();
            assert_eq!(header.ty, quiche::Type::Initial);
            let mut source_id = [0_u8; quiche::MAX_CONN_ID_LEN];
            rand_bytes(&mut source_id).unwrap();
            let source_id = quiche::ConnectionId::from_ref(&source_id);
            connection = Some(quiche::accept(&source_id, None, local, from, &mut config).unwrap());
        }
        let connection = connection.as_mut().unwrap();
        match connection.recv(&mut received[..read], quiche::RecvInfo { from, to: local }) {
            Ok(_) | Err(quiche::Error::Done) => {}
            Err(error) => panic!("multiplexed H3 content fixture recv failed: {error}"),
        }
        if connection.is_established() && http3.is_none() {
            http3 = Some(quiche::h3::Connection::with_transport(connection, &h3_config).unwrap());
        }
        if let Some(http3) = http3.as_mut() {
            poll_multiplexed_content_h3(
                http3,
                connection,
                &mut received,
                &fast_body,
                &mut slow_stream,
                &mut fast_sent,
            );
        }
    }
}

fn poll_multiplexed_content_h3(
    http3: &mut quiche::h3::Connection,
    connection: &mut quiche::Connection,
    received: &mut [u8],
    fast_body: &[u8],
    slow_stream: &mut Option<u64>,
    fast_sent: &mut bool,
) {
    loop {
        match http3.poll(connection) {
            Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                let path = list
                    .iter()
                    .find(|header| header.name() == b":path")
                    .map(NameValue::value)
                    .expect("H3 request path missing");
                let headers = [
                    quiche::h3::Header::new(b":status", b"200"),
                    quiche::h3::Header::new(b"content-encoding", b"gzip"),
                ];
                http3
                    .send_response(connection, stream_id, &headers, false)
                    .unwrap();
                match path {
                    b"/slow" => *slow_stream = Some(stream_id),
                    b"/fast" => {
                        http3
                            .send_body(connection, stream_id, fast_body, true)
                            .unwrap();
                        *fast_sent = true;
                    }
                    _ => panic!("unexpected multiplexed H3 request path"),
                }
            }
            Ok((stream_id, quiche::h3::Event::Data)) => loop {
                match http3.recv_body(connection, stream_id, received) {
                    Ok(_) => {}
                    Err(quiche::h3::Error::Done) => break,
                    Err(error) => panic!("multiplexed H3 content body failed: {error}"),
                }
            },
            Ok((_, _)) => {}
            Err(quiche::h3::Error::Done) => break,
            Err(error) => panic!("multiplexed H3 content fixture poll failed: {error}"),
        }
    }
}

async fn run_content_h3_origin(
    socket: Arc<UdpSocket>,
    local: SocketAddr,
    mut config: quiche::Config,
    response: Bytes,
) {
    let mut connection = None;
    let h3_config = quiche::h3::Config::new().unwrap();
    let mut http3 = None;
    let mut request_body = Vec::new();
    let mut received = vec![0_u8; 65_535];
    let mut outgoing = vec![0_u8; 1_350];
    loop {
        let (read, from) = timeout(Duration::from_secs(5), socket.recv_from(&mut received))
            .await
            .expect("proxy H3 content client stalled")
            .unwrap();
        if connection.is_none() {
            let header =
                quiche::Header::from_slice(&mut received[..read], quiche::MAX_CONN_ID_LEN).unwrap();
            assert_eq!(header.ty, quiche::Type::Initial);
            let mut source_id = [0_u8; quiche::MAX_CONN_ID_LEN];
            rand_bytes(&mut source_id).unwrap();
            let source_id = quiche::ConnectionId::from_ref(&source_id);
            connection = Some(quiche::accept(&source_id, None, local, from, &mut config).unwrap());
        }
        let connection = connection.as_mut().unwrap();
        match connection.recv(&mut received[..read], quiche::RecvInfo { from, to: local }) {
            Ok(_) | Err(quiche::Error::Done) => {}
            Err(error) => panic!("proxy H3 content fixture recv failed: {error}"),
        }
        if connection.is_established() && http3.is_none() {
            http3 = Some(quiche::h3::Connection::with_transport(connection, &h3_config).unwrap());
        }
        let mut response_sent = false;
        if let Some(http3) = http3.as_mut() {
            loop {
                match http3.poll(connection) {
                    Ok((_, quiche::h3::Event::Headers { list, .. })) => {
                        assert!(list.iter().any(|header| {
                            header.name() == b"content-encoding"
                                && header.value() == CODING_HEADER.as_bytes()
                        }));
                        assert!(!list.iter().any(|header| {
                            matches!(header.name(), b"content-length" | b"etag")
                        }));
                    }
                    Ok((stream_id, quiche::h3::Event::Data)) => loop {
                        match http3.recv_body(connection, stream_id, &mut received) {
                            Ok(read) => request_body.extend_from_slice(&received[..read]),
                            Err(quiche::h3::Error::Done) => break,
                            Err(error) => panic!("proxy H3 content body failed: {error}"),
                        }
                    },
                    Ok((stream_id, quiche::h3::Event::Finished)) => {
                        assert_eq!(decode_stack(&request_body).await, b"xrequest");
                        let content_length = response.len().to_string();
                        let headers = [
                            quiche::h3::Header::new(b":status", b"200"),
                            quiche::h3::Header::new(b"content-encoding", CODING_HEADER.as_bytes()),
                            quiche::h3::Header::new(b"content-length", content_length.as_bytes()),
                            quiche::h3::Header::new(b"etag", b"\"stale\""),
                        ];
                        http3
                            .send_response(connection, stream_id, &headers, false)
                            .unwrap();
                        http3
                            .send_body(connection, stream_id, &response, true)
                            .unwrap();
                        response_sent = true;
                    }
                    Ok((_, _)) => {}
                    Err(quiche::h3::Error::Done) => break,
                    Err(error) => panic!("proxy H3 content fixture poll failed: {error}"),
                }
            }
        }
        loop {
            match connection.send(&mut outgoing) {
                Ok((written, send_info)) => {
                    socket
                        .send_to(&outgoing[..written], send_info.to)
                        .await
                        .unwrap();
                }
                Err(quiche::Error::Done) => break,
                Err(error) => panic!("proxy H3 content fixture send failed: {error}"),
            }
        }
        if response_sent {
            return;
        }
    }
}

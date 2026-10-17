use super::*;
use std::sync::Mutex as StdMutex;
use tokio::{
    io::AsyncWriteExt,
    sync::{Notify, oneshot},
};
use transmog_core::{
    intercept::{
        BoxHookFuture, ExchangeInterceptor, HookInitError, RequestHeadAction, RequestHeadEvent,
    },
    observe::{BoxObserverFuture, ObserverEvent},
};
use transmog_tls::{LoadedTrust, TrustError, TrustSource};

struct TestTrust(Vec<u8>);
impl TrustSource for TestTrust {
    fn load(&self) -> Result<LoadedTrust, TrustError> {
        Ok(LoadedTrust {
            certificates_der: vec![self.0.clone()],
            source_description: "cancellation test".into(),
            source_version: None,
            diagnostics: Vec::new(),
        })
    }
}

#[derive(Clone)]
struct CancellationHook {
    pause_head: bool,
    blocked: Arc<Notify>,
    failed: Arc<Notify>,
    release_cleanup: Arc<Notify>,
    failures: Arc<StdMutex<Vec<ExchangeFailure>>>,
}
impl InterceptorFactory for CancellationHook {
    fn create(&self, _: &ExchangeMetadata) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(self.clone()))
    }
}
impl ExchangeInterceptor for CancellationHook {
    fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        Box::pin(async move {
            if self.pause_head && event.head.target.path == "/speculative" {
                self.blocked.notify_one();
                std::future::pending::<()>().await;
            }
            RequestHeadAction::Continue
        })
    }
    fn on_failed(&self, failure: ExchangeFailure) -> BoxHookFuture<'_, ()> {
        Box::pin(async move {
            self.failures.lock().unwrap().push(failure);
            self.failed.notify_one();
            self.release_cleanup.notified().await;
        })
    }
}

#[derive(Default)]
struct Events(StdMutex<Vec<ObserverEvent>>);
impl Observer for Events {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        self.0.lock().unwrap().push(event);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep cancellation, drain and terminal evidence in one flow.
async fn cancellation_keeps_activity_until_already_started_terminal_hooks_finish() {
    let hook = CancellationHook {
        pause_head: false,
        blocked: Arc::new(Notify::new()),
        failed: Arc::new(Notify::new()),
        release_cleanup: Arc::new(Notify::new()),
        failures: Arc::default(),
    };
    let factory = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "terminal",
            Arc::new(hook.clone()),
            InterceptorRequirement::Required,
        )],
        transmog_core::intercept::HookLimits::default(),
    );
    let target = Target {
        scheme: "https".into(),
        authority: "example.test".into(),
        host: "example.test".into(),
        port: 443,
        path: "/".into(),
        query: None,
    };
    let metadata = Arc::new(ExchangeMetadata::from_session(
        &SessionMetadata {
            session_id: SessionId(1),
            downstream_connection_id: ConnectionId(1),
            stream_id: StreamId(1),
            client_addr: "127.0.0.1:1000".parse().unwrap(),
            proxy_addr: "127.0.0.1:2000".parse().unwrap(),
            client_identity: transmog_core::ClientIdentity::default(),
            ingress_version: HttpLegVersion::Http2,
            egress_version: None,
        },
        target.clone(),
    ));
    let events = Arc::new(Events::default());
    let hub = ObserverHub::new(vec![(events.clone(), ObserverConfig::default())]);
    let observer = Arc::new(hub.start_exchange(metadata.clone()));
    observer
        .emit(ObserverEventKind::ExchangeStarted {
            metadata: metadata.clone(),
        })
        .await;
    let performance = PerformanceRecorder::new(std::time::SystemTime::now(), Instant::now());
    performance.mark(Milestone::RequestHeaders);
    let activity = Arc::new(ActivityTracker::default());
    let cleanup = Arc::new(StdMutex::new(JoinSet::new()));
    let mut cancellation = RequestCancellation::new(
        activity.acquire(ActivityKind::Request).unwrap(),
        ExchangeCancellation::new(),
        cleanup.clone(),
    );
    cancellation.start(observer, metadata.clone(), performance);
    cancellation.initializing_chain = Some(factory.create_exchange((*metadata).clone()).unwrap());
    cancellation
        .initializing_chain
        .as_mut()
        .unwrap()
        .request_head(RequestHead {
            method: "GET".into(),
            target,
            headers: HeaderBlock::default(),
            source_version: HttpLegVersion::Http2,
        })
        .await
        .unwrap();
    let chain = cancellation.share_chain();
    let waiter = tokio::spawn(async move {
        chain
            .failed(ExchangeFailure {
                metadata,
                stage: ExchangeStage::Upstream,
                kind: ExchangeFailureKind::Upstream,
                request_committed: false,
                response_committed: false,
                message: "Upstream unavailable".into(),
            })
            .await
    });
    hook.failed.notified().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    drop(cancellation);
    let generation = activity.drain();
    let mut drained = Box::pin(activity.idle(generation));
    assert!(
        timeout(Duration::from_millis(50), &mut drained)
            .await
            .is_err(),
        "cancellation reported idle while terminal cleanup was still running"
    );
    hook.release_cleanup.notify_one();
    assert!(timeout(Duration::from_secs(1), drained).await.unwrap());
    let mut cleanup = std::mem::take(&mut *cleanup.lock().unwrap());
    while cleanup.join_next().await.is_some() {}
    hub.shutdown().await;
    assert_eq!(hook.failures.lock().unwrap().len(), 1);
    assert_eq!(
        events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event.kind, ObserverEventKind::Failed(_)))
            .count(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn h2_reset_records_cancellation_and_preserves_the_next_navigation() {
    for (pause_head, stop_proxy) in [(false, false), (true, false), (false, true)] {
        let blocked = Arc::new(Notify::new());
        let origin_ca = ProxyCa::generate("Cancellation origin", 2).unwrap();
        let leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
            .acceptor(&leaf)
            .unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let origin_blocked = blocked.clone();
        let origin_task = tokio::spawn(async move {
            let (socket, _) = origin.accept().await.unwrap();
            let tls = tokio_boring::accept(&acceptor, socket).await.unwrap();
            let service = service_fn(move |request: Request<Incoming>| {
                let blocked = origin_blocked.clone();
                async move {
                    if request.uri().path() == "/speculative" {
                        blocked.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
                        Bytes::from_static(b"navigation"),
                    )))
                }
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(tls), service)
                .await;
        });
        let trust = Arc::new(
            TrustSnapshot::load(&TestTrust(origin_ca.certificate().to_der().unwrap()), 1).unwrap(),
        );
        let proxy_ca = ProxyCa::generate("Cancellation proxy", 2).unwrap();
        let browser_trust = Arc::new(
            TrustSnapshot::load(&TestTrust(proxy_ca.certificate().to_der().unwrap()), 1).unwrap(),
        );
        let hook = CancellationHook {
            pause_head,
            blocked,
            failed: Arc::new(Notify::new()),
            release_cleanup: Arc::new(Notify::new()),
            failures: Arc::default(),
        };
        let config = ProxyConfig {
            route_policy: RoutePolicy::Http2Only,
            limits: RuntimeLimits {
                shutdown_timeout: Duration::from_millis(50),
                ..RuntimeLimits::default()
            },
            ..ProxyConfig::default()
        };
        let components = ProxyComponents::new(
            InterceptorChainFactory::new(
                vec![InterceptorRegistration::new(
                    "cancellation",
                    Arc::new(hook.clone()),
                    InterceptorRequirement::Required,
                )],
                config.limits.hooks,
            ),
            Arc::new(
                CachedMitmCertificateResolver::new(
                    proxy_ca,
                    config.limits.leaf_cache_capacity,
                    config.limits.leaf_validity_days,
                )
                .unwrap(),
            ),
        );
        let events = Arc::new(Events::default());
        let components = components.with_observer(
            events.clone(),
            ObserverConfig {
                delivery: transmog_core::observe::ObserverDeliveryPolicy::WaitForCapacity,
                callback_timeout: None,
                ..ObserverConfig::default()
            },
        );
        let proxy = ProxyServer::bind_with_components(config, trust, components)
            .await
            .unwrap();
        let control = proxy.control();
        let address = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let mut shutdown_tx = Some(shutdown_tx);
        let server = tokio::spawn(proxy.serve(async {
            let _ = shutdown_rx.await;
        }));
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket
            .write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    origin_addr.port(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
        }
        assert!(head.starts_with(b"HTTP/1.1 200"));
        let browser_tls =
            UpstreamTlsContextFactory::new(browser_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x02h2")
                .unwrap()
                .build();
        let tls = tokio_boring::connect(browser_tls.configure().unwrap(), "localhost", socket)
            .await
            .unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
                .await
                .unwrap();
        let browser = tokio::spawn(connection);
        let request = |path| {
            Request::builder()
                .uri(format!("https://localhost:{}{path}", origin_addr.port()))
                .body(http_body_util::Empty::<Bytes>::new())
                .unwrap()
        };
        let mut pending = tokio::spawn(sender.send_request(request("/speculative")));
        timeout(Duration::from_secs(3), hook.blocked.notified())
            .await
            .expect("speculative request never blocked");
        if stop_proxy {
            shutdown_tx.take().unwrap().send(()).unwrap();
        } else {
            pending.abort(); // Hyper sends RST_STREAM while preserving the H2 connection.
            assert!((&mut pending).await.unwrap_err().is_cancelled());
        }
        timeout(Duration::from_secs(3), hook.failed.notified())
            .await
            .expect("reset did not run terminal hook cleanup");
        assert_eq!(
            control.activity().requests,
            1,
            "cleanup must remain tracked until evidence is queued"
        );
        if !stop_proxy {
            let response = timeout(
                Duration::from_secs(3),
                sender.send_request(request("/navigation")),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.into_body().collect().await.unwrap().to_bytes(),
                "navigation"
            );
        }
        hook.release_cleanup.notify_one();
        if let Some(shutdown_tx) = shutdown_tx {
            let generation = control.begin_drain();
            assert!(
                timeout(Duration::from_secs(3), control.wait_for_drain(generation))
                    .await
                    .unwrap()
            );
            shutdown_tx.send(()).unwrap();
        }
        timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let failures = hook.failures.lock().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].kind,
            if stop_proxy {
                ExchangeFailureKind::Shutdown
            } else {
                ExchangeFailureKind::Cancelled
            }
        );
        assert_eq!(failures[0].request_committed, !pause_head);
        assert!(!failures[0].response_committed);
        let events = events.0.lock().unwrap();
        let failed_id = failures[0].metadata.exchange_id;
        let request_events = events
            .iter()
            .filter(|event| event.exchange_id == failed_id)
            .collect::<Vec<_>>();
        assert_eq!(
            request_events
                .iter()
                .filter(|event| matches!(event.kind, ObserverEventKind::Failed(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind, ObserverEventKind::Completed(_)))
                .count(),
            usize::from(!stop_proxy)
        );
        assert!(
            request_events
                .windows(2)
                .all(|pair| pair[1].sequence == pair[0].sequence + 1)
        );
        assert!(request_events.iter().any(|event| matches!(&event.kind, ObserverEventKind::Performance(evidence) if evidence.elapsed_micros().is_some())));
        browser.abort();
        pending.abort();
        origin_task.abort();
    }
}

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
    for (pause_head, stop_proxy, manual_cancel) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (true, false, true),
    ] {
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
        } else if manual_cancel {
            let id = events
                .0
                .lock()
                .unwrap()
                .iter()
                .find_map(|event| {
                    matches!(event.kind, ObserverEventKind::ExchangeStarted { .. })
                        .then_some(event.exchange_id)
                })
                .expect("active request identity");
            assert_eq!(control.cancel_requests(&[id]).await, vec![id]);
            let response = timeout(Duration::from_secs(3), &mut pending)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(response.status().as_u16(), 499);
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn manual_cancel_stops_an_idle_download_after_headers_and_retains_terminal_evidence() {
    use tokio::io::AsyncReadExt;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        let (mut socket, _) = origin.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nContent-Type: application/octet-stream\r\n\r\npart").await.unwrap();
        socket.flush().await.unwrap();
        // The origin deliberately leaves the download idle. Cancel must not wait
        // for the body idle timeout or for another origin chunk.
        let mut byte = [0_u8];
        let _ = socket.read(&mut byte).await;
    });
    let ca = ProxyCa::generate("Manual download cancellation", 2).unwrap();
    let trust =
        Arc::new(TrustSnapshot::load(&TestTrust(ca.certificate().to_der().unwrap()), 1).unwrap());
    let config = ProxyConfig::default();
    let certificates = Arc::new(
        CachedMitmCertificateResolver::new(
            ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )
        .unwrap(),
    );
    let events = Arc::new(Events::default());
    let components = ProxyComponents::new(
        InterceptorChainFactory::new(Vec::new(), config.limits.hooks),
        certificates,
    )
    .with_observer(events.clone(), ObserverConfig::default());
    let proxy = ProxyServer::bind_with_components(config, trust, components)
        .await
        .unwrap();
    let control = proxy.control();
    let address = proxy.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(proxy.serve(async {
        let _ = shutdown_rx.await;
    }));
    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(
            format!("GET http://{origin_addr}/download HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(
            timeout(Duration::from_secs(3), client.read_u8())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(head.starts_with(b"HTTP/1.1 200"));
    let mut bytes = [0_u8; 4];
    timeout(Duration::from_secs(3), client.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"part");
    let id = events
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|event| {
            matches!(event.kind, ObserverEventKind::ExchangeStarted { .. })
                .then_some(event.exchange_id)
        })
        .unwrap();
    assert_eq!(control.cancel_requests(&[id]).await, vec![id]);
    timeout(Duration::from_secs(3), async {
        loop {
            if events.0.lock().unwrap().iter().any(|event| event.exchange_id == id && matches!(&event.kind, ObserverEventKind::Failed(failure) if failure.kind == ExchangeFailureKind::Cancelled)) { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("manual cancellation did not retain terminal evidence");
    let mut remainder = Vec::new();
    timeout(Duration::from_secs(3), client.read_to_end(&mut remainder))
        .await
        .expect("download did not stop")
        .unwrap();
    assert!(remainder.len() < 1_000_000);
    assert_eq!(
        control.cancel_requests(&[id]).await,
        [] as [transmog_core::intercept::ExchangeId; 0]
    );
    shutdown_tx.send(()).unwrap();
    timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(3), origin_task)
        .await
        .expect("canceled download kept its origin connection open")
        .unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Compare admission and relay cancellation with surviving connections.
async fn manual_cancel_stops_only_the_selected_opaque_connect_and_retains_evidence() {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tokio::io::AsyncReadExt;

    struct TunnelEvents {
        events: Arc<Events>,
        pause_first: AtomicBool,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }
    impl Observer for TunnelEvents {
        fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
            let pause = matches!(event.kind, ObserverEventKind::ExchangeStarted { .. })
                && self.pause_first.swap(false, Ordering::Relaxed);
            self.events.0.lock().unwrap().push(event);
            Box::pin(async move {
                if pause {
                    self.started.notify_one();
                    self.release.notified().await;
                }
                Ok(())
            })
        }
    }

    async fn open_tunnel(address: SocketAddr, destination: SocketAddr) -> TcpStream {
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(
                timeout(Duration::from_secs(3), client.read_u8())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        assert!(headers.starts_with(b"HTTP/1.1 200"));
        client
    }

    for pending in [true, false] {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = origin.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let origin_closed = Arc::new(Notify::new());
        let origin_task = {
            let accepted = accepted.clone();
            let origin_closed = origin_closed.clone();
            tokio::spawn(async move {
                let mut connections = JoinSet::new();
                loop {
                    let (mut socket, _) = origin.accept().await.unwrap();
                    accepted.fetch_add(1, Ordering::Relaxed);
                    let origin_closed = origin_closed.clone();
                    connections.spawn(async move {
                        let mut selected = false;
                        let mut prefix = Vec::new();
                        let mut bytes = [0_u8; 4096];
                        loop {
                            let count = socket.read(&mut bytes).await.unwrap_or(0);
                            if count == 0 {
                                if selected { origin_closed.notify_one(); }
                                break;
                            }
                            if bytes[..count].starts_with(b"GET ") {
                                let mut headers = bytes[..count].to_vec();
                                while !headers.ends_with(b"\r\n\r\n") {
                                    headers.push(socket.read_u8().await.unwrap());
                                }
                                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nlive").await.unwrap();
                                break;
                            }
                            prefix.extend(bytes[..count].iter().take(6_usize.saturating_sub(prefix.len())).copied());
                            selected |= prefix == b"cancel";
                            if socket.write_all(&bytes[..count]).await.is_err() { break; }
                        }
                    });
                }
            })
        };
        let ca = ProxyCa::generate("Opaque cancellation", 2).unwrap();
        let trust = Arc::new(
            TrustSnapshot::load(&TestTrust(ca.certificate().to_der().unwrap()), 1).unwrap(),
        );
        let config = ProxyConfig {
            decrypt_https: false,
            ..ProxyConfig::default()
        };
        let certificates = Arc::new(
            CachedMitmCertificateResolver::new(
                ca,
                config.limits.leaf_cache_capacity,
                config.limits.leaf_validity_days,
            )
            .unwrap(),
        );
        let events = Arc::new(Events::default());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let components = ProxyComponents::new(
            InterceptorChainFactory::new(Vec::new(), config.limits.hooks),
            certificates,
        )
        .with_observer(
            Arc::new(TunnelEvents {
                events: events.clone(),
                pause_first: AtomicBool::new(pending),
                started: started.clone(),
                release: release.clone(),
            }),
            ObserverConfig {
                queue_capacity: std::num::NonZeroUsize::new(1).unwrap(),
                delivery: transmog_core::observe::ObserverDeliveryPolicy::WaitForCapacity,
                ..ObserverConfig::default()
            },
        );
        let proxy = ProxyServer::bind_with_components(config, trust, components)
            .await
            .unwrap();
        let control = proxy.control();
        let address = proxy.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(proxy.serve(async {
            let _ = shutdown_rx.await;
        }));
        let mut selected = if pending {
            let mut client = TcpStream::connect(address).await.unwrap();
            client
                .write_all(
                    format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
            timeout(Duration::from_secs(3), started.notified())
                .await
                .unwrap();
            client
        } else {
            let mut client = open_tunnel(address, destination).await;
            client.write_all(b"cancel").await.unwrap();
            let mut echoed = [0_u8; 6];
            timeout(Duration::from_secs(3), client.read_exact(&mut echoed))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&echoed, b"cancel");
            client
        };
        let id = events
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| {
                matches!(event.kind, ObserverEventKind::ExchangeStarted { .. })
                    .then_some(event.exchange_id)
            })
            .unwrap();
        let mut other = if pending {
            None
        } else {
            Some(open_tunnel(address, destination).await)
        };
        assert_eq!(control.cancel_requests(&[id]).await, vec![id]);
        release.notify_one();
        timeout(Duration::from_secs(3), async {
            loop {
                if events.0.lock().unwrap().iter().any(|event| event.exchange_id == id && matches!(&event.kind, ObserverEventKind::Failed(failure) if failure.kind == ExchangeFailureKind::Cancelled && failure.message.contains("Transmog"))) { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("opaque CONNECT lost cancellation evidence");
        let failure = events
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| {
                if event.exchange_id == id
                    && let ObserverEventKind::Failed(failure) = &event.kind
                {
                    Some(failure.clone())
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(failure.stage, ExchangeStage::Terminal);
        assert_eq!(failure.response_committed, !pending);
        assert_eq!(failure.request_committed, !pending);
        if pending {
            let mut response = Vec::new();
            while !response.ends_with(b"\r\n\r\n") {
                response.push(
                    timeout(Duration::from_secs(3), selected.read_u8())
                        .await
                        .unwrap()
                        .unwrap(),
                );
            }
            assert!(response.starts_with(b"HTTP/1.1 499"));
            assert_eq!(
                accepted.load(Ordering::Relaxed),
                0,
                "cancelled admission still dialed the origin"
            );
            other = Some(open_tunnel(address, destination).await);
        } else {
            let mut remainder = Vec::new();
            timeout(Duration::from_secs(3), selected.read_to_end(&mut remainder))
                .await
                .expect("selected CONNECT client stayed open")
                .unwrap();
            timeout(Duration::from_secs(3), origin_closed.notified())
                .await
                .expect("selected CONNECT origin stayed open");
        }
        let other = other.as_mut().unwrap();
        other.write_all(b"other").await.unwrap();
        let mut echoed = [0_u8; 5];
        timeout(Duration::from_secs(3), other.read_exact(&mut echoed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&echoed, b"other");
        let mut ordinary = TcpStream::connect(address).await.unwrap();
        ordinary.write_all(format!("GET http://{destination}/after HTTP/1.1\r\nHost: {destination}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(3), ordinary.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert!(response.ends_with(b"live"));
        shutdown_tx.send(()).unwrap();
        timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        origin_task.abort();
    }
}

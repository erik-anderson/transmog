mod websocket_h2_tests {
    use super::*;
    use transmog_core::observe::{BodyObservation, ObservationInterest, ObserverEvent};
    use transmog_http::WebSocketIo;
    use transmog_websocket::FrameDecoder;

    struct Events(tokio::sync::mpsc::UnboundedSender<ObserverEvent>);
    impl Observer for Events {
        fn on_event(&self, event: ObserverEvent) -> transmog_core::observe::BoxObserverFuture<'_> {
            let _ = self.0.send(event);
            Box::pin(async { Ok(()) })
        }
    }
    struct Fixture {
        address: SocketAddr,
        origin: SocketAddr,
        trust: Arc<TrustSnapshot>,
        control: ProxyControl,
        stop: oneshot::Sender<()>,
        proxy_task: tokio::task::JoinHandle<Result<(), ProxyRuntimeError>>,
        origin_task: tokio::task::JoinHandle<()>,
        connections: Arc<std::sync::atomic::AtomicUsize>,
        events: tokio::sync::mpsc::UnboundedReceiver<ObserverEvent>,
    }
    impl Fixture {
        async fn start(route_policy: RoutePolicy, support: bool) -> Self {
            Self::with_hooks(route_policy, support, Vec::new()).await
        }
        #[allow(clippy::too_many_lines)]
        async fn with_hooks(
            route_policy: RoutePolicy,
            support: bool,
            hooks: Vec<InterceptorRegistration>,
        ) -> Self {
            let origin_ca = ProxyCa::generate("H2 WebSocket origin", 2).unwrap();
            let leaf = origin_ca
                .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
                .unwrap();
            let acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
                .acceptor(&leaf)
                .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = listener.local_addr().unwrap();
            let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = connections.clone();
            let origin_task = tokio::spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                loop {
                    let (socket, _) = listener.accept().await.unwrap();
                    count.fetch_add(1, Ordering::Relaxed);
                    let acceptor = acceptor.clone();
                    tasks.spawn(async move {
                        let socket = tokio_boring::accept(&acceptor, socket).await.unwrap();
                        if socket.ssl().selected_alpn_protocol() == Some(b"h2") {
                            let mut builder =
                                hyper::server::conn::http2::Builder::new(TokioExecutor::new());
                            if support {
                                builder.enable_connect_protocol();
                            }
                            let _ = builder
                                .serve_connection(TokioIo::new(socket), service_fn(origin_request))
                                .await;
                        } else {
                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(TokioIo::new(socket), service_fn(origin_request))
                                .with_upgrades()
                                .await;
                        }
                    });
                    while tasks.try_join_next().is_some() {}
                }
            });
            let upstream_trust = Arc::new(
                TrustSnapshot::load(
                    &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                    101,
                )
                .unwrap(),
            );
            let ca = ProxyCa::generate("H2 WebSocket proxy", 2).unwrap();
            let trust = Arc::new(
                TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 1)
                    .unwrap(),
            );
            let config = ProxyConfig {
                route_policy,
                limits: RuntimeLimits {
                    shutdown_timeout: Duration::from_secs(1),
                    header_read_timeout: Duration::from_secs(3),
                    ..RuntimeLimits::default()
                },
                ..ProxyConfig::default()
            };
            let (tx, events) = tokio::sync::mpsc::unbounded_channel();
            let components = ProxyComponents::new(
                InterceptorChainFactory::new(hooks, config.limits.hooks),
                Arc::new(
                    CachedMitmCertificateResolver::new(
                        ca,
                        config.limits.leaf_cache_capacity,
                        config.limits.leaf_validity_days,
                    )
                    .unwrap(),
                ),
            )
            .with_observer(
                Arc::new(Events(tx)),
                ObserverConfig {
                    interest: ObservationInterest {
                        request_body: BodyObservation::Full,
                        response_body: BodyObservation::Full,
                        ..ObservationInterest::default()
                    },
                    ..ObserverConfig::default()
                },
            );
            let proxy = ProxyServer::bind_with_components(config, upstream_trust, components)
                .await
                .unwrap();
            let address = proxy.local_addr().unwrap();
            let control = proxy.control();
            let (stop, stopped) = oneshot::channel();
            let proxy_task = tokio::spawn(proxy.serve(async {
                let _ = stopped.await;
            }));
            Self {
                address,
                origin,
                trust,
                control,
                stop,
                proxy_task,
                origin_task,
                connections,
                events,
            }
        }
        async fn client(&self, h2: bool) -> Client {
            let mut tcp = TcpStream::connect(self.address).await.unwrap();
            tcp.write_all(
                format!(
                    "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                    self.origin.port(),
                    self.origin.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
            assert!(read_http_head(&mut tcp).await.starts_with(b"HTTP/1.1 200"));
            let alpn: &[u8] = if h2 { b"\x02h2" } else { b"\x08http/1.1" };
            let tls =
                UpstreamTlsContextFactory::new(self.trust.clone(), UpstreamTlsPolicy::default())
                    .hyper_connector_builder(alpn)
                    .unwrap()
                    .build();
            let tls = tokio_boring::connect(tls.configure().unwrap(), "localhost", tcp)
                .await
                .unwrap();
            let (sender, task) = if h2 {
                let (sender, connection) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
                        .await
                        .unwrap();
                let (enabled, ready) = oneshot::channel();
                let task = tokio::spawn(async move {
                    let mut enabled = Some(enabled);
                    let mut connection = Box::pin(connection);
                    loop {
                        tokio::select! {
                            result = &mut connection => { let _ = result; break; },
                            () = tokio::time::sleep(Duration::from_millis(1)), if enabled.is_some() => {
                                if connection.is_extended_connect_protocol_enabled()
                                    && let Some(enabled) = enabled.take() { let _ = enabled.send(()); }
                            }
                        }
                    }
                });
                timeout(Duration::from_secs(3), ready)
                    .await
                    .unwrap()
                    .unwrap();
                (Sender::H2(sender), task)
            } else {
                let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
                    .await
                    .unwrap();
                (
                    Sender::H1(sender),
                    tokio::spawn(async {
                        connection.with_upgrades().await.unwrap();
                    }),
                )
            };
            Client {
                sender,
                _task: task,
                authority: format!("localhost:{}", self.origin.port()),
            }
        }
        async fn finish(mut self) -> Vec<ObserverEvent> {
            self.stop.send(()).unwrap();
            timeout(Duration::from_secs(4), self.proxy_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            self.origin_task.abort();
            let mut events = Vec::new();
            while let Some(event) = self.events.recv().await {
                events.push(event);
            }
            events
        }
    }
    enum Sender {
        H1(hyper::client::conn::http1::SendRequest<http_body_util::Empty<Bytes>>),
        H2(hyper::client::conn::http2::SendRequest<http_body_util::Empty<Bytes>>),
    }
    struct Client {
        sender: Sender,
        _task: tokio::task::JoinHandle<()>,
        authority: String,
    }
    impl Client {
        async fn request(&mut self, path: &str, websocket: bool) -> Response<Incoming> {
            let h2 = matches!(self.sender, Sender::H2(_));
            let mut request = Request::builder()
                .method(if h2 && websocket { "CONNECT" } else { "GET" })
                .uri(format!("https://{}{path}", self.authority))
                .body(http_body_util::Empty::new())
                .unwrap();
            if websocket {
                request.headers_mut().insert("sec-websocket-extensions", HeaderValue::from_static("permessage-deflate; client_no_context_takeover; server_no_context_takeover"));
                request
                    .headers_mut()
                    .insert("sec-websocket-version", HeaderValue::from_static("13"));
                request
                    .headers_mut()
                    .insert("sec-websocket-protocol", HeaderValue::from_static("chat"));
                if h2 {
                    request
                        .extensions_mut()
                        .insert(hyper::ext::Protocol::from_static("websocket"));
                } else {
                    for (key, value) in [
                        ("connection", "Upgrade"),
                        ("upgrade", "websocket"),
                        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
                    ] {
                        request
                            .headers_mut()
                            .insert(key, HeaderValue::from_static(value));
                    }
                }
            }
            timeout(Duration::from_secs(5), async {
                match &mut self.sender {
                    Sender::H1(sender) => sender.send_request(request).await.unwrap(),
                    Sender::H2(sender) => sender.send_request(request).await.unwrap(),
                }
            })
            .await
            .unwrap()
        }
        async fn open(&mut self) -> Box<dyn WebSocketIo> {
            let mut response = self.request("/socket", true).await;
            assert_eq!(
                response.status(),
                if matches!(self.sender, Sender::H2(_)) {
                    200
                } else {
                    101
                }
            );
            assert_eq!(response.headers()["sec-websocket-protocol"], "chat");
            if matches!(self.sender, Sender::H2(_)) {
                for name in [
                    "connection",
                    "upgrade",
                    "sec-websocket-accept",
                    "content-length",
                ] {
                    assert!(!response.headers().contains_key(name), "{name}");
                }
            } else {
                assert_eq!(
                    response.headers()["sec-websocket-accept"],
                    "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
                );
            }
            Box::new(TokioIo::new(
                hyper::upgrade::on(&mut response).await.unwrap(),
            ))
        }
    }
    async fn origin_request(
        mut request: Request<Incoming>,
    ) -> Result<Response<http_body_util::Full<Bytes>>, Infallible> {
        let h2 = request.version() == Version::HTTP_2;
        if request.uri().path() == "/events" {
            return Ok(Response::builder()
                .header("content-type", "text/event-stream")
                .body(http_body_util::Full::new(Bytes::from_static(
                    b"data: H2 event\n\n",
                )))
                .unwrap());
        }
        if request.uri().path() == "/ordinary" {
            return Ok(Response::new(http_body_util::Full::new(
                Bytes::from_static(b"ordinary"),
            )));
        }
        if request.uri().path() == "/reject" {
            return Ok(Response::builder()
                .status(403)
                .body(http_body_util::Full::new(Bytes::from_static(b"declined")))
                .unwrap());
        }
        if request.uri().path() == "/decline200" {
            return Ok(Response::new(http_body_util::Full::new(
                Bytes::from_static(b"not a websocket"),
            )));
        }
        if h2 {
            assert_eq!(request.method(), Method::CONNECT);
            assert_eq!(
                request
                    .extensions()
                    .get::<hyper::ext::Protocol>()
                    .unwrap()
                    .as_str(),
                "websocket"
            );
            for name in ["connection", "upgrade", "sec-websocket-key", "host"] {
                assert!(!request.headers().contains_key(name));
            }
        } else {
            assert_eq!(request.method(), Method::GET);
        }
        let mut response = Response::builder()
            .status(if h2 { 200 } else { 101 })
            .header(
                "sec-websocket-extensions",
                "permessage-deflate; client_no_context_takeover; server_no_context_takeover",
            )
            .header("sec-websocket-protocol", "chat");
        if !h2 {
            response = response
                .header("connection", "Upgrade")
                .header("upgrade", "websocket")
                .header(
                    "sec-websocket-accept",
                    transmog_websocket::expected_accept(
                        request.headers()["sec-websocket-key"].to_str().unwrap(),
                    ),
                );
        }
        let upgrade = hyper::upgrade::on(&mut request);
        tokio::spawn(async {
            let mut io = TokioIo::new(upgrade.await.unwrap());
            let mut decoder =
                FrameDecoder::new(Direction::ClientToServer, FrameLimits::default(), true).unwrap();
            let mut buffer = [0; 16384];
            while let Ok(read) = io.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                decoder.push(&buffer[..read]).unwrap();
                while let Some(mut frame) = decoder.next_frame().unwrap() {
                    if frame.opcode == 9 {
                        frame.opcode = 10;
                    }
                    let encoded =
                        encode_websocket_frame(&frame, Direction::ServerToClient, None).unwrap();
                    if io.write_all(&encoded).await.is_err() {
                        return;
                    }
                    if frame.opcode == 8 {
                        let _ = io.shutdown().await;
                        return;
                    }
                }
            }
        });
        Ok(response
            .body(http_body_util::Full::new(Bytes::new()))
            .unwrap())
    }
    async fn echo(io: &mut Box<dyn WebSocketIo>, opcode: u8, payload: Bytes) {
        exchange_frame(
            io,
            WebSocketFrame {
                fin: true,
                compressed: false,
                opcode,
                payload,
            },
        )
        .await;
    }
    async fn exchange_frame(io: &mut Box<dyn WebSocketIo>, frame: WebSocketFrame) {
        let opcode = frame.opcode;
        let sent =
            encode_websocket_frame(&frame, Direction::ClientToServer, Some([1, 2, 3, 4])).unwrap();
        let expected = encode_websocket_frame(
            &WebSocketFrame {
                opcode: if opcode == 9 { 10 } else { opcode },
                ..frame
            },
            Direction::ServerToClient,
            None,
        )
        .unwrap();
        timeout(Duration::from_secs(4), async {
            io.write_all(&sent).await.unwrap();
            let mut received = vec![0; expected.len()];
            io.read_exact(&mut received).await.unwrap();
            assert_eq!(received, expected);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn websocket_h1_h2_bridge_matrix_observes_both_directions() {
        for (ingress_h2, route) in [
            (false, RoutePolicy::Http1Only),
            (true, RoutePolicy::Http1Only),
            (false, RoutePolicy::Http2Only),
            (true, RoutePolicy::Http2Only),
            (true, RoutePolicy::Auto),
        ] {
            let fixture = Fixture::start(route, true).await;
            let mut client = fixture.client(ingress_h2).await;
            let mut io = client.open().await;
            echo(&mut io, 1, Bytes::from_static(b"hello H2")).await;
            let mut codec = PerMessageDeflateCodec::new(
                transmog_websocket::PerMessageDeflate {
                    client_no_context_takeover: true,
                    server_no_context_takeover: true,
                    ..transmog_websocket::PerMessageDeflate::default()
                },
                Direction::ClientToServer,
            )
            .unwrap();
            exchange_frame(
                &mut io,
                WebSocketFrame {
                    fin: true,
                    compressed: true,
                    opcode: 1,
                    payload: codec.compress(b"compressed H2").unwrap().into(),
                },
            )
            .await;
            echo(&mut io, 8, Bytes::from_static(&[3, 232])).await;
            assert_eq!(
                timeout(Duration::from_secs(2), io.read(&mut [0; 1]))
                    .await
                    .unwrap()
                    .unwrap(),
                0,
                "clean WebSocket close must end the H2 stream without reset"
            );
            timeout(Duration::from_secs(2), async {
                while fixture.control.activity().websockets != 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            let generation = fixture.control.begin_drain();
            assert!(
                timeout(
                    Duration::from_secs(1),
                    fixture.control.wait_for_drain(generation)
                )
                .await
                .unwrap()
            );
            let events = fixture.finish().await;
            assert!(events.iter().any(|event| matches!(&event.kind, ObserverEventKind::StreamState(state) if state.status == StreamStatus::Closed)));
            for sender in ["client", "server"] {
                assert!(events.iter().any(|event| matches!(&event.kind, ObserverEventKind::StreamMessage(message) if message.sender == sender && message.payload == "compressed H2" && message.compressed)));
                assert!(events.iter().any(|event| matches!(&event.kind, ObserverEventKind::StreamMessage(message) if message.sender == sender && message.payload == "hello H2")), "missing {sender} observation");
            }
            assert!(events.iter().any(|event| matches!(&event.kind, ObserverEventKind::StreamState(state) if state.status == StreamStatus::Open)));
        }
    }

    #[tokio::test]
    async fn websocket_h2_multiplex_reset_drain_resume_and_shutdown() {
        let fixture = Fixture::start(RoutePolicy::Http2Only, true).await;
        let mut client = fixture.client(true).await;
        let first = client.open().await;
        let mut second = client.open().await;
        assert_eq!(
            fixture.connections.load(Ordering::Relaxed),
            1,
            "WebSockets should share the origin H2 connection"
        );
        assert_eq!(fixture.control.activity().websockets, 2);
        let generation = fixture.control.begin_drain();
        assert!(
            timeout(
                Duration::from_millis(100),
                fixture.control.wait_for_drain(generation)
            )
            .await
            .is_err()
        );
        assert!(fixture.control.resume_drain(generation));
        drop(first); // RST_STREAM must only terminate its own relay.
        echo(&mut second, 2, Bytes::from(vec![42; 192 * 1024])).await;
        echo(&mut second, 9, Bytes::from_static(b"ping")).await;
        let response = client.request("/ordinary", false).await;
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "ordinary"
        );
        let response = client.request("/events", false).await;
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "data: H2 event\n\n"
        );
        let events = fixture.finish().await;
        assert!(events.iter().any(|event| matches!(&event.kind, ObserverEventKind::StreamMessage(message) if message.kind == "event" && message.payload == "H2 event")));
        assert!(events.iter().any(|event| matches!(&event.kind, ObserverEventKind::StreamState(state) if state.status == StreamStatus::Failed)));
        assert!(events.iter().any(|event| matches!(&event.kind, ObserverEventKind::StreamState(state) if state.status == StreamStatus::Interrupted)));
    }

    #[tokio::test]
    async fn websocket_h2_synthetic_http_responses_cannot_open_unowned_streams() {
        let cases: [(Arc<dyn InterceptorFactory>, &str); 2] = [
            (Arc::new(LocalResponseHandler), "synthetic"),
            (Arc::new(StreamingLocalResponseHandler), "stream-asset"),
        ];
        for (factory, body) in cases {
            let fixture = Fixture::with_hooks(
                RoutePolicy::Http2Only,
                true,
                vec![InterceptorRegistration::new(
                    "local",
                    factory,
                    InterceptorRequirement::Required,
                )],
            )
            .await;
            let mut client = fixture.client(true).await;
            let response = client.request("/socket", true).await;
            assert_eq!(response.status(), 502);
            assert_eq!(
                response.into_body().collect().await.unwrap().to_bytes(),
                body
            );
            assert_eq!(fixture.connections.load(Ordering::Relaxed), 0);
            assert_eq!(fixture.control.activity().websockets, 0);
            fixture.finish().await;
        }
    }

    #[tokio::test]
    async fn websocket_h2_rejects_ordinary_and_unknown_extended_connect() {
        let fixture = Fixture::start(RoutePolicy::Http2Only, true).await;
        let mut client = fixture.client(true).await;
        let Sender::H2(sender) = &mut client.sender else {
            unreachable!()
        };
        for protocol in [None, Some("unsupported")] {
            let mut request = Request::builder()
                .method("CONNECT")
                .uri(if protocol.is_some() {
                    format!("https://{}/socket", client.authority)
                } else {
                    client.authority.clone()
                })
                .body(http_body_util::Empty::<Bytes>::new())
                .unwrap();
            if let Some(protocol) = protocol {
                request
                    .extensions_mut()
                    .insert(hyper::ext::Protocol::from_static(protocol));
            }
            let response = sender.send_request(request).await.unwrap();
            assert_eq!(response.status(), 400);
            let _ = response.into_body().collect().await.unwrap();
        }
        assert_eq!(fixture.connections.load(Ordering::Relaxed), 0);
        let mut io = client.open().await;
        echo(&mut io, 1, Bytes::from_static(b"still usable")).await;
        fixture.finish().await;
    }

    #[tokio::test]
    async fn websocket_h2_requires_settings_and_auto_falls_back_before_connect() {
        for (route, expected) in [(RoutePolicy::Http2Only, 502), (RoutePolicy::Auto, 200)] {
            let fixture = Fixture::start(route, false).await;
            let mut client = fixture.client(true).await;
            let mut response = client.request("/socket", true).await;
            assert_eq!(response.status(), expected);
            if expected == 200 {
                let mut io: Box<dyn WebSocketIo> = Box::new(TokioIo::new(
                    hyper::upgrade::on(&mut response).await.unwrap(),
                ));
                echo(&mut io, 1, Bytes::from_static(b"fallback")).await;
            }
            fixture.finish().await;
        }
    }

    #[tokio::test]
    async fn websocket_h2_rejection_retains_body_and_h1_200_never_opens_tunnel() {
        for route in [RoutePolicy::Http1Only, RoutePolicy::Http2Only] {
            let fixture = Fixture::start(route, true).await;
            let mut client = fixture.client(true).await;
            let response = client.request("/reject", true).await;
            assert_eq!(response.status(), 403);
            assert_eq!(
                response.into_body().collect().await.unwrap().to_bytes(),
                "declined"
            );
            if route == RoutePolicy::Http1Only {
                let response = client.request("/decline200", true).await;
                assert_eq!(response.status(), 502);
                assert_eq!(
                    response.into_body().collect().await.unwrap().to_bytes(),
                    "not a websocket"
                );
            }
            fixture.finish().await;
        }
    }
}

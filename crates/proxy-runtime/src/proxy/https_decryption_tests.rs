// Real TLS coverage for live HTTPS-mode and certificate-generation publication.

struct LiveHttpsOrigin {
    address: SocketAddr,
    certificate: Vec<u8>,
    root: Vec<u8>,
    requests: Arc<AtomicU64>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    task: JoinHandle<()>,
}

impl LiveHttpsOrigin {
    async fn start() -> Self {
        let ca = ProxyCa::generate("Live HTTPS origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let certificate = leaf.certificate.to_der().unwrap();
        let root = ca.certificate().to_der().unwrap();
        let acceptor = Arc::new(
            DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
                .acceptor(&leaf)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicU64::new(0));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (observed_requests, request_entered, release_request) =
            (requests.clone(), entered.clone(), release.clone());
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                let (requests, entered, release) = (
                    observed_requests.clone(),
                    request_entered.clone(),
                    release_request.clone(),
                );
                while connections.try_join_next().is_some() {}
                connections.spawn(async move {
                    let Ok(tls) = tokio_boring::accept(&acceptor, stream).await else {
                        return;
                    };
                    let service = service_fn(move |mut request: Request<Incoming>| {
                        let (requests, entered, release) =
                            (requests.clone(), entered.clone(), release.clone());
                        async move {
                            requests.fetch_add(1, Ordering::Relaxed);
                            if matches!(request.uri().path(), "/pause" | "/upgrade") {
                                entered.notify_one();
                                release.notified().await;
                            }
                            if request.uri().path() == "/upgrade" {
                                let upgraded = hyper::upgrade::on(&mut request);
                                tokio::spawn(async move {
                                    let mut stream = TokioIo::new(upgraded.await.unwrap());
                                    let mut bytes = [0; 4];
                                    stream.read_exact(&mut bytes).await.unwrap();
                                    assert_eq!(&bytes, b"ping");
                                    stream.write_all(b"pong").await.unwrap();
                                });
                                return Ok(Response::builder()
                                    .status(101)
                                    .header("connection", "Upgrade")
                                    .header("upgrade", "websocket")
                                    .header("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
                                    .body(DownstreamBody::empty())
                                    .unwrap());
                            }
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .header("content-length", "2")
                                    .body(DownstreamBody::from_bytes(Bytes::from_static(b"ok")))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), service)
                        .with_upgrades()
                        .await;
                });
            }
        });
        Self {
            address,
            certificate,
            root,
            requests,
            entered,
            release,
            task,
        }
    }
}

impl Drop for LiveHttpsOrigin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct LiveHttpsCertificates {
    resolver: Option<CachedMitmCertificateResolver>,
    calls: AtomicU64,
}

impl LiveHttpsCertificates {
    fn usable(ca: ProxyCa) -> Arc<Self> {
        Arc::new(Self {
            resolver: Some(CachedMitmCertificateResolver::new(ca, 8, 1).unwrap()),
            calls: AtomicU64::new(0),
        })
    }
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            resolver: None,
            calls: AtomicU64::new(0),
        })
    }
}

impl DownstreamCertificateResolver for LiveHttpsCertificates {
    fn resolve(
        &self,
        identity: &EndpointIdentity,
        now: std::time::SystemTime,
    ) -> Result<transmog_tls::IssuedLeaf, CertificateResolverError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.resolver
            .as_ref()
            .expect("certificate-free mode opened a downstream certificate")
            .resolve(identity, now)
    }
}

#[derive(Default)]
struct LiveHttpsEvidence {
    hooks: AtomicU64,
    methods: StdMutex<Vec<String>>,
}

impl InterceptorFactory for LiveHttpsEvidence {
    fn create(
        &self,
        metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        self.hooks.fetch_add(1, Ordering::Relaxed);
        transmog_core::intercept::NoopInterceptorFactory.create(metadata)
    }
}

impl Observer for LiveHttpsEvidence {
    fn on_event(
        &self,
        event: transmog_core::observe::ObserverEvent,
    ) -> transmog_core::observe::BoxObserverFuture<'_> {
        if let ObserverEventKind::RequestHeadFinalized(head) = event.kind {
            self.methods.lock().unwrap().push(head.method);
        }
        Box::pin(async { Ok(()) })
    }
}

struct LiveHttpsCaller;
impl ClientIdentityResolver for LiveHttpsCaller {
    fn resolve(&self, _: SocketAddr, _: SocketAddr) -> transmog_core::ClientIdentity {
        transmog_core::ClientIdentity::LocalProcess {
            pid: 1,
            name: Some("live.exe".into()),
        }
    }
}

struct LiveHttpsProxy {
    address: SocketAddr,
    control: ProxyControl,
    evidence: Arc<LiveHttpsEvidence>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), ProxyRuntimeError>>,
}

impl LiveHttpsProxy {
    async fn start(
        origin: &LiveHttpsOrigin,
        enabled: bool,
        certificates: Arc<LiveHttpsCertificates>,
    ) -> Self {
        let evidence = Arc::new(LiveHttpsEvidence::default());
        let hooks = InterceptorChainFactory::new(
            vec![InterceptorRegistration::new(
                "live-https",
                evidence.clone(),
                InterceptorRequirement::Required,
            )],
            RuntimeLimits::default().hooks,
        );
        let components = ProxyComponents::new(hooks, certificates)
            .with_client_identity_resolver(Arc::new(LiveHttpsCaller))
            .with_observer(evidence.clone(), ObserverConfig::default());
        let proxy = ProxyServer::bind_with_components(
            ProxyConfig {
                decrypt_https: enabled,
                route_policy: RoutePolicy::Http1Only,
                isolate_outbound_connections: false,
                ..Default::default()
            },
            live_https_trust(vec![origin.root.clone()]),
            components,
        )
        .await
        .unwrap();
        let address = proxy.local_addr().unwrap();
        let control = proxy.control();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(proxy.serve(async {
            let _ = stopped.await;
        }));
        Self {
            address,
            control,
            evidence,
            stop,
            task,
        }
    }
    async fn finish(self) -> Arc<LiveHttpsEvidence> {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.evidence
    }
}

fn live_https_trust(roots: Vec<Vec<u8>>) -> Arc<TrustSnapshot> {
    Arc::new(TrustSnapshot::load(&StaticTrust(roots), 1).unwrap())
}

async fn live_https_connect(proxy: SocketAddr, origin: SocketAddr) -> TcpStream {
    let mut transport = TcpStream::connect(proxy).await.unwrap();
    transport
        .write_all(
            format!(
                "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                origin.port(),
                origin.port()
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

async fn live_https_handshake(
    transport: TcpStream,
    trust: Arc<TrustSnapshot>,
) -> tokio_boring::SslStream<TcpStream> {
    let connector = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default())
        .hyper_connector_builder(b"\x08http/1.1")
        .unwrap()
        .build();
    timeout(
        Duration::from_secs(3),
        tokio_boring::connect(connector.configure().unwrap(), "localhost", transport),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn live_https_request(browser: &mut tokio_boring::SslStream<TcpStream>, origin: SocketAddr) {
    browser
        .write_all(
            format!(
                "GET /live HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                origin.port()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    live_https_response(browser).await;
}

async fn live_https_response(browser: &mut tokio_boring::SslStream<TcpStream>) -> Vec<u8> {
    let response = timeout(Duration::from_secs(3), read_http_head(browser))
        .await
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
    let mut body = [0; 2];
    timeout(Duration::from_secs(3), browser.read_exact(&mut body))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&body, b"ok");
    response
}

#[tokio::test]
async fn live_https_on_off_on_preserves_listener_observation_and_admitted_tls_generations() {
    let origin = LiveHttpsOrigin::start().await;
    let ca_a = ProxyCa::generate("Live HTTPS interception A", 2).unwrap();
    let ca_b = ProxyCa::generate("Live HTTPS interception B", 2).unwrap();
    let issuer_a = ca_a.certificate().subject_name().to_der().unwrap();
    let issuer_b = ca_b.certificate().subject_name().to_der().unwrap();
    let browser_trust = live_https_trust(vec![
        origin.root.clone(),
        ca_a.certificate().to_der().unwrap(),
        ca_b.certificate().to_der().unwrap(),
    ]);
    let certificates_a = LiveHttpsCertificates::usable(ca_a);
    let certificates_b = LiveHttpsCertificates::usable(ca_b);
    let proxy = LiveHttpsProxy::start(&origin, true, certificates_a.clone()).await;
    let admitted_on = live_https_connect(proxy.address, origin.address).await;
    assert_eq!(certificates_a.calls.load(Ordering::Relaxed), 0);
    // Publishing B with mode off must not change A's already admitted, delayed handshake.
    proxy
        .control
        .set_https_decryption_with_certificates(false, certificates_b.clone());
    assert!(!proxy.control.https_decryption());
    let mut old_inspected = live_https_handshake(admitted_on, browser_trust.clone()).await;
    assert_eq!(
        old_inspected
            .ssl()
            .peer_certificate()
            .unwrap()
            .issuer_name()
            .to_der()
            .unwrap(),
        issuer_a
    );
    let mut retired = Vec::new();
    timeout(
        Duration::from_secs(3),
        old_inspected.read_to_end(&mut retired),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        retired.is_empty(),
        "a retired idle connection must not synthesize a response"
    );
    let mut opaque = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        browser_trust.clone(),
    )
    .await;
    assert_eq!(
        opaque.ssl().peer_certificate().unwrap().to_der().unwrap(),
        origin.certificate
    );
    live_https_request(&mut opaque, origin.address).await;
    assert_eq!(proxy.evidence.hooks.load(Ordering::Relaxed), 0);
    proxy
        .control
        .set_https_decryption_with_certificates(true, certificates_b.clone());
    assert!(proxy.control.https_decryption());
    live_https_request(&mut opaque, origin.address).await;
    let mut new_inspected = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        browser_trust,
    )
    .await;
    assert_eq!(
        new_inspected
            .ssl()
            .peer_certificate()
            .unwrap()
            .issuer_name()
            .to_der()
            .unwrap(),
        issuer_b
    );
    live_https_request(&mut new_inspected, origin.address).await;
    assert_eq!(proxy.evidence.hooks.load(Ordering::Relaxed), 1);
    assert_eq!(certificates_a.calls.load(Ordering::Relaxed), 1);
    assert_eq!(certificates_b.calls.load(Ordering::Relaxed), 1);
    drop((old_inspected, opaque, new_inspected));
    let evidence = proxy.finish().await;
    let mut methods = evidence.methods.lock().unwrap().clone();
    methods.sort();
    assert_eq!(methods, ["CONNECT", "GET"]);
}

#[tokio::test]
async fn certificate_free_start_can_enable_live_and_excluded_connections_still_bypass() {
    let origin = LiveHttpsOrigin::start().await;
    let ca = ProxyCa::generate("Live HTTPS enabled after certificate-free start", 2).unwrap();
    let issuer = ca.certificate().subject_name().to_der().unwrap();
    let trust = live_https_trust(vec![
        origin.root.clone(),
        ca.certificate().to_der().unwrap(),
    ]);
    let disabled = LiveHttpsCertificates::disabled();
    let enabled = LiveHttpsCertificates::usable(ca);
    let proxy = LiveHttpsProxy::start(&origin, false, disabled.clone()).await;
    let mut old_opaque = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        trust.clone(),
    )
    .await;
    live_https_request(&mut old_opaque, origin.address).await;
    assert_eq!(disabled.calls.load(Ordering::Relaxed), 0);
    assert_eq!(proxy.evidence.hooks.load(Ordering::Relaxed), 0);
    proxy
        .control
        .set_https_decryption_with_certificates(true, enabled.clone());
    let mut inspected = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        trust.clone(),
    )
    .await;
    assert_eq!(
        inspected
            .ssl()
            .peer_certificate()
            .unwrap()
            .issuer_name()
            .to_der()
            .unwrap(),
        issuer
    );
    live_https_request(&mut inspected, origin.address).await;
    live_https_request(&mut old_opaque, origin.address).await;
    proxy
        .control
        .set_process_inspection(ProcessInspectionPolicy {
            mode: ProcessInspectionMode::Except,
            include: vec![],
            exclude: vec!["live.exe".into()],
        })
        .unwrap();
    let mut excluded = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        trust,
    )
    .await;
    assert_eq!(
        excluded.ssl().peer_certificate().unwrap().to_der().unwrap(),
        origin.certificate
    );
    live_https_request(&mut excluded, origin.address).await;
    assert_eq!(enabled.calls.load(Ordering::Relaxed), 1);
    assert_eq!(disabled.calls.load(Ordering::Relaxed), 0);
    assert_eq!(proxy.evidence.hooks.load(Ordering::Relaxed), 1);
    drop((old_opaque, inspected, excluded));
    let evidence = proxy.finish().await;
    let mut methods = evidence.methods.lock().unwrap().clone();
    methods.sort();
    assert_eq!(methods, ["CONNECT", "CONNECT", "GET"]);
}

#[tokio::test]
async fn live_https_off_finishes_an_admitted_h1_request_then_closes_keepalive() {
    let origin = LiveHttpsOrigin::start().await;
    let ca = ProxyCa::generate("Live HTTPS H1 retirement", 2).unwrap();
    let trust = live_https_trust(vec![
        origin.root.clone(),
        ca.certificate().to_der().unwrap(),
    ]);
    let proxy = LiveHttpsProxy::start(&origin, true, LiveHttpsCertificates::usable(ca)).await;
    let mut browser = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        trust.clone(),
    )
    .await;
    browser
        .write_all(
            format!(
                "GET /pause HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                origin.address.port()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(3), origin.entered.notified())
        .await
        .unwrap();
    proxy.control.set_https_decryption(false);
    origin.release.notify_one();
    let headers = live_https_response(&mut browser).await;
    assert!(
        String::from_utf8_lossy(&headers)
            .to_ascii_lowercase()
            .contains("connection: close")
    );
    let mut extra = Vec::new();
    timeout(Duration::from_secs(3), browser.read_to_end(&mut extra))
        .await
        .unwrap()
        .unwrap();
    assert!(
        extra.is_empty(),
        "retired H1 connection sent bytes after its final response"
    );
    assert_eq!(origin.requests.load(Ordering::Relaxed), 1);
    let mut raw = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        trust,
    )
    .await;
    assert_eq!(
        raw.ssl().peer_certificate().unwrap().to_der().unwrap(),
        origin.certificate
    );
    live_https_request(&mut raw, origin.address).await;
    assert_eq!(proxy.evidence.hooks.load(Ordering::Relaxed), 1);
    drop((browser, raw));
    proxy.finish().await;
}

#[tokio::test]
async fn live_https_off_preserves_an_already_admitted_websocket_upgrade() {
    let origin = LiveHttpsOrigin::start().await;
    let ca = ProxyCa::generate("Live HTTPS upgrading WebSocket", 2).unwrap();
    let trust = live_https_trust(vec![
        origin.root.clone(),
        ca.certificate().to_der().unwrap(),
    ]);
    let proxy = LiveHttpsProxy::start(&origin, true, LiveHttpsCertificates::usable(ca)).await;
    let mut browser = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        trust,
    )
    .await;
    browser.write_all(format!("GET /upgrade HTTP/1.1\r\nHost: localhost:{}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n", origin.address.port()).as_bytes()).await.unwrap();
    timeout(Duration::from_secs(3), origin.entered.notified())
        .await
        .unwrap();
    proxy.control.set_https_decryption(false);
    origin.release.notify_one();
    let head = timeout(Duration::from_secs(3), read_http_head(&mut browser))
        .await
        .unwrap();
    let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 101"));
    assert!(head.contains("connection: upgrade"));
    assert!(!head.contains("connection: close"));
    browser.write_all(b"ping").await.unwrap();
    let mut reply = [0; 4];
    timeout(Duration::from_secs(3), browser.read_exact(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&reply, b"pong");
    assert_eq!(proxy.evidence.hooks.load(Ordering::Relaxed), 1);
    drop(browser);
    proxy.finish().await;
}

fn live_https_h2_get(stream: u32, path: &str, authority: &str) -> Vec<u8> {
    // HPACK static indexes: :method GET, :scheme https, literal :path and :authority.
    let mut block = vec![0x82, 0x87, 0x04, u8::try_from(path.len()).unwrap()];
    block.extend_from_slice(path.as_bytes());
    block.extend_from_slice(&[0x01, u8::try_from(authority.len()).unwrap()]);
    block.extend_from_slice(authority.as_bytes());
    let length = u32::try_from(block.len()).unwrap().to_be_bytes();
    let mut frame = Vec::from(&length[1..]);
    frame.extend_from_slice(&[1, 5]); // HEADERS, END_HEADERS | END_STREAM.
    frame.extend_from_slice(&stream.to_be_bytes());
    frame.extend_from_slice(&block);
    frame
}

async fn live_https_h2_frame(
    browser: &mut tokio_boring::SslStream<TcpStream>,
) -> (u8, u8, u32, Vec<u8>) {
    let mut header = [0_u8; 9];
    browser.read_exact(&mut header).await.unwrap();
    let length =
        (usize::from(header[0]) << 16) | (usize::from(header[1]) << 8) | usize::from(header[2]);
    let stream = u32::from_be_bytes(header[5..].try_into().unwrap()) & 0x7fff_ffff;
    let mut body = vec![0; length];
    browser.read_exact(&mut body).await.unwrap();
    (header[3], header[4], stream, body)
}

async fn live_https_h2_expect_refusal(
    browser: &mut tokio_boring::SslStream<TcpStream>,
    refused_stream: u32,
) -> Option<Vec<u8>> {
    let mut ping = None;
    let mut goaway = false;
    timeout(Duration::from_secs(3), async {
        loop {
            let (kind, flags, stream, body) = live_https_h2_frame(browser).await;
            match kind {
                4 if flags & 1 == 0 => browser
                    .write_all(b"\x00\x00\x00\x04\x01\x00\x00\x00\x00")
                    .await
                    .unwrap(),
                6 if flags & 1 == 0 => ping = Some(body),
                7 => goaway = true,
                1 if stream == refused_stream => {
                    panic!("refused stream received a synthetic HTTP response");
                }
                3 if stream == refused_stream => {
                    assert_eq!(body, [0, 0, 0, 7], "late stream must be REFUSED_STREAM");
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(goaway, "Off must send GOAWAY before the held-PING race");
    ping
}

#[tokio::test]
async fn live_https_off_refuses_late_h2_streams_while_admitted_streams_finish() {
    let origin = LiveHttpsOrigin::start().await;
    let ca = ProxyCa::generate("Live HTTPS H2 retirement", 2).unwrap();
    let trust = live_https_trust(vec![
        origin.root.clone(),
        ca.certificate().to_der().unwrap(),
    ]);
    let certificates = LiveHttpsCertificates::usable(ca);
    let proxy = LiveHttpsProxy::start(&origin, true, certificates.clone()).await;
    let connector = UpstreamTlsContextFactory::new(trust.clone(), UpstreamTlsPolicy::default())
        .hyper_connector_builder(b"\x02h2")
        .unwrap()
        .build();
    let mut browser = tokio_boring::connect(
        connector.configure().unwrap(),
        "localhost",
        live_https_connect(proxy.address, origin.address).await,
    )
    .await
    .unwrap();
    assert_eq!(
        browser.ssl().selected_alpn_protocol(),
        Some(b"h2".as_slice())
    );
    browser
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00\x00\x04\x00\x00\x00\x00\x00")
        .await
        .unwrap();
    let authority = format!("localhost:{}", origin.address.port());
    browser
        .write_all(&live_https_h2_get(1, "/pause", &authority))
        .await
        .unwrap();
    timeout(Duration::from_secs(3), origin.entered.notified())
        .await
        .unwrap();
    proxy.control.set_https_decryption(false);
    let mut opaque = live_https_handshake(
        live_https_connect(proxy.address, origin.address).await,
        trust,
    )
    .await;
    assert_eq!(
        opaque.ssl().peer_certificate().unwrap().to_der().unwrap(),
        origin.certificate
    );
    live_https_request(&mut opaque, origin.address).await;
    // Rapid re-enable cannot reopen the retired generation. Withhold the
    // graceful-GOAWAY PING acknowledgement to exercise its admission race.
    proxy
        .control
        .set_https_decryption_with_certificates(true, certificates);
    browser
        .write_all(&live_https_h2_get(3, "/must-not-run", &authority))
        .await
        .unwrap();
    let mut ping = live_https_h2_expect_refusal(&mut browser, 3).await;
    assert_eq!(origin.requests.load(Ordering::Relaxed), 2);
    assert_eq!(proxy.evidence.hooks.load(Ordering::Relaxed), 1);
    origin.release.notify_one();
    let mut received = Vec::new();
    timeout(Duration::from_secs(3), async {
        loop {
            let (kind, flags, stream, body) = live_https_h2_frame(&mut browser).await;
            if kind == 6 && flags & 1 == 0 {
                ping = Some(body.clone());
            }
            assert!(!(kind == 3 && stream == 1), "the admitted stream was reset");
            if stream == 1 && kind == 0 {
                received.extend_from_slice(&body);
            }
            if stream == 1 && matches!(kind, 0 | 1) && flags & 1 != 0 {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(received, b"ok");
    if let Some(ping) = ping {
        browser
            .write_all(b"\x00\x00\x08\x06\x01\x00\x00\x00\x00")
            .await
            .unwrap();
        browser.write_all(&ping).await.unwrap();
    }
    drop((browser, opaque));
    let evidence = proxy.finish().await;
    let mut methods = evidence.methods.lock().unwrap().clone();
    methods.sort();
    assert_eq!(methods, ["CONNECT", "GET"]);
}

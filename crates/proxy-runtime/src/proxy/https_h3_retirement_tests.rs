// A cached interception H3 connection must not participate in an opaque HTTPS run.

struct H3RetirementHooks(Arc<std::sync::atomic::AtomicUsize>);

impl InterceptorFactory for H3RetirementHooks {
    fn create(
        &self,
        metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        EditingHandler.create(metadata)
    }
}

struct H3RetirementObserver(Arc<StdMutex<Vec<String>>>);

impl Observer for H3RetirementObserver {
    fn on_event(
        &self,
        event: transmog_core::observe::ObserverEvent,
    ) -> transmog_core::observe::BoxObserverFuture<'_> {
        if let ObserverEventKind::RequestHeadFinalized(head) = event.kind {
            self.0.lock().unwrap().push(head.method);
        }
        Box::pin(async { Ok(()) })
    }
}

async fn h3_retirement_browser(
    proxy: SocketAddr,
    origin: SocketAddr,
    trust: Arc<TrustSnapshot>,
) -> tokio_boring::SslStream<TcpStream> {
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
        timeout(Duration::from_secs(5), read_http_head(&mut transport))
            .await
            .unwrap()
            .starts_with(b"HTTP/1.1 200")
    );
    let connector = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default())
        .hyper_connector_builder(b"\x08http/1.1")
        .unwrap()
        .build();
    timeout(
        Duration::from_secs(5),
        tokio_boring::connect(connector.configure().unwrap(), "localhost", transport),
    )
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One real H3-to-opaque transition, including hook and observer evidence.
async fn disabling_https_bypasses_cached_h3_without_caller_isolation() {
    let origin_ca = ProxyCa::generate("HTTPS retirement H3 origin", 2).unwrap();
    let origin_leaf = origin_ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let origin_certificate = origin_leaf.certificate.to_der().unwrap();
    let tcp_acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
        .acceptor(&origin_leaf)
        .unwrap();
    let bind_ip = tokio::net::lookup_host(("localhost", 0))
        .await
        .unwrap()
        .next()
        .unwrap()
        .ip();
    let (origin, h3_task) = spawn_h3_origin(origin_leaf, bind_ip).await;
    // TCP and QUIC share one authority, so accidentally reusing the H3 route
    // after disabling cannot succeed by connecting to a different destination.
    let tcp_origin = TcpListener::bind(origin).await.unwrap();
    let tcp_task = tokio::spawn(async move {
        let (socket, _) = tcp_origin.accept().await.unwrap();
        let mut tls = tokio_boring::accept(&tcp_acceptor, socket).await.unwrap();
        let head = String::from_utf8(read_http_head(&mut tls).await)
            .unwrap()
            .to_ascii_lowercase();
        assert!(head.starts_with("get /opaque http/1.1\r\n"));
        assert!(
            !head.contains("x-from-breakpoint"),
            "opaque HTTPS entered inspection hooks"
        );
        tls.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nopaque-origin",
        )
        .await
        .unwrap();
        tls.shutdown().await.unwrap();
    });
    let proxy_ca = ProxyCa::generate("HTTPS retirement interception", 2).unwrap();
    let browser_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![
                origin_ca.certificate().to_der().unwrap(),
                proxy_ca.certificate().to_der().unwrap(),
            ]),
            1,
        )
        .unwrap(),
    );
    let origin_trust = Arc::new(
        TrustSnapshot::load(
            &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
            1,
        )
        .unwrap(),
    );
    let hooks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let methods = Arc::new(StdMutex::new(Vec::new()));
    let config = ProxyConfig {
        route_policy: RoutePolicy::Http3Only,
        isolate_outbound_connections: false,
        h3: H3TransportLimits {
            idle_timeout: Duration::from_secs(2),
            ..Default::default()
        },
        ..Default::default()
    };
    let components = ProxyComponents::new(
        InterceptorChainFactory::new(
            vec![InterceptorRegistration::new(
                "h3-retirement",
                Arc::new(H3RetirementHooks(hooks.clone())),
                InterceptorRequirement::Required,
            )],
            config.limits.hooks,
        ),
        Arc::new(CachedMitmCertificateResolver::new(proxy_ca, 8, 1).unwrap()),
    )
    .with_observer(
        Arc::new(H3RetirementObserver(methods.clone())),
        ObserverConfig::default(),
    );
    let proxy = ProxyServer::bind_with_components(config, origin_trust, components)
        .await
        .unwrap();
    let address = proxy.local_addr().unwrap();
    let control = proxy.control();
    let mut evidence = proxy.subscribe_evidence();
    let (stop, stopped) = oneshot::channel();
    let proxy_task = tokio::spawn(proxy.serve(async {
        let _ = stopped.await;
    }));
    let mut inspected = h3_retirement_browser(address, origin, browser_trust.clone()).await;
    inspected
        .write_all(
            format!(
                "GET /cached-h3 HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                origin.port()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), inspected.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&response)
            .to_ascii_lowercase()
            .contains("x-intercepted: yes")
    );
    let proof = timeout(Duration::from_secs(5), evidence.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proof.egress_version, HttpLegVersion::Http3);
    assert_eq!(proof.h3.as_ref().unwrap().alpn, "h3");
    assert_eq!(hooks.load(Ordering::Relaxed), 1);
    control.set_https_decryption(false);
    let mut opaque = h3_retirement_browser(address, origin, browser_trust).await;
    assert_eq!(
        opaque.ssl().peer_certificate().unwrap().to_der().unwrap(),
        origin_certificate
    );
    opaque
        .write_all(
            format!(
                "GET /opaque HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                origin.port()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(5), opaque.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.ends_with(b"opaque-origin"));
    assert!(
        !String::from_utf8_lossy(&response)
            .to_ascii_lowercase()
            .contains("x-intercepted")
    );
    assert_eq!(
        hooks.load(Ordering::Relaxed),
        1,
        "opaque HTTPS created another hook chain"
    );
    assert!(!control.https_decryption());
    drop((inspected, opaque));
    stop.send(()).unwrap();
    timeout(Duration::from_secs(5), proxy_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(5), tcp_task)
        .await
        .unwrap()
        .unwrap();
    h3_task.finish().await;
    let mut observed = methods.lock().unwrap().clone();
    observed.sort();
    assert_eq!(
        observed,
        ["CONNECT", "GET"],
        "opaque payloads entered HTTP capture instead of retaining only the CONNECT summary"
    );
}

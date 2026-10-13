mod pqc_tests {
    use super::*;
    use boring::ssl::{SslAcceptor, SslSignatureAlgorithm, SslVersion};
    use http::header::HOST;
    use http_body_util::{BodyExt, Full};
    use transmog_tls::{LeafKeyAlgorithm, MlDsaVariant};

    const GROUPS: &str = "X25519MLKEM768:X25519:P-256:P-384";
    struct PqcObserver(Arc<StdMutex<Vec<transmog_core::performance::PerformanceEvidence>>>);
    impl transmog_core::observe::Observer for PqcObserver {
        fn on_event(
            &self,
            event: transmog_core::observe::ObserverEvent,
        ) -> transmog_core::observe::BoxObserverFuture<'_> {
            if let ObserverEventKind::Performance(evidence) = event.kind {
                self.0.lock().unwrap().push(evidence);
            }
            Box::pin(async { Ok(()) })
        }
    }
    enum OriginTask {
        Tcp(JoinHandle<()>),
        Quic(TestH3Origin),
    }
    impl OriginTask {
        async fn finish(self) {
            match self {
                Self::Tcp(task) => {
                    task.abort();
                    let _ = task.await;
                }
                Self::Quic(origin) => origin.finish().await,
            }
        }
    }

    async fn origin(ca: &ProxyCa, route: RoutePolicy, hybrid: bool) -> (SocketAddr, OriginTask) {
        let algorithm = if hybrid {
            LeafKeyAlgorithm::MlDsa(MlDsaVariant::MlDsa44)
        } else {
            LeafKeyAlgorithm::EcdsaP256
        };
        let leaf = ca
            .issue_with_algorithm(EndpointIdentity::parse("localhost").unwrap(), 1, algorithm)
            .unwrap();
        let groups = if hybrid { GROUPS } else { "X25519:P-256:P-384" };
        let ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        if route == RoutePolicy::Http3Only {
            let (address, task) = spawn_h3_origin_with_groups(leaf, ip, groups, 2).await;
            return (address, OriginTask::Quic(task));
        }
        let mut context = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        context
            .set_min_proto_version(Some(SslVersion::TLS1_3))
            .unwrap();
        context.set_curves_list(groups).unwrap();
        context.set_certificate(&leaf.certificate).unwrap();
        context.set_private_key(&leaf.private_key).unwrap();
        context.set_alpn_select_callback(|_, offered| {
            boring::ssl::select_next_proto(b"\x02h2\x08http/1.1", offered)
                .ok_or(boring::ssl::AlpnError::NOACK)
        });
        let context = context.build();
        let listener = TcpListener::bind((ip, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let tls = tokio_boring::accept(&context, socket).await.unwrap();
            assert_eq!(tls.ssl().curve(), Some(if hybrid { 0x11ec } else { 29 }));
            let service = hyper::service::service_fn(|_| async {
                Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(
                    b"ok",
                ))))
            });
            if route == RoutePolicy::Http2Only {
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            } else {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            }
        });
        (address, OriginTask::Tcp(task))
    }

    async fn case(route: RoutePolicy, browser_hybrid: bool, origin_hybrid: bool) {
        let origin_ca = ProxyCa::generate("PQC matrix origin root", 1).unwrap();
        let (address, origin) = origin(&origin_ca, route, origin_hybrid).await;
        let trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                1,
            )
            .unwrap(),
        );
        let ca = ProxyCa::generate("PQC matrix proxy root", 1).unwrap();
        let client_trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 1).unwrap(),
        );
        let config = ProxyConfig {
            route_policy: route,
            ..ProxyConfig::default()
        };
        let performance = Arc::new(StdMutex::new(Vec::new()));
        let observers = ObserverHub::new(vec![(
            Arc::new(PqcObserver(performance.clone())),
            transmog_core::observe::ObserverConfig::default(),
        )]);
        let components = ProxyComponents::new(
            InterceptorChainFactory::new(vec![], config.limits.hooks),
            Arc::new(CachedMitmCertificateResolver::new(ca, 16, 1).unwrap()),
        )
        .with_observers(observers);
        let proxy = ProxyServer::bind_with_components(config, trust, components)
            .await
            .unwrap();
        let proxy_address = proxy.local_addr().unwrap();
        let mut evidence = proxy.subscribe_evidence();
        let (shutdown, stopped) = oneshot::channel();
        let proxy_task = tokio::spawn(proxy.serve(async move {
            let _ = stopped.await;
        }));
        let (mut client, client_task) =
            connect_client(address, proxy_address, client_trust, browser_hybrid).await;
        for _ in 0..2 {
            let response = client
                .send_request(
                    Request::builder()
                        .uri("/pqc")
                        .header(HOST, format!("localhost:{}", address.port()))
                        .header("x-from-breakpoint", "yes")
                        .body(Full::new(Bytes::new()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                !response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .is_empty()
            );
            let proof = evidence.recv().await.unwrap();
            assert_eq!(
                proof.egress_version,
                match route {
                    RoutePolicy::Http1Only => HttpLegVersion::Http1,
                    RoutePolicy::Http2Only => HttpLegVersion::Http2,
                    _ => HttpLegVersion::Http3,
                }
            );
        }
        timeout(Duration::from_secs(3), async {
            loop {
                if performance
                    .lock()
                    .unwrap()
                    .iter()
                    .flat_map(|sample| &sample.transports)
                    .any(|transport| transport.leg == "upstream" && transport.shared)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let recorded = performance.lock().unwrap().clone();
        drop(client);
        client_task.abort();
        let _ = client_task.await;
        shutdown.send(()).unwrap();
        timeout(Duration::from_secs(5), proxy_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        origin.finish().await;
        verify_facts(&recorded, browser_hybrid, origin_hybrid);
    }

    type PqcClient = hyper::client::conn::http1::SendRequest<Full<Bytes>>;
    async fn connect_client(
        address: SocketAddr,
        proxy_address: SocketAddr,
        client_trust: Arc<TrustSnapshot>,
        browser_hybrid: bool,
    ) -> (PqcClient, JoinHandle<Result<(), hyper::Error>>) {
        let mut tcp = TcpStream::connect(proxy_address).await.unwrap();
        tcp.write_all(
            format!(
                "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
                address.port(),
                address.port()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        assert!(read_http_head(&mut tcp).await.starts_with(b"HTTP/1.1 200"));
        let mut context =
            UpstreamTlsContextFactory::new(client_trust, UpstreamTlsPolicy::default())
                .hyper_connector_builder(b"\x08http/1.1")
                .unwrap();
        if !browser_hybrid {
            context.set_curves_list("X25519:P-256:P-384").unwrap();
            context
                .set_verify_algorithm_prefs(&[SslSignatureAlgorithm::ECDSA_SECP256R1_SHA256])
                .unwrap();
        }
        let tls = tokio_boring::connect(context.build().configure().unwrap(), "localhost", tcp)
            .await
            .unwrap();
        assert_eq!(
            tls.ssl().curve(),
            Some(if browser_hybrid { 0x11ec } else { 29 })
        );
        let (client, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .unwrap();
        (client, tokio::spawn(connection))
    }

    fn verify_facts(
        recorded: &[transmog_core::performance::PerformanceEvidence],
        browser_hybrid: bool,
        origin_hybrid: bool,
    ) {
        for (leg, hybrid, signature) in [
            ("client", browser_hybrid, "Local handshake signature"),
            ("upstream", origin_hybrid, "Peer handshake signature"),
        ] {
            let tls = recorded
                .iter()
                .flat_map(|sample| &sample.transports)
                .find(|transport| {
                    transport.leg == leg
                        && transport
                            .tls
                            .as_ref()
                            .is_some_and(|tls| tls.details.contains_key("Negotiated group"))
                })
                .unwrap()
                .tls
                .as_ref()
                .unwrap();
            assert_eq!(
                tls.details["Negotiated group"],
                if hybrid { "X25519MLKEM768" } else { "X25519" }
            );
            assert_eq!(
                tls.details[signature],
                if hybrid {
                    "mldsa44"
                } else {
                    "ecdsa_secp256r1_sha256"
                }
            );
            let key = boring::x509::X509::from_der(&tls.server_chain[0].der)
                .unwrap()
                .public_key()
                .unwrap();
            assert_eq!(
                MlDsaVariant::for_key(&key),
                hybrid.then_some(MlDsaVariant::MlDsa44)
            );
        }
    }

    #[tokio::test]
    async fn pqc_records_independent_legs_and_reused_connections_for_h1_h2_h3() {
        for route in [
            RoutePolicy::Http1Only,
            RoutePolicy::Http2Only,
            RoutePolicy::Http3Only,
        ] {
            for (browser, origin) in [(true, true), (true, false), (false, true), (false, false)] {
                case(route, browser, origin).await;
            }
        }
    }
}

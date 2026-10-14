//! Accepted and rejected ECH handshakes exercise the production measured adapter.
use boring::{
    hpke::HpkeKey,
    ssl::{SslAcceptor, SslEchKeys, SslMethod},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use transmog_core::performance::PerformanceRecorder;
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory,
    UpstreamTlsPolicy,
    ech::{EchCandidatesLookup, EchConfigSource, EchLookup, EchOffer},
};
mod support;

#[derive(Debug)]
struct Fixed {
    config: Arc<[u8]>,
    port: u16,
}
impl EchConfigSource for Fixed {
    fn lookup(&self, _: String, _: u16, _: Vec<String>) -> EchLookup {
        let offer = EchOffer {
            config: self.config.clone(),
            endpoint: "127.0.0.1".into(),
            port: self.port,
        };
        Box::pin(async move { Some(offer) })
    }
}
#[derive(Debug)]
struct Alternatives(Vec<EchOffer>);
impl EchConfigSource for Alternatives {
    fn lookup(&self, _: String, _: u16, _: Vec<String>) -> EchLookup {
        let offer = self.0.first().cloned();
        Box::pin(async move { offer })
    }
    fn lookup_candidates(&self, _: String, _: u16, _: Vec<String>) -> EchCandidatesLookup {
        let offers = self.0.clone();
        Box::pin(async move { offers })
    }
}
fn configs(seed: u8) -> (Vec<u8>, SslEchKeys) {
    let key = HpkeKey::dhkem_p256_sha256(&[seed; 32]).unwrap();
    let config =
        transmog_tls_native::marshal_ech_config(&key, "public.example.test", seed).unwrap();
    let mut keys = SslEchKeys::builder().unwrap();
    keys.add_key(true, &config, key).unwrap();
    let mut list = u16::try_from(config.len()).unwrap().to_be_bytes().to_vec();
    list.extend_from_slice(&config);
    (list, keys.build())
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Real rejection/rotation cases share fixture ownership and evidence assertions.
async fn ech_acceptance_rotation_rollback_and_unauthenticated_rejection() {
    for scenario in [
        "accepted",
        "alternatives",
        "rotation",
        "rollback",
        "rollback-tls12",
        "untrusted",
        "untrusted-alternatives",
    ] {
        let ca = ProxyCa::generate("ECH fixture", 2).unwrap();
        let wrong = ProxyCa::generate("Untrusted public name", 2).unwrap();
        let (initial, _) = configs(7);
        let (_, current) = configs(if matches!(scenario, "accepted" | "alternatives") {
            7
        } else {
            8
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        let mut servers = Vec::new();
        let untrusted = scenario.starts_with("untrusted");
        let rejected = !matches!(scenario, "accepted" | "alternatives");
        for phase in 0..if rejected { 2 } else { 1 } {
            let root = if untrusted && phase == 0 { &wrong } else { &ca };
            let name = if rejected && phase == 0 {
                "public.example.test"
            } else {
                "hidden.example.test"
            };
            let leaf = root
                .issue(EndpointIdentity::parse(name).unwrap(), 1)
                .unwrap();
            let mut tls = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
            tls.set_certificate(&leaf.certificate).unwrap();
            tls.set_private_key(&leaf.private_key).unwrap();
            if !scenario.starts_with("rollback") {
                tls.set_ech_keys(&current).unwrap();
            }
            if scenario == "rollback-tls12" {
                tls.set_max_proto_version(Some(boring::ssl::SslVersion::TLS1_2))
                    .unwrap();
                tls.set_alpn_select_callback(|_, offered| {
                    boring::ssl::select_next_proto(b"\x08http/1.1", offered)
                        .ok_or(boring::ssl::AlpnError::NOACK)
                });
            }
            servers.push(tls.build());
        }
        let counted = connections.clone();
        let received = requests.clone();
        let server = tokio::spawn(async move {
            for tls in servers {
                let Ok(Ok((socket, _))) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await
                else {
                    break;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                let Ok(Ok(tls)) = tokio::time::timeout(
                    Duration::from_secs(2),
                    tokio_boring::accept(&tls, socket),
                )
                .await
                else {
                    continue;
                };
                let received = received.clone();
                let service = hyper::service::service_fn(move |_| {
                    received.fetch_add(1, Ordering::SeqCst);
                    async {
                        Ok::<_, std::convert::Infallible>(http::Response::new(
                            http_body_util::Full::new(bytes::Bytes::from_static(b"ECH response")),
                        ))
                    }
                });
                let _ = tokio::time::timeout(
                    Duration::from_millis(500),
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(tls), service),
                )
                .await;
            }
        });
        let trust =
            PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "ECH fixture").unwrap();
        let source: Arc<dyn EchConfigSource> = if scenario.ends_with("alternatives") {
            let (other, _) = configs(8);
            let offers = if untrusted {
                vec![
                    EchOffer {
                        config: initial.clone().into(),
                        endpoint: "127.0.0.1".into(),
                        port,
                    },
                    EchOffer {
                        config: other.into(),
                        endpoint: "127.0.0.1".into(),
                        port,
                    },
                ]
            } else {
                vec![
                    EchOffer {
                        config: other.into(),
                        endpoint: "127.0.0.2".into(),
                        port,
                    },
                    EchOffer {
                        config: initial.clone().into(),
                        endpoint: "127.0.0.1".into(),
                        port,
                    },
                ]
            };
            Arc::new(Alternatives(offers))
        } else {
            Arc::new(Fixed {
                config: initial.into(),
                port,
            })
        };
        let factory = UpstreamTlsContextFactory::new(
            Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
            UpstreamTlsPolicy::default(),
        )
        .with_ech_config_source(source);
        let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
        let client = HyperOriginClient::new(&factory)
            .unwrap()
            .with_performance(Some(recorder.clone()));
        let mut request = support::request(port);
        request.head.target.host = "hidden.example.test".into();
        request.head.target.authority = format!("hidden.example.test:{port}");
        let result = client
            .execute(
                request,
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(
            result.is_ok(),
            !untrusted,
            "{scenario}: {:?}",
            result.as_ref().err().map(ToString::to_string)
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            usize::from(!untrusted),
            "HTTP reached the rejection connection"
        );
        assert_eq!(
            connections.load(Ordering::SeqCst),
            if scenario == "rotation" || scenario.starts_with("rollback") {
                2
            } else {
                1
            },
            "{scenario}"
        );
        let evidence = recorder.snapshot();
        if untrusted {
            assert!(
                evidence
                    .transports
                    .iter()
                    .filter_map(|t| t.tls.as_ref())
                    .all(|tls| !tls
                        .details
                        .values()
                        .any(|value| value.contains("authenticated retry available")))
            );
        } else {
            assert!(
                evidence
                    .transports
                    .iter()
                    .filter_map(|t| t.tls.as_ref())
                    .any(|tls| tls
                        .details
                        .get("ECH service endpoint")
                        .is_some_and(|v| v == "127.0.0.1"))
            );
            assert!(
                evidence
                    .transports
                    .iter()
                    .filter_map(|t| t.tls.as_ref())
                    .any(|tls| tls.details.get("Encrypted Client Hello").is_some_and(
                        |value| value
                            == if scenario.starts_with("rollback") {
                                "Not negotiated"
                            } else {
                                "Accepted"
                            }
                    )),
                "{scenario}: {:?}",
                evidence.transports
            );
            if scenario == "rollback-tls12" {
                assert!(
                    evidence
                        .transports
                        .iter()
                        .filter_map(|t| t.tls.as_ref())
                        .any(|tls| tls
                            .details
                            .get("TLS False Start")
                            .is_some_and(|v| v == "Used")),
                    "Rollback must exercise an eligible TLS 1.2 False Start handshake"
                );
            }
        }
        server.abort();
    }
}

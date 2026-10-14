//! Live verification and pool invalidation without changing system certificates.
#[path = "support/trust_anchors.rs"]
mod anchors;
mod support;
use boring::{
    pkey::{PKey, Private},
    ssl::{AlpnError, SslAcceptor, SslMethod},
    x509::X509,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use support::local_certificates::certificate as local_certificate;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use transmog_core::{
    TlsEvidence, network_profile::NetworkProfileSnapshot, performance::PerformanceRecorder,
};
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_tls::{
    ProxyCa, UpstreamTlsContextFactory,
    local_trust::{LocalCertificates, LocalTrustMonitor},
};

fn factory(public: &ProxyCa, local: LocalCertificates) -> UpstreamTlsContextFactory {
    anchors::factory(public, anchors::catalog(&[(public.certificate(), 1)], 42))
        .with_network_profile(NetworkProfileSnapshot::default())
        .unwrap()
        .with_local_trust_monitor(LocalTrustMonitor::fixed(local))
}
fn acceptor(cert: &X509, key: &PKey<Private>, chain: &[X509], h2: bool) -> SslAcceptor {
    let mut context = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    context.set_certificate(cert).unwrap();
    context.set_private_key(key).unwrap();
    for cert in chain {
        context.add_extra_chain_cert(cert.clone()).unwrap();
    }
    context
        .set_alpn_select_callback(move |_, _| if h2 { Ok(b"h2") } else { Err(AlpnError::NOACK) });
    context.build()
}
async fn probe(
    factory: &UpstreamTlsContextFactory,
    cert: &X509,
    key: &PKey<Private>,
    chain: &[X509],
) -> (bool, TlsEvidence) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = acceptor(cert, key, chain, false);
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        if let Ok(mut io) = tokio_boring::accept(&acceptor, tcp).await {
            let mut data = [0; 4096];
            assert!(io.read(&mut data).await.unwrap() > 0);
            io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        }
    });
    let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
    let client = HyperOriginClient::new(factory)
        .unwrap()
        .with_performance(Some(recorder.clone()));
    let okay = client
        .execute(
            support::request(port),
            HyperEgressMode::Http1Only,
            1024,
            Duration::from_secs(3),
        )
        .await
        .is_ok();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
    let evidence = recorder
        .snapshot()
        .transports
        .into_iter()
        .find_map(|t| t.tls)
        .unwrap();
    (okay, evidence)
}
fn self_signed(name: &str, ca: bool, expired: bool) -> (X509, PKey<Private>) {
    local_certificate(name, ca, expired, None, None, None)
}

#[tokio::test]
async fn local_root_path_length_limits_issuer_paths() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    for limit in [0, 1] {
        let (cert, key) = local_certificate("limited root", true, false, None, Some(limit), None);
        let root = ProxyCa::from_pem(
            &cert.to_pem().unwrap(),
            &key.private_key_to_pem_pkcs8().unwrap(),
        )
        .unwrap();
        let trust = factory(
            &public,
            LocalCertificates {
                anchors: vec![cert.to_der().unwrap()],
                ..Default::default()
            },
        );
        let first = anchors::intermediate(&root);
        let (second, key) =
            local_certificate("second intermediate", true, false, Some(&first), None, None);
        let second = ProxyCa::from_pem(
            &second.to_pem().unwrap(),
            &key.private_key_to_pem_pkcs8().unwrap(),
        )
        .unwrap();
        for (issuer, chain, count) in [
            (&root, vec![], 0),
            (&first, vec![first.certificate().clone()], 1),
            (
                &second,
                vec![second.certificate().clone(), first.certificate().clone()],
                2,
            ),
        ] {
            let leaf = issuer
                .issue(
                    transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
                    1,
                )
                .unwrap();
            let (accepted, evidence) =
                probe(&trust, &leaf.certificate, &leaf.private_key, &chain).await;
            assert_eq!(
                accepted,
                count <= limit,
                "pathLen={limit}, intermediates={count}"
            );
            assert_eq!(
                evidence.verification == Some(transmog_core::VerificationResult::Verified),
                count <= limit
            );
        }
        // A self-issued key rollover does not consume a path-length slot.
        let (cert, key) = local_certificate("limited root", true, false, Some(&root), None, None);
        assert!(!cert.verify(&key).unwrap());
        let rollover = ProxyCa::from_pem(
            &cert.to_pem().unwrap(),
            &key.private_key_to_pem_pkcs8().unwrap(),
        )
        .unwrap();
        let leaf = rollover
            .issue(
                transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
                1,
            )
            .unwrap();
        assert!(
            probe(&trust, &leaf.certificate, &leaf.private_key, &[cert])
                .await
                .0
        );
    }
}

#[tokio::test]
async fn non_self_signed_root_ca_is_an_issuer_anchor_but_not_an_exact_leaf() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let unknown = ProxyCa::generate("untrusted issuer", 2).unwrap();
    let (cert, key) = local_certificate("localhost", true, false, Some(&unknown), None, None);
    assert!(!cert.verify(&key).unwrap());
    let trust = factory(
        &public,
        LocalCertificates {
            anchors: vec![cert.to_der().unwrap()],
            ..Default::default()
        },
    );
    assert!(!probe(&trust, &cert, &key, &[]).await.0);
    let issuer = ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap();
    let leaf = issuer
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    assert!(
        probe(&trust, &leaf.certificate, &leaf.private_key, &[])
            .await
            .0
    );
    let (cert, key) = self_signed("localhost", true, false);
    let trust = factory(
        &public,
        LocalCertificates {
            anchors: vec![cert.to_der().unwrap()],
            ..Default::default()
        },
    );
    assert!(probe(&trust, &cert, &key, &[]).await.0);
}

#[tokio::test]
async fn exact_local_leaf_trust_compares_canonical_names_and_checks_self_signature() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let other = ProxyCa::generate("localhost", 2).unwrap();
    for signer in [None, Some(&other)] {
        let (cert, key) =
            local_certificate("LOCALHOST", false, false, signer, None, Some("localhost"));
        assert_ne!(
            cert.subject_name().to_der().unwrap(),
            cert.issuer_name().to_der().unwrap()
        );
        assert_eq!(
            cert.verify(&cert.public_key().unwrap()).unwrap_or(false),
            signer.is_none()
        );
        assert_eq!(cert.verify(&key).unwrap(), signer.is_none());
        for root_role in [false, true] {
            let mut inventory = LocalCertificates::default();
            if root_role {
                inventory.anchors.push(cert.to_der().unwrap());
            } else {
                inventory.trusted_leaves.push(cert.to_der().unwrap());
            }
            let (accepted, _) = probe(&factory(&public, inventory), &cert, &key, &[]).await;
            assert_eq!(accepted, signer.is_none());
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One live role matrix shares the same generated roots.
async fn distinct_local_roles_preserve_public_trust_and_explicit_distrust() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let local = ProxyCa::generate("local root", 2).unwrap();
    let leaf = local
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    let der = local.certificate().to_der().unwrap();
    for (inventory, expected) in [
        (LocalCertificates::default(), false),
        (
            LocalCertificates {
                anchors: vec![der.clone()],
                ..Default::default()
            },
            true,
        ),
        (
            LocalCertificates {
                intermediates: vec![der.clone()],
                ..Default::default()
            },
            false,
        ),
        (
            LocalCertificates {
                trusted_leaves: vec![der.clone()],
                ..Default::default()
            },
            false,
        ),
        (
            LocalCertificates {
                trusted_leaves: vec![leaf.certificate.to_der().unwrap()],
                ..Default::default()
            },
            false,
        ),
        (
            LocalCertificates {
                anchors: vec![der.clone()],
                disallowed: vec![der.clone()],
                ..Default::default()
            },
            false,
        ),
        (
            LocalCertificates {
                anchors: vec![der],
                disallowed: vec![leaf.certificate.to_der().unwrap()],
                ..Default::default()
            },
            false,
        ),
    ] {
        assert_eq!(
            probe(
                &factory(&public, inventory),
                &leaf.certificate,
                &leaf.private_key,
                &[]
            )
            .await
            .0,
            expected
        );
    }
    let intermediate = anchors::intermediate(&public);
    let leaf = intermediate
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    let issuer_der = intermediate.certificate().to_der().unwrap();
    for (inventory, expected) in [
        (
            LocalCertificates {
                intermediates: vec![issuer_der.clone()],
                ..Default::default()
            },
            true,
        ),
        (
            LocalCertificates {
                intermediates: vec![issuer_der.clone()],
                disallowed: vec![issuer_der.clone()],
                ..Default::default()
            },
            false,
        ),
        (
            LocalCertificates {
                intermediates: vec![issuer_der.clone()],
                disallowed: vec![public.certificate().to_der().unwrap()],
                ..Default::default()
            },
            false,
        ),
    ] {
        let mut profile = NetworkProfileSnapshot::default();
        profile.settings.trust_anchor_ids = false;
        let factory = factory(&public, inventory.clone())
            .with_network_profile(profile)
            .unwrap()
            .with_local_trust_monitor(LocalTrustMonitor::fixed(inventory));
        assert_eq!(
            probe(&factory, &leaf.certificate, &leaf.private_key, &[])
                .await
                .0,
            expected
        );
    }
}

#[tokio::test]
async fn exact_self_signed_local_leaves_still_require_server_name_and_expiry() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    for (name, expired, expected) in [
        ("localhost", false, true),
        ("wrong.test", false, false),
        ("localhost", true, false),
    ] {
        let (cert, key) = self_signed(name, false, expired);
        for root_role in [false, true] {
            let inventory = if root_role {
                LocalCertificates {
                    anchors: vec![cert.to_der().unwrap()],
                    ..Default::default()
                }
            } else {
                LocalCertificates {
                    trusted_leaves: vec![cert.to_der().unwrap()],
                    ..Default::default()
                }
            };
            let (okay, evidence) = probe(&factory(&public, inventory), &cert, &key, &[]).await;
            assert_eq!(
                okay, expected,
                "{name}, expired {expired}, ROOT {root_role}: {:?}",
                evidence.errors
            );
        }
    }
}

#[tokio::test]
async fn local_override_respects_profile_switches_and_expired_anchor_validity() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let local = ProxyCa::generate("local root", 2).unwrap();
    let leaf = local
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    let inventory = LocalCertificates {
        anchors: vec![local.certificate().to_der().unwrap()],
        ..Default::default()
    };
    let mut profile = NetworkProfileSnapshot::default();
    profile.settings.local_certificates = false;
    let public_only = factory(&public, inventory.clone())
        .with_network_profile(profile.clone())
        .unwrap();
    assert!(
        !probe(&public_only, &leaf.certificate, &leaf.private_key, &[])
            .await
            .0
    );
    profile.settings.local_certificates = true;
    profile.settings.root_store = transmog_core::network_profile::RootStore::Os;
    let os_only = factory(&public, inventory)
        .with_network_profile(profile)
        .unwrap();
    assert!(
        !probe(&os_only, &leaf.certificate, &leaf.private_key, &[])
            .await
            .0
    );
    let (cert, key) = self_signed("expired local root", true, true);
    let expired = ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap();
    let leaf = expired
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    let trust = factory(
        &public,
        LocalCertificates {
            anchors: vec![cert.to_der().unwrap()],
            ..Default::default()
        },
    );
    assert!(
        !probe(&trust, &leaf.certificate, &leaf.private_key, &[])
            .await
            .0
    );
}

struct Origin {
    port: u16,
    count: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Origin {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn origin(local: &ProxyCa, h2: bool) -> Origin {
    let leaf = local
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    let acceptor = acceptor(&leaf.certificate, &leaf.private_key, &[], h2);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let notified = started.clone();
    let released = release.clone();
    let task = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted=listener.accept() => {
                    let (tcp,_)=accepted.unwrap(); let acceptor=acceptor.clone(); let counted=counted.clone(); let started=notified.clone(); let release=released.clone();
                    tasks.spawn(async move {
                        let Ok(tls)=tokio_boring::accept(&acceptor,tcp).await else {return;};
                        let number=counted.fetch_add(1,Ordering::SeqCst)+1;
                        let service=hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                            let started=started.clone(); let release=release.clone(); async move {
                                if request.uri().path()=="/slow" { started.notify_one(); release.notified().await; }
                                Ok::<_,std::convert::Infallible>(http::Response::new(http_body_util::Full::new(bytes::Bytes::from(number.to_string()))))
                            }
                        });
                        let io=hyper_util::rt::TokioIo::new(tls);
                        if h2 { let _=hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new()).serve_connection(io,service).await; }
                        else { let _=hyper::server::conn::http1::Builder::new().serve_connection(io,service).await; }
                    });
                }
                _=tasks.join_next(), if !tasks.is_empty()=>{}
            }
        }
    });
    Origin {
        port,
        count,
        started,
        release,
        task,
    }
}

#[tokio::test]
async fn local_removal_retires_h1_h2_pools_and_preserves_admitted_work_and_evidence() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let local = ProxyCa::generate("local root", 2).unwrap();
    let inventory = LocalCertificates {
        anchors: vec![local.certificate().to_der().unwrap()],
        ..Default::default()
    };
    for h2 in [false, true] {
        let origin = origin(&local, h2).await;
        let (monitor, publisher) = LocalTrustMonitor::controlled(inventory.clone());
        let factory = factory(&public, inventory.clone()).with_local_trust_monitor(monitor);
        let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
        let client = HyperOriginClient::new(&factory)
            .unwrap()
            .with_performance(Some(recorder.clone()));
        let mode = if h2 {
            HyperEgressMode::Http2Only
        } else {
            HyperEgressMode::Http1Only
        };
        let request = support::request(origin.port);
        assert!(
            client
                .execute(request.clone(), mode, 1024, Duration::from_secs(3))
                .await
                .is_ok()
        );
        let old = recorder.snapshot().transports[0].tls.clone().unwrap();
        let mut slow = request.clone();
        slow.head.target.path = "/slow".into();
        let admitted = client.clone();
        let running = tokio::spawn(async move {
            admitted
                .execute(slow, mode, 1024, Duration::from_secs(5))
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), origin.started.notified())
            .await
            .unwrap();
        publisher.replace(LocalCertificates::default());
        assert!(
            client
                .execute(request.clone(), mode, 1024, Duration::from_secs(3))
                .await
                .is_err()
        );
        origin.release.notify_one();
        assert!(running.await.unwrap().is_ok());
        publisher.replace(inventory.clone());
        assert!(
            client
                .execute(request, mode, 1024, Duration::from_secs(3))
                .await
                .is_ok()
        );
        assert_eq!(origin.count.load(Ordering::SeqCst), 2);
        assert_eq!(old.details["Local trust generation"], "1");
        assert_eq!(
            recorder.snapshot().transports[0]
                .tls
                .as_ref()
                .unwrap()
                .details["Local trust SHA-256"],
            old.details["Local trust SHA-256"]
        );
        assert!(
            recorder
                .snapshot()
                .transports
                .iter()
                .filter_map(|t| t.tls.as_ref())
                .any(|t| t
                    .details
                    .get("Local trust generation")
                    .is_some_and(|v| v == "3"))
        );
    }
}

#[tokio::test]
async fn cached_tls_session_cannot_bypass_a_local_root_removal() {
    use http_body_util::BodyExt;
    let public = ProxyCa::generate("public root", 2).unwrap();
    let local = ProxyCa::generate("local session root", 2).unwrap();
    let origin = origin(&local, false).await;
    let inventory = LocalCertificates {
        anchors: vec![local.certificate().to_der().unwrap()],
        ..Default::default()
    };
    let (monitor, publisher) = LocalTrustMonitor::controlled(inventory.clone());
    let mut profile = NetworkProfileSnapshot::default();
    profile.settings.tls_versions = transmog_core::network_profile::TlsVersions::Tls12Only;
    profile.settings.protocols.http3 = false;
    profile.settings.trust_anchor_ids = false;
    let factory = factory(&public, inventory)
        .with_local_trust_monitor(monitor)
        .with_network_profile(profile)
        .unwrap();
    factory.prepare_trust().await.unwrap();
    let captures = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = captures.clone();
    let mut tcp = hyper_util::client::legacy::connect::HttpConnector::new();
    tcp.enforce_http(false);
    let mut connector = hyper_boring::HttpsConnector::with_connector(
        tcp,
        factory.hyper_connector_builder(b"\x08http/1.1").unwrap(),
    )
    .unwrap();
    connector.set_ssl_callback(move |ssl, _| {
        let frozen = factory.for_connection();
        frozen.configure_ssl_metadata(ssl).unwrap();
        let capture = frozen.capture(Some("localhost".into()), vec!["http/1.1".into()]);
        capture.attach_ssl(ssl)?;
        recorded.lock().unwrap().push(capture);
        Ok(())
    });
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .pool_max_idle_per_host(0)
        .build::<_, http_body_util::Empty<bytes::Bytes>>(connector);
    let uri: http::Uri = format!("https://localhost:{}/", origin.port)
        .parse()
        .unwrap();
    for _ in 0..2 {
        let response = tokio::time::timeout(Duration::from_secs(3), client.get(uri.clone()))
            .await
            .unwrap()
            .unwrap();
        response.into_body().collect().await.unwrap();
    }
    assert_eq!(
        captures.lock().unwrap()[1].snapshot().details["Session resumed"],
        "Yes"
    );
    publisher.replace(LocalCertificates::default());
    assert!(
        tokio::time::timeout(Duration::from_secs(3), client.get(uri))
            .await
            .unwrap()
            .is_err()
    );
    let evidence = captures.lock().unwrap();
    assert_eq!(evidence.len(), 3);
    assert!(matches!(
        evidence[2].snapshot().verification,
        Some(transmog_core::VerificationResult::Rejected { .. })
    ));
    assert_eq!(
        evidence[2].snapshot().details["Local trust generation"],
        "2"
    );
}

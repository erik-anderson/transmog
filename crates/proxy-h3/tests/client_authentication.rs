//! Real QUIC peers exercise authorization, interactive ownership and unrelated traffic.
#[path = "support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use quic::origin;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use support::{Fixture, request};
use transmog_core::{
    BodyFrame,
    client_authentication::{
        ClientCertificateAction, ClientCertificatePolicy, ClientCertificateRule,
        ClientCertificateSelector,
    },
    connection_policy::NetworkIsolationKey,
};
use transmog_h3::{H3OriginClient, H3TransportLimits};
use transmog_tls::{
    PemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    client_authentication::PolicyClientAuthenticationResolver,
    client_prompts::{ClientCertificateAnswer, ClientCertificateBroker},
};
fn client(f: &Fixture, resolver: Arc<PolicyClientAuthenticationResolver>) -> H3OriginClient {
    client_with_limits(f, resolver, H3TransportLimits::default())
}
fn client_with_limits(
    f: &Fixture,
    resolver: Arc<PolicyClientAuthenticationResolver>,
    limits: H3TransportLimits,
) -> H3OriginClient {
    let trust = PemTrustSource::from_pem(&f.ca.certificate_pem().unwrap(), "QUIC fixture").unwrap();
    H3OriginClient::new(
        UpstreamTlsContextFactory::new(
            Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
            UpstreamTlsPolicy::default(),
        )
        .with_client_certificate_resolver(resolver, "process:1"),
        limits,
    )
}
async fn execute(c: &H3OriginClient, logical: u16, peer: u16) -> String {
    tokio::time::timeout(Duration::from_secs(3), execute_request(c, logical, peer))
        .await
        .expect("HTTP/3 fixture request did not complete")
}
async fn execute_request(c: &H3OriginClient, logical: u16, peer: u16) -> String {
    let result = c.execute(request(logical), peer, 1024).await.unwrap();
    let mut bytes = Vec::new();
    for frame in result.response.body {
        if let BodyFrame::Data(data) = frame {
            bytes.extend(data);
        }
    }
    String::from_utf8(bytes).unwrap()
}
fn policy(action: ClientCertificateAction) -> ClientCertificatePolicy {
    ClientCertificatePolicy {
        rules: vec![ClientCertificateRule {
            id: "host".into(),
            enabled: true,
            host: "localhost".into(),
            port: 443,
            mode: action,
        }],
        ..Default::default()
    }
}
async fn pending(b: &ClientCertificateBroker, n: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if b.pending().iter().map(|p| p.connections).sum::<usize>() == n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn real_h3_uses_logical_origin_and_retires_authorization_and_caller_pools() {
    let fixture = Fixture::new();
    let server = origin(&fixture).await;
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            policy(ClientCertificateAction::Automatic {
                selector: ClientCertificateSelector {
                    identity: Some("test-client".into()),
                    ..Default::default()
                },
            }),
            fixture.source.clone(),
            None,
        )
        .unwrap(),
    );
    let evidence = transmog_core::performance::PerformanceRecorder::new(
        std::time::SystemTime::now(),
        std::time::Instant::now(),
    );
    let client = client(&fixture, resolver.clone()).with_performance(Some(evidence.clone()));
    assert_eq!(execute(&client, 443, server.port).await, "1:true");
    let snapshot = evidence.snapshot();
    let tls = snapshot.transports.last().unwrap().tls.as_ref().unwrap();
    assert_eq!(tls.client_chain.len(), 1);
    assert_eq!(
        tls.client_chain[0].sha256,
        fixture.source.credential.fingerprint().unwrap()
    );
    assert_eq!(execute(&client, 443, server.port).await, "1:true");
    let other = client
        .clone()
        .with_network_isolation(NetworkIsolationKey::new("process:2"));
    assert_eq!(execute(&other, 443, server.port).await, "2:true");
    resolver
        .replace_policy(ClientCertificatePolicy::default())
        .unwrap();
    assert_eq!(execute(&client, 443, server.port).await, "3:false");
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn pending_h3_does_not_lock_out_unrelated_origins_and_unregisters_abandoned_owner() {
    let fixture = Fixture::new();
    let blocked = quic::origin_with_idle_timeout(&fixture, Duration::from_secs(15)).await;
    let independent = origin(&fixture).await;
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            policy(ClientCertificateAction::Prompt),
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let client = client(&fixture, resolver);
    let clone = client.clone();
    let port = blocked.port;
    // An intentional certificate prompt must not consume the completion budget
    // while the independent origin performs its own real QUIC handshake.
    let waiting = tokio::spawn(async move { execute_request(&clone, 443, port).await });
    pending(&broker, 1).await;
    assert_eq!(execute(&client, 8443, independent.port).await, "1:false");
    assert!(broker.answer(
        broker.pending()[0].id,
        ClientCertificateAnswer::Identity("test-client".into())
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), waiting)
            .await
            .expect("Selected HTTP/3 request did not complete")
            .unwrap(),
        "1:true"
    );
    let clone = client.clone();
    let waiting = tokio::spawn(async move { execute_request(&clone, 443, port).await });
    pending(&broker, 1).await;
    waiting.abort();
    let _ = waiting.await;
    pending(&broker, 0).await;
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 1);
    assert_eq!(blocked.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn missing_pins_and_renewal_rules_do_not_reuse_anonymous_or_server_specific_choices() {
    for identity in [Some("missing".into()), None] {
        let fixture = Fixture::new();
        let server = origin(&fixture).await;
        let authenticated = identity.is_none();
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                policy(ClientCertificateAction::Automatic {
                    selector: ClientCertificateSelector {
                        identity,
                        subject: [("CN".into(), "Client".into())].into(),
                        ..Default::default()
                    },
                }),
                fixture.source.clone(),
                None,
            )
            .unwrap(),
        );
        let client = client(&fixture, resolver);
        assert_eq!(
            execute(&client, 443, server.port).await,
            format!("1:{authenticated}")
        );
        assert_eq!(
            execute(&client, 443, server.port).await,
            format!("2:{authenticated}")
        );
    }
}

#[tokio::test]
async fn ephemeral_h3_handshakes_share_the_physical_connection_bound_and_shutdown_cleans_up() {
    let fixture = Fixture::new();
    let server = origin(&fixture).await;
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            policy(ClientCertificateAction::Prompt),
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let trust =
        PemTrustSource::from_pem(&fixture.ca.certificate_pem().unwrap(), "fixture").unwrap();
    let client = H3OriginClient::new(
        UpstreamTlsContextFactory::new(
            Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
            UpstreamTlsPolicy::default(),
        )
        .with_client_certificate_resolver(resolver, "process:1"),
        H3TransportLimits {
            max_pool_entries: 1,
            ..Default::default()
        },
    );
    let clone = client.clone();
    let port = server.port;
    let first = tokio::spawn(async move { clone.execute(request(443), port, 1024).await });
    pending(&broker, 1).await;
    assert!(matches!(
        client.execute(request(443), port, 1024).await,
        Err(transmog_h3::H3OriginError::PoolCapacityExceeded { limit: 1 })
    ));
    let stale = broker.pending()[0].id;
    broker.shutdown();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    pending(&broker, 0).await;
    assert!(!broker.answer(stale, ClientCertificateAnswer::NoCertificate));
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn quic_peer_idle_timeout_unregisters_its_pending_decision() {
    let fixture = Fixture::new();
    let server = origin(&fixture).await;
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            policy(ClientCertificateAction::Prompt),
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let client = client(&fixture, resolver);
    let port = server.port;
    let waiting = tokio::spawn(async move { client.execute(request(443), port, 1024).await });
    pending(&broker, 1).await;
    let id = broker.pending()[0].id;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap(),
        Err(transmog_h3::H3OriginError::ClientAuthenticationFailed)
    ));
    pending(&broker, 0).await;
    assert!(!broker.answer(id, ClientCertificateAnswer::NoCertificate));
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_authorization_epochs_release_capacity_before_replacement() {
    let fixture = Fixture::new();
    let server = origin(&fixture).await;
    let automatic = policy(ClientCertificateAction::Automatic {
        selector: ClientCertificateSelector {
            identity: Some("test-client".into()),
            ..Default::default()
        },
    });
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(automatic.clone(), fixture.source.clone(), None)
            .unwrap(),
    );
    let client = client_with_limits(
        &fixture,
        resolver.clone(),
        H3TransportLimits {
            max_pool_entries: 1,
            ..Default::default()
        },
    );
    for id in 1..=8 {
        let authenticated = id % 2 == 1;
        if id > 1 {
            resolver
                .replace_policy(if authenticated {
                    automatic.clone()
                } else {
                    ClientCertificatePolicy::default()
                })
                .unwrap();
        }
        let expected = format!("{id}:{authenticated}");
        assert_eq!(execute(&client, 443, server.port).await, expected);
        assert_eq!(execute(&client, 443, server.port).await, expected);
    }
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn retiring_an_active_epoch_preserves_streaming_and_the_physical_limit() {
    let fixture = Fixture::new();
    let server = quic::paused_origin(&fixture).await;
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            policy(ClientCertificateAction::Automatic {
                selector: ClientCertificateSelector {
                    identity: Some("test-client".into()),
                    ..Default::default()
                },
            }),
            fixture.source.clone(),
            None,
        )
        .unwrap(),
    );
    let client = client_with_limits(
        &fixture,
        resolver.clone(),
        H3TransportLimits {
            max_pool_entries: 1,
            ..Default::default()
        },
    );
    let port = server.port;
    let capacity = std::num::NonZeroUsize::new(1).unwrap();
    let (sender, body) = transmog_core::BodyStream::channel(capacity);
    drop(sender);
    let mut old = tokio::time::timeout(
        Duration::from_secs(2),
        client.execute_duplex_streaming(
            transmog_core::StreamingRequest {
                head: request(443).head,
                body,
            },
            port,
            1024,
            capacity,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    resolver
        .replace_policy(ClientCertificatePolicy::default())
        .unwrap();
    assert!(matches!(
        client.execute(request(443), port, 1024).await,
        Err(transmog_h3::H3OriginError::PoolCapacityExceeded { limit: 1 })
    ));
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    server.release.send_replace(true);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), old.response.body.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        BodyFrame::Data(bytes::Bytes::from_static(b"1:true"))
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(2), old.response.body.recv())
            .await
            .unwrap()
            .is_none()
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match client.execute(request(443), port, 1024).await {
                Ok(response) => {
                    assert_eq!(
                        response.response.body,
                        vec![BodyFrame::Data(bytes::Bytes::from_static(b"2:false"))]
                    );
                    break;
                }
                Err(transmog_h3::H3OriginError::PoolCapacityExceeded { limit: 1 }) => {
                    tokio::task::yield_now().await;
                }
                other => panic!("replacement failed: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn invalid_public_client_limits_return_typed_errors_before_network_io() {
    let fixture = Fixture::new();
    let server = origin(&fixture).await;
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            ClientCertificatePolicy::default(),
            fixture.source.clone(),
            None,
        )
        .unwrap(),
    );
    for limits in [
        H3TransportLimits {
            pending_requests_per_connection: 0,
            ..Default::default()
        },
        H3TransportLimits {
            pending_requests_per_connection: usize::MAX,
            ..Default::default()
        },
        H3TransportLimits {
            max_pool_entries: 0,
            ..Default::default()
        },
        H3TransportLimits {
            max_pool_entries: usize::MAX,
            ..Default::default()
        },
    ] {
        let client = client_with_limits(&fixture, resolver.clone(), limits)
            .with_client_certificate_resolver(resolver.clone(), "another caller");
        assert!(matches!(
            client.execute(request(443), server.port, 1024).await,
            Err(transmog_h3::H3OriginError::Config(
                transmog_h3::H3ConfigError::InvalidLimits(_)
            ))
        ));
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn certificate_cancellation_survives_a_dual_stack_address_race() {
    let fixture = Fixture::new();
    let server = quic::origin_at(&fixture, "[::1]:0").await;
    let _blackhole = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, server.port))
        .await
        .unwrap();
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            policy(ClientCertificateAction::Prompt),
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let client = client_with_limits(
        &fixture,
        resolver,
        H3TransportLimits {
            idle_timeout: Duration::from_millis(500),
            ..Default::default()
        },
    );
    let peer = server.port;
    let job = tokio::spawn(async move { client.execute(request(443), peer, 1024).await });
    pending(&broker, 1).await;
    assert!(broker.answer(broker.pending()[0].id, ClientCertificateAnswer::Cancel));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap(),
        Err(transmog_h3::H3OriginError::ClientAuthenticationFailed)
    ));
    pending(&broker, 0).await;
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
}

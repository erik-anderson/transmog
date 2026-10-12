//! Live TLS integration verifies pool boundaries and pending handshake ownership.
mod support;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use support::*;
use transmog_core::client_authentication::{ClientCertificateDefault, ClientCertificatePolicy};
use transmog_http::HyperEgressMode;
use transmog_tls::{
    client_authentication::PolicyClientAuthenticationResolver,
    client_prompts::{ClientCertificateAnswer, ClientCertificateBroker},
};
#[tokio::test]
async fn disabling_pooling_uses_distinct_sequential_and_concurrent_h1_and_h2_connections() {
    for h2 in [false, true] {
        let fixture = Fixture::new();
        let origin = origin(&fixture, h2, false, Duration::from_secs(5)).await;
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                automatic(origin.port),
                fixture.source.clone(),
                None,
            )
            .unwrap(),
        );
        let client = fixture.client(resolver, "process:1");
        assert_eq!(execute(&client, origin.port, h2).await, "1:true");
        assert_eq!(execute(&client, origin.port, h2).await, "1:true");
        client.set_server_connection_reuse(false);
        assert_eq!(execute(&client, origin.port, h2).await, "2:true");
        assert_eq!(execute(&client, origin.port, h2).await, "3:true");
        let (left, right) = tokio::join!(
            execute(&client, origin.port, h2),
            execute(&client, origin.port, h2)
        );
        assert_ne!(left, right);
        assert_eq!(origin.count.load(Ordering::SeqCst), 5);
        client.set_server_connection_reuse(true);
        assert_eq!(execute(&client, origin.port, h2).await, "1:true");
    }
}

#[tokio::test]
async fn authentication_reuses_only_authorized_caller_epoch_and_never_resumes_tls() {
    for h2 in [false, true] {
        let fixture = Fixture::new();
        let origin = origin(&fixture, h2, false, Duration::from_secs(5)).await;
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                automatic(origin.port),
                fixture.source.clone(),
                None,
            )
            .unwrap(),
        );
        let client = fixture.client(resolver.clone(), "process:1");
        assert_eq!(execute(&client, origin.port, h2).await, "1:true");
        assert_eq!(execute(&client, origin.port, h2).await, "1:true");
        let other = client.for_client_certificate_caller("process:2").unwrap();
        assert_eq!(execute(&other, origin.port, h2).await, "2:true");
        resolver
            .replace_policy(ClientCertificatePolicy::default())
            .unwrap();
        assert_eq!(execute(&client, origin.port, h2).await, "3:false");
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.source.reads.load(Ordering::SeqCst), 2);
        assert_eq!(origin.resumed.load(Ordering::SeqCst), 0);
    }
    let fixture = Fixture::new();
    let origin = origin(&fixture, false, true, Duration::from_secs(5)).await;
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            automatic(origin.port),
            fixture.source.clone(),
            None,
        )
        .unwrap(),
    );
    let client = fixture.client(resolver, "process:1");
    assert_eq!(execute(&client, origin.port, false).await, "1:true");
    assert_eq!(execute(&client, origin.port, false).await, "2:true");
    assert_eq!(origin.resumed.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.source.reads.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn prompt_owners_coalesce_resume_and_cancel_without_reusing_their_grant() {
    for h2 in [false, true] {
        let fixture = Fixture::new();
        let origin = origin(&fixture, h2, false, Duration::from_secs(5)).await;
        let broker = ClientCertificateBroker::default();
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                ClientCertificatePolicy {
                    default: ClientCertificateDefault::Prompt,
                    ..Default::default()
                },
                fixture.source.clone(),
                Some(Arc::new(broker.clone())),
            )
            .unwrap(),
        );
        let client = fixture.client(resolver, "process:1");
        let clone = client.clone();
        let port = origin.port;
        let first = tokio::spawn(async move { execute(&clone, port, h2).await });
        let clone = client.clone();
        let second = tokio::spawn(async move { execute(&clone, port, h2).await });
        wait_pending(&broker, 2).await;
        assert_eq!(broker.pending().len(), 1);
        let stale = broker.pending()[0].id;
        first.abort();
        let _ = first.await;
        wait_pending(&broker, 1).await;
        second.abort();
        let _ = second.await;
        wait_pending(&broker, 0).await;
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
        assert!(!broker.answer(
            stale,
            ClientCertificateAnswer::Identity("test-client".into())
        ));
        let clone = client.clone();
        let selected = tokio::spawn(async move { execute(&clone, port, h2).await });
        wait_pending(&broker, 1).await;
        assert!(broker.answer(
            broker.pending()[0].id,
            ClientCertificateAnswer::Identity("test-client".into())
        ));
        assert!(selected.await.unwrap().ends_with(":true"));
        let clone = client.clone();
        let anonymous = tokio::spawn(async move { execute(&clone, port, h2).await });
        wait_pending(&broker, 1).await;
        assert!(broker.answer(
            broker.pending()[0].id,
            ClientCertificateAnswer::NoCertificate
        ));
        assert!(anonymous.await.unwrap().ends_with(":false"));
        assert_eq!(origin.count.load(Ordering::SeqCst), 4);
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn peer_timeout_closes_and_unregisters_a_paused_certificate_request() {
    let fixture = Fixture::new();
    let origin = origin(&fixture, false, false, Duration::from_millis(500)).await;
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            ClientCertificatePolicy {
                default: ClientCertificateDefault::Prompt,
                ..Default::default()
            },
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let client = fixture.client(resolver, "process:1");
    let port = origin.port;
    let pending = tokio::spawn(async move {
        client
            .execute(
                request(port),
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(2),
            )
            .await
    });
    wait_pending(&broker, 1).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    wait_pending(&broker, 0).await;
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn anonymous_fallback_for_a_missing_pin_is_never_reused_as_the_pinned_identity() {
    for h2 in [false, true] {
        let fixture = Fixture::new();
        let origin = origin(&fixture, h2, false, Duration::from_secs(5)).await;
        let mut policy = automatic(origin.port);
        let transmog_core::client_authentication::ClientCertificateAction::Automatic { selector } =
            &mut policy.rules[0].mode
        else {
            unreachable!()
        };
        selector.identity = Some("missing".into());
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(policy, fixture.source.clone(), None).unwrap(),
        );
        let client = fixture.client(resolver, "process:1");
        assert_eq!(execute(&client, origin.port, h2).await, "1:false");
        assert_eq!(execute(&client, origin.port, h2).await, "2:false");
        assert_eq!(fixture.source.reads.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn shutdown_deadline_and_policy_revocation_discard_pending_answers() {
    for action in 0..3 {
        let fixture = Fixture::new();
        let origin = origin(&fixture, false, false, Duration::from_secs(5)).await;
        let broker = ClientCertificateBroker::default();
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                ClientCertificatePolicy {
                    default: ClientCertificateDefault::Prompt,
                    ..Default::default()
                },
                fixture.source.clone(),
                Some(Arc::new(broker.clone())),
            )
            .unwrap(),
        );
        let client = fixture.client(resolver.clone(), "process:1");
        let port = origin.port;
        let pending = tokio::spawn(async move {
            let request = client.execute(
                request(port),
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(2),
            );
            if action == 2 {
                tokio::time::timeout(Duration::from_secs(1), request)
                    .await
                    .is_err()
            } else {
                request.await.is_err()
            }
        });
        wait_pending(&broker, 1).await;
        let id = broker.pending()[0].id;
        match action {
            0 => broker.shutdown(),
            1 => {
                resolver
                    .replace_policy(ClientCertificatePolicy::default())
                    .unwrap();
            }
            _ => {}
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), pending)
                .await
                .unwrap()
                .unwrap()
        );
        wait_pending(&broker, 0).await;
        assert!(!broker.answer(id, ClientCertificateAnswer::NoCertificate));
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn physical_transport_capture_records_only_the_selected_public_wire_chain() {
    for h2 in [false, true] {
        let fixture = Fixture::new();
        let origin = origin(&fixture, h2, false, Duration::from_secs(5)).await;
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                automatic(origin.port),
                fixture.source.clone(),
                None,
            )
            .unwrap(),
        );
        let evidence = transmog_core::performance::PerformanceRecorder::new(
            std::time::SystemTime::now(),
            std::time::Instant::now(),
        );
        let client = fixture
            .client(resolver, "process:1")
            .with_performance(Some(evidence.clone()));
        assert_eq!(execute(&client, origin.port, h2).await, "1:true");
        let observations = evidence.snapshot();
        let tls = observations
            .transports
            .last()
            .unwrap()
            .tls
            .as_ref()
            .unwrap();
        assert_eq!(tls.client_chain.len(), 1);
        assert!(tls.details["Client certificate"].contains("queued"));
        assert_eq!(
            tls.client_chain[0].sha256,
            fixture.source.credential.fingerprint().unwrap()
        );
    }
}

#[tokio::test]
async fn embedding_epoch_zero_is_an_installed_controller_and_cannot_skip_selection() {
    use transmog_tls::client_authentication::{
        CertificateRequest, ClientAuthenticationContext, ClientAuthenticationFuture,
        ClientAuthenticationResolver,
    };
    struct EpochZero(Arc<PolicyClientAuthenticationResolver>);
    impl ClientAuthenticationResolver for EpochZero {
        fn generation(&self) -> u64 {
            0
        }
        fn resolve(
            &self,
            mut context: ClientAuthenticationContext,
            request: CertificateRequest,
        ) -> ClientAuthenticationFuture {
            context.generation = self.0.generation();
            self.0.resolve(context, request)
        }
    }
    let fixture = Fixture::new();
    let origin = origin(&fixture, false, true, Duration::from_secs(5)).await;
    let resolver = Arc::new(EpochZero(Arc::new(
        PolicyClientAuthenticationResolver::new(
            automatic(origin.port),
            fixture.source.clone(),
            None,
        )
        .unwrap(),
    )));
    let client = fixture.client(resolver, "embedder");
    assert_eq!(execute(&client, origin.port, false).await, "1:true");
    assert_eq!(execute(&client, origin.port, false).await, "2:true");
    assert_eq!(fixture.source.reads.load(Ordering::SeqCst), 2);
    assert_eq!(origin.resumed.load(Ordering::SeqCst), 0);
}

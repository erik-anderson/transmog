//! Composer uses the production transports and owns each pending certificate decision.
#[path = "../../proxy-h3/tests/support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use support::{Fixture, automatic, origin, wait_pending};
use transmog_app::{ProxyRoute, SystemReplayExecutor};
use transmog_core::{
    HeaderBlock,
    client_authentication::{ClientCertificateDefault, ClientCertificatePolicy},
    intercept::ExchangeCancellation,
};
use transmog_session::{
    ReplayCredentialPolicy, ReplayExecutor, ReplayLimits, ReplayRequest, ReplayResponse,
    ReplayRisk, ValidatedReplayRequest,
};
use transmog_tls::{
    PemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    client_authentication::PolicyClientAuthenticationResolver,
    client_prompts::{ClientCertificateAnswer, ClientCertificateBroker},
};

fn validated(port: u16) -> ValidatedReplayRequest {
    ReplayRequest {
        method: "GET".into(),
        target: support::request(port).head.target,
        headers: HeaderBlock::new(),
        body: bytes::Bytes::new(),
        risk: ReplayRisk::RejectNonIdempotent,
        credentials: ReplayCredentialPolicy::Reject,
    }
    .validate(ReplayLimits::default())
    .unwrap()
}
fn factory(fixture: &Fixture) -> UpstreamTlsContextFactory {
    let roots =
        PemTrustSource::from_pem(&fixture.ca.certificate_pem().unwrap(), "Composer fixture")
            .unwrap();
    UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&roots, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
}
fn text(response: ReplayResponse) -> String {
    let body = response.body;
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn a_closed_viewer_cancels_its_composer_wait_without_revoking_other_callers() {
    use transmog_app::{AppConfig, Application, ClientCertificateManager};
    for route in [ProxyRoute::Http1, ProxyRoute::Http2, ProxyRoute::Http3] {
        let fixture = Fixture::new();
        let tcp = origin(
            &fixture,
            matches!(route, ProxyRoute::Http2),
            false,
            Duration::from_secs(5),
        )
        .await;
        let udp = quic::origin(&fixture).await;
        let port = if matches!(route, ProxyRoute::Http3) {
            udp.port
        } else {
            tcp.port
        };
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
        let manager =
            ClientCertificateManager::new(fixture.source.clone(), resolver.clone(), broker.clone());
        let app = |caller| {
            Application::new(AppConfig {
                replay_executor: Some(Arc::new(
                    SystemReplayExecutor::with_tls_factory(route, factory(&fixture))
                        .unwrap()
                        .with_client_certificate_resolver(resolver.clone(), caller)
                        .unwrap(),
                )),
                ..Default::default()
            })
            .unwrap()
        };
        let viewer = app("composer:viewer-1");
        let main = app("composer:main");
        let input = || {
            serde_json::from_value(
                serde_json::json!({"method":"GET","url":format!("https://localhost:{port}/")}),
            )
            .unwrap()
        };
        let owner = viewer.clone();
        let request = input();
        let waiting_viewer = tokio::spawn(async move { owner.execute_composer(request).await });
        wait_pending(&broker, 1).await;
        let owner = main.clone();
        let request = input();
        let waiting_main = tokio::spawn(async move { owner.execute_composer(request).await });
        wait_pending(&broker, 2).await;
        viewer.cancel_composer_requests();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), waiting_viewer)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        wait_pending(&broker, 1).await;
        assert!(manager.status(Some("composer:viewer-1")).pending.is_empty());
        let live = manager.status(Some("composer:main")).pending.remove(0);
        manager
            .answer(
                &live.id,
                transmog_app::ClientCertificateDecision::NoCertificate,
            )
            .unwrap();
        assert!(
            waiting_main
                .await
                .unwrap()
                .unwrap()
                .body
                .ends_with(":false")
        );
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn main_shutdown_preserves_open_viewer_certificate_handshakes() {
    use transmog_app::{
        AppConfig, Application, ClientCertificateDecision, ClientCertificateManager,
    };
    use transmog_tls::client_authentication::ClientAuthenticationResolver;
    for route in [ProxyRoute::Http1, ProxyRoute::Http2, ProxyRoute::Http3] {
        let fixture = Fixture::new();
        let tcp = origin(
            &fixture,
            matches!(route, ProxyRoute::Http2),
            false,
            Duration::from_secs(5),
        )
        .await;
        let udp = quic::origin(&fixture).await;
        let port = if matches!(route, ProxyRoute::Http3) {
            udp.port
        } else {
            tcp.port
        };
        let broker = ClientCertificateBroker::default();
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                ClientCertificatePolicy::default(),
                fixture.source.clone(),
                Some(Arc::new(broker.clone())),
            )
            .unwrap(),
        );
        let manager =
            ClientCertificateManager::new(fixture.source.clone(), resolver.clone(), broker.clone());
        let main = Application::new(AppConfig {
            client_certificates: Some(manager.clone()),
            ..Default::default()
        })
        .unwrap();
        main.save_client_certificate_policy(ClientCertificatePolicy {
            default: ClientCertificateDefault::Prompt,
            rules: vec![],
        })
        .unwrap();
        let generation = resolver.generation();
        let viewer = Application::new(AppConfig {
            replay_executor: Some(Arc::new(
                SystemReplayExecutor::with_tls_factory(route, factory(&fixture))
                    .unwrap()
                    .with_client_certificate_resolver(resolver.clone(), "composer:viewer-1")
                    .unwrap(),
            )),
            ..Default::default()
        })
        .unwrap();
        let request = serde_json::from_value(
            serde_json::json!({"method":"GET", "url":format!("https://localhost:{port}/")}),
        )
        .unwrap();
        let waiting = tokio::spawn(async move { viewer.execute_composer(request).await });
        wait_pending(&broker, 1).await;
        let id = manager.status(Some("composer:viewer-1")).pending[0]
            .id
            .clone();
        main.shutdown().await.unwrap();
        assert_eq!(resolver.generation(), generation);
        assert_eq!(manager.status(Some("composer:viewer-1")).pending[0].id, id);
        manager
            .answer(&id, ClientCertificateDecision::NoCertificate)
            .unwrap();
        assert!(waiting.await.unwrap().unwrap().body.ends_with(":false"));
        wait_pending(&broker, 0).await;
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn application_certificate_commands_drive_live_composer_handshakes_on_every_route() {
    use transmog_app::{
        AppConfig, Application, ClientCertificateDecision, ClientCertificateManager,
    };
    for route in [
        ProxyRoute::Http1,
        ProxyRoute::Http2,
        ProxyRoute::Http3,
        ProxyRoute::Auto,
    ] {
        let fixture = Fixture::new();
        let tcp = origin(
            &fixture,
            matches!(route, ProxyRoute::Http2),
            false,
            Duration::from_secs(5),
        )
        .await;
        let udp = quic::origin(&fixture).await;
        let port = if matches!(route, ProxyRoute::Http3 | ProxyRoute::Auto) {
            udp.port
        } else {
            tcp.port
        };
        let broker = ClientCertificateBroker::default();
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                ClientCertificatePolicy::default(),
                fixture.source.clone(),
                Some(Arc::new(broker.clone())),
            )
            .unwrap(),
        );
        let manager =
            ClientCertificateManager::new(fixture.source.clone(), resolver.clone(), broker.clone());
        let executor = SystemReplayExecutor::with_tls_factory(route, factory(&fixture))
            .unwrap()
            .with_client_certificate_resolver(resolver, "composer:main")
            .unwrap();
        let app = Application::new(AppConfig {
            client_certificates: Some(manager.clone()),
            replay_executor: Some(Arc::new(executor)),
            ..Default::default()
        })
        .unwrap();
        if matches!(route, ProxyRoute::Auto) {
            let mut profile = transmog_app::NetworkProfileSnapshot {
                id: "custom-h3".into(),
                name: "H3 only".into(),
                ..Default::default()
            };
            profile.settings.protocols = transmog_core::connection_policy::AllowedHttpProtocols {
                http1: false,
                http2: false,
                http3: true,
            };
            app.save_network_profiles(1, vec![profile], Some("custom-h3".into()))
                .await
                .unwrap();
        }
        let input = || {
            serde_json::from_value(
                serde_json::json!({"method":"GET","url":format!("https://localhost:{port}/")}),
            )
            .unwrap()
        };
        assert_eq!(app.execute_composer(input()).await.unwrap().body, "1:false");
        assert_eq!(
            manager.identities().await.unwrap()[0].fingerprint,
            fixture.source.credential.fingerprint().unwrap()
        );
        app.save_client_certificate_policy(ClientCertificatePolicy {
            default: ClientCertificateDefault::Prompt,
            ..Default::default()
        })
        .unwrap();
        let task_app = app.clone();
        let request = input();
        let waiting = tokio::spawn(async move { task_app.execute_composer(request).await });
        wait_pending(&broker, 1).await;
        let status = manager.status(None);
        assert!(manager.status(Some("composer:other")).pending.is_empty());
        let id = status.pending[0].id.clone();
        let detail = manager.request(&id).unwrap();
        assert_eq!(detail.pending.caller, "Composer");
        assert_eq!(detail.choices[0].subject, "CN=Client");
        assert!(!serde_json::to_string(&detail).unwrap().contains("private"));
        assert!(
            manager
                .answer(
                    &id,
                    ClientCertificateDecision::Certificate {
                        identity: "not-an-eligible-identity".into()
                    }
                )
                .is_err()
        );
        assert_eq!(manager.status(None).pending.len(), 1);
        manager
            .answer(
                &id,
                ClientCertificateDecision::Certificate {
                    identity: "test-client".into(),
                },
            )
            .unwrap();
        let authenticated = waiting.await.unwrap().unwrap();
        assert_eq!(authenticated.body, "2:true");
        assert!(
            authenticated
                .detail
                .performance
                .transports
                .iter()
                .filter_map(|transport| transport.tls.as_ref())
                .any(|tls| tls.client_chain.len() == 1)
        );
        assert!(
            app.inspect_certificates(&authenticated.detail.id)
                .unwrap()
                .connections
                .iter()
                .any(|connection| connection.client_chain.len() == 1)
        );
        assert!(
            manager
                .answer(&id, ClientCertificateDecision::NoCertificate)
                .is_err()
        );
        let task_app = app.clone();
        let request = input();
        let waiting = tokio::spawn(async move { task_app.execute_composer(request).await });
        wait_pending(&broker, 1).await;
        app.shutdown().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), waiting)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        wait_pending(&broker, 0).await;
        app.save_client_certificate_policy(automatic(port)).unwrap();
        assert_eq!(app.execute_composer(input()).await.unwrap().body, "4:true");
        app.save_client_certificate_policy(ClientCertificatePolicy::default())
            .unwrap();
        assert_eq!(app.execute_composer(input()).await.unwrap().body, "5:false");
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn composer_routes_use_the_real_certificate_handshake_and_a_separate_caller_pool() {
    for route in [ProxyRoute::Http1, ProxyRoute::Http2, ProxyRoute::Http3] {
        let fixture = Fixture::new();
        let tcp = origin(
            &fixture,
            matches!(route, ProxyRoute::Http2),
            false,
            Duration::from_secs(5),
        )
        .await;
        let udp = quic::origin(&fixture).await;
        let port = if matches!(route, ProxyRoute::Http3) {
            udp.port
        } else {
            tcp.port
        };
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(automatic(port), fixture.source.clone(), None)
                .unwrap(),
        );
        let composer = SystemReplayExecutor::with_tls_factory(route, factory(&fixture))
            .unwrap()
            .with_client_certificate_resolver(resolver.clone(), "composer:run:1")
            .unwrap();
        assert_eq!(
            text(
                composer
                    .execute(validated(port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "1:true"
        );
        assert_eq!(
            text(
                composer
                    .execute(validated(port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "1:true"
        );
        let other = composer
            .clone()
            .with_client_certificate_resolver(resolver.clone(), "composer:run:2")
            .unwrap();
        assert_eq!(
            text(
                other
                    .execute(validated(port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "2:true"
        );
        resolver
            .replace_policy(ClientCertificatePolicy::default())
            .unwrap();
        assert_eq!(
            text(
                composer
                    .execute(validated(port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "3:false"
        );
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn cancelling_buffered_and_file_composer_replays_unregisters_their_pending_handshakes() {
    for route in [ProxyRoute::Http1, ProxyRoute::Http2, ProxyRoute::Http3] {
        for file in [false, true] {
            let fixture = Fixture::new();
            let tcp = origin(
                &fixture,
                matches!(route, ProxyRoute::Http2),
                false,
                Duration::from_secs(5),
            )
            .await;
            let udp = quic::origin(&fixture).await;
            let port = if matches!(route, ProxyRoute::Http3) {
                udp.port
            } else {
                tcp.port
            };
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
            let composer = SystemReplayExecutor::with_tls_factory(route, factory(&fixture))
                .unwrap()
                .with_client_certificate_resolver(resolver, "composer:run:1")
                .unwrap();
            let cancellation = ExchangeCancellation::new();
            let cancel = cancellation.clone();
            let pending = tokio::spawn(async move {
                if file {
                    composer
                        .execute_file(validated(port), tempfile::tempfile().unwrap(), 0, cancel)
                        .await
                } else {
                    composer.execute(validated(port), cancel).await
                }
            });
            wait_pending(&broker, 1).await;
            let stale = broker.pending()[0].id;
            assert_eq!(broker.pending()[0].context.caller, "composer:run:1");
            cancellation.cancel();
            assert!(pending.await.unwrap().is_err());
            wait_pending(&broker, 0).await;
            assert!(!broker.answer(stale, ClientCertificateAnswer::NoCertificate));
            assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
        }
    }
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "creates owned non-exportable CurrentUser/My keys"]
async fn real_windows_controller_signs_all_composer_routes_and_retires_deleted_keys() {
    use transmog_tls::{
        client_authentication::{ClientAuthenticationResolver, ClientCredentialSource},
        windows_client_certificates::{
            WindowsClientCertificateController, WindowsCurrentUserCertificateSource,
        },
    };
    let owned = support::windows::WindowsFixture::new("RSA");
    let credential = WindowsCurrentUserCertificateSource
        .credentials()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == owned.id)
        .unwrap();
    let mut fixture = Fixture::new();
    fixture.source = Arc::new(support::Source {
        credential,
        reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let h1 = origin(&fixture, false, false, Duration::from_secs(5)).await;
    let h2 = origin(&fixture, true, false, Duration::from_secs(5)).await;
    let h3 = quic::origin(&fixture).await;
    let mut policy = ClientCertificatePolicy::default();
    for port in [h1.port, h2.port, h3.port] {
        let mut rule = automatic(port).rules.remove(0);
        rule.id = port.to_string();
        let transmog_core::client_authentication::ClientCertificateAction::Automatic { selector } =
            &mut rule.mode
        else {
            unreachable!()
        };
        selector.identity = Some(owned.id.clone());
        policy.rules.push(rule);
    }
    let controller = WindowsClientCertificateController::new(policy)
        .await
        .unwrap();
    let mut executors = Vec::new();
    for (route, port) in [
        (ProxyRoute::Http1, h1.port),
        (ProxyRoute::Http2, h2.port),
        (ProxyRoute::Http3, h3.port),
    ] {
        let executor = SystemReplayExecutor::with_tls_factory(route, factory(&fixture))
            .unwrap()
            .with_client_certificate_resolver(controller.resolver(), "composer:windows:1")
            .unwrap();
        assert_eq!(
            text(
                executor
                    .execute(validated(port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "1:true"
        );
        assert_eq!(
            text(
                executor
                    .execute(validated(port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "1:true"
        );
        executors.push((executor, port));
    }
    let generation = controller.resolver().generation();
    owned.change("DeleteKey");
    tokio::time::timeout(Duration::from_secs(7), async {
        while controller.resolver().generation() == generation {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    for (executor, port) in &executors {
        assert_eq!(
            text(
                executor
                    .execute(validated(*port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "2:false"
        );
        assert_eq!(
            text(
                executor
                    .execute(validated(*port), ExchangeCancellation::new())
                    .await
                    .unwrap()
            ),
            "3:false"
        );
    }
    drop(controller);
    assert_eq!(h1.resumed.load(Ordering::SeqCst), 0);
    assert_eq!(h2.resumed.load(Ordering::SeqCst), 0);
}

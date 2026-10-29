use super::*;

#[tokio::test]
async fn live_rules_replay_uses_native_https_validation_and_retains_tls_context() {
    use boring::ssl::{SslAcceptor, SslMethod};
    use transmog_tls::{EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot};
    let ca = ProxyCa::generate("Replay fixture", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("127.0.0.1").unwrap(), 1)
        .unwrap();
    let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    acceptor.set_certificate(&leaf.certificate).unwrap();
    acceptor.set_private_key(&leaf.private_key).unwrap();
    let acceptor = acceptor.build();
    let trust = TrustSnapshot::load(
        &PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "Replay fixture").unwrap(),
        1,
    )
    .unwrap();
    let h = https_harness(trust).await;
    let owner = full(&h);
    let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = origin.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = origin.accept().await.unwrap();
        let mut tls = tokio_boring::accept(&acceptor, socket).await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(tls.read_u8().await.unwrap());
        }
        tls.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhttps-result",
        )
        .await
        .unwrap();
    });
    let result=h.call(&owner,"transmog_request_send",json!({"operationId":"tls","mode":"live-rules","request":{"method":"GET","url":format!("https://{address}/")}})).await;
    assert!(
        result.is_ok(),
        "{result:?}; snapshots={:?}; server finished={}",
        h.adapter
            .application
            .session_service()
            .catalog()
            .project_retained(|snapshot| (
                snapshot.request_number,
                snapshot.metadata.client_addr,
                snapshot.state(),
                snapshot.request_heads.clone()
            )),
        server.is_finished()
    );
    let sent = result.unwrap();
    assert_eq!(
        sent["status"],
        200,
        "{}",
        value(
            h.adapter
                .application
                .session_detail(sent["id"].as_str().unwrap())
                .unwrap()
        )
        .unwrap()
    );
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let body = h
        .call(
            &owner,
            "transmog_body_read",
            json!({"id":sent["id"],"boundary":"client-response"}),
        )
        .await
        .unwrap();
    assert_eq!(
        STANDARD.decode(body["data"].as_str().unwrap()).unwrap(),
        b"https-result"
    );
    let context = h
        .call(
            &owner,
            "transmog_exchange_get",
            json!({"id":sent["id"],"include":["transport"]}),
        )
        .await
        .unwrap();
    assert!(
        context["transport"]["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|transport| transport["tls"].is_object())
    );
    h.adapter.application.shutdown().await.unwrap();
}

async fn https_harness(trust: transmog_tls::TrustSnapshot) -> TrafficHarness {
    let root = tempfile::tempdir().unwrap();
    let access = AccessManager::open(None).unwrap();
    access.set_enabled(true).unwrap();
    let application = Application::new(AppConfig {
        upstream_trust: Some(Arc::new(trust)),
        body_store: Some(transmog_app::BodyStoreConfig::product_default(
            root.path().join("bodies"),
        )),
        ..Default::default()
    })
    .unwrap();
    let adapter = AgentApplication::new(
        application,
        access.clone(),
        root.path().into(),
        Arc::new(NoProxy),
    )
    .unwrap();
    let certificate_path = root.path().join("intercept.pem");
    let private_key_path = root.path().join("intercept.key");
    let h = TrafficHarness {
        harness: Harness {
            _root: root,
            adapter,
            access,
        },
        body: b"https-fixture".to_vec(),
    };
    h.adapter
        .application
        .create_ca(transmog_app::CaCreateRequest {
            certificate_path: certificate_path.clone(),
            private_key_path: private_key_path.clone(),
            common_name: "Replay interception fixture".into(),
            validity_days: 2,
        })
        .await
        .unwrap();
    let mut state = h.adapter.application.product_state();
    state.preferences.configure_system_proxy = false;
    h.adapter.application.save_product_state(state).unwrap();
    let profiles = h
        .adapter
        .application
        .product_state()
        .preferences
        .network_profiles;
    let mut profile = profiles.active_profile.clone();
    profile.id = "custom-https-fixture".into();
    profile.name = "HTTPS fixture".into();
    profile.settings.root_store = transmog_app::RootStore::Os;
    profile.settings.local_certificates = false;
    h.adapter
        .application
        .save_network_profiles(profiles.revision, vec![profile.clone()], Some(profile.id))
        .await
        .unwrap();
    h.adapter
        .application
        .start_proxy(
            transmog_app::ProxyStartRequest {
                ca_certificate_path: certificate_path,
                ca_private_key_path: private_key_path,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    h
}

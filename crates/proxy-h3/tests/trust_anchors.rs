//! Live QUIC metadata helpers, matched IDs and bounded selection recovery.
#[path = "../../proxy-http/tests/support/trust_anchors.rs"]
mod anchors;
#[path = "support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant, SystemTime},
};
use transmog_core::{VerificationResult, performance::PerformanceRecorder};
use transmog_h3::{H3OriginClient, H3TransportLimits};
use transmog_tls::ProxyCa;

fn config(ca: &ProxyCa, id: Option<u8>, available: Option<u8>) -> quiche::Config {
    let leaf = ca
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    let mut context = boring::ssl::SslContext::builder(boring::ssl::SslMethod::tls()).unwrap();
    if let Some(id) = id {
        transmog_tls_native::add_server_credential_for_trust_anchor(
            &mut context,
            &[leaf.certificate],
            &leaf.private_key,
            &[id],
        )
        .unwrap();
    } else {
        context.set_certificate(&leaf.certificate).unwrap();
        context.set_private_key(&leaf.private_key).unwrap();
    }
    if let Some(id) = available {
        transmog_tls_native::configure_available_trust_anchors(&mut context, &[1, id]).unwrap();
    }
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, context).unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(3000);
    config.set_max_recv_udp_payload_size(1350);
    config.set_max_send_udp_payload_size(1350);
    config.set_initial_max_data(1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(256 * 1024);
    config.set_initial_max_stream_data_bidi_remote(256 * 1024);
    config.set_initial_max_stream_data_uni(256 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    config
}
#[tokio::test]
async fn quic_uses_an_omitted_intermediate_and_records_a_server_match() {
    let root = anchors::issuer_root("QUIC configured root");
    let issuer = anchors::intermediate(&root);
    let factory = anchors::factory(&root, anchors::catalog(&[(issuer.certificate(), 1)], 42));
    let origin =
        quic::origin_configs(vec![config(&issuer, Some(1), None)], "127.0.0.1:0", true).await;
    let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
    let client = H3OriginClient::new(factory, H3TransportLimits::default())
        .with_performance(Some(recorder.clone()));
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.execute(support::request(origin.port), origin.port, 1024),
    )
    .await
    .unwrap()
    .unwrap_or_else(|error| panic!("{error}; capture: {:?}", recorder.snapshot()));
    assert_eq!(result.response.head.status, 200);
    let evidence = recorder.snapshot();
    let tls = evidence
        .transports
        .iter()
        .rev()
        .find_map(|t| t.tls.as_ref())
        .unwrap();
    assert_eq!(
        tls.verification,
        Some(VerificationResult::Verified),
        "{evidence:#?}"
    );
    assert_eq!(
        tls.details["Trust Anchor IDs"],
        "Server reported a matching certificate chain"
    );
    assert_eq!(tls.server_chain.len(), 3);
}

#[tokio::test]
async fn quic_uses_selected_public_trust_instead_of_configured_os_roots() {
    let os = ProxyCa::generate("QUIC OS root", 2).unwrap();
    let public = ProxyCa::generate("QUIC public root", 2).unwrap();
    let mut profile = transmog_core::network_profile::NetworkProfileSnapshot::default();
    profile.settings.trust_anchor_ids = false;
    profile.settings.local_certificates = false;
    let factory = anchors::factory(&os, anchors::catalog(&[(public.certificate(), 1)], 42))
        .with_network_profile(profile)
        .unwrap();
    let origin = quic::origin_configs(vec![config(&public, None, None)], "127.0.0.1:0", true).await;
    let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
    let client = H3OriginClient::new(factory, H3TransportLimits::default())
        .with_performance(Some(recorder.clone()));
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client.execute(support::request(origin.port), origin.port, 1024),
    )
    .await
    .unwrap()
    .unwrap_or_else(|error| panic!("{error}; capture: {:?}", recorder.snapshot()));
    assert_eq!(response.response.head.status, 200);
    let evidence = recorder.snapshot();
    let tls = evidence
        .transports
        .iter()
        .filter_map(|t| t.tls.as_ref())
        .find(|tls| tls.verification == Some(VerificationResult::Verified))
        .unwrap();
    assert_eq!(tls.trust_source.as_deref(), Some("local public CA fixture"));
    assert_eq!(tls.details["Root store version"], "42");
    assert_eq!(
        tls.details["Trust Anchor IDs"],
        "Disabled by network profile"
    );
}
#[tokio::test]
async fn quic_recovery_is_bounded_before_any_http_request() {
    for succeeds in [true, false] {
        let trusted = ProxyCa::generate("QUIC configured trust", 2).unwrap();
        let unknown = ProxyCa::generate("QUIC metadata only", 2).unwrap();
        let factory = anchors::factory(
            &trusted,
            anchors::catalog(
                &[(unknown.certificate(), 1), (trusted.certificate(), 2)],
                42,
            ),
        );
        let origin = quic::origin_configs(
            vec![
                config(&unknown, None, Some(2)),
                config(if succeeds { &trusted } else { &unknown }, None, Some(2)),
            ],
            "127.0.0.1:0",
            true,
        )
        .await;
        let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
        let client = H3OriginClient::new(factory, H3TransportLimits::default())
            .with_performance(Some(recorder.clone()));
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.execute(support::request(origin.port), origin.port, 1024),
        )
        .await
        .unwrap();
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(
            origin.count.load(Ordering::SeqCst),
            2,
            "{:#?}",
            recorder.snapshot()
        );
        assert_eq!(
            origin.requests.load(Ordering::SeqCst),
            usize::from(succeeds)
        );
        let evidence = recorder.snapshot();
        // Happy Eyeballs may also record an unavailable IPv6 localhost socket.
        let attempts = evidence
            .transports
            .iter()
            .filter(|t| t.tls.as_ref().is_some_and(|tls| tls.verification.is_some()))
            .collect::<Vec<_>>();
        assert_eq!(attempts.len(), 2);
        assert!(matches!(
            attempts[0].tls.as_ref().unwrap().verification,
            Some(VerificationResult::Rejected { .. })
        ));
        assert_eq!(
            attempts[1].tls.as_ref().unwrap().details["Requested Trust Anchor IDs"],
            "0102"
        );
    }
}

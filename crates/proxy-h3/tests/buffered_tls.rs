//! H3 telemetry must not reject an irrelevant malformed wire certificate.
#[path = "support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use boring::ssl::{SslContext, SslMethod};
use std::{sync::Arc, time::Duration};
use transmog_h3::{H3OriginClient, H3TransportLimits};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};
#[tokio::test]
async fn h3_accepts_a_valid_path_and_records_raw_unused_certificate_bytes() {
    let fixture = support::Fixture::new();
    let leaf = fixture
        .ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
    tls.set_session_id_context(b"buffered-session").unwrap();
    transmog_tls_native::set_context_chain_and_key(
        &mut tls,
        &[leaf.certificate.to_der().unwrap(), vec![0x30, 0]],
        &leaf.private_key,
    )
    .unwrap();
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(3000);
    config.set_initial_max_data(1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(256 * 1024);
    config.set_initial_max_stream_data_bidi_remote(256 * 1024);
    config.set_initial_max_stream_data_uni(256 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    let origin = quic::origin_configs(vec![config], "127.0.0.1:0", true).await;
    let trust =
        PemTrustSource::from_pem(&fixture.ca.certificate_pem().unwrap(), "buffer fixture").unwrap();
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    );
    let client = H3OriginClient::new(factory, H3TransportLimits::default());
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client.execute(support::request(origin.port), origin.port, 1024),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.telemetry.peer_chain_sha256.len(), 2);
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.retire_cached_connections().await;
    let recorder = transmog_core::performance::PerformanceRecorder::new(
        std::time::SystemTime::now(),
        std::time::Instant::now(),
    );
    let response = client
        .with_performance(Some(recorder.clone()))
        .execute(support::request(origin.port), origin.port, 1024)
        .await
        .unwrap();
    assert_eq!(response.telemetry.peer_chain_sha256.len(), 2);
    assert!(
        recorder
            .snapshot()
            .transports
            .iter()
            .any(|transport| transport.tls_resumed == Some(true)),
        "Malformed unused wire issuer prevented buffered session resumption"
    );
}

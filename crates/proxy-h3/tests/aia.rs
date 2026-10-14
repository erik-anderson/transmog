//! QUIC certificate workers discover missing issuers without blocking packet delivery.
#[path = "../../proxy-http/tests/support/aia.rs"]
mod aia;
#[path = "../../proxy-http/tests/support/trust_anchors.rs"]
mod anchors;
#[path = "support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use std::{sync::Arc, time::Duration};
use transmog_h3::{H3OriginClient, H3TransportLimits};
use transmog_tls::issuer_source::{HttpIssuerSource, IssuerSource};

#[tokio::test]
async fn quic_missing_issuer_verifies_off_thread_and_reuses_untrusted_cache() {
    let root = aia::root("QUIC AIA root");
    let issuer = anchors::intermediate(&root);
    let fetcher = aia::issuer(&issuer.certificate().to_der().unwrap()).await;
    let (leaf, key) = aia::leaf(
        &issuer,
        &[format!("http://127.0.0.1:{}/issuer", fetcher.port)],
        "localhost",
    );
    let mut tls = boring::ssl::SslContext::builder(boring::ssl::SslMethod::tls()).unwrap();
    tls.set_certificate(&leaf).unwrap();
    tls.set_private_key(&key).unwrap();
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
    let cache = Arc::new(HttpIssuerSource::default());
    let factory = aia::factory(&root).with_issuer_source(cache.clone());
    for _ in 0..2 {
        let client = H3OriginClient::new(factory.clone(), H3TransportLimits::default());
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.execute(support::request(origin.port), origin.port, 1024),
        )
        .await
        .unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            cache.cached_issuers(),
            vec![issuer.certificate().to_der().unwrap()]
        );
    }
    assert_eq!(fetcher.requests.lock().unwrap().len(), 1);
    assert_eq!(origin.requests.load(std::sync::atomic::Ordering::SeqCst), 2);
}

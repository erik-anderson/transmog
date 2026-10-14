//! QUIC tickets are reused across physical connections, with caller isolation.
#[path = "support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use transmog_core::{connection_policy::NetworkIsolationKey, performance::PerformanceRecorder};
use transmog_h3::{H3OriginClient, H3TransportLimits};
use transmog_tls::{PemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy};

#[tokio::test]
async fn retired_quic_connections_resume_only_in_the_same_caller_partition() {
    let fixture = support::Fixture::new();
    let origin = quic::origin_without_client_authentication(&fixture).await;
    let trust =
        PemTrustSource::from_pem(&fixture.ca.certificate_pem().unwrap(), "ticket fixture").unwrap();
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    );
    let client = H3OriginClient::new(factory, H3TransportLimits::default());
    let mut resumed = Vec::new();
    for phase in 0..3 {
        let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
        let mut connection = client.clone().with_performance(Some(recorder.clone()));
        if phase == 2 {
            connection =
                connection.with_network_isolation(NetworkIsolationKey::new("different caller"));
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            connection.execute(support::request(origin.port), origin.port, 1024),
        )
        .await
        .unwrap()
        .unwrap();
        resumed.push(
            recorder
                .snapshot()
                .transports
                .iter()
                .find_map(|t| t.tls_resumed)
                .unwrap(),
        );
        // Allow post-handshake tickets to arrive before closing the idle owner.
        tokio::time::sleep(Duration::from_millis(50)).await;
        client.retire_cached_connections().await;
    }
    assert_eq!(resumed, [false, true, false]);
}

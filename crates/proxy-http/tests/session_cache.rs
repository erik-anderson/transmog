//! Real origin tickets survive physical connection closure within their partition.
mod support;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_tls::{PemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy};

#[tokio::test]
async fn closed_tcp_connections_resume_but_new_caller_pools_do_not() {
    let fixture = support::Fixture::new();
    let origin = support::origin(&fixture, false, true, Duration::from_secs(5)).await;
    let trust =
        PemTrustSource::from_pem(&fixture.ca.certificate_pem().unwrap(), "ticket fixture").unwrap();
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    );
    let client = HyperOriginClient::new(&factory).unwrap();
    for _ in 0..3 {
        client
            .execute(
                support::request(origin.port),
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(5),
            )
            .await
            .unwrap();
    }
    assert_eq!(origin.count.load(Ordering::SeqCst), 3);
    assert_eq!(origin.resumed.load(Ordering::SeqCst), 2);
    let isolated = client
        .for_client_certificate_caller("different caller")
        .unwrap();
    isolated
        .execute(
            support::request(origin.port),
            HyperEgressMode::Http1Only,
            1024,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(origin.resumed.load(Ordering::SeqCst), 2);
}

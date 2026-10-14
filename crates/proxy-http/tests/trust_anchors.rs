//! Live TCP recovery is bounded, before HTTP data, and retains ordinary trust.
#[path = "support/trust_anchors.rs"]
mod anchors;
mod support;
use boring::ssl::Ssl;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use transmog_core::{VerificationResult, performance::PerformanceRecorder};
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_tls::ProxyCa;

#[tokio::test]
async fn fresh_tcp_connection_automatically_uses_newer_discovered_metadata() {
    use transmog_tls::trust_anchors::PkiMetadataProvider;
    let root = ProxyCa::generate("auto-discovery root", 2).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(PkiMetadataProvider::new(
        anchors::catalog(&[(root.certificate(), 1)], 42),
        Some(directory.path().into()),
    ));
    let factory = anchors::factory(&root, provider.current()).with_pki_metadata(provider.clone());
    let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
    let client = HyperOriginClient::new(&factory)
        .unwrap()
        .with_performance(Some(recorder.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptors = [
        anchors::acceptor(&root, Some(1), None),
        anchors::acceptor(&root, Some(2), None),
    ];
    let server = tokio::spawn(async move {
        for acceptor in acceptors {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut io =
                tokio_boring::SslStreamBuilder::new(Ssl::new(acceptor.context()).unwrap(), tcp)
                    .accept()
                    .await
                    .unwrap();
            let mut request = [0; 4096];
            assert!(io.read(&mut request).await.unwrap() > 0);
            io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        }
    });
    client
        .execute(
            support::request(port),
            HyperEgressMode::Http1Only,
            1024,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    let path = directory.path().join("1804");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(
        path.join("crs.pb"),
        anchors::catalog_bytes(&[(root.certificate(), 2)], 43),
    )
    .unwrap();
    provider.refresh();
    client
        .execute(
            support::request(port),
            HyperEgressMode::Http1Only,
            1024,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    server.await.unwrap();
    let evidence = recorder.snapshot();
    assert_eq!(evidence.transports.len(), 2);
    for (index, version) in ["42", "43"].into_iter().enumerate() {
        let tls = evidence.transports[index].tls.as_ref().unwrap();
        assert_eq!(tls.details["PKI metadata version"], version);
        assert_eq!(
            tls.details["Trust Anchor IDs"],
            "Server reported a matching certificate chain"
        );
    }
}

#[tokio::test]
async fn server_can_omit_an_advertised_intermediate_without_gaining_trust() {
    let root = ProxyCa::generate("configured root", 2).unwrap();
    let issuer = anchors::intermediate(&root);
    let metadata = anchors::catalog(&[(issuer.certificate(), 1)], 42);
    let factory = anchors::factory(&root, metadata);
    let client = HyperOriginClient::new(&factory).unwrap();
    let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
    let client = client.with_performance(Some(recorder.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = anchors::acceptor(&issuer, Some(1), None);
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut io =
            tokio_boring::SslStreamBuilder::new(Ssl::new(acceptor.context()).unwrap(), tcp)
                .accept()
                .await
                .unwrap();
        let mut request = [0; 4096];
        assert!(io.read(&mut request).await.unwrap() > 0);
        io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    let response = client
        .execute(
            support::request(port),
            HyperEgressMode::Http1Only,
            1024,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(response.head.status, 200);
    server.await.unwrap();
    let snapshot = recorder.snapshot();
    let tls = snapshot
        .transports
        .iter()
        .find_map(|t| t.tls.as_ref())
        .unwrap();
    assert_eq!(tls.verification, Some(VerificationResult::Verified));
    assert_eq!(
        tls.details["Trust Anchor IDs"],
        "Server reported a matching certificate chain"
    );
    assert_eq!(tls.server_chain.len(), 3);
}

#[tokio::test]
async fn selection_recovery_retries_once_before_http_and_retains_both_connections() {
    for succeeds in [true, false] {
        let trusted = ProxyCa::generate("configured trust", 2).unwrap();
        let unknown = ProxyCa::generate("metadata only", 2).unwrap();
        let metadata = anchors::catalog(
            &[(unknown.certificate(), 1), (trusted.certificate(), 2)],
            42,
        );
        let factory = anchors::factory(&trusted, metadata);
        let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
        let client = HyperOriginClient::new(&factory)
            .unwrap()
            .with_performance(Some(recorder.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let first = anchors::acceptor(&unknown, None, Some(2));
        let second = anchors::acceptor(if succeeds { &trusted } else { &unknown }, None, Some(2));
        let server = tokio::spawn(async move {
            for (number, acceptor) in [first, second].into_iter().enumerate() {
                let (tcp, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let mut ssl = Ssl::new(acceptor.context()).unwrap();
                ssl.set_alpn_protos(b"\x08http/1.1").unwrap();
                let result = tokio_boring::SslStreamBuilder::new(ssl, tcp).accept().await;
                if number == 0 || !succeeds {
                    assert!(result.is_err(), "metadata CA became trusted");
                } else {
                    let mut io = result.unwrap();
                    let mut request = [0; 4096];
                    let n = io.read(&mut request).await.unwrap();
                    assert!(n > 0);
                    io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .unwrap();
                }
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err(),
                "more than one retry"
            );
        });
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            client.execute(
                support::request(port),
                HyperEgressMode::Http1Only,
                1024,
                Duration::from_secs(3),
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.is_ok(), succeeds);
        server.await.unwrap();
        let evidence = recorder.snapshot();
        assert_eq!(evidence.transports.len(), 2);
        assert!(matches!(
            evidence.transports[0].tls.as_ref().unwrap().verification,
            Some(VerificationResult::Rejected { .. })
        ));
        assert!(
            evidence.transports[1]
                .tls
                .as_ref()
                .unwrap()
                .details
                .contains_key("Trust Anchor ID recovery")
        );
        assert_eq!(
            evidence.transports[0].tls.as_ref().unwrap().details["Requested Trust Anchor IDs"],
            "01010102"
        );
        assert_eq!(
            evidence.transports[1].tls.as_ref().unwrap().details["Requested Trust Anchor IDs"],
            "0102"
        );
    }
}

//! Physical-connection evidence through Hyper's actual TLS session cache.
use boring::ssl::{Ssl, SslStream, SslVersion};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_boring::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    time::Duration,
};
use transmog_tls::{
    DownstreamTlsContextFactory, DownstreamTlsPolicy, EndpointIdentity, PemTrustSource, ProxyCa,
    TlsCapture, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

#[tokio::test]
async fn resumed_tls12_records_original_group_without_a_fresh_key_exchange() {
    let ca = ProxyCa::generate("TLS 1.2 resumption root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("127.0.0.1").unwrap(), 1)
        .unwrap();
    let context_capture = TlsCapture::new(transmog_core::TlsEvidence::default());
    let acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy {
        minimum_version: SslVersion::TLS1_2,
        maximum_version: SslVersion::TLS1_2,
    })
    .acceptor_captured(&leaf, Some(&context_capture))
    .unwrap();
    let (address, server) = resumption_server(acceptor);
    let source =
        PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "resumption fixture root")
            .unwrap();
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&source, 1).unwrap()),
        UpstreamTlsPolicy {
            minimum_version: SslVersion::TLS1_2,
            maximum_version: SslVersion::TLS1_2,
            ..UpstreamTlsPolicy::default()
        },
    );
    let captures: Vec<_> = (0..2)
        .map(|_| factory.capture(Some("127.0.0.1".into()), vec!["http/1.1".into()]))
        .collect();
    let callback_captures = captures.clone();
    let next = Mutex::new(0);
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    let mut https = HttpsConnector::with_connector(
        http,
        factory.hyper_connector_builder(b"\x08http/1.1").unwrap(),
    )
    .unwrap();
    https.set_ssl_callback(move |ssl, _| {
        let mut index = next.lock().unwrap();
        ssl.set_curves_list(if *index == 0 { "P-256" } else { "X25519:P-256" })?;
        callback_captures[*index].attach_ssl(ssl)?;
        *index += 1;
        Ok(())
    });
    let client = Client::builder(TokioExecutor::new())
        .pool_max_idle_per_host(0)
        .build::<_, Empty<Bytes>>(https);
    for _ in 0..2 {
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            client.get(format!("https://{address}/").parse().unwrap()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), 200);
        let body = tokio::time::timeout(Duration::from_secs(5), response.into_body().collect())
            .await
            .unwrap()
            .unwrap()
            .to_bytes();
        assert_eq!(body.as_ref(), b"ok");
    }
    for legs in [
        captures
            .iter()
            .map(TlsCapture::snapshot)
            .collect::<Vec<_>>(),
        server.join().unwrap(),
    ] {
        assert_eq!(legs[0].details["Negotiated group"], "P-256");
        assert_eq!(legs[0].details["Key exchange"], "Classical key exchange");
        let resumed = &legs[1].details;
        assert_eq!(resumed["Session resumed"], "Yes");
        assert_eq!(resumed["Original session group"], "P-256");
        assert_eq!(resumed["Original session group ID"], "0x0017");
        assert_eq!(
            resumed["Key exchange"],
            "Not performed (TLS 1.2 session resumed)"
        );
        assert!(!resumed.contains_key("Negotiated group"));
        assert!(!resumed.contains_key("Negotiated group ID"));
    }
}

fn resumption_server(
    acceptor: boring::ssl::SslAcceptor,
) -> (
    std::net::SocketAddr,
    std::thread::JoinHandle<Vec<transmog_core::TlsEvidence>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut evidence = Vec::new();
        for _ in 0..2 {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let capture = TlsCapture::new(transmog_core::TlsEvidence::default());
            let mut ssl = Ssl::new(acceptor.context()).unwrap();
            capture.attach_ssl(&mut ssl).unwrap();
            let mut tls = SslStream::new(ssl, tcp).unwrap();
            tls.accept().unwrap();
            let mut head = [0; 8192];
            let mut count = 0;
            while !head[..count].windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                assert!(count < head.len());
                let read = tls.read(&mut head[count..]).unwrap();
                assert_ne!(read, 0);
                count += read;
            }
            tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
            capture.completed(tls.ssl(), true);
            evidence.push(capture.snapshot());
        }
        evidence
    });
    (address, server)
}

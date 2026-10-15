//! HTTP/1 parser-to-serializer metadata and hop-header wire regressions.
use bytes::Bytes;
use http_body_util::Empty;
use hyper_util::rt::TokioIo;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use transmog_core::{
    CanonicalRequest, HeaderBlock, HttpLegVersion, RequestHead, Target,
    network_profile::NetworkProfileSnapshot,
};
use transmog_http::{HyperEgressMode, HyperOriginClient, header_block_from_hyper};
use transmog_tls::{
    PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

async fn read_head(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(socket.read_u8().await.unwrap());
        assert!(bytes.len() < 4096);
    }
    String::from_utf8(bytes).unwrap()
}

async fn parsed_headers() -> HeaderBlock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender = Arc::new(std::sync::Mutex::new(Some(sender)));
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let service =
            hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                sender
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(header_block_from_hyper(request.headers(), request.extensions()).unwrap())
                    .unwrap();
                async {
                    Ok::<_, std::convert::Infallible>(http::Response::new(Empty::<Bytes>::new()))
                }
            });
        hyper::server::conn::http1::Builder::new()
            .preserve_header_case(true)
            .serve_connection(TokioIo::new(socket), service)
            .await
            .unwrap();
    });
    let mut client = TcpStream::connect(address).await.unwrap();
    client.write_all(b"GET / HTTP/1.1\r\nHost: stale.example\r\nX-Z: one\r\nx-A: middle\r\nx-z: two\r\nConnection: close, x-remove\r\nx-remove: secret\r\n\r\n").await.unwrap();
    read_head(&mut client).await;
    server.await.unwrap();
    receiver.await.unwrap()
}

#[tokio::test]
async fn h1_preserves_spelling_interleaved_duplicates_and_reuse_without_forwarding_hop_fields() {
    let headers = parsed_headers().await;
    assert_eq!(
        headers
            .iter()
            .take(4)
            .map(transmog_core::HeaderField::name)
            .collect::<Vec<_>>(),
        [b"Host".as_slice(), b"X-Z", b"x-A", b"x-z"]
    );
    let ca = ProxyCa::generate("H1 wire fixture", 1).unwrap();
    let trust = PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "wire fixture").unwrap();
    for (mode, reusable) in [
        (HyperEgressMode::Http1Only, true),
        (HyperEgressMode::Http1Only, false),
        (HyperEgressMode::Auto, true),
        (HyperEgressMode::Auto, false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut profile = NetworkProfileSnapshot::default();
        profile.settings.root_store = transmog_core::network_profile::RootStore::Os;
        profile.settings.reuse_connections = reusable;
        let factory = UpstreamTlsContextFactory::new(
            Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
            UpstreamTlsPolicy::default(),
        )
        .with_network_profile(profile)
        .unwrap();
        let client = HyperOriginClient::new(&factory).unwrap();
        let origin = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            for path in if reusable {
                vec!["/one", "/two"]
            } else {
                vec!["/one"]
            } {
                let received = read_head(&mut socket).await;
                assert_eq!(
                    received,
                    format!(
                        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: {}\r\nX-Z: one\r\nx-A: middle\r\nx-z: two\r\n\r\n",
                        if reusable { "keep-alive" } else { "close" }
                    )
                );
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();
            }
        });
        for path in if reusable {
            vec!["/one", "/two"]
        } else {
            vec!["/one"]
        } {
            let request = CanonicalRequest {
                head: RequestHead {
                    method: "GET".into(),
                    target: Target {
                        scheme: "http".into(),
                        authority: address.to_string(),
                        host: "127.0.0.1".into(),
                        port: address.port(),
                        path: path.into(),
                        query: None,
                    },
                    headers: headers.clone(),
                    source_version: HttpLegVersion::Http1,
                },
                body: Vec::new(),
            };
            client
                .execute(request, mode, 1024, Duration::from_secs(3))
                .await
                .unwrap();
        }
        origin.await.unwrap();
    }
}

#[test]
fn public_original_name_api_rejects_header_injection() {
    let mut cases = hyper::ext::HeaderCaseMap::default();
    assert!(
        cases
            .append_name(Bytes::from_static(b"X-Good\r\nInjected: yes"))
            .is_err()
    );
}

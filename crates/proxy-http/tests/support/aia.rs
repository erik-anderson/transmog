//! Missing-issuer fixtures shared by the TCP and QUIC adapters.
#![allow(dead_code)]
use crate::support::local_certificates::{CertificateOptions, configured_certificate};
use boring::{
    pkey::{PKey, Private},
    x509::{X509, X509Extension},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};
use transmog_tls::{
    PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

pub struct AiaOrigin {
    pub port: u16,
    pub requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}
impl Drop for AiaOrigin {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn responses(responses: HashMap<String, Vec<u8>>) -> AiaOrigin {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let responses = Arc::new(responses);
    let task = tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {
                incoming = listener.accept() => {
                    let (mut socket, _) = incoming.unwrap();
                    let seen = seen.clone();
                    let responses = responses.clone();
                    children.spawn(async move {
                        let mut request = Vec::new();
                        let mut input = [0; 1024];
                        while !request.ends_with(b"\r\n\r\n") {
                            let n = socket.read(&mut input).await.unwrap();
                            if n == 0 { return; }
                            request.extend_from_slice(&input[..n]);
                            assert!(request.len() < 16384);
                        }
                        let request = String::from_utf8(request).unwrap();
                        let path = request.split_whitespace().nth(1).unwrap().to_owned();
                        seen.lock().unwrap().push(request);
                        if let Some(response) = responses.get(&path) { let _ = socket.write_all(response).await; }
                        else { std::future::pending::<()>().await; }
                    });
                }
                _ = children.join_next(), if !children.is_empty() => {},
            }
        }
    });
    AiaOrigin {
        port,
        requests,
        task,
    }
}
pub fn ok(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}
pub async fn issuer(body: &[u8]) -> AiaOrigin {
    responses(HashMap::from([("/issuer".into(), ok(body))])).await
}
pub fn factory(root: &ProxyCa) -> UpstreamTlsContextFactory {
    let source =
        PemTrustSource::from_pem(&root.certificate_pem().unwrap(), "AIA test root").unwrap();
    UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&source, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
}
pub fn root(name: &str) -> ProxyCa {
    let mut options = CertificateOptions::new(name);
    options.ca = true;
    let (cert, key) = configured_certificate(options);
    ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap()
}
#[allow(deprecated)] // Fixture generation mirrors the extension's real wire encoding.
pub fn leaf(issuer: &ProxyCa, urls: &[String], dns: &str) -> (X509, PKey<Private>) {
    let mut options = CertificateOptions::new("AIA leaf");
    options.issuer = Some(issuer);
    options.dns_name = Some(dns);
    if !urls.is_empty() {
        let value = urls
            .iter()
            .map(|url| format!("caIssuers;URI:{url}"))
            .collect::<Vec<_>>()
            .join(",");
        options
            .extensions
            .push(X509Extension::new(None, None, "authorityInfoAccess", &value).unwrap());
    }
    configured_certificate(options)
}
pub async fn probe(
    factory: &UpstreamTlsContextFactory,
    leaf: &X509,
    key: &PKey<Private>,
    expected: bool,
) {
    let (port, server) = tls_origin(leaf, key).await;
    let result = transmog_http::HyperOriginClient::new(factory)
        .unwrap()
        .execute(
            crate::support::request(port),
            transmog_http::HyperEgressMode::Http1Only,
            1024,
            Duration::from_secs(3),
        )
        .await;
    assert_eq!(
        result.is_ok(),
        expected,
        "{}",
        result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap(),
        expected
    );
}
pub async fn tls_origin(leaf: &X509, key: &PKey<Private>) -> (u16, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut tls =
        boring::ssl::SslAcceptor::mozilla_intermediate_v5(boring::ssl::SslMethod::tls()).unwrap();
    tls.set_certificate(leaf).unwrap();
    tls.set_private_key(key).unwrap();
    let tls = tls.build();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        if let Ok(mut tls) = tokio_boring::accept(&tls, tcp).await {
            let mut request = [0; 4096];
            assert!(tls.read(&mut request).await.unwrap() > 0);
            tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            true
        } else {
            false
        }
    });
    (port, server)
}

//! Shared local, untrusted TLS peers and observable software identities.
#![allow(dead_code)]
#[cfg(windows)]
pub mod windows;
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::{PKey, Private},
    ssl::{
        AlpnError, SslAcceptor, SslMethod, SslSessionCacheMode, SslVerifyMode, select_next_proto,
    },
    x509::{
        X509, X509NameBuilder,
        extension::{
            AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage,
            SubjectKeyIdentifier,
        },
        store::X509StoreBuilder,
    },
};
use bytes::Bytes;
use http_body_util::Full;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use transmog_core::{
    BodyFrame, CanonicalRequest, HeaderBlock, HttpLegVersion, RequestHead, Target,
    client_authentication::{
        ClientCertificateAction, ClientCertificatePolicy, ClientCertificateRule,
        ClientCertificateSelector,
    },
};
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory,
    UpstreamTlsPolicy,
    client_authentication::{
        ClientCredential, ClientCredentialInventoryFuture, ClientCredentialSource, ClientSigningKey,
    },
    client_prompts::ClientCertificateBroker,
};

pub fn key() -> PKey<Private> {
    PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap()
}
pub fn certificate(
    name: &str,
    key: &PKey<Private>,
    issuer: Option<(&X509, &PKey<Private>)>,
    ca: bool,
    digest: MessageDigest,
) -> X509 {
    static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let mut subject = X509NameBuilder::new().unwrap();
    subject.append_entry_by_text("CN", name).unwrap();
    let subject = subject.build();
    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder
        .set_serial_number(
            &BigNum::from_u32(SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                .unwrap()
                .to_asn1_integer()
                .unwrap(),
        )
        .unwrap();
    builder.set_subject_name(&subject).unwrap();
    builder
        .set_issuer_name(issuer.map_or(subject.as_ref(), |(cert, _)| cert.subject_name()))
        .unwrap();
    builder.set_pubkey(key).unwrap();
    builder
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    builder
        .set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    let mut constraints = BasicConstraints::new();
    constraints.critical();
    let mut usage = KeyUsage::new();
    usage.critical().digital_signature();
    if ca {
        constraints.ca().pathlen(1);
        usage.key_cert_sign();
    }
    builder
        .append_extension(&constraints.build().unwrap())
        .unwrap();
    builder.append_extension(&usage.build().unwrap()).unwrap();
    builder
        .append_extension(&ExtendedKeyUsage::new().client_auth().build().unwrap())
        .unwrap();
    let ski = SubjectKeyIdentifier::new()
        .build(&builder.x509v3_context(None, None))
        .unwrap();
    builder.append_extension(&ski).unwrap();
    if let Some((issuer, _)) = issuer {
        let aki = AuthorityKeyIdentifier::new()
            .keyid(true)
            .build(&builder.x509v3_context(Some(issuer), None))
            .unwrap();
        builder.append_extension(&aki).unwrap();
    }
    builder
        .sign(issuer.map_or(key, |(_, key)| key), digest)
        .unwrap();
    builder.build()
}
pub struct Key {
    key: PKey<Private>,
    pub signatures: Arc<AtomicUsize>,
}
impl ClientSigningKey for Key {
    fn algorithms(&self) -> &[u16] {
        &[0x0403]
    }
    fn available(&self) -> bool {
        true
    }
    fn sign(&self, _: u16, input: &[u8]) -> Result<Vec<u8>, String> {
        self.signatures.fetch_add(1, Ordering::SeqCst);
        let mut signer = boring::sign::Signer::new(MessageDigest::sha256(), &self.key).unwrap();
        signer.update(input).unwrap();
        Ok(signer.sign_to_vec().unwrap())
    }
}
pub struct Source {
    pub credential: Arc<ClientCredential>,
    pub reads: Arc<AtomicUsize>,
}
impl ClientCredentialSource for Source {
    fn credentials(&self) -> ClientCredentialInventoryFuture {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let credential = self.credential.clone();
        Box::pin(async move { Ok(vec![credential]) })
    }
}
pub struct Fixture {
    pub ca: ProxyCa,
    pub source: Arc<Source>,
    pub signatures: Arc<AtomicUsize>,
}
impl Fixture {
    pub fn new() -> Self {
        let ca = ProxyCa::generate("HTTP certificate integration", 2).unwrap();
        let root_key = PKey::private_key_from_pem(&ca.private_key_pem_pkcs8().unwrap()).unwrap();
        let private = key();
        let leaf = certificate(
            "Client",
            &private,
            Some((ca.certificate(), &root_key)),
            false,
            MessageDigest::sha256(),
        );
        let signatures = Arc::new(AtomicUsize::new(0));
        let source = Arc::new(Source {
            credential: Arc::new(ClientCredential {
                id: "test-client".into(),
                chain: vec![leaf, ca.certificate().clone()],
                key: Arc::new(Key {
                    key: private,
                    signatures: signatures.clone(),
                }),
            }),
            reads: Arc::new(AtomicUsize::new(0)),
        });
        Self {
            ca,
            source,
            signatures,
        }
    }
    pub fn client(
        &self,
        resolver: Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>,
        caller: &str,
    ) -> HyperOriginClient {
        let trust =
            PemTrustSource::from_pem(&self.ca.certificate_pem().unwrap(), "fixture").unwrap();
        HyperOriginClient::new(
            &UpstreamTlsContextFactory::new(
                Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
                UpstreamTlsPolicy::default(),
            )
            .with_client_certificate_resolver(resolver, caller),
        )
        .unwrap()
    }
}
pub struct Origin {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
    pub count: Arc<AtomicUsize>,
    pub resumed: Arc<AtomicUsize>,
    pub requests: Arc<AtomicUsize>,
}
impl Drop for Origin {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn origin(f: &Fixture, h2: bool, close: bool, deadline: Duration) -> Origin {
    origin_advertising(f, h2, close, deadline, None).await
}
pub async fn origin_advertising(
    f: &Fixture,
    h2: bool,
    close: bool,
    deadline: Duration,
    advertisement: Option<http::HeaderValue>,
) -> Origin {
    origin_options(f, h2, close, deadline, advertisement, true).await
}
pub async fn origin_without_client_authentication(
    f: &Fixture,
    advertisement: http::HeaderValue,
) -> Origin {
    origin_options(
        f,
        false,
        false,
        Duration::from_secs(5),
        Some(advertisement),
        false,
    )
    .await
}
async fn origin_options(
    f: &Fixture,
    h2: bool,
    close: bool,
    deadline: Duration,
    advertisement: Option<http::HeaderValue>,
    request_certificate: bool,
) -> Origin {
    let identity =
        f.ca.issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    builder.set_certificate(&identity.certificate).unwrap();
    builder.set_private_key(&identity.private_key).unwrap();
    let mut trust = X509StoreBuilder::new().unwrap();
    trust
        .add_cert(f.source.credential.chain.last().unwrap().clone())
        .unwrap();
    builder.set_cert_store(trust.build());
    builder.set_verify(if request_certificate {
        SslVerifyMode::PEER
    } else {
        SslVerifyMode::NONE
    });
    builder.set_session_cache_mode(SslSessionCacheMode::SERVER);
    builder
        .set_session_id_context(b"certificate-integration")
        .unwrap();
    builder.set_alpn_select_callback(move |_, wire| {
        select_next_proto(if h2 { b"\x02h2" } else { b"\x08http/1.1" }, wire)
            .ok_or(AlpnError::NOACK)
    });
    let acceptor = builder.build();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let resumed = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    let session_count = resumed.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let request_count = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let id = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let acceptor = acceptor.clone();
            let session_count = session_count.clone();
            let advertisement = advertisement.clone();
            let request_count = request_count.clone();
            tokio::spawn(async move {
                let Ok(Ok(tls)) =
                    tokio::time::timeout(deadline, tokio_boring::accept(&acceptor, tcp)).await
                else {
                    return;
                };
                if tls.ssl().session_reused() {
                    session_count.fetch_add(1, Ordering::SeqCst);
                }
                let authenticated = tls.ssl().peer_certificate().is_some();
                let service = hyper::service::service_fn(move |_| {
                    request_count.fetch_add(1, Ordering::SeqCst);
                    let mut response = http::Response::new(Full::new(Bytes::from(format!(
                        "{id}:{authenticated}"
                    ))));
                    if close && !h2 {
                        response
                            .headers_mut()
                            .insert("connection", http::HeaderValue::from_static("close"));
                    }
                    if let Some(value) = &advertisement {
                        response.headers_mut().insert("alt-svc", value.clone());
                    }
                    async move { Ok::<_, std::convert::Infallible>(response) }
                });
                if h2 {
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(tls), service)
                        .await;
                } else {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), service)
                        .await;
                }
            });
        }
    });
    Origin {
        port,
        task,
        count,
        resumed,
        requests,
    }
}
pub fn request(port: u16) -> CanonicalRequest {
    CanonicalRequest {
        head: RequestHead {
            method: "GET".into(),
            target: Target {
                scheme: "https".into(),
                host: "localhost".into(),
                port,
                authority: format!("localhost:{port}"),
                path: "/".into(),
                query: None,
            },
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http1,
        },
        body: vec![],
    }
}
pub async fn execute(client: &HyperOriginClient, port: u16, h2: bool) -> String {
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        client.execute(
            request(port),
            if h2 {
                HyperEgressMode::Http2Only
            } else {
                HyperEgressMode::Http1Only
            },
            1024,
            Duration::from_secs(2),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let mut bytes = Vec::new();
    for frame in result.body {
        if let BodyFrame::Data(data) = frame {
            bytes.extend(data);
        }
    }
    String::from_utf8(bytes).unwrap()
}
pub fn automatic(port: u16) -> ClientCertificatePolicy {
    ClientCertificatePolicy {
        rules: vec![ClientCertificateRule {
            id: "automatic".into(),
            enabled: true,
            host: "localhost".into(),
            port,
            mode: ClientCertificateAction::Automatic {
                selector: ClientCertificateSelector {
                    identity: Some("test-client".into()),
                    ..Default::default()
                },
            },
        }],
        ..Default::default()
    }
}
pub async fn wait_pending(broker: &ClientCertificateBroker, connections: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if broker
                .pending()
                .iter()
                .map(|p| p.connections)
                .sum::<usize>()
                == connections
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

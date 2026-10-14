//! Real handshakes prove that selection follows a verified `CertificateRequest`.
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::{PKey, Private},
    ssl::{SslAcceptor, SslMethod, SslSignatureAlgorithm, SslVerifyMode, SslVersion},
    x509::{
        X509, X509NameBuilder,
        extension::{ExtendedKeyUsage, KeyUsage},
        store::X509StoreBuilder,
    },
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Waker,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Notify,
};
use transmog_tls::client_authentication::{
    CertificateRequest, ClientAuthenticationAttempt, ClientAuthenticationContext,
    ClientAuthenticationDecision, ClientAuthenticationFuture, ClientAuthenticationResolver,
    ClientCredential, ClientSigningKey,
};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory,
    UpstreamTlsPolicy,
};
use transmog_tls_native::{
    SelectionResult, clear_client_credentials, set_client_certificate_callback,
};

struct TestSigningKey {
    key: PKey<Private>,
    available: AtomicBool,
    signatures: AtomicUsize,
}
impl ClientSigningKey for TestSigningKey {
    fn algorithms(&self) -> &[u16] {
        &[0x0403]
    }
    fn available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
    fn sign(&self, algorithm: u16, input: &[u8]) -> Result<Vec<u8>, String> {
        assert_eq!(algorithm, 0x0403);
        self.signatures.fetch_add(1, Ordering::SeqCst);
        let mut signer = boring::sign::Signer::new(MessageDigest::sha256(), &self.key)
            .map_err(|_| "test key failure")?;
        signer.update(input).map_err(|_| "test signing failure")?;
        signer
            .sign_to_vec()
            .map_err(|_| "test signing failure".into())
    }
}
struct TestResolver {
    epoch: std::sync::atomic::AtomicU64,
    calls: AtomicUsize,
    notice: Notify,
    decision: Mutex<Option<ClientAuthenticationFuture>>,
}
impl ClientAuthenticationResolver for TestResolver {
    fn generation(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    fn resolve(
        &self,
        context: ClientAuthenticationContext,
        request: CertificateRequest,
    ) -> ClientAuthenticationFuture {
        assert_eq!(context.host, "localhost");
        assert_eq!(context.caller, "process:test");
        assert_ne!(context.port, 0);
        assert!(matches!(
            request.tls_version,
            Some(SslVersion::TLS1_2 | SslVersion::TLS1_3)
        ));
        assert!(!request.signature_algorithms.is_empty());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.notice.notify_one();
        self.decision
            .lock()
            .unwrap()
            .take()
            .expect("only one real request")
    }
}
fn resolver(decision: ClientAuthenticationFuture) -> Arc<TestResolver> {
    Arc::new(TestResolver {
        epoch: std::sync::atomic::AtomicU64::new(1),
        calls: AtomicUsize::new(0),
        notice: Notify::new(),
        decision: Mutex::new(Some(decision)),
    })
}
fn credential(ca: &ProxyCa) -> (Arc<ClientCredential>, Arc<TestSigningKey>) {
    let (certificate, key) = client_identity(ca);
    let key = Arc::new(TestSigningKey {
        key,
        available: AtomicBool::new(true),
        signatures: AtomicUsize::new(0),
    });
    (
        Arc::new(ClientCredential {
            id: "fixture-client".into(),
            chain: vec![certificate, ca.certificate().clone()],
            key: key.clone(),
        }),
        key,
    )
}
fn attempt(resolver: Arc<TestResolver>, port: u16) -> ClientAuthenticationAttempt {
    ClientAuthenticationAttempt::new(
        resolver,
        ClientAuthenticationContext {
            host: "localhost".into(),
            port,
            caller: "process:test".into(),
            generation: 1,
        },
        transmog_tls::TlsCapture::new(transmog_core::TlsEvidence::default()),
    )
}

struct TestInventory {
    identities: Vec<Arc<ClientCredential>>,
    reads: Arc<AtomicUsize>,
}
impl transmog_tls::client_authentication::ClientCredentialSource for TestInventory {
    fn credentials(&self) -> transmog_tls::client_authentication::ClientCredentialInventoryFuture {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let identities = self.identities.clone();
        Box::pin(async move { Ok(identities) })
    }
}

fn client_identity(ca: &ProxyCa) -> (X509, PKey<Private>) {
    client_identity_with_expiry(ca, false)
}
fn client_identity_with_expiry(ca: &ProxyCa, expired: bool) -> (X509, PKey<Private>) {
    client_identity_on_curve(ca, expired, Nid::X9_62_PRIME256V1)
}
fn client_identity_on_curve(ca: &ProxyCa, expired: bool, curve: Nid) -> (X509, PKey<Private>) {
    client_identity_with_usage(
        ca,
        expired,
        curve,
        Some(KeyUsage::new().digital_signature().build().unwrap()),
    )
}
fn client_identity_with_usage(
    ca: &ProxyCa,
    expired: bool,
    curve: Nid,
    key_usage: Option<boring::x509::X509Extension>,
) -> (X509, PKey<Private>) {
    let key =
        PKey::from_ec_key(EcKey::generate(&EcGroup::from_curve_name(curve).unwrap()).unwrap())
            .unwrap();
    client_identity_with_key(ca, expired, key, key_usage)
}
fn client_identity_with_key(
    ca: &ProxyCa,
    expired: bool,
    key: PKey<Private>,
    key_usage: Option<boring::x509::X509Extension>,
) -> (X509, PKey<Private>) {
    let ca_key = PKey::private_key_from_pem(&ca.private_key_pem_pkcs8().unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "Test client identity")
        .unwrap();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_serial_number(&BigNum::from_u32(17).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    cert.set_subject_name(&name.build()).unwrap();
    cert.set_issuer_name(ca.certificate().subject_name())
        .unwrap();
    cert.set_pubkey(&key).unwrap();
    let not_before = if expired {
        Asn1Time::from_unix(0).unwrap()
    } else {
        Asn1Time::days_from_now(0).unwrap()
    };
    let not_after = if expired {
        Asn1Time::from_unix(1).unwrap()
    } else {
        Asn1Time::days_from_now(1).unwrap()
    };
    cert.set_not_before(&not_before).unwrap();
    cert.set_not_after(&not_after).unwrap();
    if let Some(key_usage) = key_usage {
        cert.append_extension(&key_usage).unwrap();
    }
    cert.append_extension(&ExtendedKeyUsage::new().client_auth().build().unwrap())
        .unwrap();
    cert.sign(&ca_key, MessageDigest::sha256()).unwrap();
    (cert.build(), key)
}

struct MlDsaSigningKey {
    key: PKey<Private>,
    algorithms: [u16; 1],
    signatures: AtomicUsize,
}
impl ClientSigningKey for MlDsaSigningKey {
    fn algorithms(&self) -> &[u16] {
        &self.algorithms
    }
    fn available(&self) -> bool {
        true
    }
    fn sign(&self, algorithm: u16, input: &[u8]) -> Result<Vec<u8>, String> {
        assert_eq!(algorithm, self.algorithms[0]);
        self.signatures.fetch_add(1, Ordering::SeqCst);
        boring::sign::Signer::new_without_digest(&self.key)
            .and_then(|mut signer| signer.sign_oneshot_to_vec(input))
            .map_err(|_| "ML-DSA fixture signing failed".into())
    }
}

#[tokio::test]
async fn ml_dsa_client_credentials_require_exact_scheme_and_tls13_and_sign_real_requests() {
    use transmog_tls::MlDsaVariant;
    for variant in [
        MlDsaVariant::MlDsa44,
        MlDsaVariant::MlDsa65,
        MlDsaVariant::MlDsa87,
    ] {
        let ca = ProxyCa::generate("ML-DSA client root", 2).unwrap();
        let (cert, key) = client_identity_with_key(
            &ca,
            false,
            variant.generate_key().unwrap(),
            Some(KeyUsage::new().digital_signature().build().unwrap()),
        );
        let expected = cert.to_der().unwrap();
        let algorithm = variant.signature_algorithm();
        let key = Arc::new(MlDsaSigningKey {
            key,
            algorithms: [variant.signature_scheme()],
            signatures: AtomicUsize::new(0),
        });
        let identity = Arc::new(ClientCredential {
            id: "ml-dsa-client".into(),
            chain: vec![cert, ca.certificate().clone()],
            key: key.clone(),
        });
        let mut request = CertificateRequest {
            tls_version: Some(SslVersion::TLS1_3),
            signature_algorithms: vec![variant.signature_scheme()],
            ..CertificateRequest::default()
        };
        assert!(identity.compatible(&request));
        request.tls_version = Some(SslVersion::TLS1_2);
        assert!(!identity.compatible(&request));
        request.tls_version = Some(SslVersion::TLS1_3);
        request.signature_algorithms = vec![0x0403];
        assert!(!identity.compatible(&request));
        for other in [
            MlDsaVariant::MlDsa44,
            MlDsaVariant::MlDsa65,
            MlDsaVariant::MlDsa87,
        ] {
            if other != variant {
                request.signature_algorithms = vec![other.signature_scheme()];
                assert!(!identity.compatible(&request));
            }
        }
        let (address, server) = server_with_constraints(
            &ca,
            SslVersion::TLS1_3,
            SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT,
            1,
            Some(&[algorithm]),
        );
        let resolver = resolver(Box::pin(async move {
            ClientAuthenticationDecision::Certificate(identity)
        }));
        let attempt = attempt(resolver.clone(), address.port());
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl("localhost")
            .unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let mut tls = tokio::time::timeout(
            Duration::from_secs(5),
            attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
        )
        .await
        .unwrap()
        .unwrap();
        tls.write_all(&[1]).await.unwrap();
        let mut response = [0; 7];
        tls.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"private");
        assert_eq!(server.join().unwrap().unwrap().to_der().unwrap(), expected);
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(key.signatures.load(Ordering::SeqCst), 1);
    }
}

fn factory(ca: &ProxyCa) -> UpstreamTlsContextFactory {
    let source =
        PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "request test root").unwrap();
    UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&source, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
}

fn server(
    ca: &ProxyCa,
    version: SslVersion,
    verify: SslVerifyMode,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Option<X509>>) {
    server_with_issuer_count(ca, version, verify, 1)
}
fn server_with_issuer_count(
    ca: &ProxyCa,
    version: SslVersion,
    verify: SslVerifyMode,
    issuer_count: usize,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Option<X509>>) {
    server_with_constraints(ca, version, verify, issuer_count, None)
}
fn server_with_constraints(
    ca: &ProxyCa,
    version: SslVersion,
    verify: SslVerifyMode,
    issuer_count: usize,
    algorithms: Option<&[SslSignatureAlgorithm]>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Option<X509>>) {
    let identity = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    builder.set_min_proto_version(Some(version)).unwrap();
    builder.set_max_proto_version(Some(version)).unwrap();
    builder.set_certificate(&identity.certificate).unwrap();
    builder.set_private_key(&identity.private_key).unwrap();
    let mut store = X509StoreBuilder::new().unwrap();
    store.add_cert(ca.certificate().clone()).unwrap();
    builder.set_cert_store(store.build());
    builder.set_verify(verify);
    if let Some(algorithms) = algorithms {
        builder.set_verify_algorithm_prefs(algorithms).unwrap();
    }
    let mut names = boring::stack::Stack::new().unwrap();
    for _ in 0..issuer_count {
        names
            .push(
                boring::x509::X509Name::from_der(
                    &ca.certificate().subject_name().to_der().unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
    }
    builder.set_client_ca_list(names);
    let acceptor = builder.build();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let Ok(mut stream) = acceptor.accept(tcp) else {
            return None;
        };
        // On the server this API excludes the leaf. These fixtures are issued
        // directly by a root, so no root or intermediate should be transmitted.
        assert!(
            stream
                .ssl()
                .peer_cert_chain()
                .is_none_or(boring::stack::StackRef::is_empty)
        );
        let certificate = stream.ssl().peer_certificate();
        let mut byte = [0];
        if stream.read_exact(&mut byte).is_ok() {
            // Identical request, observably different response under optional auth.
            stream
                .write_all(if certificate.is_some() {
                    b"private"
                } else {
                    b"public!"
                })
                .unwrap();
        }
        certificate
    });
    (address, worker)
}

#[tokio::test]
async fn actual_request_controls_identity_and_optional_auth_changes_response() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for (verify, allow) in [
            (SslVerifyMode::NONE, true),
            (SslVerifyMode::PEER, false),
            (SslVerifyMode::PEER, true),
        ] {
            let ca = ProxyCa::generate("Request fixture root", 2).unwrap();
            let (cert, key) = client_identity(&ca);
            let (address, server) = server(&ca, version, verify);
            let connector = factory(&ca).hyper_connector().unwrap();
            let mut configuration = connector.configure().unwrap();
            configuration.set_min_proto_version(Some(version)).unwrap();
            configuration.set_max_proto_version(Some(version)).unwrap();
            let mut ssl = configuration.into_ssl("localhost").unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let calls = count.clone();
            let expected_issuer = ca.certificate().subject_name().to_der().unwrap();
            set_client_certificate_callback(&mut ssl, move |ssl, request| {
                calls.fetch_add(1, Ordering::SeqCst);
                assert!(request.issuers.contains(&expected_issuer));
                assert!(request.signature_algorithms.contains(&0x0403));
                clear_client_credentials(ssl);
                if allow {
                    ssl.set_certificate(&cert).unwrap();
                    ssl.set_private_key(&key).unwrap();
                }
                SelectionResult::Ready
            })
            .unwrap();
            let tcp = TcpStream::connect(address).await.unwrap();
            let mut stream = tokio::time::timeout(
                Duration::from_secs(3),
                tokio_boring::SslStreamBuilder::new(ssl, tcp).connect(),
            )
            .await
            .unwrap()
            .unwrap();
            stream.write_all(&[1]).await.unwrap();
            let mut response = [0; 7];
            stream.read_exact(&mut response).await.unwrap();
            let requested = verify == SslVerifyMode::PEER;
            assert_eq!(count.load(Ordering::SeqCst), usize::from(requested));
            assert_eq!(
                &response,
                if requested && allow {
                    b"private"
                } else {
                    b"public!"
                }
            );
            assert_eq!(server.join().unwrap().is_some(), requested && allow);
        }
    }
}

#[tokio::test]
async fn suspended_request_resumes_same_handshake_after_explicit_selection() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        let ca = ProxyCa::generate("Paused fixture root", 2).unwrap();
        let (cert, key) = client_identity(&ca);
        let (address, server) = server(
            &ca,
            version,
            SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT,
        );
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut configuration = connector.configure().unwrap();
        configuration.set_min_proto_version(Some(version)).unwrap();
        configuration.set_max_proto_version(Some(version)).unwrap();
        let mut ssl = configuration.into_ssl("localhost").unwrap();
        let approved = Arc::new(AtomicBool::new(false));
        let decision = approved.clone();
        let requested = Arc::new(Notify::new());
        let notice = requested.clone();
        set_client_certificate_callback(&mut ssl, move |ssl, _| {
            clear_client_credentials(ssl);
            if !decision.load(Ordering::Acquire) {
                notice.notify_one();
                return SelectionResult::Pending;
            }
            ssl.set_certificate(&cert).unwrap();
            ssl.set_private_key(&key).unwrap();
            SelectionResult::Ready
        })
        .unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let mut handshake = Box::pin(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect());
        let waker = Arc::new(Mutex::new(None::<Waker>));
        let wake = waker.clone();
        let worker = tokio::spawn(async move {
            std::future::poll_fn(|cx| {
                *wake.lock().unwrap() = Some(cx.waker().clone());
                handshake.as_mut().poll(cx)
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), requested.notified())
            .await
            .unwrap();
        assert!(!worker.is_finished());
        approved.store(true, Ordering::Release);
        waker.lock().unwrap().take().unwrap().wake();
        let mut stream = tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        stream.write_all(&[1]).await.unwrap();
        let mut response = [0; 7];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"private");
        assert!(server.join().unwrap().is_some());
    }
}

fn quic_config(context: boring::ssl::SslContextBuilder) -> quiche::Config {
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, context).unwrap();
    config.set_application_protos(&[b"h3"]).unwrap();
    config.set_max_idle_timeout(2_000);
    config.set_max_recv_udp_payload_size(1350);
    config.set_max_send_udp_payload_size(1350);
    config.set_initial_max_data(65_536);
    config.set_initial_max_stream_data_bidi_local(16_384);
    config.set_initial_max_stream_data_bidi_remote(16_384);
    config.set_initial_max_streams_bidi(10);
    config
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One live QUIC scene owns both endpoints, suspension and wire evidence.
async fn quic_resumes_certificate_lookup_and_server_verifies_client_signature() {
    let ca = ProxyCa::generate("QUIC request fixture", 2).unwrap();
    let (credential, key) = credential(&ca);
    let expected = credential.chain[0].to_der().unwrap();
    let identity = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let mut server_tls = boring::ssl::SslContext::builder(SslMethod::tls()).unwrap();
    server_tls.set_certificate(&identity.certificate).unwrap();
    server_tls.set_private_key(&identity.private_key).unwrap();
    let mut trust = X509StoreBuilder::new().unwrap();
    trust.add_cert(ca.certificate().clone()).unwrap();
    server_tls.set_cert_store(trust.build());
    server_tls.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
    let mut client_tls = factory(&ca).quiche_context_builder().unwrap();
    let server_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let client_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_address = server_socket.local_addr().unwrap();
    let client_address = client_socket.local_addr().unwrap();
    let (decision, receiver) = tokio::sync::oneshot::channel();
    let mut decision = Some(decision);
    let resolver = resolver(Box::pin(async move { receiver.await.unwrap() }));
    let attempt = attempt(resolver.clone(), server_address.port());
    attempt.attach_context(&mut client_tls).unwrap();
    let mut client_config = quic_config(client_tls);
    let mut server_config = quic_config(server_tls);
    let client_id = quiche::ConnectionId::from_ref(b"client-fixture-id");
    let server_id = quiche::ConnectionId::from_ref(b"server-fixture-id");
    let mut client = quiche::connect(
        None,
        &client_id,
        client_address,
        server_address,
        &mut client_config,
    )
    .unwrap();
    transmog_tls_native::configure_quic_server_name(client.as_mut(), "localhost").unwrap();
    let mut server = quiche::accept(
        &server_id,
        None,
        server_address,
        client_address,
        &mut server_config,
    )
    .unwrap();
    let mut buffer = vec![0; 65_536];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_pause = false;
    attempt
        .drive(async {
            loop {
                for (connection, socket) in
                    [(&mut client, &client_socket), (&mut server, &server_socket)]
                {
                    while let Ok((written, info)) = connection.send(&mut buffer) {
                        socket.send_to(&buffer[..written], info.to).await.unwrap();
                    }
                }
                for (connection, socket, address) in [
                    (&mut client, &client_socket, client_address),
                    (&mut server, &server_socket, server_address),
                ] {
                    while let Ok((read, from)) = socket.try_recv_from(&mut buffer) {
                        match connection
                            .recv(&mut buffer[..read], quiche::RecvInfo { from, to: address })
                        {
                            Ok(_) | Err(quiche::Error::Done) => (),
                            result => panic!("QUIC handshake failed: {result:?}"),
                        }
                    }
                }
                if resolver.calls.load(Ordering::SeqCst) > 0 && !saw_pause {
                    assert!(!client.is_established());
                    assert!(!server.is_established());
                    assert!(server.peer_cert_chain().is_none());
                    saw_pause = true;
                    decision
                        .take()
                        .unwrap()
                        .send(ClientAuthenticationDecision::Certificate(
                            credential.clone(),
                        ))
                        .unwrap();
                }
                if client.is_established() && server.is_established() {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "QUIC lookup failed to resume"
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
    assert!(saw_pause);
    assert_eq!(server.peer_cert_chain().unwrap()[0], expected);
    assert_eq!(key.signatures.load(Ordering::SeqCst), 1);
    let evidence = attempt.evidence();
    assert_eq!(
        evidence.details["Client certificate"],
        "Certificate queued to TLS transport"
    );
    assert_eq!(server.peer_cert_chain().unwrap().len(), 1);
    assert_eq!(evidence.client_chain.len(), 1);
    assert_eq!(
        evidence.client_chain[0].der.as_ref(),
        server.peer_cert_chain().unwrap()[0]
    );
}

#[tokio::test]
async fn embedding_decision_and_background_signer_are_verified_by_live_server() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for requested in [false, true] {
            let ca = ProxyCa::generate("Embedding request fixture", 2).unwrap();
            let (identity, key) = credential(&ca);
            let (address, server) = server(
                &ca,
                version,
                if requested {
                    SslVerifyMode::PEER
                } else {
                    SslVerifyMode::NONE
                },
            );
            let resolver = resolver(Box::pin(async move {
                ClientAuthenticationDecision::Certificate(identity)
            }));
            let attempt = attempt(resolver.clone(), address.port());
            let connector = factory(&ca).hyper_connector().unwrap();
            let mut config = connector.configure().unwrap();
            config.set_min_proto_version(Some(version)).unwrap();
            config.set_max_proto_version(Some(version)).unwrap();
            let mut ssl = config.into_ssl("localhost").unwrap();
            attempt.attach_ssl(&mut ssl).unwrap();
            let tcp = TcpStream::connect(address).await.unwrap();
            let mut stream = tokio::time::timeout(
                Duration::from_secs(3),
                attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
            )
            .await
            .unwrap()
            .unwrap();
            stream.write_all(&[1]).await.unwrap();
            let mut response = [0; 7];
            stream.read_exact(&mut response).await.unwrap();
            assert_eq!(&response, if requested { b"private" } else { b"public!" });
            assert_eq!(
                resolver.calls.load(Ordering::SeqCst),
                usize::from(requested)
            );
            assert_eq!(
                key.signatures.load(Ordering::SeqCst),
                usize::from(requested)
            );
            let observed = server.join().unwrap();
            assert_eq!(observed.is_some(), requested);
            let evidence = attempt.evidence();
            if requested {
                assert_eq!(
                    evidence.details["Client certificate"],
                    "Certificate queued to TLS transport"
                );
                assert_eq!(evidence.client_chain.len(), 1);
                assert_eq!(
                    evidence.client_chain[0].der.as_ref(),
                    observed.unwrap().to_der().unwrap()
                );
            } else {
                assert!(evidence.client_chain.is_empty());
                assert!(!evidence.details.contains_key("Client certificate"));
            }
        }
    }
}

#[tokio::test]
async fn incompatible_or_missing_selected_identity_attempts_anonymous_handshake() {
    for invalidity in ["missing", "wrong issuer", "wrong EKU", "expired"] {
        let ca = ProxyCa::generate("Anonymous fallback fixture", 2).unwrap();
        let (mut identity, key) = credential(&ca);
        match invalidity {
            "missing" => key.available.store(false, Ordering::Release),
            "wrong issuer" => {
                identity = credential(&ProxyCa::generate("Other issuer", 2).unwrap()).0;
            }
            "wrong EKU" => {
                let server_leaf = ca
                    .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
                    .unwrap();
                Arc::make_mut(&mut identity).chain[0] = server_leaf.certificate;
            }
            "expired" => {
                Arc::make_mut(&mut identity).chain[0] = client_identity_with_expiry(&ca, true).0;
            }
            _ => unreachable!(),
        }
        let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
        let resolver = resolver(Box::pin(async move {
            ClientAuthenticationDecision::Certificate(identity)
        }));
        let attempt = attempt(resolver, address.port());
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl("localhost")
            .unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let mut stream = tokio::time::timeout(
            Duration::from_secs(3),
            attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
        )
        .await
        .unwrap()
        .unwrap();
        stream.write_all(&[1]).await.unwrap();
        let mut response = [0; 7];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"public!");
        assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
        assert!(server.join().unwrap().is_none());
    }
}

#[tokio::test]
async fn untrusted_or_wrong_host_server_never_queries_embedding_app_or_key() {
    for wrong_host in [false, true] {
        let ca = ProxyCa::generate("Untrusted request fixture", 2).unwrap();
        let other_ca = ProxyCa::generate("Trusted fixture root", 2).unwrap();
        let (identity, key) = credential(&ca);
        let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
        let resolver = resolver(Box::pin(async move {
            ClientAuthenticationDecision::Certificate(identity)
        }));
        let attempt = attempt(resolver.clone(), address.port());
        let connector = factory(if wrong_host { &ca } else { &other_ca })
            .hyper_connector()
            .unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl(if wrong_host {
                "other.example"
            } else {
                "localhost"
            })
            .unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_secs(3),
                attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
        assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
        assert!(server.join().unwrap().is_none());
    }
}

#[tokio::test]
async fn pending_decision_cancellation_closes_waiter_and_rejects_late_selection() {
    let ca = ProxyCa::generate("Cancelled request fixture", 2).unwrap();
    let (identity, key) = credential(&ca);
    let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
    let (decision, receiver) = tokio::sync::oneshot::channel();
    let resolver = resolver(Box::pin(async move { receiver.await.unwrap() }));
    let attempt = attempt(resolver.clone(), address.port());
    let driver = attempt.clone();
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut ssl = connector
        .configure()
        .unwrap()
        .into_ssl("localhost")
        .unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let worker = tokio::spawn(async move {
        driver
            .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), resolver.notice.notified())
        .await
        .unwrap();
    assert!(!worker.is_finished());
    attempt.cancel("origin closed while waiting");
    assert!(
        decision
            .send(ClientAuthenticationDecision::Certificate(identity))
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
    assert!(server.join().unwrap().is_none());
}

#[tokio::test]
async fn removing_identity_or_changing_authorization_while_waiting_does_not_sign() {
    for change_policy in [false, true] {
        let ca = ProxyCa::generate("Stale approval fixture", 2).unwrap();
        let (identity, key) = credential(&ca);
        let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
        let (decision, receiver) = tokio::sync::oneshot::channel();
        let resolver = resolver(Box::pin(async move { receiver.await.unwrap() }));
        let attempt = attempt(resolver.clone(), address.port());
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl("localhost")
            .unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let worker = tokio::spawn(async move {
            let mut stream = attempt
                .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
                .await?;
            stream.write_all(&[1]).await.map_err(|_| "write failed")?;
            let mut response = [0; 7];
            stream
                .read_exact(&mut response)
                .await
                .map_err(|_| "read failed")?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(response)
        });
        tokio::time::timeout(Duration::from_secs(3), resolver.notice.notified())
            .await
            .unwrap();
        if change_policy {
            resolver.epoch.store(2, Ordering::Release);
        } else {
            key.available.store(false, Ordering::Release);
        }
        decision
            .send(ClientAuthenticationDecision::Certificate(identity))
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
        if change_policy {
            assert!(result.is_err());
        } else {
            assert_eq!(&result.unwrap(), b"public!");
        }
        assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
        assert!(server.join().unwrap().is_none());
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // An explicit policy matrix shares one independent live-server assertion path.
async fn host_policy_uses_first_enabled_match_and_never_falls_back_to_another_identity() {
    use transmog_core::client_authentication::{
        ClientCertificateAction as Action, ClientCertificatePolicy, ClientCertificateRule,
        ClientCertificateSelector,
    };
    use transmog_tls::client_authentication::PolicyClientAuthenticationResolver;
    for scenario in [
        "unconfigured",
        "automatic",
        "first denies",
        "first missing",
        "disabled",
        "ambiguous",
        "unique renewal",
        "wrong port",
    ] {
        let ca = ProxyCa::generate("Policy fixture root", 2).unwrap();
        let (identity, key) = credential(&ca);
        let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
        let selector = ClientCertificateSelector {
            identity: Some(identity.id.clone()),
            ..ClientCertificateSelector::default()
        };
        let rule = |id: &str, mode: Action| ClientCertificateRule {
            id: id.into(),
            enabled: true,
            host: "localhost".into(),
            port: address.port(),
            mode,
        };
        let automatic = || Action::Automatic {
            selector: selector.clone(),
        };
        let mut policy = ClientCertificatePolicy::default();
        let mut identities = vec![identity.clone()];
        match scenario {
            "unconfigured" => (),
            "automatic" => policy.rules.push(rule("auto", automatic())),
            "first denies" => {
                policy.rules.push(rule("deny", Action::NoCertificate));
                policy.rules.push(rule("auto", automatic()));
            }
            "first missing" => {
                policy.rules.push(rule(
                    "missing",
                    Action::Automatic {
                        selector: ClientCertificateSelector {
                            identity: Some("missing identity".into()),
                            ..ClientCertificateSelector::default()
                        },
                    },
                ));
                policy.rules.push(rule("auto", automatic()));
            }
            "disabled" => {
                let mut disabled = rule("deny", Action::NoCertificate);
                disabled.enabled = false;
                policy.rules.push(disabled);
                policy.rules.push(rule("auto", automatic()));
            }
            "ambiguous" | "unique renewal" => {
                let renewal = ClientCertificateSelector {
                    subject: std::collections::BTreeMap::from([(
                        "CN".into(),
                        "Test client identity".into(),
                    )]),
                    ..ClientCertificateSelector::default()
                };
                policy
                    .rules
                    .push(rule("renewal", Action::Automatic { selector: renewal }));
                if scenario == "ambiguous" {
                    let mut second = (*identity).clone();
                    second.id = "second-provider-identity".into();
                    identities.push(Arc::new(second));
                }
            }
            "wrong port" => {
                let mut wrong = rule("auto", automatic());
                wrong.port = if address.port() == 65535 {
                    443
                } else {
                    address.port() + 1
                };
                policy.rules.push(wrong);
            }
            _ => unreachable!(),
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                policy,
                Arc::new(TestInventory {
                    identities,
                    reads: reads.clone(),
                }),
                None,
            )
            .unwrap(),
        );
        let attempt = ClientAuthenticationAttempt::new(
            resolver,
            ClientAuthenticationContext {
                host: "localhost".into(),
                port: address.port(),
                caller: "process:policy-test".into(),
                generation: 1,
            },
            transmog_tls::TlsCapture::new(transmog_core::TlsEvidence::default()),
        );
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl("localhost")
            .unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let mut stream = tokio::time::timeout(
            Duration::from_secs(3),
            attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
        )
        .await
        .unwrap()
        .unwrap();
        stream.write_all(&[1]).await.unwrap();
        let mut response = [0; 7];
        stream.read_exact(&mut response).await.unwrap();
        let allowed = matches!(scenario, "automatic" | "disabled" | "unique renewal");
        assert_eq!(
            &response,
            if allowed { b"private" } else { b"public!" },
            "{scenario}"
        );
        assert_eq!(
            key.signatures.load(Ordering::SeqCst),
            usize::from(allowed),
            "{scenario}"
        );
        assert_eq!(
            reads.load(Ordering::SeqCst),
            usize::from(!matches!(
                scenario,
                "unconfigured" | "first denies" | "wrong port"
            )),
            "{scenario}"
        );
        assert_eq!(server.join().unwrap().is_some(), allowed, "{scenario}");
    }
}

#[tokio::test]
async fn aborting_handshake_owner_unregisters_pending_decision() {
    let ca = ProxyCa::generate("Dropped owner fixture", 2).unwrap();
    let (identity, key) = credential(&ca);
    let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
    let (decision, receiver) = tokio::sync::oneshot::channel();
    let resolver = resolver(Box::pin(async move { receiver.await.unwrap() }));
    let attempt = attempt(resolver.clone(), address.port());
    let owner = attempt.clone();
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut ssl = connector
        .configure()
        .unwrap()
        .into_ssl("localhost")
        .unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let worker = tokio::spawn(async move {
        owner
            .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), resolver.notice.notified())
        .await
        .unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(
        decision
            .send(ClientAuthenticationDecision::Certificate(identity))
            .is_err()
    );
    assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
    assert!(server.join().unwrap().is_none());
}

#[tokio::test]
async fn decision_deadline_expires_and_rejects_late_selection() {
    let ca = ProxyCa::generate("Decision deadline fixture", 2).unwrap();
    let (identity, key) = credential(&ca);
    let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
    let (decision, receiver) = tokio::sync::oneshot::channel();
    let resolver = resolver(Box::pin(async move { receiver.await.unwrap() }));
    let attempt = attempt(resolver.clone(), address.port());
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut ssl = connector
        .configure()
        .unwrap()
        .into_ssl("localhost")
        .unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let worker = tokio::spawn(async move {
        attempt
            .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), resolver.notice.notified())
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    assert!(worker.await.unwrap().is_err());
    assert!(
        decision
            .send(ClientAuthenticationDecision::Certificate(identity))
            .is_err()
    );
    assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
    assert!(server.join().unwrap().is_none());
}

struct EmptyInventoryPrompt {
    calls: AtomicUsize,
}
impl transmog_tls::client_authentication::ClientAuthenticationPrompt for EmptyInventoryPrompt {
    fn prompt(
        &self,
        _: ClientAuthenticationContext,
        _: CertificateRequest,
        candidates: Vec<Arc<ClientCredential>>,
        _: Option<String>,
    ) -> ClientAuthenticationFuture {
        assert!(candidates.is_empty());
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            ClientAuthenticationDecision::NoCertificate(
                "explicit continue without certificate".into(),
            )
        })
    }
}

#[tokio::test]
async fn global_and_host_prompt_are_invoked_even_with_no_eligible_identities() {
    use transmog_core::client_authentication::{
        ClientCertificateAction, ClientCertificateDefault, ClientCertificatePolicy,
        ClientCertificateRule,
    };
    use transmog_tls::client_authentication::PolicyClientAuthenticationResolver;
    for global in [false, true] {
        let ca = ProxyCa::generate("Empty prompt fixture", 2).unwrap();
        let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
        let mut policy = ClientCertificatePolicy::default();
        if global {
            policy.default = ClientCertificateDefault::Prompt;
        } else {
            policy.rules.push(ClientCertificateRule {
                id: "prompt".into(),
                enabled: true,
                host: "localhost".into(),
                port: address.port(),
                mode: ClientCertificateAction::Prompt,
            });
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let prompt = Arc::new(EmptyInventoryPrompt {
            calls: AtomicUsize::new(0),
        });
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                policy,
                Arc::new(TestInventory {
                    identities: vec![],
                    reads: reads.clone(),
                }),
                Some(prompt.clone()),
            )
            .unwrap(),
        );
        let attempt = ClientAuthenticationAttempt::new(
            resolver,
            ClientAuthenticationContext {
                host: "localhost".into(),
                port: address.port(),
                caller: "process:empty-prompt".into(),
                generation: 1,
            },
            transmog_tls::TlsCapture::new(transmog_core::TlsEvidence::default()),
        );
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl("localhost")
            .unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let mut stream = tokio::time::timeout(
            Duration::from_secs(3),
            attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
        )
        .await
        .unwrap()
        .unwrap();
        stream.write_all(&[1]).await.unwrap();
        let mut response = [0; 7];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"public!");
        assert_eq!(prompt.calls.load(Ordering::SeqCst), 1);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert!(server.join().unwrap().is_none());
    }
}

struct PendingPrompt {
    entered: Notify,
    decision: Mutex<Option<ClientAuthenticationFuture>>,
}
impl transmog_tls::client_authentication::ClientAuthenticationPrompt for PendingPrompt {
    fn prompt(
        &self,
        _: ClientAuthenticationContext,
        _: CertificateRequest,
        candidates: Vec<Arc<ClientCredential>>,
        _: Option<String>,
    ) -> ClientAuthenticationFuture {
        assert_eq!(candidates.len(), 1);
        self.entered.notify_one();
        self.decision.lock().unwrap().take().unwrap()
    }
}

#[tokio::test]
async fn changing_policy_or_inventory_wakes_and_cancels_prompt_without_waiting_for_user() {
    use transmog_core::client_authentication::{ClientCertificateDefault, ClientCertificatePolicy};
    use transmog_tls::client_authentication::PolicyClientAuthenticationResolver;
    for change_inventory in [false, true] {
        let ca = ProxyCa::generate("Revoked prompt fixture", 2).unwrap();
        let (identity, key) = credential(&ca);
        let (address, server) = server(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER);
        let (decision, receiver) = tokio::sync::oneshot::channel();
        let prompt = Arc::new(PendingPrompt {
            entered: Notify::new(),
            decision: Mutex::new(Some(Box::pin(async move { receiver.await.unwrap() }))),
        });
        let policy = ClientCertificatePolicy {
            default: ClientCertificateDefault::Prompt,
            rules: vec![],
        };
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                policy,
                Arc::new(TestInventory {
                    identities: vec![identity.clone()],
                    reads: Arc::new(AtomicUsize::new(0)),
                }),
                Some(prompt.clone()),
            )
            .unwrap(),
        );
        let attempt = ClientAuthenticationAttempt::new(
            resolver.clone(),
            ClientAuthenticationContext {
                host: "localhost".into(),
                port: address.port(),
                caller: "process:revoked-prompt".into(),
                generation: 1,
            },
            transmog_tls::TlsCapture::new(transmog_core::TlsEvidence::default()),
        );
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl("localhost")
            .unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let worker = tokio::spawn(async move {
            attempt
                .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), prompt.entered.notified())
            .await
            .unwrap();
        if change_inventory {
            resolver.inventory_changed().unwrap();
        } else {
            resolver
                .replace_policy(ClientCertificatePolicy::default())
                .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(3), worker)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(
            decision
                .send(ClientAuthenticationDecision::Certificate(identity))
                .is_err()
        );
        assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
        assert!(server.join().unwrap().is_none());
    }
}

#[tokio::test]
async fn oversized_request_constraints_decline_identity_and_still_attempt_connection() {
    let ca = ProxyCa::generate("Bounded request fixture", 2).unwrap();
    let (identity, key) = credential(&ca);
    let (address, server) =
        server_with_issuer_count(&ca, SslVersion::TLS1_3, SslVerifyMode::PEER, 257);
    let resolver = resolver(Box::pin(async move {
        ClientAuthenticationDecision::Certificate(identity)
    }));
    let attempt = attempt(resolver, address.port());
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut ssl = connector
        .configure()
        .unwrap()
        .into_ssl("localhost")
        .unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let mut stream = tokio::time::timeout(
        Duration::from_secs(3),
        attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
    )
    .await
    .unwrap()
    .unwrap();
    stream.write_all(&[1]).await.unwrap();
    let mut response = [0; 7];
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"public!");
    assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
    assert!(server.join().unwrap().is_none());
    let evidence = attempt.evidence();
    assert!(evidence.client_chain.is_empty());
    assert!(evidence.details["Client certificate request"].contains("exceed supported limits"));
}

#[tokio::test]
async fn required_client_authentication_is_only_confirmed_by_explicit_server_alert() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        let ca = ProxyCa::generate("Required certificate fixture", 2).unwrap();
        let (address, server) = server(
            &ca,
            version,
            SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT,
        );
        let resolver = resolver(Box::pin(async {
            ClientAuthenticationDecision::NoCertificate("no host authorization".into())
        }));
        let attempt = attempt(resolver, address.port());
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut config = connector.configure().unwrap();
        config.set_min_proto_version(Some(version)).unwrap();
        config.set_max_proto_version(Some(version)).unwrap();
        let mut ssl = config.into_ssl("localhost").unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
        )
        .await
        .unwrap();
        if let Ok(mut stream) = result {
            // TLS 1.3's rejection can arrive after connect returns. Read its
            // alert without adding application bytes that can race peer closure.
            assert!(stream.read_exact(&mut [0]).await.is_err());
        }
        assert!(server.join().unwrap().is_none());
        let evidence = attempt.evidence();
        assert!(evidence.client_chain.is_empty());
        if version == SslVersion::TLS1_3 {
            assert_eq!(
                evidence.details.get("Peer TLS alert").map(String::as_str),
                Some("116"),
                "{evidence:?}"
            );
            assert_eq!(
                evidence.details["Client certificate requirement"],
                "Server requires a client certificate"
            );
        } else {
            assert!(
                !evidence
                    .details
                    .contains_key("Client certificate requirement")
            );
        }
    }
}

struct PanicSigningKey {
    panic_in_availability: bool,
    available_calls: AtomicUsize,
    entered: AtomicBool,
}
impl ClientSigningKey for PanicSigningKey {
    fn algorithms(&self) -> &[u16] {
        &[0x0403]
    }
    fn available(&self) -> bool {
        if self.available_calls.fetch_add(1, Ordering::SeqCst) > 0 && self.panic_in_availability {
            self.entered.store(true, Ordering::Release);
            panic!("fixture: signing availability panicked");
        }
        true
    }
    fn sign(&self, _: u16, _: &[u8]) -> Result<Vec<u8>, String> {
        self.entered.store(true, Ordering::Release);
        panic!("fixture: signing provider panicked");
    }
}

#[tokio::test]
async fn provider_panic_ends_handshake_without_external_timeout() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for panic_in_availability in [false, true] {
            let ca = ProxyCa::generate("Provider panic fixture", 2).unwrap();
            let (mut identity, _) = credential(&ca);
            let key = Arc::new(PanicSigningKey {
                panic_in_availability,
                available_calls: AtomicUsize::new(0),
                entered: AtomicBool::new(false),
            });
            Arc::make_mut(&mut identity).key = key.clone();
            let (address, peer) = server(&ca, version, SslVerifyMode::PEER);
            let resolver = resolver(Box::pin(async move {
                ClientAuthenticationDecision::Certificate(identity)
            }));
            let attempt = attempt(resolver, address.port());
            let connector = factory(&ca).hyper_connector().unwrap();
            let mut config = connector.configure().unwrap();
            config.set_min_proto_version(Some(version)).unwrap();
            config.set_max_proto_version(Some(version)).unwrap();
            let mut ssl = config.into_ssl("localhost").unwrap();
            attempt.attach_ssl(&mut ssl).unwrap();
            let tcp = TcpStream::connect(address).await.unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
            )
            .await;
            assert!(key.entered.load(Ordering::Acquire), "signing worker ran");
            assert!(peer.join().unwrap().is_none());
            assert!(result.is_ok(), "provider panic left handshake pending");
            assert!(result.unwrap().is_err());
            assert!(attempt.evidence().valid());
            assert!(!format!("{:?}", attempt.evidence()).contains("fixture:"));
        }
    }
}

struct CurveSigningKey {
    key: PKey<Private>,
    algorithms: Vec<u16>,
    signatures: AtomicUsize,
}
impl ClientSigningKey for CurveSigningKey {
    fn algorithms(&self) -> &[u16] {
        &self.algorithms
    }
    fn available(&self) -> bool {
        true
    }
    fn sign(&self, algorithm: u16, input: &[u8]) -> Result<Vec<u8>, String> {
        assert!(self.algorithms.contains(&algorithm));
        self.signatures.fetch_add(1, Ordering::SeqCst);
        let digest = match algorithm {
            0x0403 => MessageDigest::sha256(),
            0x0503 => MessageDigest::sha384(),
            0x0603 => MessageDigest::sha512(),
            _ => panic!("incompatible scheme reached signer"),
        };
        let mut signer = boring::sign::Signer::new(digest, &self.key).unwrap();
        signer.update(input).unwrap();
        Ok(signer.sign_to_vec().unwrap())
    }
}

#[tokio::test]
async fn signature_eligibility_uses_negotiated_version_and_leaf_key() {
    for (version, curve, algorithms, authenticated) in [
        (SslVersion::TLS1_2, Nid::SECP384R1, vec![0x0403], true),
        (SslVersion::TLS1_3, Nid::SECP384R1, vec![0x0403], false),
        (SslVersion::TLS1_3, Nid::SECP384R1, vec![0x0503], true),
        (SslVersion::TLS1_3, Nid::SECP521R1, vec![0x0603], true),
        (
            SslVersion::TLS1_3,
            Nid::SECP384R1,
            vec![0x0403, 0x0503, 0x0503],
            true,
        ),
        (
            SslVersion::TLS1_3,
            Nid::X9_62_PRIME256V1,
            vec![0x0503],
            false,
        ),
        (
            SslVersion::TLS1_2,
            Nid::X9_62_PRIME256V1,
            vec![0x0401],
            false,
        ),
        (
            SslVersion::TLS1_3,
            Nid::X9_62_PRIME256V1,
            vec![0x0401],
            false,
        ),
        (
            SslVersion::TLS1_3,
            Nid::X9_62_PRIME256V1,
            vec![0xfeff],
            false,
        ),
    ] {
        check_signature_eligibility(version, curve, algorithms, authenticated).await;
    }
}

async fn check_signature_eligibility(
    version: SslVersion,
    curve: Nid,
    algorithms: Vec<u16>,
    authenticated: bool,
) {
    let ca = ProxyCa::generate("Signature eligibility fixture", 2).unwrap();
    let (certificate, private_key) = client_identity_on_curve(&ca, false, curve);
    let key = Arc::new(CurveSigningKey {
        key: private_key,
        algorithms,
        signatures: AtomicUsize::new(0),
    });
    let identity = Arc::new(ClientCredential {
        id: "curve-fixture".into(),
        chain: vec![certificate, ca.certificate().clone()],
        key: key.clone(),
    });
    // Incomplete protocol information must never authorize an identity.
    assert!(!identity.compatible(&CertificateRequest::default()));
    let (address, peer) = server_with_constraints(
        &ca,
        version,
        SslVerifyMode::PEER,
        1,
        Some(&[
            SslSignatureAlgorithm::ECDSA_SECP256R1_SHA256,
            SslSignatureAlgorithm::ECDSA_SECP384R1_SHA384,
            SslSignatureAlgorithm::ECDSA_SECP521R1_SHA512,
            SslSignatureAlgorithm::RSA_PKCS1_SHA256,
        ]),
    );
    let resolver = resolver(Box::pin(async move {
        ClientAuthenticationDecision::Certificate(identity)
    }));
    let attempt = attempt(resolver, address.port());
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut config = connector.configure().unwrap();
    config.set_min_proto_version(Some(version)).unwrap();
    config.set_max_proto_version(Some(version)).unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let mut stream = tokio::time::timeout(
        Duration::from_secs(3),
        attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
    )
    .await
    .unwrap()
    .unwrap();
    stream.write_all(&[1]).await.unwrap();
    let mut response = [0; 7];
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(
        &response,
        if authenticated {
            b"private"
        } else {
            b"public!"
        },
        "version={version:?}, curve={curve:?}, algorithms={:?}, evidence={:?}",
        key.algorithms(),
        attempt.evidence()
    );
    let observed = peer.join().unwrap();
    assert_eq!(observed.is_some(), authenticated);
    assert_eq!(
        key.signatures.load(Ordering::SeqCst),
        usize::from(authenticated)
    );
    let evidence = attempt.evidence();
    assert!(evidence.valid());
    assert_eq!(!evidence.client_chain.is_empty(), authenticated);
    if let Some(observed) = observed {
        assert_eq!(
            evidence.client_chain[0].der.as_ref(),
            observed.to_der().unwrap()
        );
    }
}

#[tokio::test]
async fn unicode_decision_reason_keeps_live_capture_evidence_valid() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        let ca = ProxyCa::generate("Unicode reason fixture", 2).unwrap();
        let (address, peer) = server(&ca, version, SslVerifyMode::PEER);
        let reason = format!("{}{}", "a".repeat(2047), "🦀".repeat(1400));
        let resolver = resolver(Box::pin(async move {
            ClientAuthenticationDecision::NoCertificate(reason)
        }));
        let attempt = attempt(resolver, address.port());
        let connector = factory(&ca).hyper_connector().unwrap();
        let mut config = connector.configure().unwrap();
        config.set_min_proto_version(Some(version)).unwrap();
        config.set_max_proto_version(Some(version)).unwrap();
        let mut ssl = config.into_ssl("localhost").unwrap();
        attempt.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).await.unwrap();
        let mut stream = tokio::time::timeout(
            Duration::from_secs(3),
            attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
        )
        .await
        .unwrap()
        .unwrap();
        stream.write_all(&[1]).await.unwrap();
        let mut response = [0; 7];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"public!");
        assert!(peer.join().unwrap().is_none());
        let evidence = attempt.evidence();
        assert!(evidence.valid());
        assert_eq!(
            evidence.details["Client certificate decision"],
            "a".repeat(2047)
        );
        assert!(evidence.client_chain.is_empty());
    }
}

#[tokio::test]
async fn signing_key_usage_controls_optional_authentication() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for (key_usage, authenticated) in [
            (
                Some(
                    KeyUsage::new()
                        .critical()
                        .digital_signature()
                        .build()
                        .unwrap(),
                ),
                true,
            ),
            (
                Some(KeyUsage::new().critical().key_agreement().build().unwrap()),
                false,
            ),
            (
                Some(
                    KeyUsage::new()
                        .digital_signature()
                        .key_agreement()
                        .build()
                        .unwrap(),
                ),
                true,
            ),
            (None, true),
        ] {
            let ca = ProxyCa::generate("Signing usage fixture", 2).unwrap();
            let (certificate, private_key) =
                client_identity_with_usage(&ca, false, Nid::X9_62_PRIME256V1, key_usage);
            let key = Arc::new(TestSigningKey {
                key: private_key,
                available: AtomicBool::new(true),
                signatures: AtomicUsize::new(0),
            });
            let identity = Arc::new(ClientCredential {
                id: "key-usage-fixture".into(),
                chain: vec![certificate, ca.certificate().clone()],
                key: key.clone(),
            });
            let (address, peer) = server(&ca, version, SslVerifyMode::PEER);
            let resolver = resolver(Box::pin(async move {
                ClientAuthenticationDecision::Certificate(identity)
            }));
            let attempt = attempt(resolver, address.port());
            let connector = factory(&ca).hyper_connector().unwrap();
            let mut config = connector.configure().unwrap();
            config.set_min_proto_version(Some(version)).unwrap();
            config.set_max_proto_version(Some(version)).unwrap();
            let mut ssl = config.into_ssl("localhost").unwrap();
            attempt.attach_ssl(&mut ssl).unwrap();
            let tcp = TcpStream::connect(address).await.unwrap();
            let mut stream = tokio::time::timeout(
                Duration::from_secs(3),
                attempt.drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect()),
            )
            .await
            .unwrap()
            .unwrap();
            stream.write_all(&[1]).await.unwrap();
            let mut response = [0; 7];
            stream.read_exact(&mut response).await.unwrap();
            assert_eq!(
                &response,
                if authenticated {
                    b"private"
                } else {
                    b"public!"
                }
            );
            assert_eq!(peer.join().unwrap().is_some(), authenticated);
            assert_eq!(
                key.signatures.load(Ordering::SeqCst),
                usize::from(authenticated)
            );
            let evidence = attempt.evidence();
            assert!(evidence.valid());
            assert_eq!(!evidence.client_chain.is_empty(), authenticated);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancellationPhase {
    Resolve,
    Poll,
    PollPending,
    CompleteDrop,
    PendingDrop,
}
struct ReentrantResolver {
    phase: CancellationPhase,
    target: Mutex<Option<ClientAuthenticationAttempt>>,
    identity: Arc<ClientCredential>,
    notice: Arc<Notify>,
    drops: Arc<AtomicUsize>,
}
impl ClientAuthenticationResolver for ReentrantResolver {
    fn generation(&self) -> u64 {
        1
    }
    fn resolve(
        &self,
        _: ClientAuthenticationContext,
        _: CertificateRequest,
    ) -> ClientAuthenticationFuture {
        let attempt = self.target.lock().unwrap().as_ref().unwrap().clone();
        if self.phase == CancellationPhase::Resolve {
            attempt.cancel("cancelled while creating decision");
        }
        Box::pin(ReentrantDecision {
            phase: self.phase,
            attempt,
            identity: self.identity.clone(),
            notice: self.notice.clone(),
            drops: self.drops.clone(),
        })
    }
}
struct ReentrantDecision {
    phase: CancellationPhase,
    attempt: ClientAuthenticationAttempt,
    identity: Arc<ClientCredential>,
    notice: Arc<Notify>,
    drops: Arc<AtomicUsize>,
}
impl std::future::Future for ReentrantDecision {
    type Output = ClientAuthenticationDecision;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        if matches!(
            self.phase,
            CancellationPhase::Poll | CancellationPhase::PollPending
        ) {
            self.attempt.cancel("cancelled while polling decision");
        }
        self.notice.notify_one();
        if matches!(
            self.phase,
            CancellationPhase::PendingDrop | CancellationPhase::PollPending
        ) {
            std::task::Poll::Pending
        } else {
            // Cancellation must win even over a ready certificate answer.
            std::task::Poll::Ready(ClientAuthenticationDecision::Certificate(
                self.identity.clone(),
            ))
        }
    }
}
impl Drop for ReentrantDecision {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        if matches!(
            self.phase,
            CancellationPhase::CompleteDrop | CancellationPhase::PendingDrop
        ) {
            self.attempt
                .cancel("cancelled while removing pending decision");
        }
    }
}

async fn check_reentrant_cancellation(version: SslVersion, phase: CancellationPhase) {
    let ca = ProxyCa::generate("Reentrant cancellation fixture", 2).unwrap();
    let (identity, key) = credential(&ca);
    let (address, peer) = server(&ca, version, SslVerifyMode::PEER);
    let drops = Arc::new(AtomicUsize::new(0));
    let resolver = Arc::new(ReentrantResolver {
        phase,
        target: Mutex::new(None),
        identity,
        notice: Arc::new(Notify::new()),
        drops: drops.clone(),
    });
    let attempt = ClientAuthenticationAttempt::new(
        resolver.clone(),
        ClientAuthenticationContext {
            host: "localhost".into(),
            port: address.port(),
            caller: "process:test".into(),
            generation: 1,
        },
        transmog_tls::TlsCapture::new(transmog_core::TlsEvidence::default()),
    );
    *resolver.target.lock().unwrap() = Some(attempt.clone());
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut config = connector.configure().unwrap();
    config.set_min_proto_version(Some(version)).unwrap();
    config.set_max_proto_version(Some(version)).unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let driver = attempt.clone();
    let handshake = tokio::spawn(async move {
        driver
            .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
            .await
    });
    if phase == CancellationPhase::PendingDrop {
        resolver.notice.notified().await;
        attempt.cancel("pending decision cancelled by owner");
    }
    assert!(handshake.await.unwrap().is_err());
    assert!(peer.join().unwrap().is_none());
    assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let evidence = attempt.evidence();
    assert!(evidence.valid());
    assert!(evidence.client_chain.is_empty());
    assert!(evidence.details["Client certificate"].starts_with("Cancelled:"));
    let target = resolver.target.lock().unwrap().take();
    drop(target);
}

#[test]
fn embedding_cancellation_reenters_decision_lifecycle_without_deadlock() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for phase in [
            CancellationPhase::Resolve,
            CancellationPhase::Poll,
            CancellationPhase::PollPending,
            CancellationPhase::CompleteDrop,
            CancellationPhase::PendingDrop,
        ] {
            let (done, finished) = std::sync::mpsc::channel();
            // A dedicated thread keeps the watchdog runnable if this regresses
            // into a synchronous mutex deadlock in the handshake reactor.
            let worker = std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(check_reentrant_cancellation(version, phase));
                done.send(()).unwrap();
            });
            assert!(
                finished.recv_timeout(Duration::from_secs(3)).is_ok(),
                "{version:?} {phase:?} cancellation deadlocked"
            );
            worker.join().unwrap();
        }
    }
}

struct PanickingPendingCleanup {
    notice: Arc<Notify>,
    drops: Arc<AtomicUsize>,
}
impl std::future::Future for PanickingPendingCleanup {
    type Output = ClientAuthenticationDecision;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.notice.notify_one();
        std::task::Poll::Pending
    }
}
impl Drop for PanickingPendingCleanup {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        panic!("fixture private cleanup diagnostic");
    }
}

#[tokio::test]
async fn cleanup_panic_cannot_strand_cancellation_or_panic_owner_abort() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for abort_owner in [false, true] {
            let ca = ProxyCa::generate("Cleanup failure fixture", 2).unwrap();
            let (address, peer) = server(&ca, version, SslVerifyMode::PEER);
            let notice = Arc::new(Notify::new());
            let drops = Arc::new(AtomicUsize::new(0));
            let resolver = resolver(Box::pin(PanickingPendingCleanup {
                notice: notice.clone(),
                drops: drops.clone(),
            }));
            let attempt = attempt(resolver, address.port());
            let connector = factory(&ca).hyper_connector().unwrap();
            let mut config = connector.configure().unwrap();
            config.set_min_proto_version(Some(version)).unwrap();
            config.set_max_proto_version(Some(version)).unwrap();
            let mut ssl = config.into_ssl("localhost").unwrap();
            attempt.attach_ssl(&mut ssl).unwrap();
            let tcp = TcpStream::connect(address).await.unwrap();
            let driver = attempt.clone();
            let handshake = tokio::spawn(async move {
                driver
                    .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
                    .await
            });
            tokio::time::timeout(Duration::from_secs(3), notice.notified())
                .await
                .unwrap();
            if abort_owner {
                handshake.abort();
            } else {
                assert!(
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        attempt.cancel("owner cancelled");
                    }))
                    .is_ok(),
                    "cleanup panic escaped cancellation"
                );
                attempt.cancel("repeated cancellation");
            }
            let result = tokio::time::timeout(Duration::from_secs(3), handshake)
                .await
                .unwrap();
            if abort_owner {
                assert!(result.unwrap_err().is_cancelled());
            } else {
                assert!(result.unwrap().is_err());
            }
            assert!(peer.join().unwrap().is_none());
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            let evidence = attempt.evidence();
            assert!(evidence.valid());
            assert!(evidence.client_chain.is_empty());
            assert!(evidence.details["Client certificate"].starts_with("Cancelled:"));
            assert_eq!(
                evidence.details["Client certificate cleanup"],
                "Pending request cleanup failed"
            );
            assert!(!format!("{evidence:?}").contains("fixture private cleanup diagnostic"));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SigningGatePhase {
    Availability,
    Signing,
}
#[derive(Default)]
struct SigningGate {
    released: Mutex<bool>,
    signal: std::sync::Condvar,
}
impl SigningGate {
    fn wait(&self) {
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.signal.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.signal.notify_all();
    }
}
struct ReleaseSigningGate(Arc<SigningGate>);
impl Drop for ReleaseSigningGate {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct GatedSigningKey {
    key: PKey<Private>,
    phase: SigningGatePhase,
    reactor_thread: std::thread::ThreadId,
    notice: Arc<Notify>,
    gate: Arc<SigningGate>,
    signatures: AtomicUsize,
}
impl ClientSigningKey for GatedSigningKey {
    fn algorithms(&self) -> &[u16] {
        &[0x0403]
    }
    fn available(&self) -> bool {
        if self.phase == SigningGatePhase::Availability
            && std::thread::current().id() != self.reactor_thread
        {
            self.notice.notify_one();
            self.gate.wait();
        }
        true
    }
    fn sign(&self, algorithm: u16, input: &[u8]) -> Result<Vec<u8>, String> {
        assert_ne!(std::thread::current().id(), self.reactor_thread);
        assert_eq!(algorithm, 0x0403);
        self.signatures.fetch_add(1, Ordering::SeqCst);
        if self.phase == SigningGatePhase::Signing {
            self.notice.notify_one();
            self.gate.wait();
        }
        let mut signer = boring::sign::Signer::new(MessageDigest::sha256(), &self.key).unwrap();
        signer.update(input).unwrap();
        Ok(signer.sign_to_vec().unwrap())
    }
}

async fn check_signing_cancellation(version: SslVersion, phase: SigningGatePhase) {
    let ca = ProxyCa::generate("Signing admission fixture", 2).unwrap();
    let (certificate, private_key) = client_identity(&ca);
    let notice = Arc::new(Notify::new());
    let gate = Arc::new(SigningGate::default());
    // Always release a started blocking worker, even if a test assertion fails.
    let _release = ReleaseSigningGate(gate.clone());
    let key = Arc::new(GatedSigningKey {
        key: private_key,
        phase,
        reactor_thread: std::thread::current().id(),
        notice: notice.clone(),
        gate: gate.clone(),
        signatures: AtomicUsize::new(0),
    });
    let identity = Arc::new(ClientCredential {
        id: "signing-admission-fixture".into(),
        chain: vec![certificate, ca.certificate().clone()],
        key: key.clone(),
    });
    let (address, peer) = server(&ca, version, SslVerifyMode::PEER);
    let resolver = resolver(Box::pin(async move {
        ClientAuthenticationDecision::Certificate(identity)
    }));
    let attempt = attempt(resolver, address.port());
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut config = connector.configure().unwrap();
    config.set_min_proto_version(Some(version)).unwrap();
    config.set_max_proto_version(Some(version)).unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let driver = attempt.clone();
    let handshake = tokio::spawn(async move {
        driver
            .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), notice.notified())
        .await
        .unwrap();
    let expected = usize::from(phase == SigningGatePhase::Signing);
    assert_eq!(key.signatures.load(Ordering::SeqCst), expected);
    attempt.cancel("caller disconnected");
    gate.release();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), handshake)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    // The error has released SSL's provider. Once only this test owns the key,
    // the blocking worker has released it too and cannot start a late signature.
    tokio::time::timeout(Duration::from_secs(3), async {
        while Arc::strong_count(&key) != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(key.signatures.load(Ordering::SeqCst), expected);
    assert!(
        peer.join().unwrap().is_none(),
        "cancelled signature authenticated the peer"
    );
    assert!(attempt.evidence().valid());
}

#[tokio::test]
async fn cancelled_lookup_cannot_start_signing_but_started_signing_can_finish() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for phase in [SigningGatePhase::Availability, SigningGatePhase::Signing] {
            check_signing_cancellation(version, phase).await;
        }
    }
}

struct QueuedEligibilityKey {
    availability: AtomicUsize,
    signatures: AtomicUsize,
}
impl ClientSigningKey for QueuedEligibilityKey {
    fn algorithms(&self) -> &[u16] {
        &[0x0403]
    }
    fn available(&self) -> bool {
        self.availability.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn sign(&self, _: u16, _: &[u8]) -> Result<Vec<u8>, String> {
        self.signatures.fetch_add(1, Ordering::SeqCst);
        Err("unexpected signature".into())
    }
}

#[derive(Clone, Copy, Debug)]
enum QueuedLookupStop {
    Cancel,
    Abort,
    Deadline,
    Authorization,
}

#[test]
fn cancelled_owner_does_not_start_queued_eligibility_lookup() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
            for policy_selection in [false, true] {
                for stop in [
                    QueuedLookupStop::Cancel,
                    QueuedLookupStop::Abort,
                    QueuedLookupStop::Deadline,
                    QueuedLookupStop::Authorization,
                ] {
                    if matches!(stop, QueuedLookupStop::Authorization) && !policy_selection {
                        continue;
                    }
                    check_queued_eligibility(version, policy_selection, stop).await;
                }
            }
        }
    });
}

async fn check_queued_eligibility(
    version: SslVersion,
    policy_selection: bool,
    stop: QueuedLookupStop,
) {
    // Hold the sole blocking worker so neither filtering nor final eligibility
    // can start. Always release it, including when a test assertion fails.
    let gate = Arc::new(SigningGate::default());
    let _release = ReleaseSigningGate(gate.clone());
    let started = Arc::new(Notify::new());
    let blocker_gate = gate.clone();
    let blocker_started = started.clone();
    let blocker = tokio::task::spawn_blocking(move || {
        blocker_started.notify_one();
        blocker_gate.wait();
    });
    tokio::time::timeout(Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    let ca = ProxyCa::generate("Queued eligibility fixture", 2).unwrap();
    let (mut identity, _) = credential(&ca);
    let key = Arc::new(QueuedEligibilityKey {
        availability: AtomicUsize::new(0),
        signatures: AtomicUsize::new(0),
    });
    Arc::make_mut(&mut identity).key = key.clone();
    let (address, peer) = server(&ca, version, SslVerifyMode::PEER);
    let policy = policy_selection.then(|| queued_lookup_policy(identity.clone()));
    let resolver: Arc<dyn ClientAuthenticationResolver> = if let Some(policy) = &policy {
        drop(identity);
        policy.clone()
    } else {
        resolver(Box::pin(async move {
            ClientAuthenticationDecision::Certificate(identity)
        }))
    };
    let attempt = ClientAuthenticationAttempt::new(
        resolver,
        ClientAuthenticationContext {
            host: "localhost".into(),
            port: address.port(),
            caller: "process:test".into(),
            generation: 1,
        },
        transmog_tls::TlsCapture::new(transmog_core::TlsEvidence::default()),
    );
    let connector = factory(&ca).hyper_connector().unwrap();
    let mut config = connector.configure().unwrap();
    config.set_min_proto_version(Some(version)).unwrap();
    config.set_max_proto_version(Some(version)).unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    attempt.attach_ssl(&mut ssl).unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let driver = attempt.clone();
    let handshake = tokio::spawn(async move {
        driver
            .drive(tokio_boring::SslStreamBuilder::new(ssl, tcp).connect())
            .await
    });
    // Inventory/embedding futures are immediately ready. Waiting means the
    // selected blocking stage is enqueued and its JoinHandle is being polled.
    wait_for_queued_lookup(&attempt).await;
    assert_eq!(key.availability.load(Ordering::SeqCst), 0);
    match stop {
        QueuedLookupStop::Cancel => attempt.cancel("owner stopped"),
        QueuedLookupStop::Abort => handshake.abort(),
        QueuedLookupStop::Authorization => {
            policy.as_ref().unwrap().inventory_changed().unwrap();
        }
        QueuedLookupStop::Deadline => {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(61)).await;
            tokio::time::resume();
        }
    }
    let result = tokio::time::timeout(Duration::from_secs(3), handshake)
        .await
        .unwrap();
    if matches!(stop, QueuedLookupStop::Abort) {
        assert!(result.unwrap_err().is_cancelled());
    } else {
        assert!(result.unwrap().is_err());
    }
    assert!(attempt.evidence().valid());
    drop(attempt);
    drop(policy);
    gate.release();
    blocker.await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while Arc::strong_count(&key) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(peer.join().unwrap().is_none());
    assert_eq!(key.signatures.load(Ordering::SeqCst), 0);
    assert_eq!(
        key.availability.load(Ordering::SeqCst),
        0,
        "version={version:?}, policy={policy_selection}, stop={stop:?}"
    );
}

fn queued_lookup_policy(
    identity: Arc<ClientCredential>,
) -> Arc<transmog_tls::client_authentication::PolicyClientAuthenticationResolver> {
    Arc::new(
        transmog_tls::client_authentication::PolicyClientAuthenticationResolver::new(
            transmog_core::client_authentication::ClientCertificatePolicy {
                default: transmog_core::client_authentication::ClientCertificateDefault::Prompt,
                ..Default::default()
            },
            Arc::new(TestInventory {
                identities: vec![identity],
                reads: Arc::new(AtomicUsize::new(0)),
            }),
            None,
        )
        .unwrap(),
    )
}
async fn wait_for_queued_lookup(attempt: &ClientAuthenticationAttempt) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while attempt
            .evidence()
            .details
            .get("Client certificate")
            .map(String::as_str)
            != Some("Waiting for decision")
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

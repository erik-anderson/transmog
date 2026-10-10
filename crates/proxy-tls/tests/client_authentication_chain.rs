//! Live authentication distinguishes self-signed roots from necessary chain entries.
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::{PKey, Private},
    ssl::{SslAcceptor, SslMethod, SslVerifyMode, SslVersion},
    x509::{
        X509, X509NameBuilder,
        extension::{
            AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage,
            SubjectKeyIdentifier,
        },
        store::X509StoreBuilder,
    },
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use transmog_tls::client_authentication::{
    CertificateRequest, ClientAuthenticationAttempt, ClientAuthenticationContext,
    ClientAuthenticationDecision, ClientAuthenticationFuture, ClientAuthenticationResolver,
    ClientCredential, ClientSigningKey,
};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TlsCapture, TrustSnapshot,
    UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

fn key() -> PKey<Private> {
    PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap()
}
fn certificate(
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
struct SigningKey(PKey<Private>);
impl ClientSigningKey for SigningKey {
    fn algorithms(&self) -> &[u16] {
        &[0x0403]
    }
    fn available(&self) -> bool {
        true
    }
    fn sign(&self, _: u16, input: &[u8]) -> Result<Vec<u8>, String> {
        let mut signer = boring::sign::Signer::new(MessageDigest::sha256(), &self.0)
            .map_err(|_| "test signing")?;
        signer.update(input).map_err(|_| "test signing")?;
        signer.sign_to_vec().map_err(|_| "test signing".into())
    }
}
struct Resolver(Arc<ClientCredential>);
impl ClientAuthenticationResolver for Resolver {
    fn generation(&self) -> u64 {
        1
    }
    fn resolve(
        &self,
        _: ClientAuthenticationContext,
        _: CertificateRequest,
    ) -> ClientAuthenticationFuture {
        let credential = self.0.clone();
        Box::pin(async move { ClientAuthenticationDecision::Certificate(credential) })
    }
}

async fn authenticate(
    version: SslVersion,
    chain: Vec<X509>,
    private_key: PKey<Private>,
    anchor: X509,
    expected: Vec<Vec<u8>>,
) {
    let original = chain
        .iter()
        .map(|cert| cert.to_der().unwrap())
        .collect::<Vec<_>>();
    let credential = Arc::new(ClientCredential {
        id: "chain-regression".into(),
        chain,
        key: Arc::new(SigningKey(private_key)),
    });
    let ca = ProxyCa::generate("Client chain server", 2).unwrap();
    let identity = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    builder.set_min_proto_version(Some(version)).unwrap();
    builder.set_max_proto_version(Some(version)).unwrap();
    builder.set_certificate(&identity.certificate).unwrap();
    builder.set_private_key(&identity.private_key).unwrap();
    let mut trust = X509StoreBuilder::new().unwrap();
    trust.add_cert(anchor).unwrap();
    // Explicitly pin the supplied anchor, including a self-signed client leaf.
    trust.set_flags(boring::x509::verify::X509VerifyFlags::PARTIAL_CHAIN);
    builder.set_cert_store(trust.build());
    builder.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
    let acceptor = builder.build();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut stream = acceptor.accept(tcp).unwrap();
        let mut wire = vec![stream.ssl().peer_certificate().unwrap().to_der().unwrap()];
        if let Some(chain) = stream.ssl().peer_cert_chain() {
            wire.extend(chain.iter().map(|cert| cert.to_der().unwrap()));
        }
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        stream.write_all(b"authenticated").unwrap();
        wire
    });
    let attempt = ClientAuthenticationAttempt::new(
        Arc::new(Resolver(credential.clone())),
        ClientAuthenticationContext {
            host: "localhost".into(),
            port: address.port(),
            caller: "test".into(),
            generation: 1,
        },
        TlsCapture::new(transmog_core::TlsEvidence::default()),
    );
    let trust =
        PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "chain test server").unwrap();
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    );
    let connector = factory.hyper_connector().unwrap();
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
    let mut response = [0; 13];
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"authenticated");
    assert_eq!(server.join().unwrap(), expected, "actual peer chain");
    let evidence = attempt.evidence();
    assert!(evidence.valid());
    assert_eq!(
        evidence
            .client_chain
            .iter()
            .map(|cert| cert.der.to_vec())
            .collect::<Vec<_>>(),
        expected,
        "capture reflects the wire"
    );
    assert_eq!(
        credential
            .chain
            .iter()
            .map(|cert| cert.to_der().unwrap())
            .collect::<Vec<_>>(),
        original,
        "local path is unchanged"
    );
}

#[tokio::test]
async fn outgoing_chain_omits_only_terminal_self_signed_ca_and_keeps_local_path() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for digest in [MessageDigest::sha256(), MessageDigest::sha1()] {
            let root_key = key();
            let root = certificate("Root", &root_key, None, true, digest);
            let leaf_key = key();
            let leaf = certificate(
                "Client",
                &leaf_key,
                Some((&root, &root_key)),
                false,
                MessageDigest::sha256(),
            );
            let expected = vec![leaf.to_der().unwrap()];
            authenticate(version, vec![leaf, root.clone()], leaf_key, root, expected).await;
        }
        let root_key = key();
        let root = certificate("Root", &root_key, None, true, MessageDigest::sha256());
        for name in ["Intermediate", "Root"] {
            let issuer_key = key();
            let issuer = certificate(
                name,
                &issuer_key,
                Some((&root, &root_key)),
                true,
                MessageDigest::sha256(),
            );
            if name == "Root" {
                assert!(issuer.issued(&issuer).is_err());
                assert!(!issuer.verify(&issuer.public_key().unwrap()).unwrap());
            }
            let leaf_key = key();
            let leaf = certificate(
                "Client",
                &leaf_key,
                Some((&issuer, &issuer_key)),
                false,
                MessageDigest::sha256(),
            );
            let expected = vec![leaf.to_der().unwrap(), issuer.to_der().unwrap()];
            let mut chain = vec![leaf, issuer];
            if name == "Intermediate" {
                chain.push(root.clone());
            }
            authenticate(version, chain, leaf_key, root.clone(), expected).await;
        }
        // A matching subject/issuer name and CA metadata alone do not establish
        // self-signature. Keep a terminal certificate whose self-signature fails;
        // a trust anchor's self-signature is not needed to verify its child.
        let mut der = root.to_der().unwrap();
        *der.last_mut().unwrap() ^= 1;
        let invalid_signature = X509::from_der(&der).unwrap();
        assert!(invalid_signature.issued(&invalid_signature).is_ok());
        assert!(
            !invalid_signature
                .verify(&invalid_signature.public_key().unwrap())
                .unwrap_or(false)
        );
        let leaf_key = key();
        let leaf = certificate(
            "Client",
            &leaf_key,
            Some((&invalid_signature, &root_key)),
            false,
            MessageDigest::sha256(),
        );
        let expected = vec![leaf.to_der().unwrap(), invalid_signature.to_der().unwrap()];
        authenticate(
            version,
            vec![leaf, invalid_signature.clone()],
            leaf_key,
            invalid_signature,
            expected,
        )
        .await;
        for ca in [false, true] {
            let leaf_key = key();
            let leaf = certificate(
                "Self-signed identity",
                &leaf_key,
                None,
                ca,
                MessageDigest::sha256(),
            );
            let expected = vec![leaf.to_der().unwrap()];
            authenticate(version, vec![leaf.clone()], leaf_key, leaf, expected).await;
        }
    }
}

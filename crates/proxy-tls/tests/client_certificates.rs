//! Public client authentication evidence across TLS versions and context types.
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::{PKey, Private},
    ssl::{Ssl, SslAcceptor, SslMethod, SslStream, SslVerifyMode, SslVersion},
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage},
        store::X509StoreBuilder,
    },
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};
use transmog_core::TlsEvidence;
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
fn signed_certificate(
    name: &str,
    key: &PKey<Private>,
    issuer: &X509,
    issuer_key: &PKey<Private>,
    ca: bool,
) -> X509 {
    let mut subject = X509NameBuilder::new().unwrap();
    subject.append_entry_by_text("CN", name).unwrap();
    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder
        .set_serial_number(
            &BigNum::from_u32(if ca { 1 } else { 2 })
                .unwrap()
                .to_asn1_integer()
                .unwrap(),
        )
        .unwrap();
    builder.set_subject_name(&subject.build()).unwrap();
    builder.set_issuer_name(issuer.subject_name()).unwrap();
    builder.set_pubkey(key).unwrap();
    builder
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    builder
        .set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    if ca {
        builder
            .append_extension(
                &BasicConstraints::new()
                    .critical()
                    .ca()
                    .pathlen(0)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        builder
            .append_extension(
                &KeyUsage::new()
                    .critical()
                    .key_cert_sign()
                    .crl_sign()
                    .build()
                    .unwrap(),
            )
            .unwrap();
    } else {
        builder
            .append_extension(
                &KeyUsage::new()
                    .critical()
                    .digital_signature()
                    .build()
                    .unwrap(),
            )
            .unwrap();
        builder
            .append_extension(&ExtendedKeyUsage::new().client_auth().build().unwrap())
            .unwrap();
    }
    builder.sign(issuer_key, MessageDigest::sha256()).unwrap();
    builder.build()
}
fn factory(ca: &ProxyCa) -> UpstreamTlsContextFactory {
    let trust = PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "client-chain regression")
        .unwrap();
    UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
}
fn socket(stream: &TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
}

#[test]
fn mutual_tls_retains_full_client_path_on_both_connection_legs() {
    let ca = ProxyCa::generate("Client chain regression root", 2).unwrap();
    let root_key = PKey::private_key_from_pem(&ca.private_key_pem_pkcs8().unwrap()).unwrap();
    let issuer_key = key();
    let issuer = signed_certificate(
        "Client issuer",
        &issuer_key,
        ca.certificate(),
        &root_key,
        true,
    );
    let client_key = key();
    let client = signed_certificate("Client identity", &client_key, &issuer, &issuer_key, false);
    let expected =
        [&client, &issuer, ca.certificate()].map(|certificate| certificate.to_der().unwrap());
    let upstream = factory(&ca).with_client_authentication(
        client,
        client_key,
        vec![issuer, ca.certificate().clone()],
    );
    let server_leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        acceptor.set_min_proto_version(Some(version)).unwrap();
        acceptor.set_max_proto_version(Some(version)).unwrap();
        acceptor.set_certificate(&server_leaf.certificate).unwrap();
        acceptor.set_private_key(&server_leaf.private_key).unwrap();
        acceptor
            .add_extra_chain_cert(ca.certificate().clone())
            .unwrap();
        let mut store = X509StoreBuilder::new().unwrap();
        store.add_cert(ca.certificate().clone()).unwrap();
        acceptor.set_cert_store(store.build());
        acceptor.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
        let acceptor = acceptor.build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            socket(&tcp);
            let mut stream = acceptor.accept(tcp).unwrap();
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [7]);
            let capture = TlsCapture::new(TlsEvidence::default());
            capture.completed(stream.ssl(), true);
            capture.snapshot().client_chain
        });
        let connector = upstream.synchronous_connector().unwrap();
        let mut ssl = connector
            .configure()
            .unwrap()
            .into_ssl("localhost")
            .unwrap();
        ssl.set_min_proto_version(Some(version)).unwrap();
        ssl.set_max_proto_version(Some(version)).unwrap();
        let capture = upstream.capture(Some("localhost".into()), vec![]);
        capture.attach_ssl(&mut ssl).unwrap();
        let tcp = TcpStream::connect(address).unwrap();
        socket(&tcp);
        let mut stream = SslStream::new(ssl, tcp).unwrap();
        stream.connect().unwrap();
        stream.write_all(&[7]).unwrap();
        capture.completed(stream.ssl(), false);
        let recorded = capture.snapshot();
        assert_eq!(
            recorded
                .client_chain
                .iter()
                .map(|certificate| certificate.der.as_ref())
                .collect::<Vec<_>>(),
            expected.iter().map(Vec::as_slice).collect::<Vec<_>>()
        );
        let received = server.join().unwrap();
        assert_eq!(received, recorded.client_chain);
        assert_eq!(recorded.server_chain.len(), 2);
        assert!(
            recorded
                .client_chain
                .iter()
                .all(|certificate| X509::from_der(&certificate.der).is_ok())
        );
    }
    // QUIC uses the same configured identity and evidence, without a nullable chain getter.
    let context = upstream.quiche_context_builder().unwrap().build();
    assert_eq!(
        transmog_tls_native::local_buffer_chain(&Ssl::new(&context).unwrap()).len(),
        3
    );
    let capture = upstream.capture(None, vec!["h3".into()]);
    capture.verified();
    assert_eq!(capture.snapshot().client_chain.len(), 3);
}

#[test]
fn empty_and_leaf_only_client_paths_are_safe_and_invalid_keys_are_rejected() {
    let ca = ProxyCa::generate("Leaf-only regression root", 2).unwrap();
    let factory = factory(&ca);
    let context = factory.synchronous_connector().unwrap();
    let capture = factory.capture(None, vec![]);
    capture.completed(&Ssl::new(context.context()).unwrap(), false);
    assert_eq!(
        capture.snapshot().client_chain,
        [] as [transmog_core::PublicCertificate; 0]
    );
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let wrong_key =
        factory
            .clone()
            .with_client_authentication(leaf.certificate.clone(), key(), vec![]);
    assert!(wrong_key.synchronous_connector().is_err());
    let factory =
        factory.with_client_authentication(leaf.certificate.clone(), leaf.private_key, vec![]);
    let context = factory.synchronous_connector().unwrap();
    let capture = factory.capture(None, vec![]);
    capture.completed(&Ssl::new(context.context()).unwrap(), false);
    assert_eq!(capture.snapshot().client_chain.len(), 1);
    assert_eq!(
        capture.snapshot().client_chain[0].der.as_ref(),
        leaf.certificate.to_der().unwrap()
    );
}

#[test]
fn low_level_client_issuers_preserve_leaf_order_and_deduplicate_on_completion() {
    let ca = ProxyCa::generate("Explicit client issuers root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let factory =
        factory(&ca).with_client_authentication(leaf.certificate.clone(), leaf.private_key, vec![]);
    let context = factory.synchronous_connector().unwrap();
    let ssl = Ssl::new(context.context()).unwrap();
    for before in [true, false] {
        let capture = TlsCapture::new(TlsEvidence::default());
        if !before {
            capture.completed(&ssl, false);
        }
        capture.client_issuers([ca.certificate().as_ref(), ca.certificate().as_ref()]);
        capture.completed(&ssl, false);
        capture.completed(&ssl, false);
        let recorded = capture.snapshot();
        assert_eq!(recorded.client_chain.len(), 2);
        assert_eq!(
            recorded.client_chain[0].der.as_ref(),
            leaf.certificate.to_der().unwrap()
        );
        assert_eq!(
            recorded.client_chain[1].der.as_ref(),
            ca.certificate().to_der().unwrap()
        );
    }
}

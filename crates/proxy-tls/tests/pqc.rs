//! Real TLS handshakes exercise the algorithms used by Chromium, without a browser pin.
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    hash::MessageDigest,
    pkey::{Id, PKey, Private},
    ssl::{
        SslAcceptor, SslConnector, SslContext, SslContextBuilder, SslMethod, SslSignatureAlgorithm,
        SslStream, SslVersion,
    },
    x509::{
        X509, X509NameBuilder,
        extension::{ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
    },
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};
use transmog_tls::{
    CachedMitmCertificateResolver, DownstreamCertificateResolver, DownstreamTlsContextFactory,
    DownstreamTlsPolicy, EndpointIdentity, LeafKeyAlgorithm, MlDsaVariant, PemTrustSource, ProxyCa,
    TlsCapture, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

const HYBRID_GROUP: u16 = 0x11ec;
const GROUPS: &str = "X25519MLKEM768:X25519:P-256:P-384";
const ML_DSA: [(i32, SslSignatureAlgorithm); 3] = [
    (
        boring_sys::EVP_PKEY_ML_DSA_44,
        SslSignatureAlgorithm::ML_DSA_44,
    ),
    (
        boring_sys::EVP_PKEY_ML_DSA_65,
        SslSignatureAlgorithm::ML_DSA_65,
    ),
    (
        boring_sys::EVP_PKEY_ML_DSA_87,
        SslSignatureAlgorithm::ML_DSA_87,
    ),
];

fn ml_dsa_leaf(ca: &ProxyCa, key_type: i32) -> (X509, PKey<Private>) {
    let key = PKey::generate(Id::from_raw(key_type)).unwrap();
    let root_key = PKey::private_key_from_pem(&ca.private_key_pem_pkcs8().unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "localhost").unwrap();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    cert.set_subject_name(&name.build()).unwrap();
    cert.set_issuer_name(ca.certificate().subject_name())
        .unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    cert.append_extension(&KeyUsage::new().digital_signature().build().unwrap())
        .unwrap();
    cert.append_extension(
        &ExtendedKeyUsage::new()
            .server_auth()
            .client_auth()
            .build()
            .unwrap(),
    )
    .unwrap();
    let san = SubjectAlternativeName::new()
        .dns("localhost")
        .build(&cert.x509v3_context(Some(ca.certificate()), None))
        .unwrap();
    cert.append_extension(&san).unwrap();
    cert.sign(&root_key, MessageDigest::sha256()).unwrap();
    (cert.build(), key)
}

fn acceptor(cert: &X509, key: &PKey<Private>, groups: &str) -> SslAcceptor {
    let mut context = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    context
        .set_min_proto_version(Some(SslVersion::TLS1_3))
        .unwrap();
    context.set_curves_list(groups).unwrap();
    context.set_certificate(cert).unwrap();
    context.set_private_key(key).unwrap();
    context.check_private_key().unwrap();
    context.build()
}

fn connector(ca: &ProxyCa) -> boring::ssl::SslConnectorBuilder {
    let mut context = SslConnector::builder(SslMethod::tls()).unwrap();
    context
        .cert_store_mut()
        .add_cert(ca.certificate().clone())
        .unwrap();
    context
        .set_min_proto_version(Some(SslVersion::TLS1_3))
        .unwrap();
    context.set_curves_list(GROUPS).unwrap();
    context
}

#[derive(Debug)]
struct RecordingStream {
    socket: TcpStream,
    written: Vec<u8>,
}
impl Read for RecordingStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.socket.read(buffer)
    }
}
impl Write for RecordingStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.socket.write(buffer)?;
        self.written.extend_from_slice(&buffer[..written]);
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}
fn socket(socket: &TcpStream) {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
}
fn handshake(
    acceptor: SslAcceptor,
    connector: &SslConnector,
) -> (SslStream<RecordingStream>, Option<u16>) {
    handshake_captured(acceptor, connector, None)
}

fn handshake_captured(
    acceptor: SslAcceptor,
    connector: &SslConnector,
    capture: Option<&TlsCapture>,
) -> (SslStream<RecordingStream>, Option<u16>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        socket(&tcp);
        let mut tls = acceptor.accept(tcp).unwrap();
        let mut byte = [0];
        tls.read_exact(&mut byte).unwrap();
        tls.write_all(&byte).unwrap();
        tls.ssl().curve()
    });
    let tcp = TcpStream::connect(address).unwrap();
    socket(&tcp);
    let mut ssl = connector
        .configure()
        .unwrap()
        .into_ssl("localhost")
        .unwrap();
    if let Some(capture) = capture {
        capture.attach_ssl(&mut ssl).unwrap();
    }
    let mut tls = SslStream::new(
        ssl,
        RecordingStream {
            socket: tcp,
            written: Vec::new(),
        },
    )
    .unwrap();
    tls.connect().unwrap();
    tls.write_all(&[7]).unwrap();
    let mut byte = [0];
    tls.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [7]);
    if let Some(capture) = capture {
        capture.completed(tls.ssl(), false);
    }
    (tls, server.join().unwrap())
}

fn u16_be(bytes: &[u8]) -> u16 {
    u16::from_be_bytes(bytes[..2].try_into().unwrap())
}
// Read the first ClientHello from the TLS records actually written by the client.
fn hello_extensions(written: &[u8]) -> BTreeMap<u16, Vec<u8>> {
    let mut handshake = Vec::new();
    let mut records = written;
    while records.len() >= 5 && records[0] == 22 {
        let length = usize::from(u16_be(&records[3..]));
        handshake.extend_from_slice(&records[5..5 + length]);
        records = &records[5 + length..];
    }
    assert_eq!(handshake[0], 1);
    let length = (usize::from(handshake[1]) << 16)
        | (usize::from(handshake[2]) << 8)
        | usize::from(handshake[3]);
    let hello = &handshake[4..4 + length];
    let mut cursor = 34;
    cursor += 1 + usize::from(hello[cursor]);
    cursor += 2 + usize::from(u16_be(&hello[cursor..]));
    cursor += 1 + usize::from(hello[cursor]);
    let length = usize::from(u16_be(&hello[cursor..]));
    let end = cursor + 2 + length;
    cursor += 2;
    let mut extensions = BTreeMap::new();
    while cursor < end {
        let kind = u16_be(&hello[cursor..]);
        let length = usize::from(u16_be(&hello[cursor + 2..]));
        cursor += 4;
        assert!(
            extensions
                .insert(kind, hello[cursor..cursor + length].to_vec())
                .is_none()
        );
        cursor += length;
    }
    assert_eq!(cursor, end);
    extensions
}

#[test]
fn boringssl_supports_chromium_hybrid_exchange_and_initial_shares() {
    let ca = ProxyCa::generate("PQC fixture root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let (tls, server_group) = handshake(
        acceptor(&leaf.certificate, &leaf.private_key, GROUPS),
        &connector(&ca).build(),
    );
    assert_eq!(tls.ssl().curve(), Some(HYBRID_GROUP));
    assert_eq!(server_group, Some(HYBRID_GROUP));
    let extensions = hello_extensions(&tls.get_ref().written);
    let groups: Vec<_> = extensions[&10][2..].chunks_exact(2).map(u16_be).collect();
    assert_eq!(groups, [HYBRID_GROUP, 29, 23, 24]);
    let mut shares = extensions[&51][2..].as_ref();
    let mut share_groups = Vec::new();
    while !shares.is_empty() {
        share_groups.push(u16_be(shares));
        let length = usize::from(u16_be(&shares[2..]));
        shares = &shares[4 + length..];
    }
    assert_eq!(share_groups, [HYBRID_GROUP, 29]);
}

#[test]
fn boringssl_verifies_each_chromium_ml_dsa_signature_scheme() {
    let ca = ProxyCa::generate("ML-DSA fixture root", 2).unwrap();
    for (key_type, algorithm) in ML_DSA {
        let (cert, key) = ml_dsa_leaf(&ca, key_type);
        let mut client = connector(&ca);
        client.set_verify_algorithm_prefs(&[algorithm]).unwrap();
        let (tls, _) = handshake(acceptor(&cert, &key, GROUPS), &client.build());
        assert_eq!(tls.ssl().peer_signature_algorithm(), Some(algorithm));
        assert!(tls.ssl().verify_result().is_ok());
        // Public certificates and private PKCS#8 keys round trip without a second crypto stack.
        let public = X509::from_der(&cert.to_der().unwrap())
            .unwrap()
            .public_key()
            .unwrap();
        let restored =
            PKey::private_key_from_pem(&key.private_key_to_pem_pkcs8().unwrap()).unwrap();
        assert!(public.public_eq(&restored));
    }
}

fn factory(ca: &ProxyCa) -> UpstreamTlsContextFactory {
    let source =
        PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "PQC fixture root").unwrap();
    UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&source, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
}

#[test]
fn production_upstream_prefers_hybrid_and_negotiates_classical_peers() {
    let ca = ProxyCa::generate("PQC upstream root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let connector = factory(&ca).hyper_connector().unwrap();
    for (groups, expected) in [
        (GROUPS, HYBRID_GROUP),
        ("X25519:P-256:P-384", 29),
        ("P-256", 23),
    ] {
        let capture = factory(&ca).capture(Some("localhost".into()), vec![]);
        let (tls, group) = handshake_captured(
            acceptor(&leaf.certificate, &leaf.private_key, groups),
            &connector,
            Some(&capture),
        );
        assert_eq!(tls.ssl().curve(), Some(expected));
        assert_eq!(group, Some(expected));
        let evidence = capture.snapshot();
        assert_eq!(
            evidence.details["Negotiated group ID"],
            format!("0x{expected:04x}")
        );
        assert_eq!(
            evidence.details["Key exchange"],
            if expected == HYBRID_GROUP {
                "Hybrid post-quantum key exchange"
            } else {
                "Classical key exchange"
            }
        );
        assert_eq!(
            evidence.details["Peer handshake signature"],
            "ecdsa_secp256r1_sha256"
        );
        assert!(evidence.valid());
    }
    let acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy {
        maximum_version: SslVersion::TLS1_2,
        ..DownstreamTlsPolicy::default()
    })
    .acceptor(&leaf)
    .unwrap();
    let (tls, group) = handshake(acceptor, &connector);
    assert_eq!(tls.ssl().version2(), Some(SslVersion::TLS1_2));
    assert_eq!(group, Some(29));
}

#[test]
fn production_downstream_prefers_hybrid_and_accepts_classical_clients() {
    let ca = ProxyCa::generate("PQC downstream root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let factory = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default());
    for (groups, expected) in [(GROUPS, HYBRID_GROUP), ("X25519:P-256:P-384", 29)] {
        let mut client = connector(&ca);
        client.set_curves_list(groups).unwrap();
        let capture = TlsCapture::new(transmog_core::TlsEvidence::default());
        let (tls, group) = handshake(
            factory.acceptor_captured(&leaf, Some(&capture)).unwrap(),
            &client.build(),
        );
        assert_eq!(tls.ssl().curve(), Some(expected));
        assert_eq!(group, Some(expected));
        let evidence = capture.snapshot();
        assert_eq!(
            evidence.details["Negotiated group ID"],
            format!("0x{expected:04x}")
        );
        assert_eq!(
            evidence.details["Local handshake signature"],
            "ecdsa_secp256r1_sha256"
        );
    }
    let mut client = connector(&ca);
    client
        .set_min_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    client
        .set_max_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    let (tls, group) = handshake(factory.acceptor(&leaf).unwrap(), &client.build());
    assert_eq!(tls.ssl().version2(), Some(SslVersion::TLS1_2));
    assert_eq!(group, Some(29));
}

fn quic_config(tls: SslContextBuilder) -> quiche::Config {
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
    config.set_application_protos(&[b"h3"]).unwrap();
    config.set_max_idle_timeout(5000);
    config.set_initial_max_data(65536);
    config.set_initial_max_stream_data_bidi_local(65536);
    config.set_initial_max_stream_data_bidi_remote(65536);
    config.set_initial_max_streams_bidi(8);
    config
}

fn quic_handshake(
    mut client_config: quiche::Config,
    mut server_config: quiche::Config,
) -> (quiche::Connection, quiche::Connection) {
    let client_address = "127.0.0.1:10000".parse().unwrap();
    let server_address = "127.0.0.1:10001".parse().unwrap();
    let mut client = quiche::connect(
        Some("localhost"),
        &quiche::ConnectionId::from_ref(&[1; 16]),
        client_address,
        server_address,
        &mut client_config,
    )
    .unwrap();
    let mut server = quiche::accept(
        &quiche::ConnectionId::from_ref(&[2; 16]),
        None,
        server_address,
        client_address,
        &mut server_config,
    )
    .unwrap();
    let mut packet = vec![0; 65535];
    for _ in 0..100 {
        while let Ok((length, info)) = client.send(&mut packet) {
            server
                .recv(
                    &mut packet[..length],
                    quiche::RecvInfo {
                        from: info.from,
                        to: info.to,
                    },
                )
                .unwrap();
        }
        while let Ok((length, info)) = server.send(&mut packet) {
            client
                .recv(
                    &mut packet[..length],
                    quiche::RecvInfo {
                        from: info.from,
                        to: info.to,
                    },
                )
                .unwrap();
        }
        if client.is_established() && server.is_established() {
            return (client, server);
        }
        assert!(
            !client.is_closed(),
            "client TLS failure: {:?}",
            client.local_error()
        );
        assert!(
            !server.is_closed(),
            "server TLS failure: {:?}",
            server.local_error()
        );
    }
    panic!("QUIC handshake failed to establish");
}

#[test]
fn production_quic_context_negotiates_hybrid_and_classical_origins() {
    let ca = ProxyCa::generate("PQC QUIC root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    for (groups, expected) in [(GROUPS, HYBRID_GROUP), ("X25519", 29)] {
        let mut server = SslContext::builder(SslMethod::tls()).unwrap();
        server.set_certificate(&leaf.certificate).unwrap();
        server.set_private_key(&leaf.private_key).unwrap();
        server.set_curves_list(groups).unwrap();
        let factory = factory(&ca);
        let capture = factory.capture(Some("localhost".into()), vec!["h3".into()]);
        let mut client_context = factory.quiche_context_builder().unwrap();
        capture.attach_context(&mut client_context).unwrap();
        let (mut client, mut server) =
            quic_handshake(quic_config(client_context), quic_config(server));
        assert_eq!(client.as_mut().curve(), Some(expected));
        assert_eq!(server.as_mut().curve(), Some(expected));
        assert_eq!(
            capture.snapshot().details["Negotiated group ID"],
            format!("0x{expected:04x}")
        );
        assert_eq!(
            capture.snapshot().details["Peer handshake signature"],
            "ecdsa_secp256r1_sha256"
        );
    }
}

#[test]
fn production_upstream_verifies_all_ml_dsa_variants_over_tcp_and_quic() {
    let ca = ProxyCa::generate("ML-DSA production root", 2).unwrap();
    let factory = factory(&ca);
    for variant in [
        MlDsaVariant::MlDsa44,
        MlDsaVariant::MlDsa65,
        MlDsaVariant::MlDsa87,
    ] {
        let leaf = ca
            .issue_with_algorithm(
                EndpointIdentity::parse("localhost").unwrap(),
                1,
                LeafKeyAlgorithm::MlDsa(variant),
            )
            .unwrap();
        let capture = factory.capture(Some("localhost".into()), vec![]);
        let (tls, _) = handshake_captured(
            acceptor(&leaf.certificate, &leaf.private_key, GROUPS),
            &factory.hyper_connector().unwrap(),
            Some(&capture),
        );
        assert_eq!(
            tls.ssl().peer_signature_algorithm(),
            Some(variant.signature_algorithm())
        );
        assert_eq!(
            capture.snapshot().details["Peer handshake signature"],
            variant.signature_algorithm().to_string()
        );

        let mut server = SslContext::builder(SslMethod::tls()).unwrap();
        server.set_certificate(&leaf.certificate).unwrap();
        server.set_private_key(&leaf.private_key).unwrap();
        server.set_curves_list(GROUPS).unwrap();
        let capture = factory.capture(Some("localhost".into()), vec!["h3".into()]);
        let mut context = factory.quiche_context_builder().unwrap();
        capture.attach_context(&mut context).unwrap();
        let (mut client, _) = quic_handshake(quic_config(context), quic_config(server));
        assert_eq!(
            client.as_mut().peer_signature_algorithm(),
            Some(variant.signature_algorithm())
        );
        assert_eq!(
            capture.snapshot().details["Peer handshake signature"],
            variant.signature_algorithm().to_string()
        );
    }
}

#[test]
fn production_downstream_selects_each_ml_dsa_variant_and_classical_fallback() {
    let ca = ProxyCa::generate("ML-DSA downstream root", 2).unwrap();
    let identity = EndpointIdentity::parse("localhost").unwrap();
    let resolver = CachedMitmCertificateResolver::new(ca.clone(), 1, 1).unwrap();
    let leaves = resolver
        .resolve_credentials(&identity, std::time::SystemTime::now())
        .unwrap();
    assert_eq!(leaves.len(), 4);
    let cached = resolver
        .resolve_credentials(&identity, std::time::SystemTime::now())
        .unwrap();
    for (leaf, cached) in leaves.iter().zip(&cached) {
        assert_eq!(
            leaf.certificate.to_der().unwrap(),
            cached.certificate.to_der().unwrap()
        );
        assert!(
            leaf.certificate
                .verify(&ca.certificate().public_key().unwrap())
                .unwrap()
        );
    }
    let factory = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default());
    for (index, scheme) in [
        SslSignatureAlgorithm::ML_DSA_44,
        SslSignatureAlgorithm::ML_DSA_65,
        SslSignatureAlgorithm::ML_DSA_87,
        SslSignatureAlgorithm::ECDSA_SECP256R1_SHA256,
    ]
    .into_iter()
    .enumerate()
    {
        let capture = TlsCapture::new(transmog_core::TlsEvidence::default());
        let mut client = connector(&ca);
        client.set_verify_algorithm_prefs(&[scheme]).unwrap();
        let (tls, _) = handshake(
            factory
                .acceptor_credentials_captured(&leaves, Some(&capture))
                .unwrap(),
            &client.build(),
        );
        assert_eq!(tls.ssl().peer_signature_algorithm(), Some(scheme));
        assert_eq!(
            tls.ssl().peer_certificate().unwrap().to_der().unwrap(),
            leaves[index].certificate.to_der().unwrap()
        );
        assert_eq!(
            capture.snapshot().details["Local handshake signature"],
            scheme.to_string()
        );
        assert_eq!(
            capture.snapshot().server_chain[0].der.as_ref(),
            leaves[index].certificate.to_der().unwrap()
        );
    }
    // The same credentials retain TLS 1.2 interoperability, with its classical leaf.
    let mut client = connector(&ca);
    client
        .set_min_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    client
        .set_max_proto_version(Some(SslVersion::TLS1_2))
        .unwrap();
    let (tls, _) = handshake(
        factory
            .acceptor_credentials_captured(&leaves, None)
            .unwrap(),
        &client.build(),
    );
    assert_eq!(
        tls.ssl().peer_certificate().unwrap().to_der().unwrap(),
        leaves[3].certificate.to_der().unwrap()
    );
    // Invalid native credentials must fail during context construction.
    let mut invalid = leaves.clone();
    invalid[0].private_key = leaves[1].private_key.clone();
    assert!(
        factory
            .acceptor_credentials_captured(&invalid, None)
            .is_err()
    );
    assert!(factory.acceptor_credentials_captured(&[], None).is_err());
    invalid[0] = ca
        .issue(EndpointIdentity::parse("other.test").unwrap(), 1)
        .unwrap();
    assert!(
        factory
            .acceptor_credentials_captured(&invalid, None)
            .is_err()
    );
}

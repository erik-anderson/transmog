//! Real TLS handshakes exercise the algorithms used by Chromium, without a browser pin.
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    hash::MessageDigest,
    pkey::{Id, PKey, Private},
    ssl::{
        CertificateCompressionAlgorithm, CertificateCompressor, ExtensionType, SslAcceptor,
        SslConnector, SslContext, SslContextBuilder, SslMethod, SslSignatureAlgorithm, SslStream,
        SslVersion,
    },
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
    },
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use transmog_tls::{
    CachedMitmCertificateResolver, DownstreamCertificateResolver, DownstreamTlsContextFactory,
    DownstreamTlsPolicy, EndpointIdentity, LeafKeyAlgorithm, MlDsaVariant, PemTrustSource, ProxyCa,
    TlsCapture, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

const HYBRID_GROUP: u16 = 0x11ec;
const GROUPS: &str = "X25519MLKEM768:X25519:P-256:P-384";
// Real schemes observed in Chrome's signature_algorithms extension.
const CHROMIUM_SIGNATURE_ALGORITHMS: [u16; 11] = [
    0x0904, 0x0905, 0x0906, 0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601,
];
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
    leaf_for_key(ca, key)
}

fn leaf_for_key(ca: &ProxyCa, key: PKey<Private>) -> (X509, PKey<Private>) {
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
    let mut key_usage = KeyUsage::new();
    key_usage.digital_signature();
    if key.id() == Id::RSA {
        key_usage.key_encipherment();
    }
    cert.append_extension(&key_usage.build().unwrap()).unwrap();
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
        let length = buffer.len().min(17);
        self.socket.read(&mut buffer[..length])
    }
}
impl Write for RecordingStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.socket.write(&buffer[..buffer.len().min(17)])?;
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
    recorded_hello(written).extensions.into_iter().collect()
}

struct RecordedHello {
    ciphers: Vec<u16>,
    extensions: Vec<(u16, Vec<u8>)>,
}

fn recorded_hello(written: &[u8]) -> RecordedHello {
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
    let cipher_length = usize::from(u16_be(&hello[cursor..]));
    let ciphers = hello[cursor + 2..cursor + 2 + cipher_length]
        .chunks_exact(2)
        .map(u16_be)
        .collect();
    cursor += 2 + cipher_length;
    cursor += 1 + usize::from(hello[cursor]);
    let length = usize::from(u16_be(&hello[cursor..]));
    let end = cursor + 2 + length;
    cursor += 2;
    let mut extensions = Vec::new();
    while cursor < end {
        let kind = u16_be(&hello[cursor..]);
        let length = usize::from(u16_be(&hello[cursor + 2..]));
        cursor += 4;
        assert!(!extensions.iter().any(|(previous, _)| *previous == kind));
        extensions.push((kind, hello[cursor..cursor + length].to_vec()));
        cursor += length;
    }
    assert_eq!(cursor, end);
    RecordedHello {
        ciphers,
        extensions,
    }
}

fn signature_algorithms(extension: &[u8]) -> Vec<u16> {
    assert_eq!(usize::from(u16_be(extension)), extension.len() - 2);
    assert_eq!(extension.len() % 2, 0);
    extension[2..].chunks_exact(2).map(u16_be).collect()
}

fn is_grease(value: u16) -> bool {
    value & 0x0f0f == 0x0a0a && value >> 8 == value & 0xff
}

#[test]
fn custom_profile_controls_actual_tcp_offers_and_capture_provenance() {
    use transmog_core::network_profile::{NetworkProfileSnapshot, TlsGrease, TlsVersions};
    let ca = ProxyCa::generate("Custom profile TLS root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    for versions in [
        TlsVersions::Tls12And13,
        TlsVersions::Tls12Only,
        TlsVersions::Tls13Only,
    ] {
        let mut profile = NetworkProfileSnapshot {
            id: "custom-diagnostic".into(),
            name: "Diagnostic".into(),
            ..Default::default()
        };
        profile.settings.root_store = transmog_core::network_profile::RootStore::Os;
        profile.settings.protocols.http3 = versions != TlsVersions::Tls12Only;
        profile.settings.tls_versions = versions;
        profile.settings.compatibility_ciphers = false;
        profile.settings.hybrid_key_exchange = false;
        profile.settings.grease = TlsGrease::Disabled;
        profile.settings.randomize_extensions = false;
        profile.settings.ech_grease = false;
        profile.settings.certificate_compression = false;
        let factory = factory(&ca).with_network_profile(profile.clone()).unwrap();
        let capture = factory.capture(Some("localhost".into()), vec!["h2".into()]);
        let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        server.set_certificate(&leaf.certificate).unwrap();
        server.set_private_key(&leaf.private_key).unwrap();
        let (tls, group) = handshake_captured(
            server.build(),
            &factory.hyper_connector().unwrap(),
            Some(&capture),
        );
        assert_eq!(group, Some(29));
        let hello = recorded_hello(&tls.get_ref().written);
        assert!(hello.ciphers.iter().all(|&cipher| !is_grease(cipher)));
        assert!(
            hello
                .extensions
                .iter()
                .all(|(kind, _)| !is_grease(*kind) && ![27, 0xfe0d].contains(kind))
        );
        let extensions: BTreeMap<_, _> = hello.extensions.into_iter().collect();
        assert_eq!(
            signature_algorithms(&extensions[&13]),
            CHROMIUM_SIGNATURE_ALGORITHMS
        );
        assert_eq!(signature_algorithms(&extensions[&10]), [29, 23, 24]);
        let tls12: Vec<_> = hello
            .ciphers
            .into_iter()
            .filter(|cipher| !matches!(*cipher, 0x1301..=0x1303))
            .collect();
        if versions != TlsVersions::Tls13Only {
            assert_eq!(tls12, [0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8]);
        }
        assert_eq!(
            tls.ssl().version2(),
            Some(if versions == TlsVersions::Tls12Only {
                SslVersion::TLS1_2
            } else {
                SslVersion::TLS1_3
            })
        );
        assert_eq!(capture.snapshot().network_profile, Some(profile));
    }
}

#[test]
fn custom_profile_controls_quic_context_before_capture_callback_reinstallation() {
    use transmog_core::network_profile::{NetworkProfileSnapshot, TlsGrease};
    let ca = ProxyCa::generate("Custom profile QUIC root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let recorder = observed.clone();
    let mut server = SslContext::builder(SslMethod::tls()).unwrap();
    server.set_certificate(&leaf.certificate).unwrap();
    server.set_private_key(&leaf.private_key).unwrap();
    server.set_select_certificate_callback(move |hello| {
        let values = [
            ExtensionType::SIGNATURE_ALGORITHMS,
            ExtensionType::SUPPORTED_GROUPS,
            ExtensionType::CERT_COMPRESSION,
            ExtensionType::ENCRYPTED_CLIENT_HELLO,
        ]
        .map(|kind| hello.get_extension(kind).map(<[u8]>::to_vec));
        recorder.lock().unwrap().push(values);
        Ok(())
    });
    let mut profile = NetworkProfileSnapshot::default();
    profile.settings.root_store = transmog_core::network_profile::RootStore::Os;
    profile.settings.grease = TlsGrease::NoSignature;
    profile.settings.hybrid_key_exchange = false;
    profile.settings.ech_grease = false;
    profile.settings.certificate_compression = false;
    let factory = factory(&ca).with_network_profile(profile.clone()).unwrap();
    let capture = factory.capture(Some("localhost".into()), vec!["h3".into()]);
    let mut client = factory.quiche_context_builder().unwrap();
    capture.attach_context(&mut client).unwrap();
    let (mut client, _) = quic_handshake(quic_config(client), quic_config(server));
    assert_eq!(client.as_mut().curve(), Some(29));
    let observed = observed.lock().unwrap();
    assert_eq!(
        signature_algorithms(observed[0][0].as_ref().unwrap()),
        CHROMIUM_SIGNATURE_ALGORITHMS
    );
    let groups = signature_algorithms(observed[0][1].as_ref().unwrap());
    assert!(is_grease(groups[0]));
    assert_eq!(&groups[1..], [29, 23, 24]);
    assert_eq!(&observed[0][2..], [None, None]);
    assert_eq!(capture.snapshot().network_profile, Some(profile));
}

fn assert_ech_grease(extension: &[u8]) {
    assert_eq!(extension[0], 0); // ClientHelloOuter
    assert_eq!(u16_be(&extension[1..]), 1); // HKDF-SHA256
    assert!(matches!(u16_be(&extension[3..]), 1 | 3)); // AES-128-GCM or ChaCha20-Poly1305
    assert_eq!(u16_be(&extension[6..]), 32); // X25519 encapsulated key
    let payload_length = usize::from(u16_be(&extension[40..]));
    assert_eq!(extension.len(), 42 + payload_length);
    assert!((144..=240).contains(&payload_length));
    assert_eq!((payload_length - 16) % 32, 0);
}

#[test]
fn production_tcp_general_grease_preserves_real_preferences() {
    let ca = ProxyCa::generate("General GREASE TCP root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let mut baseline = factory(&ca)
        .hyper_connector_builder(b"\x02h2\x08http/1.1")
        .unwrap();
    baseline.set_grease_enabled(false);
    baseline.set_permute_extensions(false);
    let (tls, _) = handshake(
        acceptor(&leaf.certificate, &leaf.private_key, GROUPS),
        &baseline.build(),
    );
    let expected = recorded_hello(&tls.get_ref().written);
    let expected_extensions: BTreeMap<_, _> = expected.extensions.into_iter().collect();
    let mut orders = Vec::new();
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for _ in 0..3 {
            let server = DownstreamTlsContextFactory::new(DownstreamTlsPolicy {
                minimum_version: version,
                maximum_version: version,
            })
            .acceptor(&leaf)
            .unwrap();
            let capture = factory(&ca).capture(Some("localhost".into()), vec![]);
            let (tls, _) = handshake_captured(
                server,
                &factory(&ca).hyper_connector().unwrap(),
                Some(&capture),
            );
            assert_eq!(tls.ssl().version2(), Some(version));
            assert!(tls.ssl().verify_result().is_ok());
            let hello = recorded_hello(&tls.get_ref().written);
            orders.push(
                hello
                    .extensions
                    .iter()
                    .filter_map(|(kind, _)| (!is_grease(*kind)).then_some(*kind))
                    .collect::<Vec<_>>(),
            );
            assert!(is_grease(hello.ciphers[0]));
            assert_eq!(&hello.ciphers[1..], expected.ciphers);
            assert!(is_grease(hello.extensions[0].0));
            assert!(hello.extensions[0].1.is_empty());
            let last = hello.extensions.last().unwrap();
            assert!(is_grease(last.0));
            assert_eq!(last.1, [0]);
            assert_ne!(hello.extensions[0].0, last.0);
            let real: BTreeMap<_, _> = hello
                .extensions
                .into_iter()
                .filter(|(kind, _)| !is_grease(*kind))
                .collect();
            assert_eq!(
                real.keys().collect::<Vec<_>>(),
                expected_extensions.keys().collect::<Vec<_>>()
            );
            let groups = signature_algorithms(&real[&10]);
            assert!(is_grease(groups[0]));
            assert_eq!(
                &groups[1..],
                signature_algorithms(&expected_extensions[&10])
            );
            let versions = &real[&43];
            assert_eq!(usize::from(versions[0]), versions.len() - 1);
            assert!(is_grease(u16_be(&versions[1..])));
            assert_eq!(&versions[3..], &expected_extensions[&43][1..]);
            let shares = &real[&51];
            assert_eq!(usize::from(u16_be(shares)), shares.len() - 2);
            assert!(is_grease(u16_be(&shares[2..])));
            assert_eq!(u16_be(&shares[4..]), 1);
            assert_eq!(shares[6], 0);
            // The real shares contain fresh key bytes, so compare group IDs and lengths.
            let shape = |bytes: &[u8]| {
                let mut remaining = bytes;
                let mut result = Vec::new();
                while !remaining.is_empty() {
                    let length = usize::from(u16_be(&remaining[2..]));
                    result.push((u16_be(remaining), length));
                    remaining = &remaining[4 + length..];
                }
                result
            };
            assert_eq!(shape(&shares[7..]), shape(&expected_extensions[&51][2..]));
            assert_eq!(real[&5], [1, 0, 0, 0, 0]);
            assert!(real[&18].is_empty());
            assert_eq!(real[&27], [2, 0, 2]);
            assert_ech_grease(&real[&0xfe0d]);
            assert!(capture.snapshot().valid());
            for kind in [0, 5, 11, 16, 18, 23, 35, 45, 0xff01] {
                assert_eq!(real[&kind], expected_extensions[&kind]);
            }
        }
    }
    // Six fresh handshakes make accidental agreement of all permutations
    // negligible, without requiring any particular random ordering.
    assert!(orders.windows(2).any(|pair| pair[0] != pair[1]));
}

#[test]
fn production_tcp_signature_grease_preserves_real_algorithms_and_handshakes() {
    let ca = ProxyCa::generate("Signature GREASE TCP root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        let mut baseline = factory(&ca)
            .hyper_connector_builder(b"\x02h2\x08http/1.1")
            .unwrap();
        transmog_tls_native::set_signature_algorithm_grease(&mut baseline, false);
        let acceptor = || {
            DownstreamTlsContextFactory::new(DownstreamTlsPolicy {
                minimum_version: version,
                maximum_version: version,
            })
            .acceptor(&leaf)
            .unwrap()
        };
        let (tls, _) = handshake(acceptor(), &baseline.build());
        let expected = signature_algorithms(&hello_extensions(&tls.get_ref().written)[&13]);
        assert_eq!(expected, CHROMIUM_SIGNATURE_ALGORITHMS);
        assert!(expected.iter().all(|&value| !is_grease(value)));
        let connector = factory(&ca).hyper_connector().unwrap();
        for _ in 0..3 {
            let (tls, _) = handshake(acceptor(), &connector);
            assert_eq!(tls.ssl().version2(), Some(version));
            assert!(tls.ssl().verify_result().is_ok());
            let advertised = signature_algorithms(&hello_extensions(&tls.get_ref().written)[&13]);
            assert!(is_grease(advertised[0]));
            assert_eq!(&advertised[1..], expected);
        }
    }
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
    .with_pki_metadata(Arc::new(
        transmog_tls::trust_anchors::PkiMetadataProvider::new(
            transmog_tls::trust_anchors::PkiMetadata::bundled(),
            None,
        ),
    ))
}

#[test]
fn chromium_trust_anchor_ids_match_actual_tcp_and_quic_client_hellos() {
    let ca = ProxyCa::generate("Trust Anchor ID wire fixture", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let ids = transmog_tls::trust_anchors::PkiMetadata::bundled();
    let expected = [
        u16::try_from(ids.requested_ids().len())
            .unwrap()
            .to_be_bytes()
            .as_slice(),
        ids.requested_ids(),
    ]
    .concat();
    for enabled in [true, false] {
        let mut profile = transmog_core::network_profile::NetworkProfileSnapshot::default();
        profile.settings.root_store = transmog_core::network_profile::RootStore::Os;
        profile.settings.trust_anchor_ids = enabled;
        let factory = factory(&ca).with_network_profile(profile).unwrap();
        let (tcp, _) = handshake(
            acceptor(&leaf.certificate, &leaf.private_key, GROUPS),
            &factory.hyper_connector().unwrap(),
        );
        let extensions = recorded_hello(&tcp.get_ref().written)
            .extensions
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(extensions.get(&0xca34), enabled.then_some(&expected));
        let observed = Arc::new(Mutex::new(None));
        let callback = observed.clone();
        let mut server = SslContext::builder(SslMethod::tls()).unwrap();
        server.set_certificate(&leaf.certificate).unwrap();
        server.set_private_key(&leaf.private_key).unwrap();
        server.set_select_certificate_callback(move |hello| {
            *callback.lock().unwrap() = hello
                .get_extension(ExtensionType::from(0xca34))
                .map(<[u8]>::to_vec);
            Ok(())
        });
        quic_handshake(
            quic_config(factory.quiche_context_builder().unwrap()),
            quic_config(server),
        );
        assert_eq!(
            *observed.lock().unwrap(),
            enabled.then_some(expected.clone())
        );
    }
}

struct CertificateEncoder(Arc<AtomicUsize>);

impl CertificateCompressor for CertificateEncoder {
    const ALGORITHM: CertificateCompressionAlgorithm = CertificateCompressionAlgorithm::BROTLI;
    const CAN_COMPRESS: bool = true;
    const CAN_DECOMPRESS: bool = false;

    fn compress<W: Write>(&self, input: &[u8], output: &mut W) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        brotli::BrotliCompress(
            &mut std::io::Cursor::new(input),
            output,
            &brotli::enc::BrotliEncoderParams::default(),
        )
        .map(|_| ())
    }
}

#[test]
fn production_clients_verify_brotli_compressed_certificates_over_tcp_and_quic() {
    let ca = ProxyCa::generate("Brotli certificate root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let compressed = Arc::new(AtomicUsize::new(0));
    let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    server
        .set_min_proto_version(Some(SslVersion::TLS1_3))
        .unwrap();
    server
        .set_max_proto_version(Some(SslVersion::TLS1_3))
        .unwrap();
    server.set_certificate(&leaf.certificate).unwrap();
    server.set_private_key(&leaf.private_key).unwrap();
    server
        .add_certificate_compression_algorithm(CertificateEncoder(compressed.clone()))
        .unwrap();
    let (tls, _) = handshake(server.build(), &factory(&ca).hyper_connector().unwrap());
    assert_eq!(tls.ssl().version2(), Some(SslVersion::TLS1_3));
    assert!(tls.ssl().verify_result().is_ok());
    assert_eq!(compressed.load(Ordering::SeqCst), 1);
    let mut server = SslContext::builder(SslMethod::tls()).unwrap();
    server.set_certificate(&leaf.certificate).unwrap();
    server.set_private_key(&leaf.private_key).unwrap();
    server
        .add_certificate_compression_algorithm(CertificateEncoder(compressed.clone()))
        .unwrap();
    let (mut client, _) = quic_handshake(
        quic_config(factory(&ca).quiche_context_builder().unwrap()),
        quic_config(server),
    );
    assert!(client.as_mut().verify_result().is_ok());
    assert_eq!(compressed.load(Ordering::SeqCst), 2);
}

struct InvalidCertificateEncoder(u8);

impl CertificateCompressor for InvalidCertificateEncoder {
    const ALGORITHM: CertificateCompressionAlgorithm = CertificateCompressionAlgorithm::BROTLI;
    const CAN_COMPRESS: bool = true;
    const CAN_DECOMPRESS: bool = false;

    fn compress<W: Write>(&self, input: &[u8], output: &mut W) -> std::io::Result<()> {
        match self.0 {
            0 => output.write_all(&[0xff; 16]),
            1 => brotli::BrotliCompress(
                &mut std::io::Cursor::new(&input[..input.len() - 1]),
                output,
                &brotli::enc::BrotliEncoderParams::default(),
            )
            .map(|_| ()),
            2 => {
                let mut oversized = input.to_vec();
                oversized.push(0);
                brotli::BrotliCompress(
                    &mut std::io::Cursor::new(oversized),
                    output,
                    &brotli::enc::BrotliEncoderParams::default(),
                )
                .map(|_| ())
            }
            _ => brotli::BrotliCompress(
                &mut std::io::Cursor::new(input),
                output,
                &brotli::enc::BrotliEncoderParams::default(),
            )
            .map(|_| ()),
        }
    }
}

#[test]
fn compressed_certificate_errors_and_untrusted_chains_fail_the_handshake() {
    let ca = ProxyCa::generate("Invalid compressed certificate root", 2).unwrap();
    let unrelated = ProxyCa::generate("Unrelated compressed certificate root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    for kind in 0..4 {
        let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        server
            .set_min_proto_version(Some(SslVersion::TLS1_3))
            .unwrap();
        server.set_certificate(&leaf.certificate).unwrap();
        server.set_private_key(&leaf.private_key).unwrap();
        server
            .add_certificate_compression_algorithm(InvalidCertificateEncoder(kind))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            socket(&tcp);
            server.build().accept(tcp).is_err()
        });
        let tcp = TcpStream::connect(address).unwrap();
        socket(&tcp);
        let connector = factory(if kind == 3 { &unrelated } else { &ca })
            .hyper_connector()
            .unwrap();
        assert!(
            connector.connect("localhost", tcp).is_err(),
            "invalid certificate case {kind} succeeded"
        );
        assert!(server.join().unwrap());
    }
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
        assert_eq!(
            evidence.details["HelloRetryRequest"],
            if expected == 23 { "Yes" } else { "No" }
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
fn production_tls12_offers_chromium_order_and_negotiates_each_suite() {
    let ca = ProxyCa::generate("ChaCha upstream root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let (rsa_certificate, rsa_key) = leaf_for_key(
        &ca,
        PKey::from_rsa(boring::rsa::Rsa::generate(2048).unwrap()).unwrap(),
    );
    for (certificate, key, cipher) in [
        (
            &leaf.certificate,
            &leaf.private_key,
            "ECDHE-ECDSA-CHACHA20-POLY1305",
        ),
        (&rsa_certificate, &rsa_key, "ECDHE-RSA-CHACHA20-POLY1305"),
        (
            &leaf.certificate,
            &leaf.private_key,
            "ECDHE-ECDSA-AES128-GCM-SHA256",
        ),
        (
            &leaf.certificate,
            &leaf.private_key,
            "ECDHE-ECDSA-AES256-GCM-SHA384",
        ),
        (&rsa_certificate, &rsa_key, "ECDHE-RSA-AES128-GCM-SHA256"),
        (&rsa_certificate, &rsa_key, "ECDHE-RSA-AES256-GCM-SHA384"),
        (&rsa_certificate, &rsa_key, "ECDHE-RSA-AES128-SHA"),
        (&rsa_certificate, &rsa_key, "ECDHE-RSA-AES256-SHA"),
        (&rsa_certificate, &rsa_key, "AES128-GCM-SHA256"),
        (&rsa_certificate, &rsa_key, "AES256-GCM-SHA384"),
        (&rsa_certificate, &rsa_key, "AES128-SHA"),
        (&rsa_certificate, &rsa_key, "AES256-SHA"),
    ] {
        let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        server.set_certificate(certificate).unwrap();
        server.set_private_key(key).unwrap();
        server
            .set_max_proto_version(Some(SslVersion::TLS1_2))
            .unwrap();
        server.set_cipher_list(cipher).unwrap();
        let preset = factory(&ca)
            .with_network_profile(
                transmog_core::network_profile::NetworkProfileSnapshot::legacy_chromium(3),
            )
            .unwrap();
        let (tls, _) = handshake(server.build(), &preset.hyper_connector().unwrap());
        assert_eq!(tls.ssl().version2(), Some(SslVersion::TLS1_2));
        assert!(tls.ssl().verify_result().is_ok());
        assert_eq!(tls.ssl().current_cipher().unwrap().name(), cipher);
        let hello = recorded_hello(&tls.get_ref().written);
        let tls12: Vec<_> = hello
            .ciphers
            .into_iter()
            .filter(|cipher| !is_grease(*cipher) && !matches!(*cipher, 0x1301..=0x1303))
            .collect();
        assert_eq!(
            tls12,
            [
                0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc013, 0xc014, 0x009c, 0x009d,
                0x002f, 0x0035
            ]
        );
    }
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

#[test]
fn production_quic_signature_grease_reaches_the_peer_and_preserves_handshakes() {
    let ca = ProxyCa::generate("Signature GREASE QUIC root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let mut expected = Vec::new();
    for enabled in [false, true] {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let recorder = observed.clone();
        let mut server = SslContext::builder(SslMethod::tls()).unwrap();
        server.set_certificate(&leaf.certificate).unwrap();
        server.set_private_key(&leaf.private_key).unwrap();
        server.set_curves_list(GROUPS).unwrap();
        server.set_select_certificate_callback(move |hello| {
            let extensions: Vec<_> = [
                ExtensionType::SIGNATURE_ALGORITHMS,
                ExtensionType::SUPPORTED_GROUPS,
                ExtensionType::SUPPORTED_VERSIONS,
                ExtensionType::KEY_SHARE,
                ExtensionType::STATUS_REQUEST,
                ExtensionType::CERTIFICATE_TIMESTAMP,
                ExtensionType::CERT_COMPRESSION,
                ExtensionType::ENCRYPTED_CLIENT_HELLO,
            ]
            .into_iter()
            .map(|kind| hello.get_extension(kind).unwrap().to_vec())
            .collect();
            recorder.lock().unwrap().push(extensions);
            Ok(())
        });
        let mut client = factory(&ca).quiche_context_builder().unwrap();
        let capture = factory(&ca).capture(Some("localhost".into()), vec!["h3".into()]);
        capture.attach_context(&mut client).unwrap();
        if !enabled {
            transmog_tls_native::set_signature_algorithm_grease(&mut client, false);
        }
        let (mut client, _) = quic_handshake(quic_config(client), quic_config(server));
        assert_eq!(client.as_mut().curve(), Some(HYBRID_GROUP));
        assert!(client.as_mut().verify_result().is_ok());
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 1);
        let advertised = signature_algorithms(&observed[0][0]);
        let groups = signature_algorithms(&observed[0][1]);
        assert!(is_grease(groups[0]));
        assert_eq!(&groups[1..], [HYBRID_GROUP, 29, 23, 24]);
        assert!(is_grease(u16_be(&observed[0][2][1..])));
        assert_eq!(&observed[0][2][3..], [3, 4]);
        assert!(is_grease(u16_be(&observed[0][3][2..])));
        assert_eq!(u16_be(&observed[0][3][4..]), 1);
        assert_eq!(observed[0][4], [1, 0, 0, 0, 0]);
        assert!(observed[0][5].is_empty());
        assert_eq!(observed[0][6], [2, 0, 2]);
        assert_ech_grease(&observed[0][7]);
        assert!(capture.snapshot().valid());
        if enabled {
            assert!(is_grease(advertised[0]));
            assert_eq!(&advertised[1..], expected);
        } else {
            assert!(advertised.iter().all(|&value| !is_grease(value)));
            assert_eq!(advertised, CHROMIUM_SIGNATURE_ALGORITHMS);
            expected = advertised;
        }
    }
}

fn quic_handshake(
    client_config: quiche::Config,
    server_config: quiche::Config,
) -> (quiche::Connection, quiche::Connection) {
    quic_handshake_with_loss(client_config, server_config, false)
}

fn quic_handshake_with_loss(
    mut client_config: quiche::Config,
    mut server_config: quiche::Config,
    lose_first: bool,
) -> (quiche::Connection, quiche::Connection) {
    quic_pair(&mut client_config, &mut server_config, lose_first, None)
}

fn quic_pair(
    client_config: &mut quiche::Config,
    server_config: &mut quiche::Config,
    lose_first: bool,
    session: Option<&[u8]>,
) -> (quiche::Connection, quiche::Connection) {
    let client_address = "127.0.0.1:10000".parse().unwrap();
    let server_address = "127.0.0.1:10001".parse().unwrap();
    let mut client = quiche::connect(
        Some("localhost"),
        &quiche::ConnectionId::from_ref(&[1; 16]),
        client_address,
        server_address,
        client_config,
    )
    .unwrap();
    if let Some(session) = session {
        client.set_session(session).unwrap();
    }
    let mut server = quiche::accept(
        &quiche::ConnectionId::from_ref(&[2; 16]),
        None,
        server_address,
        client_address,
        server_config,
    )
    .unwrap();
    let mut packet = vec![0; 1350];
    let mut lost = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        while let Ok((length, info)) = client.send(&mut packet) {
            if lose_first && !lost {
                lost = true;
                continue;
            }
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
        if client.timeout().is_some_and(|timeout| timeout.is_zero()) {
            client.on_timeout();
        }
        if server.timeout().is_some_and(|timeout| timeout.is_zero()) {
            server.on_timeout();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("QUIC handshake failed to establish");
}

#[test]
fn resumed_quic_records_the_new_group_and_the_original_signature_separately() {
    let ca = ProxyCa::generate("Resumed PQC QUIC root", 2).unwrap();
    let leaf = ca
        .issue_with_algorithm(
            EndpointIdentity::parse("localhost").unwrap(),
            1,
            LeafKeyAlgorithm::MlDsa(MlDsaVariant::MlDsa44),
        )
        .unwrap();
    let mut server = SslContext::builder(SslMethod::tls()).unwrap();
    server.set_certificate(&leaf.certificate).unwrap();
    server.set_private_key(&leaf.private_key).unwrap();
    server.set_curves_list(GROUPS).unwrap();
    server.set_session_id_context(b"pqc-quic-session").unwrap();
    let mut server = quic_config(server);
    let factory = factory(&ca);
    let (first, _) = quic_pair(
        &mut quic_config(factory.quiche_context_builder().unwrap()),
        &mut server,
        false,
        None,
    );
    let ticket = first
        .session()
        .expect("the initial connection yielded a ticket")
        .to_vec();
    for (groups, expected) in [(GROUPS, "X25519MLKEM768"), ("X25519", "X25519")] {
        let capture = factory.capture(Some("localhost".into()), vec!["h3".into()]);
        let mut context = factory.quiche_context_builder().unwrap();
        context.set_curves_list(groups).unwrap();
        capture.attach_context(&mut context).unwrap();
        let (mut client, _) =
            quic_pair(&mut quic_config(context), &mut server, false, Some(&ticket));
        assert!(client.as_mut().session_reused());
        let evidence = capture.snapshot();
        assert_eq!(evidence.details["Negotiated group"], expected);
        assert_eq!(
            evidence.details["Peer handshake signature"],
            "Not sent (session resumed)"
        );
        assert_eq!(
            evidence.details["Original peer handshake signature"],
            "mldsa44"
        );
        assert_eq!(evidence.chain_source.as_deref(), Some("resumed-session"));
        assert_eq!(evidence.details["Session resumed"], "Yes");
    }
}

#[test]
fn hybrid_quic_recovers_lost_initial_with_a_large_ml_dsa_certificate() {
    let ca = ProxyCa::generate("Lossy PQC QUIC root", 2).unwrap();
    let leaf = ca
        .issue_with_algorithm(
            EndpointIdentity::parse("localhost").unwrap(),
            1,
            LeafKeyAlgorithm::MlDsa(MlDsaVariant::MlDsa87),
        )
        .unwrap();
    let mut server = SslContext::builder(SslMethod::tls()).unwrap();
    server.set_certificate(&leaf.certificate).unwrap();
    server.set_private_key(&leaf.private_key).unwrap();
    server.set_curves_list(GROUPS).unwrap();
    let (mut client, mut server) = quic_handshake_with_loss(
        quic_config(factory(&ca).quiche_context_builder().unwrap()),
        quic_config(server),
        true,
    );
    assert_eq!(client.as_mut().curve(), Some(HYBRID_GROUP));
    assert_eq!(server.as_mut().curve(), Some(HYBRID_GROUP));
    assert_eq!(
        client.as_mut().peer_signature_algorithm(),
        Some(SslSignatureAlgorithm::ML_DSA_87)
    );
}

#[test]
fn malformed_hybrid_key_share_fails_before_application_data() {
    let ca = ProxyCa::generate("Malformed PQC root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let (tls, _) = handshake(
        acceptor(&leaf.certificate, &leaf.private_key, GROUPS),
        &connector(&ca).build(),
    );
    let wire = &tls.get_ref().written;
    let record_length = usize::from(u16_be(&wire[3..]));
    let mut hello = wire[..5 + record_length].to_vec();
    let mut cursor = 9 + 34;
    cursor += 1 + usize::from(hello[cursor]);
    cursor += 2 + usize::from(u16_be(&hello[cursor..]));
    cursor += 1 + usize::from(hello[cursor]);
    cursor += 2;
    let mut mutated = false;
    while cursor < hello.len() {
        let kind = u16_be(&hello[cursor..]);
        let length = usize::from(u16_be(&hello[cursor + 2..]));
        cursor += 4;
        if kind == 51 {
            assert_eq!(u16_be(&hello[cursor + 2..]), HYBRID_GROUP);
            // Preserve the record and extension framing but give ML-KEM a zero-length share.
            hello[cursor + 4..cursor + 6].copy_from_slice(&[0, 0]);
            mutated = true;
            break;
        }
        cursor += length;
    }
    assert!(mutated);
    let server = DownstreamTlsContextFactory::new(DownstreamTlsPolicy::default())
        .acceptor(&leaf)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        socket(&tcp);
        assert!(server.accept(tcp).is_err());
    });
    let mut client = TcpStream::connect(address).unwrap();
    socket(&client);
    client.write_all(&hello).unwrap();
    let mut alert = [0; 64];
    let count = client.read(&mut alert).unwrap();
    assert!(
        count >= 7 && alert[0] == 21,
        "Expected an immediate TLS alert"
    );
    worker.join().unwrap();
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

#[test]
fn every_downstream_credential_sends_its_intermediate_chain() {
    let root = ProxyCa::generate("Intermediate chain root", 2).unwrap();
    let intermediate = ProxyCa::generate("Intermediate chain issuer", 2).unwrap();
    let key = PKey::private_key_from_pem(&intermediate.private_key_pem_pkcs8().unwrap()).unwrap();
    let root_key = PKey::private_key_from_pem(&root.private_key_pem_pkcs8().unwrap()).unwrap();
    let mut certificate = X509::builder().unwrap();
    certificate.set_version(2).unwrap();
    certificate
        .set_serial_number(&BigNum::from_u32(2).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    certificate
        .set_subject_name(intermediate.certificate().subject_name())
        .unwrap();
    certificate
        .set_issuer_name(root.certificate().subject_name())
        .unwrap();
    certificate.set_pubkey(&key).unwrap();
    certificate
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    certificate
        .set_not_after(&Asn1Time::days_from_now(2).unwrap())
        .unwrap();
    certificate
        .append_extension(&BasicConstraints::new().critical().ca().build().unwrap())
        .unwrap();
    certificate
        .append_extension(&KeyUsage::new().key_cert_sign().crl_sign().build().unwrap())
        .unwrap();
    certificate
        .sign(&root_key, MessageDigest::sha256())
        .unwrap();
    let intermediate = ProxyCa::from_pem(
        &certificate.build().to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap();
    let resolver = CachedMitmCertificateResolver::new(intermediate.clone(), 1, 1).unwrap();
    let leaves = resolver
        .resolve_credentials(
            &EndpointIdentity::parse("localhost").unwrap(),
            std::time::SystemTime::now(),
        )
        .unwrap();
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
        // Only the root is trusted; success requires the server to send the intermediate.
        for single in [false, true] {
            let acceptor = if single {
                factory.acceptor(&leaves[index]).unwrap()
            } else {
                factory
                    .acceptor_credentials_captured(&leaves, None)
                    .unwrap()
            };
            let mut client = connector(&root);
            client.set_verify_algorithm_prefs(&[scheme]).unwrap();
            let (tls, _) = handshake(acceptor, &client.build());
            assert_eq!(tls.ssl().peer_signature_algorithm(), Some(scheme));
            assert!(tls.ssl().peer_cert_chain().unwrap().iter().any(|cert| {
                cert.to_der().unwrap() == intermediate.certificate().to_der().unwrap()
            }));
        }
    }
}

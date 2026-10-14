//! Stored certificate validation must run again when a TLS session resumes.
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::PKey,
    ssl::{SslAcceptor, SslConnector, SslMethod, SslSessionCacheMode, SslStream, SslVersion},
    x509::{X509, X509NameBuilder, extension::SubjectAlternativeName},
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use transmog_tls_native::{
    CertificatePolicy, configure_context_certificate_policy, observe_ssl_verification,
};

#[test]
#[allow(clippy::too_many_lines)] // One three-phase live handshake matrix shares all fixture ownership.
fn resumed_tls12_and_tls13_reject_a_certificate_that_expired_since_the_first_handshake() {
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();
        let key = PKey::from_ec_key(
            EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
        )
        .unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        cert.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
            .unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::from_unix(now - 1).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::from_unix(now + 4).unwrap())
            .unwrap();
        cert.append_extension(
            &SubjectAlternativeName::new()
                .dns("localhost")
                .build(&cert.x509v3_context(None, None))
                .unwrap(),
        )
        .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        let cert = cert.build();
        let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        server.set_certificate(&cert).unwrap();
        server.set_private_key(&key).unwrap();
        server.set_min_proto_version(Some(version)).unwrap();
        server.set_max_proto_version(Some(version)).unwrap();
        server.set_session_id_context(b"reverification").unwrap();
        let server = server.build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            for _ in 0..3 {
                let (socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                if let Ok(mut tls) = server.accept(socket) {
                    let _ = tls.write_all(&[7]);
                    let mut byte = [0];
                    let _ = tls.read_exact(&mut byte);
                }
            }
        });
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let cache = sessions.clone();
        let mut context = SslConnector::builder(SslMethod::tls()).unwrap();
        context.set_min_proto_version(Some(version)).unwrap();
        context.set_max_proto_version(Some(version)).unwrap();
        configure_context_certificate_policy(
            &mut context,
            Arc::new(CertificatePolicy {
                anchors: Arc::new([cert.to_der().unwrap()].into()),
                ..Default::default()
            }),
        )
        .unwrap();
        context.set_session_cache_mode(
            SslSessionCacheMode::CLIENT | SslSessionCacheMode::NO_INTERNAL_STORE,
        );
        context.set_new_session_callback(move |_, session| cache.lock().unwrap().push(session));
        let context = context.build();
        let results = Arc::new(Mutex::new(Vec::new()));
        for phase in 0..3 {
            if phase == 2 {
                std::thread::sleep(Duration::from_secs(5));
            }
            let mut ssl = context.configure().unwrap().into_ssl("localhost").unwrap();
            if phase != 0 {
                // SAFETY: the session is owned and belongs to this context and hostname.
                unsafe {
                    ssl.set_session(sessions.lock().unwrap().last().unwrap())
                        .unwrap();
                }
            }
            let recorded = results.clone();
            observe_ssl_verification(&mut ssl, move |ssl, result| {
                recorded.lock().unwrap().push((
                    ssl.session_reused(),
                    result.accepted,
                    result.error_code,
                ));
            })
            .unwrap();
            let socket = TcpStream::connect(address).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut tls = SslStream::new(ssl, socket).unwrap();
            if phase == 2 {
                assert!(
                    tls.connect().is_err(),
                    "Expired cached certificate was accepted"
                );
            } else {
                tls.connect().unwrap();
                assert_eq!(tls.ssl().session_reused(), phase != 0);
                let mut byte = [0];
                tls.read_exact(&mut byte).unwrap();
                tls.write_all(&byte).unwrap();
            }
        }
        worker.join().unwrap();
        assert_eq!(
            *results.lock().unwrap(),
            [(false, true, 0), (true, true, 0), (true, false, 10)]
        );
    }
}

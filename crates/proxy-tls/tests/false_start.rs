//! False Start permits eligible TLS 1.2 writes while preserving final-handshake evidence.
use boring::ssl::{AlpnError, SslAcceptor, SslMethod, SslStream, SslVersion, select_next_proto};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory,
    UpstreamTlsPolicy,
};

#[test]
#[allow(clippy::too_many_lines)] // Four eligibility cases share a real peer and final-flight assertion.
fn false_start_requires_tls12_aead_and_alpn_and_capture_waits_for_finished() {
    let ca = ProxyCa::generate("False Start fixture", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let trust = PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "fixture").unwrap();
    for (version, cipher, alpn, expected) in [
        (
            SslVersion::TLS1_2,
            "ECDHE-ECDSA-AES128-GCM-SHA256",
            true,
            true,
        ),
        (
            SslVersion::TLS1_2,
            "ECDHE-ECDSA-AES128-GCM-SHA256",
            false,
            false,
        ),
        (SslVersion::TLS1_2, "ECDHE-ECDSA-AES128-SHA", true, false),
        (
            SslVersion::TLS1_3,
            "ECDHE-ECDSA-AES128-GCM-SHA256",
            true,
            false,
        ),
    ] {
        let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        server.set_certificate(&leaf.certificate).unwrap();
        server.set_private_key(&leaf.private_key).unwrap();
        server.set_min_proto_version(Some(version)).unwrap();
        server.set_max_proto_version(Some(version)).unwrap();
        server.set_cipher_list(cipher).unwrap();
        if alpn {
            server.set_alpn_select_callback(|_, wire| {
                select_next_proto(b"\x02h2", wire).ok_or(AlpnError::NOACK)
            });
        }
        let server = server.build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut tls = server.accept(socket).unwrap();
            let mut byte = [0];
            tls.read_exact(&mut byte).unwrap();
            tls.write_all(&byte).unwrap();
        });
        let factory = UpstreamTlsContextFactory::new(
            Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
            UpstreamTlsPolicy {
                minimum_version: version,
                maximum_version: version,
                cipher_list: cipher.into(),
            },
        );
        let context = factory
            .synchronous_connector_builder(b"\x02h2")
            .unwrap()
            .build();
        let mut ssl = context.configure().unwrap().into_ssl("localhost").unwrap();
        let capture = factory.capture(Some("localhost".into()), vec!["h2".into()]);
        capture.attach_ssl(&mut ssl).unwrap();
        let socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut tls = SslStream::new(ssl, socket).unwrap();
        tls.connect().unwrap();
        assert_eq!(transmog_tls_native::in_false_start(tls.ssl()), expected);
        assert_eq!(tls.ssl().is_init_finished(), !expected);
        capture.completed(tls.ssl(), false);
        if expected {
            assert_eq!(
                capture.snapshot().details["Handshake completion"],
                "Waiting for server Finished"
            );
        }
        tls.write_all(&[7]).unwrap();
        let mut byte = [0];
        tls.read_exact(&mut byte).unwrap();
        assert_eq!(
            capture.snapshot().details["Handshake completion"],
            "Complete"
        );
        if expected {
            assert_eq!(capture.snapshot().details["TLS False Start"], "Used");
        }
        worker.join().unwrap();
    }
}

//! Unused malformed wire issuers must not fail an otherwise valid PKI path.
use boring::ssl::{Ssl, SslAcceptor, SslMethod, SslStream, SslVersion};
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
fn buffered_tls_ignores_a_malformed_unused_issuer_but_requires_a_trusted_path() {
    let ca = ProxyCa::generate("Buffered fixture", 2).unwrap();
    let other = ProxyCa::generate("Unrelated root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for trusted in [true, false] {
            let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
            transmog_tls_native::set_context_chain_and_key(
                &mut server,
                &[leaf.certificate.to_der().unwrap(), vec![0x30, 0x00]],
                &leaf.private_key,
            )
            .unwrap();
            server.set_min_proto_version(Some(version)).unwrap();
            server.set_max_proto_version(Some(version)).unwrap();
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
                if let Ok(mut tls) = server.accept(socket) {
                    let _ = tls.write_all(&[7]);
                    let mut byte = [0];
                    let _ = tls.read_exact(&mut byte);
                }
            });
            let root = if trusted { &ca } else { &other };
            let trust =
                PemTrustSource::from_pem(&root.certificate_pem().unwrap(), "fixture").unwrap();
            let factory = UpstreamTlsContextFactory::new(
                Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
                UpstreamTlsPolicy {
                    minimum_version: version,
                    maximum_version: version,
                    ..Default::default()
                },
            );
            let context = factory
                .tcp_context_builder(b"\x08http/1.1")
                .unwrap()
                .build();
            assert!(!context.has_x509_support());
            assert!(
                !factory
                    .quiche_context_builder()
                    .unwrap()
                    .build()
                    .has_x509_support()
            );
            let mut ssl = Ssl::new(&context).unwrap();
            ssl.set_hostname("localhost").unwrap();
            factory
                .configure_ssl_metadata_for_peer(&mut ssl, "localhost")
                .unwrap();
            let capture = factory.capture(Some("localhost".into()), vec!["http/1.1".into()]);
            capture.attach_ssl(&mut ssl).unwrap();
            let socket = TcpStream::connect(address).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut tls = SslStream::new(ssl, socket).unwrap();
            assert_eq!(tls.connect().is_ok(), trusted);
            if trusted {
                let mut byte = [0];
                tls.read_exact(&mut byte).unwrap();
                tls.write_all(&byte).unwrap();
                capture.completed(tls.ssl(), false);
                assert_eq!(
                    transmog_tls_native::peer_certificate_chain(tls.ssl()).len(),
                    2
                );
                assert_eq!(capture.snapshot().server_chain.len(), 2); // Chosen leaf/root path, not malformed wire issuer.
            }
            worker.join().unwrap();
        }
    }
}

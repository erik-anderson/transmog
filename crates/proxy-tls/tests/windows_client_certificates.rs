//! Explicit Windows acceptance test: uses real CurrentUser/My non-exportable
//! software CNG keys and verified TLS 1.2/1.3 peers. Run with --ignored; all owned
//! fixtures clean up on completion and panic. No root trust or machine keys change.
#![cfg(windows)]
use boring::{
    ssl::{SslAcceptor, SslMethod, SslSignatureAlgorithm, SslVerifyMode, SslVersion},
    x509::store::X509StoreBuilder,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    os::windows::process::CommandExt as _,
    process::Command,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use transmog_core::client_authentication::{
    ClientCertificateAction, ClientCertificatePolicy, ClientCertificateRule,
    ClientCertificateSelector,
};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TlsCapture, TrustSnapshot,
    UpstreamTlsContextFactory, UpstreamTlsPolicy,
    client_authentication::{
        ClientAuthenticationAttempt, ClientAuthenticationContext, ClientAuthenticationResolver,
        ClientCredential, ClientCredentialSource, PolicyClientAuthenticationResolver,
    },
    windows_client_certificates::WindowsCurrentUserCertificateSource,
};

struct Fixture {
    thumbprint: String,
    issuer_thumbprint: String,
    id: String,
}
impl Fixture {
    fn new(algorithm: &str) -> Self {
        let output = fixture_command("Create", None, Some(algorithm));
        // This helper emits only safe public fields; no JSON dependency is
        // needed just to read its fixture response in the TLS crate.
        let field = |name: &str| {
            let prefix = format!("\"{name}\":\"");
            output
                .split(&prefix)
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_owned()
        };
        Self {
            thumbprint: field("thumbprint"),
            issuer_thumbprint: field("issuerThumbprint"),
            id: field("id"),
        }
    }
    fn change(&self, action: &str) {
        fixture_command(action, Some(&self.thumbprint), None);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Do not panic during test unwinding, but report any cleanup failure.
        let result = command("Cleanup", Some(&self.thumbprint), None)
            .args(["-IssuerThumbprint", &self.issuer_thumbprint])
            .output();
        assert_cleanup(result);
    }
}
fn assert_cleanup(result: std::io::Result<std::process::Output>) {
    match result {
        Ok(output) if output.status.success() => (),
        other => {
            eprintln!("Windows client certificate fixture cleanup failed: {other:?}");
            assert!(std::thread::panicking(), "fixture cleanup failed");
        }
    }
}
fn command(action: &str, thumbprint: Option<&str>, algorithm: Option<&str>) -> Command {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/client-certificate-test-fixture.ps1");
    let mut command = Command::new("powershell.exe");
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for the fixture helper.
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(path)
        .args(["-Action", action]);
    if let Some(value) = thumbprint {
        command.args(["-Thumbprint", value]);
    }
    if let Some(value) = algorithm {
        command.args(["-Algorithm", value]);
    }
    command
}
fn fixture_command(action: &str, thumbprint: Option<&str>, algorithm: Option<&str>) -> String {
    let output = command(action, thumbprint, algorithm).output().unwrap();
    assert!(
        output.status.success(),
        "fixture {action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn pinned(id: &str, port: u16) -> ClientCertificatePolicy {
    ClientCertificatePolicy {
        rules: vec![ClientCertificateRule {
            id: "fixture".into(),
            enabled: true,
            host: "localhost".into(),
            port,
            mode: ClientCertificateAction::Automatic {
                selector: ClientCertificateSelector {
                    identity: Some(id.into()),
                    ..Default::default()
                },
            },
        }],
        ..Default::default()
    }
}
fn source() -> Arc<WindowsCurrentUserCertificateSource> {
    Arc::new(WindowsCurrentUserCertificateSource)
}
async fn lookup(id: &str) -> Arc<ClientCredential> {
    source()
        .credentials()
        .await
        .unwrap()
        .into_iter()
        .find(|credential| credential.id == id)
        .expect("real CNG fixture in the current-user inventory")
}
fn server(
    ca: &ProxyCa,
    credential: &ClientCredential,
    version: SslVersion,
    algorithm: u16,
) -> (
    std::net::SocketAddr,
    std::thread::JoinHandle<Option<boring::x509::X509>>,
) {
    let identity = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    builder.set_min_proto_version(Some(version)).unwrap();
    builder.set_max_proto_version(Some(version)).unwrap();
    builder.set_certificate(&identity.certificate).unwrap();
    builder.set_private_key(&identity.private_key).unwrap();
    builder.set_verify(SslVerifyMode::PEER);
    // A real server validates proof of possession and its exact fixture issuer.
    let mut store = X509StoreBuilder::new().unwrap();
    store
        .add_cert(credential.chain.last().unwrap().clone())
        .unwrap();
    store.set_flags(boring::x509::verify::X509VerifyFlags::PARTIAL_CHAIN);
    builder.set_cert_store(store.build());
    let mut issuers = boring::stack::Stack::new().unwrap();
    issuers
        .push(
            boring::x509::X509Name::from_der(
                &credential
                    .chain
                    .last()
                    .unwrap()
                    .subject_name()
                    .to_der()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    builder.set_client_ca_list(issuers);
    builder
        .set_verify_algorithm_prefs(&[SslSignatureAlgorithm::from(algorithm)])
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = builder.build();
    let peer = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut stream = acceptor.accept(tcp).unwrap();
        assert!(
            stream
                .ssl()
                .peer_cert_chain()
                .is_none_or(boring::stack::StackRef::is_empty),
            "the locally retained self-signed issuer must not be sent"
        );
        let cert = stream.ssl().peer_certificate();
        let mut input = [0];
        stream.read_exact(&mut input).unwrap();
        stream
            .write_all(if cert.is_some() {
                b"private"
            } else {
                b"public!"
            })
            .unwrap();
        cert
    });
    (address, peer)
}
async fn connect(
    fixture: &Fixture,
    credential: &ClientCredential,
    version: SslVersion,
    algorithm: u16,
    authenticated: bool,
) {
    let certificate = &credential.chain[0];
    let ca = ProxyCa::generate("Windows CNG server test", 2).unwrap();
    let (address, peer) = server(&ca, credential, version, algorithm);
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            pinned(&fixture.id, address.port()),
            source(),
            None,
        )
        .unwrap(),
    );
    let attempt = ClientAuthenticationAttempt::new(
        resolver.clone(),
        ClientAuthenticationContext {
            host: "localhost".into(),
            port: address.port(),
            caller: "process:windows-cng-test".into(),
            generation: resolver.generation(),
        },
        TlsCapture::new(transmog_core::TlsEvidence::default()),
    );
    let ssl = client_ssl(&ca, version, &attempt);
    let tcp = TcpStream::connect(address).await.unwrap();
    let mut stream = tokio::time::timeout(
        Duration::from_secs(10),
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
        "{version:?} scheme {algorithm:04x}"
    );
    let peer_certificate = peer.join().unwrap();
    assert_eq!(peer_certificate.is_some(), authenticated);
    let evidence = attempt.evidence();
    assert!(evidence.valid());
    assert_eq!(!evidence.client_chain.is_empty(), authenticated);
    if authenticated {
        assert_eq!(
            evidence.client_chain[0].der.as_ref(),
            certificate.to_der().unwrap()
        );
        assert_eq!(evidence.client_chain.len(), 1);
        assert_eq!(credential.chain.len(), 2, "full local path is retained");
    }
}

fn client_ssl(
    ca: &ProxyCa,
    version: SslVersion,
    attempt: &ClientAuthenticationAttempt,
) -> boring::ssl::Ssl {
    let trust =
        PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "owned fixture root").unwrap();
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
    ssl
}

#[tokio::test]
#[ignore = "Creates owned non-exportable keys in Windows CurrentUser/My; run explicitly"]
async fn real_windows_software_keys_authenticate_and_disappearance_is_anonymous() {
    for (algorithm, schemes) in [
        (
            "RSA",
            vec![
                (SslVersion::TLS1_2, 0x0401),
                (SslVersion::TLS1_2, 0x0201),
                (SslVersion::TLS1_2, 0x0501),
                (SslVersion::TLS1_2, 0x0601),
                (SslVersion::TLS1_2, 0x0804),
                (SslVersion::TLS1_3, 0x0804),
                (SslVersion::TLS1_3, 0x0805),
                (SslVersion::TLS1_3, 0x0806),
            ],
        ),
        (
            "ECDSA_nistP256",
            vec![
                (SslVersion::TLS1_2, 0x0403),
                (SslVersion::TLS1_2, 0x0203),
                (SslVersion::TLS1_3, 0x0403),
            ],
        ),
        (
            "ECDSA_nistP384",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0503)],
        ),
        (
            "ECDSA_nistP521",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0603)],
        ),
        (
            "ECDSA_genericP256",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0403)],
        ),
        (
            "ECDSA_genericP384",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0503)],
        ),
        (
            "ECDSA_genericP521",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0603)],
        ),
        (
            "ECDSA_genericSecP256r1",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0403)],
        ),
        (
            "ECDSA_genericX962P256v1",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0403)],
        ),
        (
            "ECDSA_genericSecP384r1",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0503)],
        ),
        (
            "ECDSA_genericSecP521r1",
            vec![(SslVersion::TLS1_2, 0x0403), (SslVersion::TLS1_3, 0x0603)],
        ),
    ] {
        check_windows_key(algorithm, schemes).await;
    }
}

async fn check_windows_key(algorithm: &str, schemes: Vec<(SslVersion, u16)>) {
    eprintln!("Verifying non-exportable {algorithm} fixture");
    let fixture = Fixture::new(algorithm);
    let credential = lookup(&fixture.id).await;
    assert_eq!(
        credential.chain.len(),
        2,
        "leaf and locally available issuer"
    );
    // Each fixture creation proves that private PKCS#8 export fails. The
    // provider only supplies public chain/identity, with no cached native key.
    for (version, scheme) in schemes {
        connect(&fixture, &credential, version, scheme, true).await;
    }
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(ClientCertificatePolicy::default(), source(), None)
            .unwrap(),
    );
    let monitor = source().monitor(Arc::downgrade(&resolver)).await.unwrap();
    let generation = resolver.generation();
    fixture.change("DeleteKey");
    assert!(!credential.key.available());
    assert!(
        credential
            .key
            .sign(credential.key.algorithms()[0], b"late operation")
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while resolver.generation() == generation {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(monitor);
    // A pinned identity with a missing key does not select another certificate.
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        connect(
            &fixture,
            &credential,
            version,
            if algorithm == "RSA" { 0x0804 } else { 0x0403 },
            false,
        )
        .await;
    }
    fixture.change("DeleteCertificate");
    assert!(!credential.key.available());
    assert!(
        !source()
            .credentials()
            .await
            .unwrap()
            .iter()
            .any(|item| item.id == fixture.id)
    );
}

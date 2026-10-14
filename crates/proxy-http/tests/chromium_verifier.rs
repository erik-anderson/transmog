//! Compare observable verification with Chromium 154 anchor and hostname rules.
#[path = "support/trust_anchors.rs"]
mod anchors;
mod support;
use boring::{
    pkey::{PKey, Private},
    ssl::{SslAcceptor, SslMethod},
    x509::X509,
};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use support::local_certificates::{CertificateOptions, configured_certificate};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use transmog_core::{
    VerificationResult, network_profile::NetworkProfileSnapshot, performance::PerformanceRecorder,
};
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_tls::{
    ProxyCa, UpstreamTlsContextFactory,
    local_trust::{LocalCertificates, LocalTrustMonitor},
    trust_anchors::{PkiMetadata, PkiMetadataProvider},
};

fn factory(
    public: &ProxyCa,
    certificate: &X509,
    expiry: bool,
    constraints: bool,
    local: bool,
) -> UpstreamTlsContextFactory {
    let metadata = Arc::new(
        PkiMetadata::decode(
            &anchors::catalog_bytes_with_flags(
                &[
                    (public.certificate(), 1, false, false),
                    (certificate, 2, expiry, constraints),
                ],
                42,
            ),
            "Chromium verifier fixture".into(),
        )
        .unwrap(),
    );
    let mut profile = NetworkProfileSnapshot::default();
    profile.settings.local_certificates = local;
    anchors::factory(public, metadata.clone())
        .with_pki_metadata(Arc::new(PkiMetadataProvider::new(metadata, None)))
        .with_network_profile(profile)
        .unwrap()
        .with_local_trust_monitor(LocalTrustMonitor::fixed(LocalCertificates::default()))
}

async fn probe(
    factory: &UpstreamTlsContextFactory,
    certificate: &X509,
    key: &PKey<Private>,
    chain: &[X509],
    expected: bool,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut server = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    server.set_certificate(certificate).unwrap();
    server.set_private_key(key).unwrap();
    for certificate in chain {
        server.add_extra_chain_cert(certificate.clone()).unwrap();
    }
    let server = server.build();
    let task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        if let Ok(mut stream) = tokio_boring::accept(&server, tcp).await {
            let mut request = [0; 4096];
            let bytes = stream.read(&mut request).await.unwrap();
            assert!(bytes > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            true
        } else {
            false
        }
    });
    let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
    let client = HyperOriginClient::new(factory)
        .unwrap()
        .with_performance(Some(recorder.clone()));
    let response = client
        .execute(
            support::request(port),
            HyperEgressMode::Http1Only,
            1024,
            Duration::from_secs(3),
        )
        .await;
    assert_eq!(
        response.is_ok(),
        expected,
        "{}",
        response
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap(),
        expected
    );
    let sample = recorder.snapshot();
    let tls = sample
        .transports
        .iter()
        .find_map(|t| t.tls.as_ref())
        .unwrap();
    assert!(tls.valid());
    assert_eq!(
        tls.details["Certificate verifier"],
        "Chromium BoringSSL PKI"
    );
    assert_eq!(
        tls.verification == Some(VerificationResult::Verified),
        expected,
        "{tls:?}"
    );
    if !expected {
        assert!(matches!(
            tls.verification,
            Some(VerificationResult::Rejected { .. })
        ));
    }
}

#[tokio::test]
async fn public_anchor_constraint_flags_apply_with_or_without_local_trust() {
    let public = ProxyCa::generate("unrelated public root", 2).unwrap();
    let mut options = CertificateOptions::new("limited public anchor");
    options.ca = true;
    options.path_length = Some(0);
    let (cert, key) = configured_certificate(options);
    let root = ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap();
    let issuer = anchors::intermediate(&root);
    let leaf = issuer
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    for local in [false, true] {
        for enforce in [false, true] {
            probe(
                &factory(&public, &cert, true, enforce, local),
                &leaf.certificate,
                &leaf.private_key,
                &[issuer.certificate().clone()],
                !enforce,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn public_anchor_used_as_server_leaf_requires_a_valid_signature() {
    let public = ProxyCa::generate("unrelated public root", 2).unwrap();
    let mut options = CertificateOptions::new("localhost");
    options.ca = true;
    let wrong_signer = ProxyCa::generate("wrong signer", 2).unwrap();
    options.issuer = Some(&wrong_signer);
    options.issuer_name = Some("localhost");
    let (cert, key) = configured_certificate(options);
    for local in [false, true] {
        probe(
            &factory(&public, &cert, false, false, local),
            &cert,
            &key,
            &[],
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn public_anchor_expiry_is_governed_by_its_metadata_flag() {
    let public = ProxyCa::generate("unrelated public root", 2).unwrap();
    let mut options = CertificateOptions::new("expired public anchor");
    options.ca = true;
    options.expired = true;
    let (cert, key) = configured_certificate(options);
    let root = ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap();
    let leaf = root
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    for local in [false, true] {
        for expiry in [false, true] {
            probe(
                &factory(&public, &cert, expiry, false, local),
                &leaf.certificate,
                &leaf.private_key,
                &[],
                !expiry,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn hostname_requires_a_subject_alternative_name() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let factory = factory(&public, public.certificate(), false, false, false);
    for san in [None, Some("localhost"), Some("wrong.test")] {
        let mut options = CertificateOptions::new("localhost");
        options.issuer = Some(&public);
        options.dns_name = san;
        let (cert, key) = configured_certificate(options);
        probe(&factory, &cert, &key, &[], san == Some("localhost")).await;
    }
}

#[tokio::test]
async fn issued_certificates_require_strong_certificate_signatures() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let trust = factory(&public, public.certificate(), false, false, false);
    for digest in [
        boring::hash::MessageDigest::sha1(),
        boring::hash::MessageDigest::sha256(),
    ] {
        let mut options = CertificateOptions::new("localhost");
        options.issuer = Some(&public);
        options.signing_digest = digest;
        let (cert, key) = configured_certificate(options);
        probe(
            &trust,
            &cert,
            &key,
            &[],
            digest == boring::hash::MessageDigest::sha256(),
        )
        .await;
    }
}

#[tokio::test]
async fn anchor_policy_constraints_enforce_policies_and_accept_valid_policy_paths() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let mut options = CertificateOptions::new("restricted anchor");
    options.ca = true;
    options.extensions.push(
        boring::x509::X509Extension::new_nid(
            None,
            None,
            boring::nid::Nid::POLICY_CONSTRAINTS,
            "critical,requireExplicitPolicy:0",
        )
        .unwrap(),
    );
    let (cert, key) = configured_certificate(options);
    let root = ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap();
    let leaf = root
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    for enforce in [false, true] {
        probe(
            &factory(&public, &cert, true, enforce, false),
            &leaf.certificate,
            &leaf.private_key,
            &[],
            !enforce,
        )
        .await;
    }
    let local = factory(&public, public.certificate(), false, false, true)
        .with_local_trust_monitor(LocalTrustMonitor::fixed(LocalCertificates {
            anchors: vec![cert.to_der().unwrap()],
            ..Default::default()
        }));
    probe(&local, &leaf.certificate, &leaf.private_key, &[], false).await;
    let mut options = CertificateOptions::new("localhost");
    options.issuer = Some(&root);
    options.extensions.push(
        boring::x509::X509Extension::new_nid(
            None,
            None,
            boring::nid::Nid::CERTIFICATE_POLICIES,
            "DER:30:07:30:05:06:03:2A:03:04",
        )
        .unwrap(),
    );
    let (valid_leaf, valid_key) = configured_certificate(options);
    probe(&local, &valid_leaf, &valid_key, &[], true).await;
    probe(
        &factory(&public, &cert, true, true, false),
        &valid_leaf,
        &valid_key,
        &[],
        true,
    )
    .await;
}

#[tokio::test]
async fn public_anchor_name_constraints_follow_the_metadata_flag() {
    let public = ProxyCa::generate("public root", 2).unwrap();
    let mut options = CertificateOptions::new("name constrained anchor");
    options.ca = true;
    options.extensions.push(
        boring::x509::X509Extension::new_nid(
            None,
            None,
            boring::nid::Nid::NAME_CONSTRAINTS,
            "critical,permitted;DNS:.allowed.test",
        )
        .unwrap(),
    );
    let (cert, key) = configured_certificate(options);
    let root = ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap();
    let leaf = root
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    for local in [false, true] {
        for enforce in [false, true] {
            probe(
                &factory(&public, &cert, true, enforce, local),
                &leaf.certificate,
                &leaf.private_key,
                &[],
                !enforce,
            )
            .await;
        }
    }
}

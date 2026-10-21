//! Local trust constraints apply to QUIC as well as TCP TLS verification.
#[path = "../../proxy-http/tests/support/trust_anchors.rs"]
mod anchors;
#[cfg(windows)]
#[path = "../../windows-trust/tests/support/ctl.rs"]
mod ctl_fixture;
#[path = "support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use boring::{
    pkey::{PKey, Private},
    ssl::{SslContext, SslMethod},
    x509::X509,
};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant, SystemTime},
};
use support::local_certificates::certificate;
use transmog_core::{
    VerificationResult, network_profile::NetworkProfileSnapshot, performance::PerformanceRecorder,
};
use transmog_h3::{H3OriginClient, H3TransportLimits};
use transmog_tls::{
    ProxyCa,
    local_trust::{LocalCertificates, LocalTrustMonitor},
};

fn config(cert: &X509, key: &PKey<Private>, chain: &[X509]) -> quiche::Config {
    let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
    tls.set_certificate(cert).unwrap();
    tls.set_private_key(key).unwrap();
    for certificate in chain {
        tls.add_extra_chain_cert(certificate.clone()).unwrap();
    }
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(1000);
    config.set_max_recv_udp_payload_size(1350);
    config.set_max_send_udp_payload_size(1350);
    config.set_initial_max_data(1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(256 * 1024);
    config.set_initial_max_stream_data_bidi_remote(256 * 1024);
    config.set_initial_max_stream_data_uni(256 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    config
}

async fn probe(
    public: &ProxyCa,
    inventory: LocalCertificates,
    cert: &X509,
    key: &PKey<Private>,
    chain: &[X509],
    expected: bool,
) {
    let factory = anchors::factory(public, anchors::catalog(&[(public.certificate(), 1)], 42))
        .with_network_profile(NetworkProfileSnapshot::default())
        .unwrap()
        .with_local_trust_monitor(LocalTrustMonitor::fixed(inventory));
    probe_factory(factory, cert, key, chain, expected).await;
}

async fn probe_factory(
    factory: transmog_tls::UpstreamTlsContextFactory,
    cert: &X509,
    key: &PKey<Private>,
    chain: &[X509],
    expected: bool,
) {
    let origin = quic::origin_configs(vec![config(cert, key, chain)], "127.0.0.1:0", true).await;
    let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
    let client = H3OriginClient::new(
        factory,
        H3TransportLimits {
            idle_timeout: Duration::from_secs(1),
            ..Default::default()
        },
    )
    .with_performance(Some(recorder.clone()));
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        client.execute(support::request(origin.port), origin.port, 1024),
    )
    .await
    .unwrap();
    assert_eq!(result.is_ok(), expected, "{result:?}");
    assert_eq!(
        origin.requests.load(Ordering::SeqCst),
        usize::from(expected)
    );
    let sample = recorder.snapshot();
    let tls = sample
        .transports
        .iter()
        .filter_map(|t| t.tls.as_ref())
        .find(|tls| tls.verification.is_some())
        .unwrap();
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
async fn quic_root_sources_keep_chromium_distrust_separate_from_full_os_distrust() {
    use transmog_core::network_profile::RootStore;
    let root = anchors::issuer_root("QUIC public distrust fixture");
    let issuer = anchors::intermediate(&root);
    let leaf = issuer
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    for roots in [
        RootStore::ChromiumBundled,
        RootStore::Os,
        RootStore::ChromeInstalled,
        RootStore::Webview2Installed,
    ] {
        let mut profile = NetworkProfileSnapshot::default();
        profile.settings.root_store = roots;
        profile.settings.local_certificates = false;
        let base = anchors::factory(&root, anchors::catalog(&[(root.certificate(), 1)], 42))
            .with_network_profile(profile)
            .unwrap()
            .with_local_trust_monitor(LocalTrustMonitor::fixed(LocalCertificates::default()));
        let chain = [issuer.certificate().clone()];
        probe_factory(
            base.clone(),
            &leaf.certificate,
            &leaf.private_key,
            &chain,
            true,
        )
        .await;
        for cert in [&leaf.certificate, issuer.certificate(), root.certificate()] {
            let der = cert.to_der().unwrap();
            let denied = base
                .clone()
                .with_local_trust_monitor(LocalTrustMonitor::fixed(LocalCertificates {
                    disallowed: vec![der.clone()],
                    ..Default::default()
                }));
            probe_factory(denied, &leaf.certificate, &leaf.private_key, &chain, false).await;
            #[cfg(windows)]
            {
                let denied = base
                    .clone()
                    .with_local_trust_monitor(LocalTrustMonitor::fixed(LocalCertificates {
                        disallowed_ctls: vec![ctl_fixture::ctl(
                            false,
                            &[
                                boring::hash::hash(boring::hash::MessageDigest::sha1(), &der)
                                    .unwrap()
                                    .to_vec(),
                            ],
                        )],
                        ..Default::default()
                    }));
                probe_factory(
                    denied,
                    &leaf.certificate,
                    &leaf.private_key,
                    &chain,
                    !matches!(roots, RootStore::Os | RootStore::ChromiumBundled),
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn quic_local_anchor_enforces_path_length() {
    let public = ProxyCa::generate("QUIC public", 2).unwrap();
    for limit in [0, 1] {
        let (cert, key) = certificate("limited QUIC root", true, false, None, Some(limit), None);
        let root = ProxyCa::from_pem(
            &cert.to_pem().unwrap(),
            &key.private_key_to_pem_pkcs8().unwrap(),
        )
        .unwrap();
        let first = anchors::intermediate(&root);
        let inventory = LocalCertificates {
            anchors: vec![cert.to_der().unwrap()],
            ..Default::default()
        };
        for (issuer, chain, expected) in [
            (&root, vec![], true),
            (&first, vec![first.certificate().clone()], limit > 0),
        ] {
            let leaf = issuer
                .issue(
                    transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
                    1,
                )
                .unwrap();
            probe(
                &public,
                inventory.clone(),
                &leaf.certificate,
                &leaf.private_key,
                &chain,
                expected,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn quic_non_self_signed_root_ca_only_trusts_issuer_paths() {
    let public = ProxyCa::generate("QUIC public", 2).unwrap();
    let unknown = ProxyCa::generate("untrusted QUIC issuer", 2).unwrap();
    let (cert, key) = certificate("localhost", true, false, Some(&unknown), None, None);
    let inventory = LocalCertificates {
        anchors: vec![cert.to_der().unwrap()],
        ..Default::default()
    };
    probe(&public, inventory.clone(), &cert, &key, &[], false).await;
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
    probe(
        &public,
        inventory,
        &leaf.certificate,
        &leaf.private_key,
        &[],
        true,
    )
    .await;
}

#[tokio::test]
async fn quic_exact_leaf_trust_uses_canonical_names_and_verifies_signature() {
    let public = ProxyCa::generate("QUIC public", 2).unwrap();
    let unknown = ProxyCa::generate("localhost", 2).unwrap();
    for signer in [None, Some(&unknown)] {
        let (cert, key) = certificate("LOCALHOST", false, false, signer, None, Some("localhost"));
        for root_role in [false, true] {
            let mut inventory = LocalCertificates::default();
            if root_role {
                inventory.anchors.push(cert.to_der().unwrap());
            } else {
                inventory.trusted_leaves.push(cert.to_der().unwrap());
            }
            probe(&public, inventory, &cert, &key, &[], signer.is_none()).await;
        }
    }
}

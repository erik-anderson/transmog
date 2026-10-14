//! The real QUIC driver must authenticate ECH rejection before a bounded retry.
#[path = "support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use boring::{
    hpke::HpkeKey,
    ssl::{SslContext, SslEchKeys, SslMethod},
};
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant, SystemTime},
};
use transmog_core::performance::PerformanceRecorder;
use transmog_h3::{H3OriginClient, H3TransportLimits};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory,
    UpstreamTlsPolicy,
    ech::{EchConfigSource, EchLookup, EchOffer},
};
#[derive(Debug)]
struct Fixed {
    config: Arc<[u8]>,
    port: u16,
}
impl EchConfigSource for Fixed {
    fn lookup(&self, _: String, port: u16, _: Vec<String>) -> EchLookup {
        assert_eq!(
            port, self.port,
            "ECH discovery must use the destination port"
        );
        let offer = EchOffer {
            config: self.config.clone(),
            endpoint: "127.0.0.1".into(),
            port: self.port,
        };
        Box::pin(async move { Some(offer) })
    }
}
fn configs(seed: u8) -> (Vec<u8>, SslEchKeys) {
    let key = HpkeKey::dhkem_p256_sha256(&[seed; 32]).unwrap();
    let config =
        transmog_tls_native::marshal_ech_config(&key, "public.example.test", seed).unwrap();
    let mut keys = SslEchKeys::builder().unwrap();
    keys.add_key(true, &config, key).unwrap();
    let mut list = u16::try_from(config.len()).unwrap().to_be_bytes().to_vec();
    list.extend_from_slice(&config);
    (list, keys.build())
}
fn server(ca: &ProxyCa, name: &str, keys: Option<&SslEchKeys>) -> quiche::Config {
    let leaf = ca.issue(EndpointIdentity::parse(name).unwrap(), 1).unwrap();
    let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
    tls.set_certificate(&leaf.certificate).unwrap();
    tls.set_private_key(&leaf.private_key).unwrap();
    if let Some(keys) = keys {
        tls.set_ech_keys(keys).unwrap();
    }
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(1000);
    config.set_initial_max_data(1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(256 * 1024);
    config.set_initial_max_stream_data_bidi_remote(256 * 1024);
    config.set_initial_max_stream_data_uni(256 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    config
}
#[tokio::test]
#[allow(clippy::too_many_lines)] // Acceptance, alternative-port routing and bounded retries share the real QUIC fixture.
async fn h3_ech_accepts_rotates_rolls_back_and_stops_after_an_authenticated_retry() {
    for scenario in [
        "accepted",
        "alternative-port",
        "rotation",
        "rollback",
        "untrusted",
        "repeat",
    ] {
        let direct_acceptance = matches!(scenario, "accepted" | "alternative-port");
        let fixture = support::Fixture::new();
        let wrong = ProxyCa::generate("Untrusted public name", 2).unwrap();
        let (initial, _) = configs(7);
        let (_, current) = configs(if direct_acceptance { 7 } else { 8 });
        let (_, newer) = configs(9);
        let first = server(
            if scenario == "untrusted" {
                &wrong
            } else {
                &fixture.ca
            },
            if direct_acceptance {
                "hidden.example.test"
            } else {
                "public.example.test"
            },
            (scenario != "rollback").then_some(&current),
        );
        let mut servers = vec![first];
        if !direct_acceptance {
            servers.push(server(
                &fixture.ca,
                if scenario == "repeat" {
                    "public.example.test"
                } else {
                    "hidden.example.test"
                },
                if scenario == "repeat" {
                    Some(&newer)
                } else {
                    (scenario != "rollback").then_some(&current)
                },
            ));
        }
        let origin = quic::origin_configs(servers, "127.0.0.1:0", true).await;
        let trust = PemTrustSource::from_pem(&fixture.ca.certificate_pem().unwrap(), "ECH fixture")
            .unwrap();
        let factory = UpstreamTlsContextFactory::new(
            Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
            UpstreamTlsPolicy::default(),
        )
        .with_ech_config_source(Arc::new(Fixed {
            config: initial.into(),
            port: origin.port,
        }));
        let recorder = PerformanceRecorder::new(SystemTime::now(), Instant::now());
        let client = H3OriginClient::new(factory, H3TransportLimits::default())
            .with_performance(Some(recorder.clone()));
        let mut request = support::request(origin.port);
        request.head.target.host = "hidden.example.test".into();
        request.head.target.authority = format!("hidden.example.test:{}", origin.port);
        if scenario == "alternative-port" {
            request.head.target.port = 443;
            request.head.target.authority = "hidden.example.test:443".into();
        }
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.execute(request, origin.port, 1024),
        )
        .await
        .unwrap();
        let accepted = !matches!(scenario, "untrusted" | "repeat");
        assert_eq!(
            result.is_ok(),
            accepted,
            "{scenario}: {:?}",
            result.as_ref().err().map(ToString::to_string)
        );
        assert_eq!(
            origin.requests.load(Ordering::SeqCst),
            usize::from(accepted),
            "HTTP reached the rejection connection"
        );
        assert_eq!(
            origin.count.load(Ordering::SeqCst),
            if direct_acceptance || scenario == "untrusted" {
                1
            } else {
                2
            },
            "{scenario}"
        );
        if accepted {
            assert!(
                recorder
                    .snapshot()
                    .transports
                    .iter()
                    .filter_map(|t| t.tls.as_ref())
                    .any(|tls| tls.details.get("Encrypted Client Hello").is_some_and(
                        |value| value
                            == if scenario == "rollback" {
                                "Not negotiated"
                            } else {
                                "Accepted"
                            }
                    ))
            );
        }
    }
}

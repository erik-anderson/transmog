//! Real authenticated ECH retries must reuse the connected peer in both adapters.
use super::*;
use boring::{
    hpke::HpkeKey,
    ssl::{SslAcceptor, SslEchKeys, SslMethod},
};
use std::{collections::VecDeque, time::Duration};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsPolicy,
    ech::{EchConfigSource, EchLookup, EchOffer},
};

#[derive(Debug)]
struct Fixed(EchOffer);
impl EchConfigSource for Fixed {
    fn lookup(&self, _: String, _: u16, _: Vec<String>) -> EchLookup {
        let offer = self.0.clone();
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

#[tokio::test]
#[allow(clippy::too_many_lines)] // Both adapters exercise the same actual rejection and DNS rotation.
async fn authenticated_retry_keeps_the_peer_without_resolving_again() {
    for measured in [false, true] {
        for scenario in ["rotation", "rollback", "repeated"] {
            let rollback = scenario == "rollback";
            let repeated = scenario == "repeated";
            let ca = ProxyCa::generate("ECH retry DNS fixture", 2).unwrap();
            let (initial, _) = configs(7);
            let (_, current) = configs(8);
            let (_, newer) = configs(9);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let peer = listener.local_addr().unwrap();
            let mut servers = Vec::new();
            for (phase, name) in [
                "public.example.test",
                if repeated {
                    "public.example.test"
                } else {
                    "hidden.example.test"
                },
            ]
            .into_iter()
            .enumerate()
            {
                let leaf = ca.issue(EndpointIdentity::parse(name).unwrap(), 1).unwrap();
                let mut tls = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
                tls.set_certificate(&leaf.certificate).unwrap();
                tls.set_private_key(&leaf.private_key).unwrap();
                if !rollback {
                    tls.set_ech_keys(if repeated && phase == 1 {
                        &newer
                    } else {
                        &current
                    })
                    .unwrap();
                }
                servers.push(tls.build());
            }
            let server = tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                for (phase, tls) in servers.into_iter().enumerate() {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut tls = tokio_boring::accept(&tls, socket).await.unwrap();
                    let mut request = [0; 4096];
                    let count = tls.read(&mut request).await;
                    if phase == 0 || repeated {
                        assert!(
                            count.is_err() || count.unwrap() == 0,
                            "HTTP reached the rejected ECH connection"
                        );
                    } else {
                        let count = count.unwrap();
                        let request = std::str::from_utf8(&request[..count]).unwrap();
                        assert!(request.contains("hidden.example.test"));
                        tls.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                    }
                }
            });
            let trust =
                PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "ECH retry fixture")
                    .unwrap();
            let factory = UpstreamTlsContextFactory::new(
                Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
                UpstreamTlsPolicy::default(),
            )
            .with_ech_config_source(Arc::new(Fixed(EchOffer {
                config: initial.into(),
                endpoint: "service.example.test".into(),
                port: peer.port(),
            })));
            let answers = Arc::new(Mutex::new(VecDeque::from([
                peer,
                // A fresh DNS lookup would reach an unavailable address.
                std::net::SocketAddr::from(([127, 0, 0, 2], peer.port())),
            ])));
            let happy = HappyEyeballsConfig::default();
            let mut resolver = HappyEyeballsResolver::new(happy);
            resolver.fixture = Some(answers.clone());
            let uri = format!("https://hidden.example.test:{}/", peer.port())
                .parse::<Uri>()
                .unwrap();
            let result = if measured {
                let connector = connector_with_resolver(
                    &factory,
                    HyperEgressMode::Http1Only,
                    happy,
                    OutboundProtocolPolicy::default(),
                    resolver,
                )
                .unwrap();
                let client = hyper_util::client::legacy::Client::builder(
                    hyper_util::rt::TokioExecutor::new(),
                )
                .build::<_, http_body_util::Empty<bytes::Bytes>>(connector);
                tokio::time::timeout(Duration::from_secs(5), client.get(uri))
                    .await
                    .unwrap()
            } else {
                let mut tcp = HttpConnector::new_with_resolver(resolver);
                tcp.enforce_http(false);
                let mut connector = ChromiumHttpsConnector::new(
                    tcp,
                    factory.tcp_context_builder(b"\x08http/1.1").unwrap(),
                    factory.clone(),
                    b"\x08http/1.1",
                )
                .unwrap();
                connector.set_ssl_callback(move |ssl, uri| {
                    factory
                        .configure_ssl_metadata_for_peer(ssl, uri.host().unwrap())
                        .map_err(|_| boring::error::ErrorStack::get())
                });
                let client = hyper_util::client::legacy::Client::builder(
                    hyper_util::rt::TokioExecutor::new(),
                )
                .build::<_, http_body_util::Empty<bytes::Bytes>>(connector);
                tokio::time::timeout(Duration::from_secs(5), client.get(uri))
                    .await
                    .unwrap()
            };
            assert_eq!(
                result.is_ok(),
                !repeated,
                "measured={measured}, scenario={scenario}: {result:?}"
            );
            assert_eq!(
                answers.lock().unwrap().len(),
                1,
                "ECH recovery queried DNS again"
            );
            tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

//! Live TLS integration verifies pool boundaries and pending handshake ownership.
#[path = "../../proxy-h3/tests/support/mod.rs"]
mod quic;
#[path = "../../proxy-http/tests/support/mod.rs"]
mod support;
use std::sync::atomic::AtomicUsize;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use transmog_core::client_authentication::{ClientCertificateDefault, ClientCertificatePolicy};
use transmog_runtime::{ClientIdentityResolver, ProxyComponents, ProxyConfig, ProxyServer};
use transmog_tls::{
    PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};
use transmog_tls::{
    client_authentication::PolicyClientAuthenticationResolver,
    client_prompts::ClientCertificateBroker,
};
struct Identity(AtomicUsize);
impl ClientIdentityResolver for Identity {
    fn resolve(
        &self,
        _: std::net::SocketAddr,
        _: std::net::SocketAddr,
    ) -> transmog_core::ClientIdentity {
        transmog_core::ClientIdentity::LocalProcess {
            pid: u32::try_from(self.0.fetch_add(1, Ordering::SeqCst)).unwrap() + 10,
            name: None,
        }
    }
}
struct Proxy {
    address: std::net::SocketAddr,
    trust: Arc<TrustSnapshot>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), transmog_runtime::ProxyRuntimeError>>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}
async fn proxy(
    f: &Fixture,
    resolver: Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>,
    h2: bool,
) -> Proxy {
    proxy_route(
        f,
        resolver,
        if h2 {
            transmog_core::RoutePolicy::Http2Only
        } else {
            transmog_core::RoutePolicy::Http1Only
        },
    )
    .await
}
async fn proxy_route(
    f: &Fixture,
    resolver: Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>,
    route: transmog_core::RoutePolicy,
) -> Proxy {
    proxy_route_identity(f, resolver, route, Arc::new(Identity(AtomicUsize::new(0)))).await
}
async fn proxy_route_identity(
    f: &Fixture,
    resolver: Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>,
    route: transmog_core::RoutePolicy,
    identity: Arc<dyn ClientIdentityResolver>,
) -> Proxy {
    proxy_route_config(
        f,
        resolver,
        route,
        identity,
        transmog_h3::H3TransportLimits::default(),
    )
    .await
}
async fn proxy_route_config(
    f: &Fixture,
    resolver: Arc<dyn transmog_tls::client_authentication::ClientAuthenticationResolver>,
    route: transmog_core::RoutePolicy,
    identity: Arc<dyn ClientIdentityResolver>,
    h3: transmog_h3::H3TransportLimits,
) -> Proxy {
    let ca = ProxyCa::generate("Intercepted certificate request", 2).unwrap();
    let roots = PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "browser trust").unwrap();
    let browser_trust = Arc::new(TrustSnapshot::load(&roots, 1).unwrap());
    let roots = PemTrustSource::from_pem(&f.ca.certificate_pem().unwrap(), "origin trust").unwrap();
    let trust = Arc::new(TrustSnapshot::load(&roots, 1).unwrap());
    let components = ProxyComponents::new(
        transmog_core::intercept::InterceptorChainFactory::new(
            vec![],
            transmog_core::intercept::HookLimits::default(),
        ),
        Arc::new(transmog_tls::CachedMitmCertificateResolver::new(ca, 32, 1).unwrap()),
    )
    .with_client_identity_resolver(identity)
    .with_client_certificate_resolver(resolver);
    let server = ProxyServer::bind_with_components(
        ProxyConfig {
            h3,
            isolate_outbound_connections: false,
            listener: transmog_runtime::ListenerConfig {
                listen_addr: "127.0.0.1:0".parse().unwrap(),
                allow_remote_clients: false,
            },
            route_policy: route,
            ..Default::default()
        },
        trust,
        components,
    )
    .await
    .unwrap();
    let address = server.local_addr().unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(async move {
        let _ = stopping.await;
    }));
    Proxy {
        address,
        trust: browser_trust,
        stop: Some(stop),
        task,
    }
}
async fn head<R: tokio::io::AsyncRead + Unpin>(stream: &mut R) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        bytes.push(stream.read_u8().await.unwrap());
        if bytes.ends_with(b"\r\n\r\n") {
            return bytes;
        }
        assert!(bytes.len() < 16384);
    }
}
async fn browser(proxy: &Proxy, port: u16) -> tokio_boring::SslStream<tokio::net::TcpStream> {
    let mut tcp = tokio::net::TcpStream::connect(proxy.address).await.unwrap();
    tcp.write_all(
        format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    assert!(head(&mut tcp).await.starts_with(b"HTTP/1.1 200"));
    let connector =
        UpstreamTlsContextFactory::new(proxy.trust.clone(), UpstreamTlsPolicy::default())
            .hyper_connector_builder(b"\x08http/1.1")
            .unwrap()
            .build();
    tokio_boring::connect(connector.configure().unwrap(), "localhost", tcp)
        .await
        .unwrap()
}
async fn get(browser: &mut tokio_boring::SslStream<tokio::net::TcpStream>, port: u16) -> String {
    browser
        .write_all(format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let head = head(browser).await;
    assert!(
        head.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    let length = String::from_utf8_lossy(&head)
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().unwrap())
        })
        .expect("retained origin content length");
    let mut bytes = vec![0; length];
    browser.read_exact(&mut bytes).await.unwrap();
    String::from_utf8(bytes).unwrap()
}
#[tokio::test]
async fn proxy_authentication_enforces_callers_even_when_general_isolation_is_disabled() {
    for h2 in [false, true] {
        let fixture = Fixture::new();
        let origin = origin(&fixture, h2, false, Duration::from_secs(5)).await;
        let resolver = Arc::new(
            PolicyClientAuthenticationResolver::new(
                automatic(origin.port),
                fixture.source.clone(),
                None,
            )
            .unwrap(),
        );
        let proxy = proxy(&fixture, resolver.clone(), h2).await;
        let mut first = browser(&proxy, origin.port).await;
        assert_eq!(get(&mut first, origin.port).await, "1:true");
        assert_eq!(get(&mut first, origin.port).await, "1:true");
        let mut second = browser(&proxy, origin.port).await;
        assert_eq!(get(&mut second, origin.port).await, "2:true");
        resolver
            .replace_policy(ClientCertificatePolicy::default())
            .unwrap();
        assert_eq!(get(&mut first, origin.port).await, "3:false");
        assert_eq!(origin.count.load(Ordering::SeqCst), 3);
        assert_eq!(origin.resumed.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.signatures.load(Ordering::SeqCst), 2);
    }
}
#[tokio::test]
async fn closing_the_browser_unregisters_the_proxys_pending_certificate_decision() {
    let fixture = Fixture::new();
    let origin = origin(&fixture, false, false, Duration::from_secs(5)).await;
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            ClientCertificatePolicy {
                default: ClientCertificateDefault::Prompt,
                ..Default::default()
            },
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let proxy = proxy(&fixture, resolver, false).await;
    let mut browser = browser(&proxy, origin.port).await;
    browser
        .write_all(format!("GET / HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", origin.port).as_bytes())
        .await
        .unwrap();
    wait_pending(&broker, 1).await;
    drop(browser);
    wait_pending(&broker, 0).await;
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelling_a_verified_quic_certificate_request_does_not_replay_over_tcp() {
    let fixture = Fixture::new();
    let udp = quic::origin(&fixture).await;
    let advertisement =
        http::HeaderValue::from_str(&format!("h3=\":{}\"; ma=60", udp.port)).unwrap();
    let tcp = origin_advertising(
        &fixture,
        false,
        false,
        Duration::from_secs(5),
        Some(advertisement),
    )
    .await;
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            ClientCertificatePolicy::default(),
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let proxy = proxy_route(&fixture, resolver.clone(), transmog_core::RoutePolicy::Auto).await;
    let mut caller = browser(&proxy, tcp.port).await;
    assert_eq!(get(&mut caller, tcp.port).await, "1:false");
    resolver
        .replace_policy(ClientCertificatePolicy {
            default: ClientCertificateDefault::Prompt,
            ..Default::default()
        })
        .unwrap();
    caller
        .write_all(format!("GET / HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", tcp.port).as_bytes())
        .await
        .unwrap();
    wait_pending(&broker, 1).await;
    assert_eq!(udp.count.load(Ordering::SeqCst), 1);
    assert_eq!(broker.pending()[0].context.port, tcp.port);
    assert!(broker.answer(
        broker.pending()[0].id,
        transmog_tls::client_prompts::ClientCertificateAnswer::Cancel
    ));
    let response = tokio::time::timeout(Duration::from_secs(2), head(&mut caller))
        .await
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 502"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    wait_pending(&broker, 0).await;
    assert_eq!(tcp.count.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dual_stack_cancellation_cannot_fall_back_to_tcp_without_a_certificate_request() {
    let fixture = Fixture::new();
    let udp = quic::origin_at(&fixture, "[::1]:0").await;
    let _blackhole = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, udp.port))
        .await
        .unwrap();
    let advertisement =
        http::HeaderValue::from_str(&format!("h3=\":{}\"; ma=60", udp.port)).unwrap();
    let tcp = origin_without_client_authentication(&fixture, advertisement).await;
    let broker = ClientCertificateBroker::default();
    let resolver = Arc::new(
        PolicyClientAuthenticationResolver::new(
            ClientCertificatePolicy::default(),
            fixture.source.clone(),
            Some(Arc::new(broker.clone())),
        )
        .unwrap(),
    );
    let proxy = proxy_route_config(
        &fixture,
        resolver.clone(),
        transmog_core::RoutePolicy::Auto,
        Arc::new(Identity(AtomicUsize::new(0))),
        transmog_h3::H3TransportLimits {
            idle_timeout: Duration::from_millis(500),
            ..Default::default()
        },
    )
    .await;
    let mut caller = browser(&proxy, tcp.port).await;
    assert_eq!(get(&mut caller, tcp.port).await, "1:false");
    resolver
        .replace_policy(ClientCertificatePolicy {
            default: ClientCertificateDefault::Prompt,
            ..Default::default()
        })
        .unwrap();
    caller
        .write_all(format!("GET / HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", tcp.port).as_bytes())
        .await
        .unwrap();
    wait_pending(&broker, 1).await;
    assert!(broker.answer(
        broker.pending()[0].id,
        transmog_tls::client_prompts::ClientCertificateAnswer::Cancel
    ));
    let response = tokio::time::timeout(Duration::from_secs(2), head(&mut caller))
        .await
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 502"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    wait_pending(&broker, 0).await;
    assert_eq!(
        tcp.count.load(Ordering::SeqCst),
        1,
        "cancelled exchange was retried over TCP"
    );
    assert_eq!(
        tcp.requests.load(Ordering::SeqCst),
        1,
        "cancelled HTTP request reached the TCP server"
    );
    assert_eq!(fixture.signatures.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn identical_or_unknown_process_attribution_cannot_share_certificate_authorization() {
    struct Attribution(bool);
    impl ClientIdentityResolver for Attribution {
        fn resolve(
            &self,
            _: std::net::SocketAddr,
            _: std::net::SocketAddr,
        ) -> transmog_core::ClientIdentity {
            if self.0 {
                transmog_core::ClientIdentity::LocalUnknown
            } else {
                transmog_core::ClientIdentity::LocalProcess {
                    pid: 10,
                    name: None,
                }
            }
        }
    }
    for unknown in [false, true] {
        for h2 in [false, true] {
            let fixture = Fixture::new();
            let origin = origin(&fixture, h2, false, Duration::from_secs(5)).await;
            let resolver = Arc::new(
                PolicyClientAuthenticationResolver::new(
                    automatic(origin.port),
                    fixture.source.clone(),
                    None,
                )
                .unwrap(),
            );
            let proxy = proxy_route_identity(
                &fixture,
                resolver,
                if h2 {
                    transmog_core::RoutePolicy::Http2Only
                } else {
                    transmog_core::RoutePolicy::Http1Only
                },
                Arc::new(Attribution(unknown)),
            )
            .await;
            let mut first = browser(&proxy, origin.port).await;
            let mut second = browser(&proxy, origin.port).await;
            assert_eq!(get(&mut first, origin.port).await, "1:true");
            assert_eq!(get(&mut first, origin.port).await, "1:true");
            assert_eq!(get(&mut second, origin.port).await, "2:true");
            assert_eq!(fixture.signatures.load(Ordering::SeqCst), 2);
        }
    }
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "creates owned non-exportable CurrentUser/My keys"]
async fn intercepted_proxy_uses_real_windows_keys_on_h1_h2_and_h3_and_recovers_after_removal() {
    use transmog_tls::{
        client_authentication::{ClientAuthenticationResolver, ClientCredentialSource},
        windows_client_certificates::{
            WindowsClientCertificateController, WindowsCurrentUserCertificateSource,
        },
    };
    let owned = support::windows::WindowsFixture::new("RSA");
    let credential = WindowsCurrentUserCertificateSource
        .credentials()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == owned.id)
        .unwrap();
    let mut fixture = Fixture::new();
    fixture.source = Arc::new(support::Source {
        credential,
        reads: Arc::new(AtomicUsize::new(0)),
    });
    let h1 = origin(&fixture, false, false, Duration::from_secs(5)).await;
    let h2 = origin(&fixture, true, false, Duration::from_secs(5)).await;
    let h3 = quic::origin(&fixture).await;
    let mut policy = ClientCertificatePolicy::default();
    for port in [h1.port, h2.port, h3.port] {
        let mut rule = automatic(port).rules.remove(0);
        rule.id = port.to_string();
        let transmog_core::client_authentication::ClientCertificateAction::Automatic { selector } =
            &mut rule.mode
        else {
            unreachable!()
        };
        selector.identity = Some(owned.id.clone());
        policy.rules.push(rule);
    }
    let controller = WindowsClientCertificateController::new(policy)
        .await
        .unwrap();
    let mut proxies = Vec::new();
    for (route, port) in [
        (transmog_core::RoutePolicy::Http1Only, h1.port),
        (transmog_core::RoutePolicy::Http2Only, h2.port),
        (transmog_core::RoutePolicy::Http3Only, h3.port),
    ] {
        let proxy = proxy_route(&fixture, controller.resolver(), route).await;
        let mut caller = browser(&proxy, port).await;
        assert_eq!(get(&mut caller, port).await, "1:true");
        assert_eq!(get(&mut caller, port).await, "1:true");
        proxies.push((proxy, caller, port));
    }
    let generation = controller.resolver().generation();
    owned.change("DeleteKey");
    tokio::time::timeout(Duration::from_secs(7), async {
        while controller.resolver().generation() == generation {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    for (_, caller, port) in &mut proxies {
        assert_eq!(get(caller, *port).await, "2:false");
        assert_eq!(get(caller, *port).await, "3:false");
    }
    assert_eq!(h1.resumed.load(Ordering::SeqCst), 0);
    assert_eq!(h2.resumed.load(Ordering::SeqCst), 0);
}

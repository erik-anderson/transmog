//! Loopback origins and the production proxy for the local browser comparison.
#[path = "fingerprint/common.rs"]
mod common;
#[path = "fingerprint/quic.rs"]
mod quic;
#[path = "fingerprint/tcp.rs"]
mod tcp;

use common::{Error, Shared};
use std::{
    io::Read,
    path::PathBuf,
    sync::{Arc, atomic::AtomicUsize},
};
use transmog_core::{
    RoutePolicy,
    intercept::{HookLimits, InterceptorChainFactory},
    network_profile::{NetworkProfileSnapshot, RootStore},
    observe::{BoxObserverFuture, Observer, ObserverConfig, ObserverEvent, ObserverEventKind},
    route::{DestinationAuthorizer, PolicyRouteSelector, RouteError, UpstreamDestination},
};
use transmog_runtime::{ProxyComponents, ProxyConfig, ProxyServer};
use transmog_tls::{
    CachedMitmCertificateResolver, EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot,
};

struct FixtureOnly(u16);
struct Failures;
impl Observer for Failures {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        if let ObserverEventKind::Failed(failure) = event.kind {
            eprintln!("proxy exchange: {failure:?}");
        }
        Box::pin(async { Ok(()) })
    }
}
impl DestinationAuthorizer for FixtureOnly {
    fn authorize(
        &self,
        _: &transmog_core::intercept::OriginalTarget,
        destination: &UpstreamDestination,
    ) -> Result<(), RouteError> {
        if destination.scheme == "https"
            && destination.host == "localhost"
            && destination.port == self.0
        {
            Ok(())
        } else {
            Err(RouteError::Unauthorized)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() != 3 || !matches!(arguments[0].as_str(), "h1" | "h2" | "h3") {
        return Err("expected h1|h2|h3 OUTPUT_DIRECTORY PROOF_TOKEN".into());
    }
    let protocol = arguments[0].clone();
    let directory = PathBuf::from(&arguments[1]);
    std::fs::create_dir_all(&directory)?;
    let origin_ca = ProxyCa::generate("Local fingerprint origin CA", 1)?;
    let proxy_ca = ProxyCa::generate("Local fingerprint proxy CA", 1)?;
    std::fs::write(
        directory.join("origin-ca.pem"),
        origin_ca.certificate_pem()?,
    )?;
    std::fs::write(directory.join("proxy-ca.pem"), proxy_ca.certificate_pem()?)?;
    let leaf = origin_ca.issue(EndpointIdentity::parse("localhost")?, 1)?;
    let shared = Arc::new(Shared {
        directory,
        proof: arguments[2].clone(),
        next: AtomicUsize::default(),
    });
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = tcp.local_addr()?;
    let udp = tokio::net::UdpSocket::bind(address).await?;
    let origin = shared.clone();
    let tcp_leaf = leaf.clone();
    let mode = protocol.clone();
    tokio::spawn(async move { tcp::serve(tcp, tcp_leaf, mode, origin).await });
    let origin = shared.clone();
    tokio::spawn(async move { quic::serve(udp, leaf, origin).await });
    // Only fixture trust differs from the recipe: do not install a root or
    // borrow the host's public/local roots. ID offers remain visible in reports.
    let mut profile = NetworkProfileSnapshot {
        id: "custom-local-fingerprint".into(),
        name: "Chromium transport with isolated fixture trust".into(),
        ..NetworkProfileSnapshot::default()
    };
    profile.settings.root_store = RootStore::Os;
    profile.settings.local_certificates = false;
    profile.settings.protocols.http2 = protocol != "h1";
    profile.settings.protocols.http3 = protocol == "h3";
    let route = if protocol == "h3" {
        RoutePolicy::Http3Only
    } else {
        RoutePolicy::Auto
    };
    let trust =
        PemTrustSource::from_pem(&origin_ca.certificate_pem()?, "isolated fingerprint origin")?;
    let trust = Arc::new(TrustSnapshot::load(&trust, 1)?);
    let resolver = Arc::new(CachedMitmCertificateResolver::new(proxy_ca, 8, 1)?);
    let components = ProxyComponents::new(
        InterceptorChainFactory::new(vec![], HookLimits::default()),
        resolver,
    )
    .with_observer(Arc::new(Failures), ObserverConfig::default())
    .with_route_selector(Arc::new(PolicyRouteSelector::new(
        Arc::new(FixtureOnly(address.port())),
        route,
        1,
        "system-trust-v1",
        "direct",
        "fixture",
    )));
    let config = ProxyConfig {
        route_policy: route,
        network_profile: Some(profile.clone()),
        ..Default::default()
    };
    let proxy = ProxyServer::bind_with_components(config, trust, components).await?;
    let mut events = proxy.subscribe_evidence();
    let sink = shared.clone();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if event.target_path == "/probe" {
                let result = serde_json::json!({"ingress":format!("{:?}",event.ingress_version), "egress":format!("{:?}",event.egress_version), "host":event.target_host, "path":event.target_path});
                let temporary = sink.directory.join("proxy-evidence.tmp");
                let saved = std::fs::write(&temporary, result.to_string()).and_then(|()| {
                    std::fs::rename(temporary, sink.directory.join("proxy-evidence.json"))
                });
                if let Err(error) = saved {
                    eprintln!("evidence: {error}");
                }
            }
        }
    });
    println!(
        "READY={}",
        serde_json::json!({"origin":address.to_string(), "proxy":proxy.local_addr()?.to_string(), "profile":profile})
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read(&mut [0]);
        let _ = stop.send(());
    });
    proxy
        .serve(async {
            let _ = stopped.await;
        })
        .await?;
    Ok(())
}

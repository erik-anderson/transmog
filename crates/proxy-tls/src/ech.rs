//! Bounded HTTPS service-binding discovery for real Encrypted `ClientHello`.
use crate::UpstreamTlsContextFactory;
use hickory_resolver::{
    TokioResolver,
    proto::rr::{
        RData, RecordType,
        rdata::svcb::{SVCB, SvcParamKey, SvcParamValue},
    },
};
use std::{fmt, future::Future, pin::Pin, sync::Arc, time::Duration};

/// One endpoint's public ECH configuration from an HTTPS service binding.
#[derive(Clone, Debug)]
pub struct EchOffer {
    /// Serialized native `ECHConfigList`, never a private key.
    pub config: Arc<[u8]>,
    /// Service endpoint hostname; the original origin remains SNI inside ECH.
    pub endpoint: String,
    /// Service endpoint port.
    pub port: u16,
}
/// Owned asynchronous service discovery result.
pub type EchLookup = Pin<Box<dyn Future<Output = Option<EchOffer>> + Send>>;
/// Embedding DNS source. Returned service bindings never confer certificate trust.
pub trait EchConfigSource: fmt::Debug + Send + Sync {
    /// Resolves a service binding compatible with the requested protocols.
    fn lookup(&self, host: String, port: u16, alpn: Vec<String>) -> EchLookup;
}
#[derive(Debug, Default)]
pub(crate) struct SystemEchSource {
    resolver: Arc<tokio::sync::Mutex<Option<SystemResolver>>>,
}
#[derive(Debug)]
struct SystemResolver {
    resolver: TokioResolver,
    fingerprint: String,
    checked: std::time::Instant,
}
impl EchConfigSource for SystemEchSource {
    fn lookup(&self, host: String, port: u16, alpn: Vec<String>) -> EchLookup {
        let cell = self.resolver.clone();
        Box::pin(async move {
            if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
                return None;
            }
            let resolver = {
                let mut state = cell.lock().await;
                if state
                    .as_ref()
                    .is_none_or(|state| state.checked.elapsed() >= Duration::from_secs(5))
                {
                    let (config, mut options) = tokio::task::spawn_blocking(
                        hickory_resolver::system_conf::read_system_conf,
                    )
                    .await
                    .ok()?
                    .ok()?;
                    let fingerprint = format!("{config:?}");
                    if state
                        .as_ref()
                        .is_none_or(|state| state.fingerprint != fingerprint)
                    {
                        options.timeout = Duration::from_secs(2);
                        options.attempts = 1;
                        options.cache_size = 1024;
                        options.edns0 = true;
                        let mut builder = TokioResolver::builder_with_config(
                            config,
                            hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
                        );
                        *builder.options_mut() = options;
                        *state = Some(SystemResolver {
                            resolver: builder.build().ok()?,
                            fingerprint,
                            checked: std::time::Instant::now(),
                        });
                    } else if let Some(state) = &mut *state {
                        state.checked = std::time::Instant::now();
                    }
                }
                state.as_ref()?.resolver.clone()
            };
            lookup(&resolver, &host, port, &alpn).await
        })
    }
}
async fn lookup(
    resolver: &TokioResolver,
    host: &str,
    port: u16,
    alpn: &[String],
) -> Option<EchOffer> {
    let mut owner = if port == 443 {
        format!("{}.", host.trim_end_matches('.'))
    } else {
        format!("_{port}._https.{}.", host.trim_end_matches('.'))
    };
    for _ in 0..8 {
        let records = resolver
            .lookup(owner.clone(), RecordType::HTTPS)
            .await
            .ok()?;
        let services = records
            .answers()
            .iter()
            .filter_map(|record| match &record.data {
                RData::HTTPS(service) => Some((&record.name, &service.0)),
                _ => None,
            })
            .take(64)
            .collect::<Vec<_>>();
        if let Some((_, alias)) = services
            .iter()
            .find(|(_, service)| service.svc_priority == 0)
        {
            if alias.target_name.is_root() {
                return None;
            }
            owner = alias.target_name.to_utf8();
            continue;
        }
        let mut services = services;
        services.sort_by_key(|(_, service)| service.svc_priority);
        for (record_owner, service) in services {
            if let Some(offer) = select(service, &record_owner.to_utf8(), port, alpn) {
                return Some(offer);
            }
        }
        return None;
    }
    None
}
fn select(service: &SVCB, owner: &str, port: u16, offered: &[String]) -> Option<EchOffer> {
    let params = &service.svc_params;
    if let Some((_, SvcParamValue::Mandatory(keys))) = params
        .iter()
        .find(|(key, _)| *key == SvcParamKey::Mandatory)
        && keys.0.iter().any(|key| {
            !matches!(
                key,
                SvcParamKey::Alpn
                    | SvcParamKey::NoDefaultAlpn
                    | SvcParamKey::Port
                    | SvcParamKey::Ipv4Hint
                    | SvcParamKey::Ipv6Hint
                    | SvcParamKey::EchConfigList
            ) || !params.iter().any(|(present, _)| present == key)
        })
    {
        return None;
    }
    let alpn = params.iter().find_map(|(_, value)| {
        if let SvcParamValue::Alpn(alpn) = value {
            Some(&alpn.0)
        } else {
            None
        }
    });
    let no_default = params
        .iter()
        .any(|(key, _)| *key == SvcParamKey::NoDefaultAlpn);
    let compatible = alpn
        .is_some_and(|protocols| protocols.iter().any(|protocol| offered.contains(protocol)))
        || (!no_default && offered.iter().any(|protocol| protocol == "http/1.1"));
    if !compatible || (no_default && alpn.is_none()) {
        return None;
    }
    let config = params.iter().find_map(|(_, value)| {
        if let SvcParamValue::EchConfigList(config) = value {
            Some(&config.0)
        } else {
            None
        }
    })?;
    if config.len() < 2
        || config.len() > 65535
        || usize::from(u16::from_be_bytes([config[0], config[1]])) != config.len() - 2
    {
        return None;
    }
    let port = params
        .iter()
        .find_map(|(_, value)| {
            if let SvcParamValue::Port(port) = value {
                Some(*port)
            } else {
                None
            }
        })
        .unwrap_or(port);
    if port == 0 {
        return None;
    }
    let endpoint = if service.target_name.is_root() {
        owner.trim_end_matches('.').to_owned()
    } else {
        service
            .target_name
            .to_utf8()
            .trim_end_matches('.')
            .to_owned()
    };
    Some(EchOffer {
        config: config.as_slice().into(),
        endpoint,
        port,
    })
}
impl UpstreamTlsContextFactory {
    /// Replaces HTTPS discovery in a new factory/pool partition.
    #[must_use]
    pub fn with_ech_config_source(mut self, source: Arc<dyn EchConfigSource>) -> Self {
        self.ech_source = source;
        self
    }
    /// Discovers a bounded ECH offer when the profile and TLS version permit it.
    pub async fn ech_offer(&self, host: &str, port: u16, alpn: Vec<String>) -> Option<EchOffer> {
        if let Some(offer) = &self.ech_retry {
            return Some(offer.clone());
        }
        if self.policy.maximum_version != boring::ssl::SslVersion::TLS1_3
            || self
                .network_profile()
                .is_some_and(|p| !p.settings.encrypted_client_hello)
        {
            return None;
        }
        tokio::time::timeout(
            Duration::from_secs(3),
            self.ech_source.lookup(host.to_owned(), port, alpn),
        )
        .await
        .ok()
        .flatten()
    }
    /// Creates exactly one pre-HTTP retry from an authenticated ECH rejection.
    pub fn ech_recovery(&self, capture: &crate::TlsCapture) -> Option<Self> {
        if self.ech_retry.is_some() {
            return None;
        }
        let config = capture.ech_retry()?;
        let evidence = capture.snapshot();
        let endpoint = evidence.details.get("ECH service endpoint")?.clone();
        let port = evidence.details.get("ECH service port")?.parse().ok()?;
        let mut retry = self.clone();
        retry.ech_retry = Some(EchOffer {
            config: config.into(),
            endpoint,
            port,
        });
        Some(retry)
    }
}

/// Records the selected service endpoint in the existing connection disclosure.
pub fn record_ech_service(ssl: &boring::ssl::SslRef, offer: &EchOffer) {
    if let Some(capture) = crate::evidence::capture_for(ssl) {
        capture.authentication_detail("ECH service endpoint", &offer.endpoint);
        capture.authentication_detail("ECH service port", &offer.port.to_string());
        if offer.config.is_empty() {
            capture.authentication_detail(
                "ECH retry",
                "Authenticated server rollback; retry without ECH",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::proto::rr::{
        Name,
        rdata::svcb::{Alpn, EchConfigList, Mandatory},
    };

    struct DnsFixture {
        resolver: TokioResolver,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for DnsFixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn dns_fixture(cname: bool) -> DnsFixture {
        use hickory_resolver::{
            config::{NameServerConfig, ResolverConfig},
            proto::{
                op::{Message, OpCode},
                rr::{
                    Record,
                    rdata::{CNAME, HTTPS},
                },
            },
        };
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut buffer = vec![0; 65535];
            loop {
                let (count, peer) = socket.recv_from(&mut buffer).await.unwrap();
                let request = Message::from_vec(&buffer[..count]).unwrap();
                let mut response = Message::response(request.metadata.id, OpCode::Query);
                let query = request.queries[0].clone();
                response.add_query(query.clone());
                let owner = if cname {
                    Name::from_ascii("canonical.example.test.").unwrap()
                } else {
                    query.name().clone()
                };
                if cname {
                    response.add_answer(Record::from_rdata(
                        query.name().clone(),
                        60,
                        RData::CNAME(CNAME(owner.clone())),
                    ));
                }
                let service = SVCB::new(
                    1,
                    Name::root(),
                    vec![
                        (
                            SvcParamKey::Alpn,
                            SvcParamValue::Alpn(Alpn(vec!["h2".into()])),
                        ),
                        (SvcParamKey::NoDefaultAlpn, SvcParamValue::NoDefaultAlpn),
                        (
                            SvcParamKey::EchConfigList,
                            SvcParamValue::EchConfigList(EchConfigList(vec![0, 2, 0, 1])),
                        ),
                    ],
                );
                response.add_answer(Record::from_rdata(owner, 60, RData::HTTPS(HTTPS(service))));
                socket
                    .send_to(&response.to_vec().unwrap(), peer)
                    .await
                    .unwrap();
            }
        });
        let mut server = NameServerConfig::udp(address.ip());
        server.connections[0].port = address.port();
        let resolver = TokioResolver::builder_with_config(
            ResolverConfig::from_name_servers(vec![server]),
            hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
        )
        .build()
        .unwrap();
        DnsFixture { resolver, task }
    }
    #[tokio::test]
    async fn dot_targets_use_the_resolved_record_owner_after_cname() {
        let dns = dns_fixture(true).await;
        for port in [443, 8443] {
            let offer = lookup(&dns.resolver, "origin.example.test", port, &["h2".into()])
                .await
                .unwrap();
            assert_eq!(offer.endpoint, "canonical.example.test");
            assert_eq!(offer.port, port);
        }
    }
    #[tokio::test]
    async fn dot_targets_keep_the_port_prefixed_record_owner() {
        let dns = dns_fixture(false).await;
        let offer = lookup(&dns.resolver, "origin.example.test", 8443, &["h2".into()])
            .await
            .unwrap();
        assert_eq!(offer.endpoint, "_8443._https.origin.example.test");
    }
    #[test]
    fn service_bindings_require_compatible_alpn_and_complete_mandatory_keys() {
        let mut service = SVCB::new(
            1,
            Name::root(),
            vec![
                (
                    SvcParamKey::Alpn,
                    SvcParamValue::Alpn(Alpn(vec!["h3".into()])),
                ),
                (SvcParamKey::NoDefaultAlpn, SvcParamValue::NoDefaultAlpn),
                (
                    SvcParamKey::EchConfigList,
                    SvcParamValue::EchConfigList(EchConfigList(vec![0, 2, 0, 1])),
                ),
            ],
        );
        assert!(select(&service, "example.com.", 443, &["h2".into()]).is_none());
        assert_eq!(
            select(&service, "example.com.", 443, &["h3".into()])
                .unwrap()
                .endpoint,
            "example.com"
        );
        service.svc_params.push((
            SvcParamKey::Mandatory,
            SvcParamValue::Mandatory(Mandatory(vec![SvcParamKey::Port])),
        ));
        assert!(select(&service, "example.com.", 443, &["h3".into()]).is_none());
        service
            .svc_params
            .push((SvcParamKey::Port, SvcParamValue::Port(8443)));
        assert_eq!(
            select(&service, "example.com.", 443, &["h3".into()])
                .unwrap()
                .port,
            8443
        );
        // Malformed duplicate/out-of-order parameters are rejected by the DNS parser.
        service
            .svc_params
            .retain(|(key, _)| *key != SvcParamKey::Mandatory);
        service.svc_params.push((
            SvcParamKey::Mandatory,
            SvcParamValue::Mandatory(Mandatory(vec![SvcParamKey::Unknown(42)])),
        ));
        assert!(select(&service, "example.com.", 443, &["h3".into()]).is_none());
    }
}

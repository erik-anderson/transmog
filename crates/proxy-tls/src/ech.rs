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
/// Owned ordered alternatives, with each endpoint's own ECH configuration.
pub type EchCandidatesLookup = Pin<Box<dyn Future<Output = Vec<EchOffer>> + Send>>;
/// Embedding DNS source. Returned service bindings never confer certificate trust.
pub trait EchConfigSource: fmt::Debug + Send + Sync {
    /// Resolves a service binding compatible with the requested protocols.
    fn lookup(&self, host: String, port: u16, alpn: Vec<String>) -> EchLookup;
    /// Resolves alternatives in preferred order. Existing embedding sources may
    /// continue implementing only the single-endpoint method.
    fn lookup_candidates(&self, host: String, port: u16, alpn: Vec<String>) -> EchCandidatesLookup {
        let lookup = self.lookup(host, port, alpn);
        Box::pin(async move { lookup.await.into_iter().collect() })
    }
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
        let lookup = self.lookup_candidates(host, port, alpn);
        Box::pin(async move { lookup.await.into_iter().next() })
    }
    fn lookup_candidates(&self, host: String, port: u16, alpn: Vec<String>) -> EchCandidatesLookup {
        let cell = self.resolver.clone();
        Box::pin(async move {
            if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
                return Vec::new();
            }
            let resolver = async {
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
                Some(state.as_ref()?.resolver.clone())
            }
            .await;
            let Some(resolver) = resolver else {
                return Vec::new();
            };
            lookup(&resolver, &host, port, &alpn).await
        })
    }
}
async fn lookup(resolver: &TokioResolver, host: &str, port: u16, alpn: &[String]) -> Vec<EchOffer> {
    let owner = if port == 443 {
        format!("{}.", host.trim_end_matches('.'))
    } else {
        format!("_{port}._https.{}.", host.trim_end_matches('.'))
    };
    let Ok(records) = resolver.lookup(owner, RecordType::HTTPS).await else {
        return Vec::new();
    };
    let services = records
        .answers()
        .iter()
        .filter_map(|record| match &record.data {
            RData::HTTPS(service) => Some((&record.name, &service.0)),
            _ => None,
        })
        .take(65)
        .collect::<Vec<_>>();
    if services.len() > 64
        || services
            .iter()
            .any(|(_, service)| service.svc_priority == 0)
    {
        // AliasMode takes precedence, but the pinned Chromium resolver does
        // not support following it. CNAME resolution is independent.
        return Vec::new();
    }
    let mut services = services
        .into_iter()
        .filter_map(|(owner, service)| {
            eligible_endpoint(service, &owner.to_utf8(), host, port)
                .map(|endpoint| (service, endpoint))
        })
        .collect::<Vec<_>>();
    if !services
        .iter()
        .any(|(service, _)| allows_default_alpn(service))
    {
        return Vec::new();
    }
    services.sort_by_key(|(service, _)| service.svc_priority);
    services
        .into_iter()
        .filter_map(|(service, endpoint)| select_offer(service, endpoint, port, alpn))
        .collect()
}
fn allows_default_alpn(service: &SVCB) -> bool {
    !service
        .svc_params
        .iter()
        .any(|(key, _)| *key == SvcParamKey::NoDefaultAlpn)
}
fn eligible_endpoint(service: &SVCB, owner: &str, host: &str, port: u16) -> Option<String> {
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
    if !allows_default_alpn(service) && alpn.is_none_or(Vec::is_empty) {
        return None;
    }
    let service_port = params
        .iter()
        .find_map(|(_, value)| {
            if let SvcParamValue::Port(port) = value {
                Some(*port)
            } else {
                None
            }
        })
        .unwrap_or(port);
    // The pinned Chromium recipe ignores port redirects in HTTPS records.
    // Alt-Svc still supplies an explicit destination port to this lookup.
    if port == 0 || service_port != port {
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
    if !endpoint.eq_ignore_ascii_case(host.trim_end_matches('.'))
        && !endpoint.eq_ignore_ascii_case(owner.trim_end_matches('.'))
    {
        return None;
    }
    Some(endpoint)
}
fn select_offer(
    service: &SVCB,
    endpoint: String,
    port: u16,
    offered: &[String],
) -> Option<EchOffer> {
    let compatible = service.svc_params.iter().any(|(_, value)| {
        matches!(value, SvcParamValue::Alpn(alpn) if alpn.0.iter().any(|protocol| offered.contains(protocol)))
    }) || (allows_default_alpn(service) && offered.iter().any(|p| p == "http/1.1"));
    if !compatible {
        return None;
    }
    let config = service.svc_params.iter().find_map(|(_, value)| {
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
        self.ech_offers(host, port, alpn).await.into_iter().next()
    }
    /// Discovers up to 64 ordered alternatives within the one discovery deadline.
    /// Once ECH is selected, transport fallback retains ECH on every alternative.
    pub async fn ech_offers(&self, host: &str, port: u16, alpn: Vec<String>) -> Vec<EchOffer> {
        if let Some(offer) = &self.ech_retry {
            return vec![offer.clone()];
        }
        if self.policy.maximum_version != boring::ssl::SslVersion::TLS1_3
            || self
                .network_profile()
                .is_some_and(|p| !p.settings.encrypted_client_hello)
        {
            return Vec::new();
        }
        tokio::time::timeout(
            Duration::from_secs(3),
            self.ech_source
                .lookup_candidates(host.to_owned(), port, alpn),
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .take(64)
        .filter(|offer| port != 0 && offer.port == port)
        .collect()
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
        Name, Record,
        rdata::svcb::{Alpn, EchConfigList, Mandatory},
    };

    fn select(service: &SVCB, owner: &str, port: u16, offered: &[String]) -> Option<EchOffer> {
        select_offer(
            service,
            eligible_endpoint(service, owner, owner, port)?,
            port,
            offered,
        )
    }

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
        dns_answers(move |query| {
            use hickory_resolver::proto::rr::rdata::{CNAME, HTTPS};
            let owner = if cname {
                Name::from_ascii("canonical.example.test.").unwrap()
            } else {
                query.name().clone()
            };
            let mut records = Vec::new();
            if cname {
                records.push(Record::from_rdata(
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
                    (
                        SvcParamKey::EchConfigList,
                        SvcParamValue::EchConfigList(EchConfigList(vec![0, 2, 0, 1])),
                    ),
                ],
            );
            records.push(Record::from_rdata(owner, 60, RData::HTTPS(HTTPS(service))));
            records
        })
        .await
    }
    async fn dns_answers(
        answers: impl Fn(&hickory_resolver::proto::op::Query) -> Vec<Record> + Send + 'static,
    ) -> DnsFixture {
        use hickory_resolver::{
            config::{NameServerConfig, ResolverConfig},
            proto::op::{Message, OpCode},
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
                for record in answers(&query) {
                    response.add_answer(record);
                }
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
    async fn discovery_retains_priority_order_and_each_endpoints_config() {
        use hickory_resolver::proto::rr::rdata::HTTPS;
        let dns = dns_answers(|query| {
            [
                (3, ".", 443),
                (1, "ORIGIN.example.test.", 443),
                (2, ".", 8443),
                (2, "origin.example.test.", 443),
            ]
            .into_iter()
            .map(|(priority, endpoint, port)| {
                let service = SVCB::new(
                    priority,
                    Name::from_ascii(endpoint).unwrap(),
                    vec![
                        (
                            SvcParamKey::Alpn,
                            SvcParamValue::Alpn(Alpn(vec!["h2".into()])),
                        ),
                        (SvcParamKey::Port, SvcParamValue::Port(port)),
                        (
                            SvcParamKey::EchConfigList,
                            SvcParamValue::EchConfigList(EchConfigList(vec![
                                0,
                                2,
                                0,
                                u8::try_from(priority).unwrap(),
                            ])),
                        ),
                    ],
                );
                Record::from_rdata(query.name().clone(), 60, RData::HTTPS(HTTPS(service)))
            })
            .collect()
        })
        .await;
        let offers = lookup(&dns.resolver, "origin.example.test", 443, &["h2".into()]).await;
        assert_eq!(
            offers
                .iter()
                .map(|o| o.endpoint.as_str())
                .collect::<Vec<_>>(),
            [
                "ORIGIN.example.test",
                "origin.example.test",
                "origin.example.test"
            ]
        );
        assert_eq!(
            offers.iter().map(|o| o.config[3]).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(offers.iter().all(|o| o.port == 443));
    }
    #[tokio::test]
    async fn chromium_eligibility_precedes_ech_and_caller_alpn_filtering() {
        use hickory_resolver::proto::rr::rdata::HTTPS;
        for (alias, default_target, default_port, mandatory, expected) in [
            (false, "origin.example.test.", 443, false, true),
            (true, "origin.example.test.", 443, false, false),
            (false, "unrelated.example.test.", 443, false, false),
            (false, "origin.example.test.", 8443, false, false),
            (false, "origin.example.test.", 443, true, false),
        ] {
            let dns = dns_answers(move |query| {
                let mut defaults = vec![(
                    SvcParamKey::Alpn,
                    SvcParamValue::Alpn(Alpn(vec!["h3".into()])),
                )];
                if mandatory {
                    defaults.insert(
                        0,
                        (
                            SvcParamKey::Mandatory,
                            SvcParamValue::Mandatory(Mandatory(vec![SvcParamKey::Unknown(42)])),
                        ),
                    );
                }
                defaults.push((SvcParamKey::Port, SvcParamValue::Port(default_port)));
                let mut records = vec![
                    SVCB::new(
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
                    ),
                    // Default ALPN on an eligible non-ECH, non-H2 service counts.
                    SVCB::new(2, Name::from_ascii(default_target).unwrap(), defaults),
                ];
                if alias {
                    records.push(SVCB::new(
                        0,
                        Name::from_ascii("alias.example.test.").unwrap(),
                        vec![],
                    ));
                }
                records
                    .into_iter()
                    .map(|service| {
                        Record::from_rdata(query.name().clone(), 60, RData::HTTPS(HTTPS(service)))
                    })
                    .collect()
            })
            .await;
            assert_eq!(
                lookup(&dns.resolver, "origin.example.test", 443, &["h2".into()])
                    .await
                    .len(),
                usize::from(expected)
            );
        }
    }
    #[tokio::test]
    async fn no_default_alpn_and_unrelated_targets_disable_discovery() {
        use hickory_resolver::proto::rr::rdata::HTTPS;
        for (target, no_default) in [
            ("origin.example.test.", true),
            ("unrelated.example.test.", true),
            ("unrelated.example.test.", false),
        ] {
            let dns = dns_answers(move |query| {
                vec![Record::from_rdata(
                    query.name().clone(),
                    60,
                    RData::HTTPS(HTTPS(SVCB::new(
                        1,
                        Name::from_ascii(target).unwrap(),
                        vec![
                            (
                                SvcParamKey::Alpn,
                                SvcParamValue::Alpn(Alpn(vec!["h2".into()])),
                            ),
                            (
                                if no_default {
                                    SvcParamKey::NoDefaultAlpn
                                } else {
                                    SvcParamKey::Port
                                },
                                if no_default {
                                    SvcParamValue::NoDefaultAlpn
                                } else {
                                    SvcParamValue::Port(443)
                                },
                            ),
                            (
                                SvcParamKey::EchConfigList,
                                SvcParamValue::EchConfigList(EchConfigList(vec![0, 2, 0, 1])),
                            ),
                        ],
                    ))),
                )]
            })
            .await;
            assert!(
                lookup(&dns.resolver, "origin.example.test", 443, &["h2".into()])
                    .await
                    .is_empty()
            );
        }
    }
    #[tokio::test]
    async fn dot_targets_use_the_resolved_record_owner_after_cname() {
        let dns = dns_fixture(true).await;
        for port in [443, 8443] {
            let offer = lookup(&dns.resolver, "origin.example.test", port, &["h2".into()])
                .await
                .into_iter()
                .next()
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
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(offer.endpoint, "_8443._https.origin.example.test");
    }
    #[test]
    fn https_port_parameters_cannot_redirect_the_requested_destination() {
        for port in [0, 25, 443, 8443, 65535] {
            let service = SVCB::new(
                1,
                Name::root(),
                vec![
                    (SvcParamKey::Port, SvcParamValue::Port(port)),
                    (
                        SvcParamKey::EchConfigList,
                        SvcParamValue::EchConfigList(EchConfigList(vec![0, 2, 0, 1])),
                    ),
                ],
            );
            let offer = select(&service, "example.test.", 443, &["http/1.1".into()]);
            assert_eq!(offer.is_some(), port == 443);
            if let Some(offer) = offer {
                assert_eq!(offer.port, 443);
            }
        }
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
        assert!(select(&service, "example.com.", 443, &["h3".into()]).is_none());
        assert_eq!(
            select(&service, "example.com.", 8443, &["h3".into()])
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

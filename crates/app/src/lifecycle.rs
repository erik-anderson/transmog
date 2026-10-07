use std::{
    fs::OpenOptions,
    io::Write,
    net::SocketAddr,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use transmog_content::{ContentLimits, ContentPolicy};
use transmog_core::{RoutePolicy, intercept::InterceptorChainFactory};
use transmog_runtime::{
    AtomicRuntimeIdGenerator, ListenerConfig, ProxyComponents, ProxyConfig, ProxyServer,
    SystemRuntimeClock,
};
use transmog_session::{ApplicationSessionService, HostIntegration, HostIntegrationPlan};
use transmog_tls::{CachedMitmCertificateResolver, ProxyCa, SystemTrustSource, TrustSnapshot};

use crate::{
    AppError, BodyStore, ErrorCategory, automation::AutomationRegistry, scripts::ScriptRegistry,
};

const MAX_CA_FILE_BYTES: u64 = 1024 * 1024;

/// Product-level upstream routing selection.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProxyRoute {
    /// Negotiate the best supported protocol.
    #[default]
    Auto,
    /// Restrict upstream traffic to HTTP/1.1.
    Http1,
    /// Restrict upstream traffic to HTTP/2.
    Http2,
    /// Restrict upstream traffic to HTTP/3.
    Http3,
}

/// Validated inputs for one proxy run.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStartRequest {
    /// PEM-encoded public CA certificate path.
    pub ca_certificate_path: PathBuf,
    /// PEM-encoded private CA key path.
    pub ca_private_key_path: PathBuf,
    /// Loopback listener endpoint; port zero requests an ephemeral port.
    pub listen: SocketAddr,
    /// Upstream routing policy.
    #[serde(default)]
    pub route: ProxyRoute,
    /// Explicit opt-in for non-loopback clients.
    #[serde(default)]
    pub allow_remote_clients: bool,
}

impl Default for ProxyStartRequest {
    fn default() -> Self {
        Self {
            ca_certificate_path: PathBuf::from("transmog-ca.pem"),
            ca_private_key_path: PathBuf::from("transmog-ca.key"),
            listen: "127.0.0.1:0".parse().expect("constant address is valid"),
            route: ProxyRoute::Auto,
            allow_remote_clients: false,
        }
    }
}

/// Inputs for explicit durable CA creation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaCreateRequest {
    /// Destination for the public certificate.
    pub certificate_path: PathBuf,
    /// Destination for the private key.
    pub private_key_path: PathBuf,
    /// Human-readable CA common name.
    pub common_name: String,
    /// Validity in days.
    pub validity_days: u32,
}

/// Public identity of a newly created CA.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaIdentity {
    /// Canonical uppercase SHA-256 certificate thumbprint.
    pub sha256: String,
    /// Public certificate path.
    pub certificate_path: PathBuf,
}

pub(crate) async fn start_proxy(
    service: &ApplicationSessionService,
    runtime_ids: Arc<AtomicRuntimeIdGenerator>,
    body_store: Option<&BodyStore>,
    automation: AutomationRegistry,
    scripts: ScriptRegistry,
    request: ProxyStartRequest,
    host: Option<Arc<dyn HostIntegration>>,
) -> Result<(), AppError> {
    if !request.listen.ip().is_loopback() && !request.allow_remote_clients {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "non-loopback listening requires explicit remote-client acknowledgement",
            false,
        ));
    }
    let (certificate, key, request) = tokio::task::spawn_blocking(move || {
        Ok::<_, AppError>((
            read_bounded(&request.ca_certificate_path)?,
            read_bounded(&request.ca_private_key_path)?,
            request,
        ))
    })
    .await
    .map_err(|_| AppError::new(ErrorCategory::Internal, "CA loading task failed", true))??;
    let ca = ProxyCa::from_pem(&certificate, &key).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "CA certificate or private key is invalid or mismatched",
            false,
        )
    })?;
    let config = ProxyConfig {
        listener: ListenerConfig {
            listen_addr: request.listen,
            allow_remote_clients: request.allow_remote_clients,
        },
        route_policy: route_policy(request.route),
        ..ProxyConfig::default()
    };
    let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).map_err(|_| {
        AppError::new(
            ErrorCategory::Unavailable,
            "operating-system trust roots could not be loaded",
            true,
        )
    })?);
    let hooks = InterceptorChainFactory::new(Vec::new(), config.limits.hooks)
        .with_registration_provider(
            "active Transmog automation",
            Arc::new(automation),
            NonZeroUsize::new(2_048).expect("automation provider limit is nonzero"),
        )
        .with_registration_provider(
            "active Transmog scripts",
            Arc::new(scripts),
            NonZeroUsize::new(64).expect("script provider limit is nonzero"),
        );
    let certificates = Arc::new(
        CachedMitmCertificateResolver::new(
            ca,
            config.limits.leaf_cache_capacity,
            config.limits.leaf_validity_days,
        )
        .map_err(|_| {
            AppError::new(
                ErrorCategory::InvalidInput,
                "CA cannot issue bounded interception certificates",
                false,
            )
        })?,
    );
    // IDs must outlive a proxy run because retained exchanges and body-cache
    // files remain addressable after stop/start.
    let mut components = ProxyComponents::new(hooks, certificates)
        .with_infrastructure(Arc::new(SystemRuntimeClock), runtime_ids)
        .with_content_policy(ContentPolicy::preserve_original_output(
            ContentLimits::default(),
        ));
    if let Some(body_store) = body_store {
        components =
            components.with_observer(Arc::new(body_store.clone()), body_store.observer_config());
    }
    // Enqueue each event for body retention before the session catalog. This
    // narrows the terminal-publication race; completed detail reads then flush
    // accepted body work, and the presentation layer handles the remaining
    // cross-dispatcher scheduling window with a bounded refresh.
    let components = service.prepare_components(components);
    let server = ProxyServer::bind_with_components(config, trust, components)
        .await
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "proxy listener could not be bound",
                true,
            )
        })?;
    match host {
        Some(integration) => {
            service
                .start_with_host(server, HostIntegrationPlan { integration })
                .await
        }
        None => service.start(server).await,
    }
    .map(|_| ())
    .map_err(AppError::from)
}

pub(crate) async fn create_ca(request: CaCreateRequest) -> Result<CaIdentity, AppError> {
    if request.common_name.trim().is_empty()
        || request.common_name.chars().count() > 128
        || !(1..=3650).contains(&request.validity_days)
        || request.certificate_path == request.private_key_path
    {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "CA creation settings are invalid",
            false,
        ));
    }
    tokio::task::spawn_blocking(move || {
        if request.certificate_path.exists() || request.private_key_path.exists() {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "CA destination already exists",
                false,
            ));
        }
        let ca = ProxyCa::generate(&request.common_name, request.validity_days)
            .map_err(|_| AppError::new(ErrorCategory::Internal, "CA generation failed", true))?;
        let key = ca
            .private_key_pem_pkcs8()
            .map_err(|_| AppError::new(ErrorCategory::Internal, "CA key encoding failed", true))?;
        let certificate = ca
            .certificate_pem()
            .map_err(|_| AppError::new(ErrorCategory::Internal, "CA encoding failed", true))?;
        write_new(&request.private_key_path, &key)?;
        if let Err(error) = write_new(&request.certificate_path, &certificate) {
            let _ = std::fs::remove_file(&request.private_key_path);
            return Err(error);
        }
        Ok(CaIdentity {
            sha256: ca
                .sha256_thumbprint()
                .map_err(|_| AppError::new(ErrorCategory::Internal, "CA identity failed", true))?,
            certificate_path: request.certificate_path,
        })
    })
    .await
    .map_err(|_| AppError::new(ErrorCategory::Internal, "CA generation task failed", true))?
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, AppError> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| AppError::new(ErrorCategory::InvalidInput, "CA file is unavailable", false))?;
    if metadata.len() > MAX_CA_FILE_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "CA file exceeds the one MiB input limit",
            false,
        ));
    }
    std::fs::read(path)
        .map_err(|_| AppError::new(ErrorCategory::InvalidInput, "CA file is unreadable", false))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Conflict,
                "CA destination exists or is unavailable",
                false,
            )
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| AppError::new(ErrorCategory::Unavailable, "CA file write failed", true))
}

const fn route_policy(route: ProxyRoute) -> RoutePolicy {
    match route {
        ProxyRoute::Auto => RoutePolicy::Auto,
        ProxyRoute::Http1 => RoutePolicy::Http1Only,
        ProxyRoute::Http2 => RoutePolicy::Http2Only,
        ProxyRoute::Http3 => RoutePolicy::Http3Only,
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    async fn read_http_head(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 32 * 1024);
            head.push(socket.read_u8().await.unwrap());
        }
        head
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn restart_retains_distinct_exchanges_and_complete_compressed_bodies() {
        let root = std::env::temp_dir().join(format!(
            "transmog-app-restart-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let ca = CaCreateRequest {
            certificate_path: root.join("ca.pem"),
            private_key_path: root.join("ca.key"),
            common_name: "Restart test CA".to_owned(),
            validity_days: 2,
        };
        create_ca(ca.clone()).await.unwrap();
        let application = crate::Application::new(crate::AppConfig {
            body_store: Some(crate::BodyStoreConfig::product_default(root.join("bodies"))),
            ..crate::AppConfig::default()
        })
        .unwrap();
        let request = ProxyStartRequest {
            ca_certificate_path: ca.certificate_path,
            ca_private_key_path: ca.private_key_path,
            route: ProxyRoute::Http1,
            ..ProxyStartRequest::default()
        };
        let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let encoded =
            crate::inspector::encode_content(&["gzip".to_owned()], b"retained response".to_vec())
                .await
                .unwrap();
        let body_length = encoded.len();
        let origin_task = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = origin.accept().await.unwrap();
                read_http_head(&mut socket).await;
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n", encoded.len()).as_bytes()).await.unwrap();
                socket.write_all(&encoded).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        let mut ids = Vec::new();
        for run in 1..=2 {
            application
                .start_proxy(request.clone(), None)
                .await
                .unwrap();
            let mut client = tokio::net::TcpStream::connect(application.status().listener.unwrap())
                .await
                .unwrap();
            client
                .write_all(
                    format!(
                        "GET http://{origin_addr}/{run} HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let response = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let head = read_http_head(&mut client).await;
                let mut body = vec![0; body_length];
                client.read_exact(&mut body).await.unwrap();
                head
            })
            .await
            .unwrap();
            assert!(response.starts_with(b"HTTP/1.1 200"));
            drop(client);
            application.shutdown().await.unwrap();
            let page = application
                .query_sessions(crate::SessionQueryInput::default())
                .unwrap();
            assert_eq!(page.sessions.len(), run, "restart reused an exchange ID");
            let row = page
                .sessions
                .iter()
                .find(|row| row.path == format!("/{run}"))
                .unwrap();
            let detail = application.session_detail(&row.id).unwrap();
            let body = detail
                .stored_bodies
                .iter()
                .find(|body| body.boundary == "client-response")
                .unwrap();
            assert_eq!(
                body.availability,
                crate::BodyAvailability::Complete,
                "{body:?}"
            );
            ids.push(row.id.clone());
        }
        assert_ne!(ids[0], ids[1]);
        for id in ids {
            let inspection = application
                .inspect_body(crate::BodyInspectionRequest {
                    session_id: id,
                    boundary: "client-response".to_owned(),
                    representation: crate::BodyRepresentation::Auto,
                    decode_content: true,
                    offset: 0,
                    max_bytes: None,
                })
                .await
                .unwrap();
            assert_eq!(inspection.display, "retained response");
        }
        origin_task.await.unwrap();
        drop(application);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn ca_creation_is_create_new_and_round_trips() {
        let root = std::env::temp_dir().join(format!("transmog-app-ca-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let request = CaCreateRequest {
            certificate_path: root.join("ca.pem"),
            private_key_path: root.join("ca.key"),
            common_name: "Transmog test CA".to_owned(),
            validity_days: 2,
        };
        let identity = create_ca(request.clone()).await.unwrap();
        assert_eq!(identity.sha256.len(), 64);
        assert!(
            ProxyCa::from_pem(
                &std::fs::read(&request.certificate_path).unwrap(),
                &std::fs::read(&request.private_key_path).unwrap()
            )
            .is_ok()
        );
        assert!(create_ca(request).await.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn start_rejects_remote_listener_without_acknowledgement() {
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let request = ProxyStartRequest {
            listen: "192.0.2.1:0".parse().unwrap(),
            ..ProxyStartRequest::default()
        };
        let assets = crate::response_assets::ResponseAssetStore::load(None).unwrap();
        let error = start_proxy(
            &service,
            Arc::new(AtomicRuntimeIdGenerator::new()),
            None,
            AutomationRegistry::load(None, Arc::new(assets.clone())).unwrap(),
            ScriptRegistry::load(None, None, Arc::new(assets)).unwrap(),
            request,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error.category, ErrorCategory::InvalidInput);
    }

    #[tokio::test]
    async fn generated_ca_starts_and_stops_a_real_loopback_proxy() {
        let root = std::env::temp_dir().join(format!("transmog-app-proxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let ca = CaCreateRequest {
            certificate_path: root.join("ca.pem"),
            private_key_path: root.join("ca.key"),
            common_name: "Transmog lifecycle test CA".to_owned(),
            validity_days: 2,
        };
        create_ca(ca.clone()).await.unwrap();
        let application = crate::Application::new(crate::AppConfig::default()).unwrap();
        let request = ProxyStartRequest {
            ca_certificate_path: ca.certificate_path,
            ca_private_key_path: ca.private_key_path,
            ..ProxyStartRequest::default()
        };
        application
            .start_proxy(request.clone(), None)
            .await
            .unwrap();
        assert_eq!(application.status().lifecycle, crate::AppLifecycle::Running);
        assert_eq!(
            application
                .start_proxy(request, None)
                .await
                .unwrap_err()
                .category,
            ErrorCategory::Conflict
        );
        application.shutdown().await.unwrap();
        assert_eq!(application.status().lifecycle, crate::AppLifecycle::Stopped);
        let _ = std::fs::remove_dir_all(root);
    }
}

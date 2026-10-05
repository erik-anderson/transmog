use std::{
    fs::OpenOptions,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use transmog_content::{ContentLimits, ContentPolicy};
use transmog_core::{RoutePolicy, intercept::InterceptorChainFactory};
use transmog_runtime::{ListenerConfig, ProxyComponents, ProxyConfig, ProxyServer};
use transmog_session::{ApplicationSessionService, HostIntegration, HostIntegrationPlan};
use transmog_tls::{CachedMitmCertificateResolver, ProxyCa, SystemTrustSource, TrustSnapshot};

use crate::{AppError, BodyStore, ErrorCategory};

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
    body_store: Option<&BodyStore>,
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
    let hooks = InterceptorChainFactory::new(Vec::new(), config.limits.hooks);
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
    let mut components = service.prepare_components(
        ProxyComponents::new(hooks, certificates).with_content_policy(
            ContentPolicy::preserve_original_output(ContentLimits::default()),
        ),
    );
    if let Some(body_store) = body_store {
        components =
            components.with_observer(Arc::new(body_store.clone()), body_store.observer_config());
    }
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
    use super::*;

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
        let error = start_proxy(&service, None, request, None)
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

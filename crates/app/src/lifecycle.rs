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
    #[allow(clippy::too_many_lines)]
    async fn captured_autoresponse_survives_source_loss_and_replays_encoded_edits() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        async fn request(listener: String, url: &str, length: usize) -> (String, Vec<u8>) {
            let mut client = tokio::net::TcpStream::connect(listener).await.unwrap();
            client
                .write_all(
                    format!(
                        "GET {url} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                        url.strip_prefix("http://")
                            .unwrap()
                            .split('/')
                            .next()
                            .unwrap()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let head = read_http_head(&mut client).await;
                let mut body = vec![0; length];
                client.read_exact(&mut body).await.unwrap();
                (String::from_utf8(head).unwrap(), body)
            })
            .await
            .unwrap()
        }

        let root = tempfile::tempdir().unwrap();
        let config = crate::AppConfig {
            body_store: Some(crate::BodyStoreConfig::product_default(
                root.path().join("bodies"),
            )),
            response_asset_root: Some(root.path().join("assets")),
            automation_path: Some(root.path().join("automation")),
            ..crate::AppConfig::default()
        };
        let application = crate::Application::new(config.clone()).unwrap();
        let ca = CaCreateRequest {
            certificate_path: root.path().join("ca.pem"),
            private_key_path: root.path().join("ca.key"),
            common_name: "Autoresponse test CA".to_owned(),
            validity_days: 2,
        };
        create_ca(ca.clone()).await.unwrap();
        let start = ProxyStartRequest {
            ca_certificate_path: ca.certificate_path,
            ca_private_key_path: ca.private_key_path,
            route: ProxyRoute::Http1,
            ..ProxyStartRequest::default()
        };
        let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/account", origin.local_addr().unwrap());
        let encoded = crate::inspector::encode_content(
            &["gzip".to_owned(), "br".to_owned()],
            b"original response".to_vec(),
        )
        .await
        .unwrap();
        let origin_body = encoded.clone();
        let origin_task = tokio::spawn(async move {
            let (mut socket, _) = origin.accept().await.unwrap();
            read_http_head(&mut socket).await;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Encoding: gzip\r\nContent-Encoding: br\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", origin_body.len()).as_bytes()).await.unwrap();
            socket.write_all(&origin_body).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        application.start_proxy(start.clone(), None).await.unwrap();
        let (_, original) =
            request(application.status().listener.unwrap(), &url, encoded.len()).await;
        assert_eq!(original, encoded);
        origin_task.await.unwrap(); // The origin is closed for every subsequent request.
        application.shutdown().await.unwrap();
        let source_id = application
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions[0]
            .id
            .clone();
        let input = crate::SessionResponseAsset {
            id: "captured".to_owned(),
            revision: 1,
            exchange_id: source_id.clone(),
            boundary: "client-response".to_owned(),
            decoded_body: None,
            preserve_content_encoding: true,
        };
        let batch = application
            .create_autoresponse_batch(crate::AutoResponseBatchInput {
                ids: vec![source_id.clone()],
                generation: 0,
            })
            .await
            .unwrap();
        assert_eq!(batch.created_ids.len(), 1);
        let before = application.response_assets().len();
        assert!(
            application
                .create_autoresponse_batch(crate::AutoResponseBatchInput {
                    ids: vec![source_id.clone(), format!("{:032x}", u128::MAX)],
                    generation: batch.status.generation
                })
                .await
                .is_err()
        );
        assert_eq!(application.response_assets().len(), before);
        assert_eq!(application.automation_status().rules, batch.status.rules);
        let asset = application
            .create_response_asset_from_session(input.clone())
            .await
            .unwrap();
        let mut rule = transmog_automation::Rule {
            id: "captured-rule".to_owned(),
            display_name: Some("Captured account".to_owned()),
            enabled: true,
            revision: 1,
            priority: -1_000_000,
            matcher: transmog_automation::RuleMatcher {
                path_prefix: Some("/account".to_owned()),
                ..Default::default()
            },
            request: transmog_automation::RequestActions {
                response_asset: Some(asset.asset_ref()),
                ..Default::default()
            },
            response: transmog_automation::ResponseActions::default(),
        };
        let candidate = application
            .validate_automation(crate::AutomationRuleSet {
                rules: vec![rule.clone()],
                generation: application.automation_status().generation,
                ..Default::default()
            })
            .unwrap();
        application
            .activate_automation(&candidate.candidate_id)
            .unwrap();
        application.start_proxy(start.clone(), None).await.unwrap();
        let (head, replayed) =
            request(application.status().listener.unwrap(), &url, encoded.len()).await;
        assert!(head.starts_with("HTTP/1.1 200"));
        assert_eq!(replayed, encoded);
        application.shutdown().await.unwrap();
        application
            .remove_traffic_entries(std::slice::from_ref(&source_id), false)
            .unwrap();
        assert!(application.session_detail(&source_id).is_err());
        let preview = application
            .inspect_response_asset(&asset.asset_ref())
            .await
            .unwrap();
        assert_eq!(preview.display, "original response");
        let edited = application
            .edit_response_asset(crate::ResponseAssetEdit {
                asset_reference: asset.asset_ref(),
                status: 200,
                headers: asset.headers.clone(),
                media_type: asset.media_type.clone(),
                decoded_body: Some(b"edited response".to_vec()),
                body_path: None,
                preserve_content_encoding: true,
            })
            .await
            .unwrap();
        rule.revision += 1;
        rule.request.response_asset = Some(edited.asset_ref());
        let candidate = application
            .validate_automation(crate::AutomationRuleSet {
                rules: vec![rule],
                generation: application.automation_status().generation,
                ..Default::default()
            })
            .unwrap();
        application
            .activate_automation(&candidate.candidate_id)
            .unwrap();
        drop(application);

        let restarted = crate::Application::new(config).unwrap();
        assert!(restarted.session_detail(&source_id).is_err());
        assert_eq!(restarted.response_assets().len(), 3);
        restarted.start_proxy(start.clone(), None).await.unwrap();
        let (head, _) = request(
            restarted.status().listener.unwrap(),
            &url,
            usize::try_from(edited.body_bytes).unwrap(),
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(
            head.to_ascii_lowercase()
                .contains("content-encoding: gzip, br")
        );
        assert!(
            head.to_ascii_lowercase()
                .contains(&format!("content-length: {}\r\n", edited.body_bytes))
        );
        restarted.shutdown().await.unwrap();
        let row = &restarted
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions[0];
        let detail = restarted.session_detail(&row.id).unwrap();
        assert_eq!(detail.auto_response.unwrap().rule_id, "captured-rule");
        let inspection = restarted
            .inspect_body(crate::BodyInspectionRequest {
                session_id: row.id.clone(),
                boundary: "client-response".to_owned(),
                representation: crate::BodyRepresentation::OriginalText,
                decode_content: true,
                offset: 0,
                max_bytes: None,
            })
            .await
            .unwrap();
        assert_eq!(inspection.display, "edited response");
        let identity_body = "updated response: café".as_bytes().to_vec();
        let headers = transmog_core::HeaderBlock::from_fields(vec![
            transmog_core::HeaderField::try_new("Content-Length", "999").unwrap(),
            transmog_core::HeaderField::try_new("content-length", "1").unwrap(),
            transmog_core::HeaderField::try_new("Content-Encoding", "gzip").unwrap(),
        ]);
        let identity = restarted
            .edit_response_asset(crate::ResponseAssetEdit {
                asset_reference: edited.asset_ref(),
                status: 201,
                headers: headers.clone(),
                media_type: edited.media_type.clone(),
                decoded_body: Some(identity_body.clone()),
                body_path: None,
                preserve_content_encoding: false,
            })
            .await
            .unwrap();
        let activate = |asset: &crate::ResponseAsset| {
            let mut document = restarted.automation_status();
            let mut rule = document.rules.pop().unwrap();
            rule.revision += 1;
            rule.request.response_asset = Some(asset.asset_ref());
            let candidate = restarted
                .validate_automation(crate::AutomationRuleSet {
                    rules: vec![rule],
                    generation: document.generation,
                    ..Default::default()
                })
                .unwrap();
            restarted
                .activate_automation(&candidate.candidate_id)
                .unwrap();
        };
        activate(&identity);
        restarted.start_proxy(start.clone(), None).await.unwrap();
        let (head, body) = request(
            restarted.status().listener.unwrap(),
            &url,
            identity_body.len(),
        )
        .await;
        assert_eq!(body, identity_body);
        assert!(head.starts_with("HTTP/1.1 201"));
        assert!(!head.to_ascii_lowercase().contains("content-encoding:"));
        assert!(
            head.to_ascii_lowercase()
                .contains(&format!("content-length: {}\r\n", body.len()))
        );
        let file = root.path().join("replacement.bin");
        let replacement_body = b"replacement file bytes";
        std::fs::write(&file, replacement_body).unwrap();
        let replacement = restarted
            .edit_response_asset(crate::ResponseAssetEdit {
                asset_reference: edited.asset_ref(),
                status: 200,
                headers,
                media_type: edited.media_type.clone(),
                decoded_body: None,
                body_path: Some(file),
                preserve_content_encoding: true,
            })
            .await
            .unwrap();
        activate(&replacement);
        let (head, body) = request(
            restarted.status().listener.unwrap(),
            &url,
            replacement_body.len(),
        )
        .await;
        assert_eq!(body, replacement_body);
        assert!(!head.to_ascii_lowercase().contains("content-encoding:"));
        assert!(
            head.to_ascii_lowercase()
                .contains(&format!("content-length: {}\r\n", body.len()))
        );
        restarted.shutdown().await.unwrap();
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

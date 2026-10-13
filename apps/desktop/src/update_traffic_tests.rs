use super::*;
use transmog_app::{
    AppConfig, AuthoredResponseAsset, AutomationRuleSet, BodyInspectionRequest, BodyRepresentation,
    BodyStoreConfig, CaCreateRequest, ProxyStartRequest, SessionQueryInput,
};
use transmog_automation::{RequestActions, ResponseActions, Rule, RuleMatcher};
use transmog_core::{HeaderBlock, HeaderField};

const FEED: &str = "https://updates.example.invalid/check";
const ARTIFACT: &[u8] = include_bytes!("../tests/fixtures/updater-artifact.txt");

fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": include_str!("../tests/fixtures/updater-public.txt").trim(),
            "requireSignedVersion": true,
        }),
    );
    tauri::test::mock_builder()
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(include_str!("../tests/fixtures/updater-public.txt").trim())
                .build(),
        )
        .build(context)
        .unwrap()
}

fn application(root: &Path) -> Application {
    Application::new(AppConfig {
        response_asset_root: Some(root.join("assets")),
        body_store: Some(BodyStoreConfig::product_default(root.join("bodies"))),
        ..AppConfig::default()
    })
    .unwrap()
}

async fn proxy_request(application: &Application, root: &Path) -> ProxyStartRequest {
    std::fs::create_dir_all(root).unwrap();
    let identity = application
        .create_ca(CaCreateRequest {
            certificate_path: root.join("ca.pem"),
            private_key_path: root.join("ca.key"),
            common_name: "Update traffic test".into(),
            validity_days: 1,
        })
        .await
        .unwrap();
    ProxyStartRequest {
        ca_certificate_path: identity.certificate_path,
        ca_private_key_path: root.join("ca.key"),
        ..ProxyStartRequest::default()
    }
}

fn respond(application: &Application, path: &str, status: u16, headers: HeaderBlock, body: &[u8]) {
    let asset = application
        .create_response_asset(AuthoredResponseAsset {
            id: path.trim_start_matches('/').into(),
            revision: 1,
            status,
            headers,
            body: body.to_vec(),
            media_type: None,
        })
        .unwrap();
    let current = application.automation_status();
    let mut rules = current.rules;
    rules.push(Rule {
        id: asset.id.clone(),
        display_name: None,
        enabled: true,
        revision: 1,
        priority: 0,
        matcher: RuleMatcher {
            method: Some("GET".into()),
            host: Some("updates.example.invalid".into()),
            path_prefix: Some(path.into()),
            ..RuleMatcher::default()
        },
        request: RequestActions {
            response_asset: Some(asset.asset_ref()),
            ..RequestActions::default()
        },
        response: ResponseActions::default(),
    });
    let candidate = application
        .validate_automation(AutomationRuleSet {
            rules,
            generation: current.generation,
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
}

fn release_fixture(application: &Application) -> Vec<u8> {
    let mut headers = HeaderBlock::new();
    headers.push(HeaderField::try_new("Location", b"/manifest".to_vec()).unwrap());
    respond(application, "/check", 302, headers, b"");
    let manifest = serde_json::to_vec(&serde_json::json!({
        "version": "0.2.0",
        "url": "https://updates.example.invalid/installer",
        "signature": include_str!("../tests/fixtures/updater-artifact.txt.sig"),
    }))
    .unwrap();
    respond(
        application,
        "/manifest",
        200,
        HeaderBlock::default(),
        &manifest,
    );
    respond(
        application,
        "/installer",
        200,
        HeaderBlock::default(),
        ARTIFACT,
    );
    manifest
}

#[test]
fn only_running_listeners_are_selected_and_wildcard_listeners_use_loopback() {
    let mut status = AppStatus {
        lifecycle: AppLifecycle::Running,
        listener: Some("0.0.0.0:8123".into()),
        summary: String::new(),
        host_restore_pending: false,
    };
    assert_eq!(
        active_proxy(&status).unwrap().as_str(),
        "http://127.0.0.1:8123/"
    );
    status.listener = Some("[::]:8124".into());
    assert_eq!(
        active_proxy(&status).unwrap().as_str(),
        "http://[::1]:8124/"
    );
    for lifecycle in [
        AppLifecycle::Stopped,
        AppLifecycle::Draining,
        AppLifecycle::Stopping,
        AppLifecycle::Failed,
    ] {
        status.lifecycle = lifecycle;
        assert!(active_proxy(&status).is_none());
    }
}

#[tokio::test]
async fn https_update_checks_redirects_and_downloads_are_inspectable_after_proxy_restart() {
    let root = tempfile::tempdir().unwrap();
    let application = application(root.path());
    let request = proxy_request(&application, root.path()).await;
    let manifest = release_fixture(&application);
    let app = mock_app();
    // Construct while stopped; the callback must discover the later start.
    let updater = update_checker(
        app.handle(),
        &application,
        &request.ca_certificate_path,
        FEED.parse().unwrap(),
    )
    .unwrap();
    application
        .start_proxy(request.clone(), None)
        .await
        .unwrap();
    let update = tokio::time::timeout(Duration::from_secs(10), updater.check())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let previous_listener = application.status().listener.unwrap();
    application.shutdown().await.unwrap();
    // Keep the old port occupied so a stale updater route cannot pass by chance.
    let _previous_listener = tokio::net::TcpListener::bind(previous_listener)
        .await
        .unwrap();
    let rows = application
        .query_sessions(SessionQueryInput::default())
        .unwrap()
        .sessions;
    assert_eq!(rows.len(), 2, "{rows:?}");
    let row = rows.iter().find(|row| row.path == "/manifest").unwrap();
    assert_eq!(row.method, "GET");
    assert_eq!(row.status, Some(200));
    let body = application
        .inspect_body(BodyInspectionRequest {
            session_id: row.id.clone(),
            boundary: "client-response".into(),
            representation: BodyRepresentation::OriginalText,
            decode_content: true,
            offset: 0,
            max_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(body.display.as_bytes(), manifest);
    // A new port and root prove the Update does not retain either from its check.
    let next = proxy_request(&application, &root.path().join("next")).await;
    application.start_proxy(next.clone(), None).await.unwrap();
    // The desktop uses a stable managed certificate path across root resets.
    std::fs::copy(&next.ca_certificate_path, &request.ca_certificate_path).unwrap();
    let bytes = tokio::time::timeout(Duration::from_secs(10), update.download(|_, _| {}, || {}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, ARTIFACT);
    application.shutdown().await.unwrap();
    let rows = application
        .query_sessions(SessionQueryInput::default())
        .unwrap()
        .sessions;
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.iter()
            .any(|row| row.path == "/installer" && row.status == Some(200))
    );
}

#[tokio::test]
async fn a_failed_update_check_keeps_the_http_response_in_traffic() {
    let root = tempfile::tempdir().unwrap();
    let application = application(root.path());
    let request = proxy_request(&application, root.path()).await;
    respond(
        &application,
        "/check",
        503,
        HeaderBlock::default(),
        b"Try again later",
    );
    application
        .start_proxy(request.clone(), None)
        .await
        .unwrap();
    let app = mock_app();
    let updater = update_checker(
        app.handle(),
        &application,
        &request.ca_certificate_path,
        FEED.parse().unwrap(),
    )
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), updater.check())
            .await
            .unwrap()
            .is_err()
    );
    application.shutdown().await.unwrap();
    let rows = application
        .query_sessions(SessionQueryInput::default())
        .unwrap()
        .sessions;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].status, Some(503));
}

#[tokio::test]
async fn intercepted_updates_still_require_a_trusted_certificate() {
    let root = tempfile::tempdir().unwrap();
    let application = application(root.path());
    let request = proxy_request(&application, root.path()).await;
    release_fixture(&application);
    application.start_proxy(request, None).await.unwrap();
    let app = mock_app();
    let updater = update_checker(
        app.handle(),
        &application,
        &root.path().join("missing.pem"),
        FEED.parse().unwrap(),
    )
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), updater.check())
            .await
            .unwrap()
            .is_err()
    );
    application.shutdown().await.unwrap();
}

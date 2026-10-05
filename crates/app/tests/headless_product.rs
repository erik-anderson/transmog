//! Real-process qualification for the UI-neutral product service.
//!
//! The repository interop driver supplies a standalone curl client, a pinned
//! Nginx origin, and the exact packaged sandbox helpers. Keeping this test in
//! `transmog-app` proves that the product workflow does not depend on Tauri,
//! `WebView2`, Monaco, or any presentation-layer state.

use std::{
    collections::BTreeSet,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::Output,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{process::Command, time::sleep};
use transmog_app::{
    AppConfig, Application, AuthoredResponseAsset, AutomationRuleSet, BodyAvailability,
    BodyInspectionRequest, BodyRepresentation, BodyStoreConfig, BreakpointDecision,
    BreakpointPhaseInput, BreakpointSettings, CaptureStartRequest, ExportFormat, ExportRequest,
    ImportResponseAsset, ProxyRoute, ProxyStartRequest, RetentionMode, ScriptDraft, SessionDetail,
    SessionQueryInput,
};
use transmog_automation::{
    HeaderCondition, HeaderOperation, HeaderPredicate, RequestActions, ResponseActions, Rule,
    RuleMatcher,
};
use transmog_control_model::{DecisionAction, HeaderField as ControlHeaderField};
use transmog_core::{HeaderBlock, HeaderField};
use transmog_script::{ScriptCapabilities, ScriptHandler, ScriptLimits, ScriptPrefilter};

const LARGE_ASSET_BYTES: usize = 1024 * 1024 + 1;
const BODY_STORE_BYTES: u64 = 3 * 1024 * 1024;

static CURL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-interop.ps1 with Docker and packaged helpers"]
#[allow(clippy::too_many_lines)]
async fn full_product_workflow_operates_headlessly_and_survives_restart() {
    let environment = Environment::from_process();
    let workspace = TestWorkspace::new();
    let large_source = workspace.root.join("large-response.bin");
    std::fs::write(&large_source, vec![b'L'; LARGE_ASSET_BYTES]).unwrap();

    let application = Application::new(workspace.config(&environment)).unwrap();
    let small = application
        .create_response_asset(AuthoredResponseAsset {
            id: "phase9-small".to_owned(),
            revision: 1,
            status: 200,
            headers: content_type("text/plain; charset=utf-8"),
            body: b"small native autoresponse\n".to_vec(),
            media_type: Some("text/plain".to_owned()),
        })
        .unwrap();
    let large = application
        .import_response_asset(ImportResponseAsset {
            id: "phase9-large".to_owned(),
            revision: 1,
            status: 200,
            headers: content_type("application/octet-stream"),
            body_path: large_source,
            media_type: Some("application/octet-stream".to_owned()),
        })
        .unwrap();
    assert_eq!(large.body_bytes, LARGE_ASSET_BYTES as u64);

    activate_native_rules(&application, &small.asset_ref(), &large.asset_ref());
    activate_script(&application, &environment.script_host);
    let listener = start(&application, &environment).await;
    application
        .start_capture(CaptureStartRequest {
            path: workspace.capture.clone(),
            max_file_bytes: 64 * 1024 * 1024,
            retain_body_samples: true,
        })
        .await
        .unwrap();

    let native = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/native"),
        &["X-Phase9: native"],
    )
    .await;
    assert_success(&native);
    assert_eq!(native.body, b"Transmog-Phase9\n");
    assert_header(&native, "x-seen-user-agent", "Transmog-Phase9");
    let native_detail = wait_for_session(&application, "/echo/native", "completed").await;
    assert!(
        native_detail
            .hook_effects
            .iter()
            .any(|effect| effect.contains("automation/phase9-native-ua@1/request"))
    );

    let scripted = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/script"),
        &["X-Phase9: script"],
    )
    .await;
    assert_success(&scripted);
    assert_eq!(scripted.body, native.body);
    assert_header(&scripted, "x-seen-user-agent", "Transmog-Phase9");
    let scripted_detail = wait_for_session(&application, "/echo/script", "completed").await;
    assert!(
        scripted_detail
            .hook_effects
            .iter()
            .any(|effect| effect.contains("script/phase9-script@1"))
    );
    assert_ne!(native_detail.hook_effects, scripted_detail.hook_effects);

    qualify_interactive_breakpoint(&application, &environment, &workspace.root, &listener).await;

    let small_native = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/small"),
        &[],
    )
    .await;
    if small_native.status != 200 {
        let detail = wait_for_session(&application, "/auto/small", "failed").await;
        panic!("native autoresponse failed: {detail:#?}");
    }
    assert_success(&small_native);
    assert_eq!(small_native.body, b"small native autoresponse\n");

    let small_script = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/script"),
        &[],
    )
    .await;
    if small_script.status != 200 {
        let detail = wait_for_session(&application, "/auto/script", "failed").await;
        panic!("script autoresponse failed: {detail:#?}");
    }
    assert_success(&small_script);
    assert_eq!(small_script.body, small_native.body);

    let large_native = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/large?run=1"),
        &[],
    )
    .await;
    assert_success(&large_native);
    assert_eq!(large_native.body.len(), LARGE_ASSET_BYTES);
    assert!(large_native.body.iter().all(|byte| *byte == b'L'));

    let changed = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("body/modify"),
        &[],
    )
    .await;
    assert_success(&changed);
    assert_eq!(changed.body, b"phase9-effective-body\n");
    let changed_detail = wait_for_session(&application, "/body/modify", "completed").await;
    wait_for_complete_bodies(&application, &changed_detail.id).await;
    let original = application
        .inspect_body(inspection(&changed_detail.id, "upstream-response"))
        .await
        .unwrap();
    let effective = application
        .inspect_body(inspection(&changed_detail.id, "client-response"))
        .await
        .unwrap();
    assert!(original.display.contains("data-origin=\"nginx\""));
    assert_eq!(effective.display, "phase9-effective-body\n");
    assert_ne!(original.metadata.sha256, effective.metadata.sha256);

    let image = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("image.png"),
        &[],
    )
    .await;
    assert_success(&image);
    assert!(image.body.starts_with(b"\x89PNG\r\n\x1a\n"));
    let image_detail = wait_for_session(&application, "/image.png", "completed").await;
    wait_for_complete_bodies(&application, &image_detail.id).await;
    let preview = application
        .inspect_body(BodyInspectionRequest {
            session_id: image_detail.id.clone(),
            boundary: "client-response".to_owned(),
            representation: BodyRepresentation::Image,
            decode_content: true,
            offset: 0,
            max_bytes: None,
        })
        .await
        .unwrap();
    let handle = preview.preview_handle.expect("image preview handle");
    let normalized = application.image_preview(&handle).expect("cached preview");
    assert!(normalized.starts_with(b"\x89PNG\r\n\x1a\n"));

    let failure = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("script/fail"),
        &[],
    )
    .await;
    assert_ne!(failure.status, 200);
    let failed_detail = wait_for_session(&application, "/script/fail", "failed").await;
    assert!(failed_detail.terminal.contains("script-error"));
    assert!(failed_detail.terminal.contains("phase9-script@1"));
    assert!(failed_detail.terminal.contains("Exception"));

    // Repeated streamed responses exceed the circular quota. The body bytes
    // disappear oldest-first while session and hook evidence remain queryable.
    for run in 2..=5 {
        let response = curl(
            &environment,
            &workspace.root,
            &listener,
            &environment.origin(&format!("auto/large?run={run}")),
            &[],
        )
        .await;
        assert_success(&response);
        assert_eq!(response.body.len(), LARGE_ASSET_BYTES);
    }
    wait_for_eviction(&application).await;
    let retained_evidence = application
        .session_detail(&native_detail.id)
        .expect("eviction must retain session evidence");
    assert!(!retained_evidence.hook_effects.is_empty());

    application.stop_capture().await.unwrap();
    let json = application
        .export_capture(ExportRequest {
            source: workspace.capture.clone(),
            destination: workspace.json_export.clone(),
            format: ExportFormat::JsonLines,
            max_source_bytes: 64 * 1024 * 1024,
        })
        .await
        .unwrap();
    let saz = application
        .export_capture(ExportRequest {
            source: workspace.capture.clone(),
            destination: workspace.saz_export.clone(),
            format: ExportFormat::SazStrict,
            max_source_bytes: 64 * 1024 * 1024,
        })
        .await
        .unwrap();
    assert!(json.records > 0 && json.bytes > 0 && json.source_sealed);
    assert!(saz.records > 0 && saz.bytes > 0 && saz.source_sealed);

    application.shutdown().await.unwrap();
    drop(application);

    // Durable automation, script revisions, and response assets are reloaded
    // into fresh service owners; ephemeral sessions and body bytes are not.
    let restarted = Application::new(workspace.config(&environment)).unwrap();
    assert_eq!(restarted.automation_status().rules.len(), 4);
    assert_eq!(restarted.script_status().active.len(), 1);
    assert_eq!(restarted.response_assets().len(), 2);
    let restarted_listener = start(&restarted, &environment).await;
    let after_restart = curl(
        &environment,
        &workspace.root,
        &restarted_listener,
        &environment.origin("echo/script"),
        &["X-Phase9: script"],
    )
    .await;
    assert_success(&after_restart);
    assert_eq!(after_restart.body, b"Transmog-Phase9\n");
    restarted.shutdown().await.unwrap();
}

async fn qualify_interactive_breakpoint(
    application: &Application,
    environment: &Environment,
    root: &Path,
    listener: &str,
) {
    application
        .enable_breakpoints(&BreakpointSettings {
            phases: BTreeSet::from([BreakpointPhaseInput::RequestHead]),
            ..BreakpointSettings::default()
        })
        .unwrap();
    let task = tokio::spawn(curl_owned(
        environment.clone(),
        root.to_owned(),
        listener.to_owned(),
        environment.origin("echo/breakpoint"),
        vec!["User-Agent: Before-Breakpoint".to_owned()],
    ));
    let paused = wait_for_pause(application).await;
    let mut head = paused
        .request_head
        .clone()
        .expect("request head breakpoint");
    head.headers
        .retain(|field| !field.name.eq_ignore_ascii_case("user-agent"));
    head.headers.push(ControlHeaderField {
        name: "user-agent".to_owned(),
        value: b"Transmog-Breakpoint".to_vec(),
    });
    application
        .decide_breakpoint(BreakpointDecision {
            decision_id: paused.decision_id,
            exchange_id: paused.exchange_id,
            action: DecisionAction::ReplaceRequestHead { head },
        })
        .unwrap();
    let response = task.await.unwrap();
    assert_success(&response);
    assert_eq!(response.body, b"Transmog-Breakpoint\n");
    application.disable_breakpoints().await;
    let detail = wait_for_session(application, "/echo/breakpoint", "completed").await;
    assert!(
        detail
            .hook_effects
            .iter()
            .any(|effect| effect.contains("transmog.session.interactive-control"))
    );
}

fn activate_native_rules(application: &Application, small: &str, large: &str) {
    let rules = vec![
        Rule {
            id: "phase9-native-ua".to_owned(),
            revision: 1,
            priority: 100,
            matcher: RuleMatcher {
                path_prefix: Some("/echo/native".to_owned()),
                request_headers: vec![HeaderPredicate {
                    name: "x-phase9".to_owned(),
                    condition: HeaderCondition::Exact(b"native".to_vec()),
                }],
                ..RuleMatcher::default()
            },
            request: RequestActions {
                headers: vec![HeaderOperation::set("user-agent", "Transmog-Phase9").unwrap()],
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        },
        autoresponse_rule("phase9-small-response", "/auto/small", small),
        autoresponse_rule("phase9-large-response", "/auto/large", large),
        Rule {
            id: "phase9-body-replacement".to_owned(),
            revision: 1,
            priority: 100,
            matcher: RuleMatcher {
                path_prefix: Some("/body/modify".to_owned()),
                ..RuleMatcher::default()
            },
            request: RequestActions::default(),
            response: ResponseActions {
                replace_body: Some(b"phase9-effective-body\n".to_vec()),
                ..ResponseActions::default()
            },
        },
    ];
    let candidate = application
        .validate_automation(AutomationRuleSet {
            rules,
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
}

fn autoresponse_rule(id: &str, path: &str, asset: &str) -> Rule {
    Rule {
        id: id.to_owned(),
        revision: 1,
        priority: 100,
        matcher: RuleMatcher {
            path_prefix: Some(path.to_owned()),
            ..RuleMatcher::default()
        },
        request: RequestActions {
            response_asset: Some(asset.to_owned()),
            ..RequestActions::default()
        },
        response: ResponseActions::default(),
    }
}

fn activate_script(application: &Application, script_host: &Path) {
    assert!(script_host.is_file());
    let source = r#"
function header(request: { headers: Array<{ name: string; value: number[] }> }, name: string): string {
  const field = request.headers.find(candidate => candidate.name.toLowerCase() === name);
  return field === undefined ? "" : String.fromCharCode(...field.value);
}

export function onRequestHead(
  _context: unknown,
  request: { path: string; headers: Array<{ name: string; value: number[] }> },
) {
  if (request.path === "/echo/script" && header(request, "x-phase9") === "script") {
    return {
      action: "headers",
      operations: [{ operation: "set", name: "user-agent", value: "Transmog-Phase9" }],
    };
  }
  if (request.path === "/auto/script") {
    return { action: "respond", assetRef: "phase9-small@1" };
  }
  if (request.path === "/script/fail") {
    throw new Error("intentional phase 9 qualification failure");
  }
  return { action: "continue" };
}
"#;
    let candidate = application
        .validate_script(ScriptDraft {
            id: "phase9-script".to_owned(),
            revision: 1,
            source: source.to_owned(),
            handlers: BTreeSet::from([ScriptHandler::RequestHead]),
            prefilter: ScriptPrefilter {
                path_prefix: Some("/".to_owned()),
                ..ScriptPrefilter::default()
            },
            capabilities: ScriptCapabilities {
                write_headers: BTreeSet::from(["user-agent".to_owned()]),
                respond: true,
                ..ScriptCapabilities::default()
            },
            limits: ScriptLimits {
                max_duration_ms: 1_000,
                ..ScriptLimits::default()
            },
            priority: 200,
        })
        .unwrap();
    application
        .activate_script(&candidate.candidate_id)
        .unwrap();
}

async fn start(application: &Application, environment: &Environment) -> String {
    let status = application
        .start_proxy(
            ProxyStartRequest {
                ca_certificate_path: environment.ca_certificate.clone(),
                ca_private_key_path: environment.ca_private_key.clone(),
                listen: "127.0.0.1:0".parse().unwrap(),
                route: ProxyRoute::Http1,
                allow_remote_clients: false,
            },
            None,
        )
        .await
        .unwrap();
    status.listener.expect("bound application listener")
}

async fn wait_for_pause(application: &Application) -> transmog_app::PausedExchange {
    for _ in 0..400 {
        if let Some(paused) = application.paused_exchanges().paused.into_iter().next() {
            return paused;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for interactive breakpoint");
}

async fn wait_for_session(application: &Application, path: &str, terminal: &str) -> SessionDetail {
    for _ in 0..400 {
        let page = application
            .query_sessions(SessionQueryInput {
                limit: Some(100),
                ..SessionQueryInput::default()
            })
            .unwrap();
        if let Some(session) = page
            .sessions
            .into_iter()
            .find(|session| session.path == path && session.terminal == terminal)
        {
            return application.session_detail(&session.id).unwrap();
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {terminal} session at {path}");
}

async fn wait_for_complete_bodies(application: &Application, session_id: &str) {
    for _ in 0..400 {
        let detail = application.session_detail(session_id).unwrap();
        let original = detail.stored_bodies.iter().any(|body| {
            body.boundary == "upstream-response" && body.availability == BodyAvailability::Complete
        });
        let effective = detail.stored_bodies.iter().any(|body| {
            body.boundary == "client-response" && body.availability == BodyAvailability::Complete
        });
        if original && effective {
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for complete retained response boundaries");
}

async fn wait_for_eviction(application: &Application) {
    for _ in 0..400 {
        let store = application.body_store().expect("configured body store");
        if store.counters().evicted_bodies > 0 {
            let page = application
                .query_sessions(SessionQueryInput {
                    limit: Some(100),
                    ..SessionQueryInput::default()
                })
                .unwrap();
            let visible = page.sessions.iter().any(|session| {
                application.session_detail(&session.id).is_ok_and(|detail| {
                    detail
                        .stored_bodies
                        .iter()
                        .any(|body| body.availability == BodyAvailability::Evicted)
                })
            });
            if visible {
                return;
            }
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for visible circular body eviction");
}

fn inspection(session_id: &str, boundary: &str) -> BodyInspectionRequest {
    BodyInspectionRequest {
        session_id: session_id.to_owned(),
        boundary: boundary.to_owned(),
        representation: BodyRepresentation::OriginalText,
        decode_content: true,
        offset: 0,
        max_bytes: Some(256 * 1024),
    }
}

fn content_type(value: &str) -> HeaderBlock {
    HeaderBlock::from_fields(vec![HeaderField::try_new("content-type", value).unwrap()])
}

#[derive(Clone)]
struct Environment {
    curl: PathBuf,
    nginx: String,
    ca_certificate: PathBuf,
    ca_private_key: PathBuf,
    script_host: PathBuf,
    preview_worker: PathBuf,
}

impl Environment {
    fn from_process() -> Self {
        Self {
            curl: required_path("TRANSMOG_CURL"),
            nginx: required("TRANSMOG_NGINX_URL"),
            ca_certificate: required_path("TRANSMOG_TEST_CA_CERT"),
            ca_private_key: required_path("TRANSMOG_TEST_CA_KEY"),
            script_host: required_path("TRANSMOG_SCRIPT_HOST"),
            preview_worker: required_path("TRANSMOG_PREVIEW_WORKER"),
        }
    }

    fn origin(&self, path: &str) -> String {
        format!("{}{path}", self.nginx)
    }
}

struct TestWorkspace {
    root: PathBuf,
    capture: PathBuf,
    json_export: PathBuf,
    saz_export: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("transmog-phase9-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        Self {
            capture: root.join("workflow.tmcap"),
            json_export: root.join("workflow.jsonl"),
            saz_export: root.join("workflow.saz"),
            root,
        }
    }

    fn config(&self, environment: &Environment) -> AppConfig {
        AppConfig {
            product_state_path: Some(self.root.join("product-state")),
            diagnostics_log_path: Some(self.root.join("diagnostics.jsonl")),
            body_store: Some(BodyStoreConfig {
                root: self.root.join("body-cache"),
                mode: RetentionMode::Circular,
                max_bytes: BODY_STORE_BYTES,
                max_body_bytes: 2 * 1024 * 1024,
                queue_capacity: NonZeroUsize::new(256).unwrap(),
                max_read_bytes: 16 * 1024 * 1024,
                retain_requests: false,
            }),
            automation_path: Some(self.root.join("automation")),
            response_asset_root: Some(self.root.join("response-assets")),
            script_workspace_path: Some(self.root.join("scripts")),
            script_host_executable: Some(environment.script_host.clone()),
            preview_worker_executable: Some(environment.preview_worker.clone()),
            ..AppConfig::default()
        }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("transmog-phase9-"))
        {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

struct CurlOutput {
    process: Output,
    status: u16,
    headers: String,
    body: Vec<u8>,
}

async fn curl(
    environment: &Environment,
    root: &Path,
    listener: &str,
    url: &str,
    headers: &[&str],
) -> CurlOutput {
    curl_owned(
        environment.clone(),
        root.to_owned(),
        listener.to_owned(),
        url.to_owned(),
        headers.iter().map(|header| (*header).to_owned()).collect(),
    )
    .await
}

async fn curl_owned(
    environment: Environment,
    root: PathBuf,
    listener: String,
    url: String,
    headers: Vec<String>,
) -> CurlOutput {
    let sequence = CURL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let header_path = root.join(format!("curl-{sequence}.headers"));
    let body_path = root.join(format!("curl-{sequence}.body"));
    let mut command = Command::new(&environment.curl);
    command
        .arg("--silent")
        .arg("--show-error")
        .arg("--http1.1")
        .arg("--connect-timeout")
        .arg("10")
        .arg("--max-time")
        .arg("30")
        .arg("--noproxy")
        .arg("")
        .arg("--proxy")
        .arg(format!("http://{listener}"))
        .arg("--dump-header")
        .arg(&header_path)
        .arg("--output")
        .arg(&body_path)
        .arg("--write-out")
        .arg("%{http_code}");
    for header in headers {
        command.arg("--header").arg(header);
    }
    let process = command.arg(url).output().await.unwrap();
    let status = String::from_utf8_lossy(&process.stdout)
        .trim()
        .parse::<u16>()
        .unwrap_or(0);
    let headers = std::fs::read_to_string(&header_path).unwrap_or_default();
    let body = std::fs::read(&body_path).unwrap_or_default();
    CurlOutput {
        process,
        status,
        headers,
        body,
    }
}

fn assert_success(output: &CurlOutput) {
    assert!(
        output.process.status.success(),
        "curl failed: {}",
        String::from_utf8_lossy(&output.process.stderr)
    );
    assert_eq!(
        output.status,
        200,
        "{}\nresponse body: {}",
        output.headers,
        String::from_utf8_lossy(&output.body)
    );
}

fn assert_header(output: &CurlOutput, name: &str, value: &str) {
    assert!(
        output.headers.lines().any(|line| {
            line.split_once(':').is_some_and(|(candidate, actual)| {
                candidate.eq_ignore_ascii_case(name) && actual.trim() == value
            })
        }),
        "missing {name}: {value} in {}",
        output.headers
    );
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing {name}; run scripts/test-interop.ps1"))
}

fn required_path(name: &str) -> PathBuf {
    PathBuf::from(required(name))
}

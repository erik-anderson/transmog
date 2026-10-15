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
    SessionQueryInput, SessionResponseAsset,
};
use transmog_automation::{
    HeaderCondition, HeaderOperation, HeaderPredicate, RequestActions, ResponseActions, Rule,
    RuleMatcher,
};
use transmog_control_model::{DecisionAction, HeaderField as ControlHeaderField};
use transmog_core::{BodyFrame, HeaderBlock, HeaderField};
use transmog_script::{ScriptCapabilities, ScriptHandler, ScriptLimits, ScriptPrefilter};

const LARGE_ASSET_BYTES: usize = 1024 * 1024 + 1;
const BODY_STORE_BYTES: u64 = 3 * 1024 * 1024;

static CURL_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static WORKSPACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through the interoperability driver with packaged helpers"]
#[allow(clippy::too_many_lines)] // One complete live transformation and historical-evidence workflow.
async fn literal_response_edits_preserve_fresh_data_codings_and_actual_outcomes() {
    use std::sync::Arc;
    use transmog_app::{TraceImportRequest, TraceSaveOptions};
    use transmog_automation::{LiteralTransform, Occurrences};
    let environment = Environment::from_process();
    let workspace = TestWorkspace::new();
    let app = Application::new(workspace.config(&environment)).unwrap();
    let edit = |id: &str,
                path: &str,
                find: &str,
                replacement: &str,
                expected_matches,
                max_bytes,
                media: &str| Rule {
        id: id.into(),
        display_name: Some(id.into()),
        enabled: true,
        revision: 1,
        priority: 0,
        matcher: RuleMatcher {
            path_prefix: Some(path.into()),
            ..Default::default()
        },
        request: RequestActions::default(),
        response: ResponseActions {
            text_transform: Some(LiteralTransform {
                find: find.into(),
                replacement: replacement.into(),
                occurrences: Occurrences::All,
                expected_matches,
                media_type: media.into(),
                encoding: "utf-8".into(),
                max_bytes,
            }),
            ..Default::default()
        },
    };
    let rules = vec![
        edit(
            "fresh-edit",
            "/text-edit/changed/",
            "old",
            "patched",
            Some(2),
            1024,
            "text/plain",
        ),
        edit(
            "not-found",
            "/text-edit/not-found",
            "absent",
            "patch",
            None,
            1024,
            "text/plain",
        ),
        edit(
            "count-failure",
            "/text-edit/count",
            "old",
            "patch",
            Some(99),
            1024,
            "text/plain",
        ),
        edit(
            "bounded",
            "/text-edit/bounded",
            "old",
            "patch",
            None,
            8,
            "text/plain",
        ),
        edit(
            "wrong-media",
            "/text-edit/wrong-media",
            "old",
            "patch",
            None,
            1024,
            "application/json",
        ),
        edit(
            "compressed-edit",
            "/encoding/",
            "through Transmog",
            "through literal editing",
            Some(1),
            1024 * 1024,
            "text/html",
        ),
    ];
    let candidate = app
        .validate_automation(AutomationRuleSet {
            rules,
            autoresponses_enabled: false,
            ..Default::default()
        })
        .unwrap();
    app.activate_automation(&candidate.candidate_id).unwrap();
    let script=app.validate_script(ScriptDraft{id:"before-literal".into(),revision:1,source:r#"
export function onResponseBody(_context: unknown, _request: unknown, _response: unknown, body:{bytes:number[]}) {
 const text=String.fromCharCode(...body.bytes).replace("seed","old");
 return {action:"replace-body",bytes:Array.from(text).map(c=>c.charCodeAt(0))};
}
"#.into(),handlers:BTreeSet::from([ScriptHandler::ResponseBody]),prefilter:ScriptPrefilter{path_prefix:Some("/text-edit/changed/".into()),..Default::default()},capabilities:ScriptCapabilities{read_bodies:true,write_body:true,..Default::default()},limits:ScriptLimits{max_duration_ms:1000,..Default::default()},priority:0}).unwrap();
    app.activate_script(&script.candidate_id).unwrap();
    let listener = start(&app, &environment).await;
    let recording = workspace.root.join("literal-recorded.tmcap");
    app.start_capture(CaptureStartRequest {
        path: recording.clone(),
        password: None,
        max_file_bytes: 64 * 1024 * 1024,
        retain_body_samples: true,
        redact_sensitive_headers: false,
        include_network_context: false,
    })
    .await
    .unwrap();
    let mut originals = Vec::new();
    for token in ["fresh-first", "fresh-other"] {
        let path = format!("text-edit/changed/{token}");
        let output = curl(
            &environment,
            &workspace.root,
            &listener,
            &environment.origin(&path),
            &[&format!("X-Fresh-Token: {token}")],
        )
        .await;
        assert_success(&output);
        assert_eq!(
            output.body,
            format!("patched token={token} patched\n").as_bytes()
        );
        assert_header(&output, "Set-Cookie", "one=1");
        assert_header(&output, "Set-Cookie", "two=2");
        assert_header(&output, "Cache-Control", "max-age=123");
        assert!(!output.headers.to_ascii_lowercase().contains("etag:"));
        let detail = wait_for_session(&app, &format!("/{path}"), "completed").await;
        assert_eq!(detail.body_edits.len(), 1);
        let report = &detail.body_edits[0].report;
        assert_eq!(report.outcome, "applied");
        assert_eq!(report.matches, Some(2));
        assert_eq!(report.replacements, 2);
        assert_ne!(report.before_sha256, report.after_sha256);
        assert_eq!(
            app.inspect_body(inspection(&detail.id, "upstream-response"))
                .await
                .unwrap()
                .display,
            format!("seed token={token} old\n")
        );
        originals.push(detail);
    }
    for (path, outcome, terminal) in [
        ("text-edit/not-found", "not-found", "completed"),
        ("text-edit/count", "count-mismatch", "failed"),
        ("text-edit/bounded", "input-limit", "failed"),
        ("text-edit/wrong-media", "", "completed"),
    ] {
        let output = curl(
            &environment,
            &workspace.root,
            &listener,
            &environment.origin(path),
            &["X-Fresh-Token: untouched"],
        )
        .await;
        let detail = wait_for_session(&app, &format!("/{path}"), terminal).await;
        if terminal == "completed" {
            assert_success(&output);
            assert_eq!(output.body, b"seed token=untouched old\n");
        } else {
            assert!(
                !output.process.status.success(),
                "failed edit appeared successful"
            );
            assert!(!String::from_utf8_lossy(&output.body).contains("patch"));
        }
        if outcome.is_empty() {
            assert!(detail.body_edits.is_empty());
            assert_header(&output, "ETag", "\"fresh-origin\"");
        } else {
            assert_eq!(detail.body_edits[0].report.outcome, outcome);
        }
        originals.push(detail);
    }
    for coding in ["gzip", "br", "deflate", "zstd", "stacked"] {
        let path = format!("encoding/{coding}");
        let output = curl(
            &environment,
            &workspace.root,
            &listener,
            &environment.origin(&path),
            &[],
        )
        .await;
        assert_success(&output);
        assert_header(
            &output,
            "Content-Encoding",
            if coding == "stacked" {
                "gzip, br, deflate, zstd"
            } else {
                coding
            },
        );
        let detail = wait_for_session(&app, &format!("/{path}"), "completed").await;
        assert_eq!(detail.body_edits[0].report.outcome, "applied");
        assert!(
            app.inspect_body(inspection(&detail.id, "client-response"))
                .await
                .unwrap()
                .display
                .contains("through literal editing")
        );
        assert!(
            app.inspect_body(inspection(&detail.id, "upstream-response"))
                .await
                .unwrap()
                .display
                .contains("through Transmog")
        );
        originals.push(detail);
    }
    for suffix in ["truncated", "unsupported"] {
        let path = format!("encoding/{suffix}");
        let output = curl(
            &environment,
            &workspace.root,
            &listener,
            &environment.origin(&path),
            &[],
        )
        .await;
        assert!(!output.process.status.success() || output.status >= 500);
        let detail = wait_for_session(&app, &format!("/{path}"), "failed").await;
        assert!(
            detail.body_edits.is_empty(),
            "codec failure claimed a completed operation"
        );
    }
    app.stop_capture().await.unwrap();
    let saved = workspace.root.join("literal-saved.tmcap");
    app.save_traffic_trace(saved.clone(), TraceSaveOptions::default())
        .await
        .unwrap();
    app.shutdown().await.unwrap();
    let restarted = Application::new(workspace.config(&environment)).unwrap();
    assert_eq!(restarted.automation_status().rules.len(), 6);
    restarted.shutdown().await.unwrap();
    for path in [recording, saved] {
        let imported_workspace = TestWorkspace::new();
        let imported = Application::new(imported_workspace.config(&environment)).unwrap();
        imported
            .import_trace(
                TraceImportRequest {
                    path,
                    password: None,
                    operation_id: "literal-import".into(),
                    max_file_bytes: u64::MAX,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        let rows = imported
            .query_sessions(SessionQueryInput {
                limit: Some(100),
                ..Default::default()
            })
            .unwrap();
        for original in &originals {
            let row = rows
                .sessions
                .iter()
                .find(|row| {
                    original.requests[0]
                        .target
                        .as_ref()
                        .unwrap()
                        .ends_with(&row.path)
                })
                .unwrap();
            assert_eq!(
                imported.session_detail(&row.id).unwrap().body_edits,
                original.body_edits
            );
        }
        imported.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through the interoperability driver with packaged helpers"]
#[allow(clippy::too_many_lines)] // Complete live composition and historical evidence workflow.
async fn header_rules_compose_with_scripts_and_preserve_fresh_response_evidence() {
    use std::sync::Arc;
    use transmog_app::{TraceImportRequest, TraceSaveOptions};
    let environment = Environment::from_process();
    let workspace = TestWorkspace::new();
    let application = Application::new(workspace.config(&environment)).unwrap();
    let field = |name: &str, value: &str| HeaderField::try_new(name, value).unwrap();
    let rule = |id: &str, priority: i32, request, response| Rule {
        id: id.into(),
        display_name: Some(id.into()),
        enabled: true,
        revision: 1,
        priority,
        matcher: RuleMatcher {
            path_prefix: Some("/echo/header-iteration/".into()),
            ..RuleMatcher::default()
        },
        request: RequestActions {
            headers: request,
            ..RequestActions::default()
        },
        response: ResponseActions {
            headers: response,
            ..ResponseActions::default()
        },
    };
    let request = rule(
        "native-request",
        0,
        vec![HeaderOperation::Set(field("User-Agent", "native-agent"))],
        vec![],
    );
    let mut response = rule(
        "native-response",
        1,
        vec![],
        vec![
            HeaderOperation::Set(field("X-Order", "native-last")),
            HeaderOperation::Set(field("Cache-Control", "max-age=123")),
            HeaderOperation::Append(field("Set-Cookie", "one=1")),
            HeaderOperation::Append(field("Set-Cookie", "two=2")),
            HeaderOperation::Remove("Server".into()),
        ],
    );
    response.matcher.response_status = Some(200);
    response.matcher.response_headers = vec![HeaderPredicate {
        name: "Content-Type".into(),
        condition: HeaderCondition::Prefix(b"text/plain".to_vec()),
    }];
    let noop = rule(
        "native-noop",
        2,
        vec![],
        vec![HeaderOperation::Remove("X-Absent".into())],
    );
    let asset = application
        .create_response_asset(AuthoredResponseAsset {
            id: "header-local".into(),
            revision: 1,
            status: 200,
            headers: HeaderBlock::new(),
            body: b"local bytes".to_vec(),
            media_type: Some("text/plain".into()),
        })
        .unwrap();
    let mut local = autoresponse_rule(
        "header-local-rule",
        "/echo/header-iteration/local",
        &asset.asset_ref(),
    );
    local.priority = -100;
    let candidate = application
        .validate_automation(AutomationRuleSet {
            rules: vec![request, response, noop, local],
            autoresponses_enabled: false,
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
    let script = application.validate_script(ScriptDraft {
        id: "header-script".into(), revision: 1,
        source: r#"
export function onRequestHead(_context: unknown, request: {headers: Array<{name:string;value:number[]}>}) {
  const token = request.headers.find(h => h.name.toLowerCase() === "x-fresh-token");
  const native = request.headers.find(h => h.name.toLowerCase() === "user-agent");
  if (!token || !native || String.fromCharCode(...native.value) !== "native-agent") throw new Error("native request did not run first");
  return {action:"headers",operations:[{operation:"set",name:"user-agent",value:String.fromCharCode(...token.value)}]};
}
export function onResponseHead() {
  return {action:"headers",operations:[{operation:"set",name:"x-order",value:"script-first"}]};
}
"#.into(),
        handlers: BTreeSet::from([ScriptHandler::RequestHead, ScriptHandler::ResponseHead]),
        prefilter: ScriptPrefilter {path_prefix: Some("/echo/header-iteration/".into()), ..ScriptPrefilter::default()},
        capabilities: ScriptCapabilities {write_headers: BTreeSet::from(["user-agent".into(), "x-order".into()]), ..ScriptCapabilities::default()},
        limits: ScriptLimits {max_duration_ms: 1_000, ..ScriptLimits::default()}, priority: 200,
    }).unwrap();
    application.activate_script(&script.candidate_id).unwrap();
    let listener = start(&application, &environment).await;
    let recording = workspace.root.join("header-recording.tmcap");
    application
        .start_capture(CaptureStartRequest {
            path: recording.clone(),
            password: None,
            max_file_bytes: 64 * 1024 * 1024,
            retain_body_samples: true,
            redact_sensitive_headers: false,
            include_network_context: false,
        })
        .await
        .unwrap();
    let mut originals = Vec::new();
    for token in ["fresh-first", "fresh-other"] {
        let path = format!("echo/header-iteration/{token}");
        let output = curl(
            &environment,
            &workspace.root,
            &listener,
            &environment.origin(&path),
            &[&format!("X-Fresh-Token: {token}")],
        )
        .await;
        assert_success(&output);
        assert_eq!(
            output.body,
            format!("{token}\n").as_bytes(),
            "header editing replaced fresh origin bytes"
        );
        assert_header(&output, "X-Order", "native-last");
        assert_header(&output, "X-Seen-User-Agent", token);
        assert_header(&output, "Cache-Control", "max-age=123");
        assert_header(&output, "Set-Cookie", "one=1");
        assert_header(&output, "Set-Cookie", "two=2");
        assert!(
            !output
                .headers
                .lines()
                .any(|h| h.to_ascii_lowercase().starts_with("server:"))
        );
        let detail = wait_for_session(&application, &format!("/{path}"), "completed").await;
        assert_eq!(
            detail.header_effects.len(),
            5,
            "{:#?}",
            detail.header_effects
        );
        assert_eq!(detail.header_effects[0].name, "native-request");
        assert!(detail.header_effects[1].hook_id.contains("header-script"));
        assert!(detail.header_effects[2].hook_id.contains("header-script"));
        assert_eq!(detail.header_effects[3].name, "native-response");
        assert_eq!(detail.header_effects[4].name, "native-noop");
        assert!(!detail.header_effects[4].changed);
        assert!(
            detail
                .header_effects
                .windows(2)
                .all(|p| p[0].sequence < p[1].sequence)
        );
        assert_eq!(
            application
                .inspect_body(inspection(&detail.id, "client-response"))
                .await
                .unwrap()
                .display,
            format!("{token}\n")
        );
        originals.push(detail);
    }
    let current = application.automation_status();
    let candidate = application
        .validate_automation(AutomationRuleSet {
            generation: current.generation,
            rules: current.rules,
            autoresponses_enabled: true,
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
    let output = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/header-iteration/local"),
        &[],
    )
    .await;
    assert_success(&output);
    assert_eq!(output.body, b"local bytes");
    let local = wait_for_session(&application, "/echo/header-iteration/local", "completed").await;
    assert!(
        local.header_effects.is_empty(),
        "local response entered later native/script hooks"
    );
    assert!(local.route_attempts.is_empty());
    application.stop_capture().await.unwrap();
    let saved = workspace.root.join("header-saved.tmcap");
    application
        .save_traffic_trace(saved.clone(), TraceSaveOptions::default())
        .await
        .unwrap();
    application.shutdown().await.unwrap();
    let restarted = Application::new(workspace.config(&environment)).unwrap();
    assert_eq!(restarted.automation_status().rules.len(), 4);
    restarted.shutdown().await.unwrap();
    for path in [recording, saved] {
        let imported_workspace = TestWorkspace::new();
        let imported = Application::new(imported_workspace.config(&environment)).unwrap();
        imported
            .import_trace(
                TraceImportRequest {
                    path,
                    password: None,
                    operation_id: "header-evidence".into(),
                    max_file_bytes: u64::MAX,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert!(imported.automation_status().rules.is_empty());
        let rows = imported
            .query_sessions(SessionQueryInput {
                limit: Some(100),
                ..SessionQueryInput::default()
            })
            .unwrap();
        for original in &originals {
            let row = rows
                .sessions
                .iter()
                .find(|row| {
                    original.requests[0]
                        .target
                        .as_ref()
                        .unwrap()
                        .ends_with(&row.path)
                })
                .unwrap();
            let detail = imported.session_detail(&row.id).unwrap();
            assert_eq!(detail.header_effects, original.header_effects);
            assert_eq!(
                imported
                    .inspect_body(inspection(&row.id, "client-response"))
                    .await
                    .unwrap()
                    .display,
                application
                    .inspect_body(inspection(&original.id, "client-response"))
                    .await
                    .unwrap()
                    .display
            );
            assert!(
                detail
                    .responses
                    .iter()
                    .any(|r| r.boundary == "upstream-response")
            );
            assert!(
                detail
                    .responses
                    .iter()
                    .any(|r| r.boundary == "client-response")
            );
        }
        imported.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through the interoperability driver with packaged helpers"]
#[allow(clippy::too_many_lines)] // One complete scoped activation and restart workflow.
async fn named_variants_switch_live_atomically_and_keep_saved_history() {
    use transmog_app::{ExperimentChange, ExperimentSelection};
    let environment = Environment::from_process();
    let workspace = TestWorkspace::new();
    let application = Application::new(workspace.config(&environment)).unwrap();
    let first = application
        .create_response_asset(AuthoredResponseAsset {
            id: "variant-body".into(),
            revision: 1,
            status: 200,
            headers: HeaderBlock::new(),
            body: b"first variant".to_vec(),
            media_type: None,
        })
        .unwrap();
    let second = application
        .create_response_asset(AuthoredResponseAsset {
            id: "variant-body".into(),
            revision: 2,
            status: 200,
            headers: HeaderBlock::new(),
            body: b"second variant".to_vec(),
            media_type: None,
        })
        .unwrap();
    let unrelated = autoresponse_rule("unrelated", "/auto/unrelated", &first.asset_ref());
    let candidate = application
        .validate_automation(AutomationRuleSet {
            rules: vec![unrelated.clone()],
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
    let original = autoresponse_rule("variant-rule", "/echo/variant/", &first.asset_ref());
    let change = |change| {
        application
            .change_experiment(application.automation_status().generation, change)
            .unwrap()
    };
    change(ExperimentChange::Create {
        id: "experiment".into(),
        name: "Response hypothesis".into(),
        variant_id: "edited".into(),
        variant_name: "Patched script".into(),
        rules: vec![original.clone()],
    });
    let selection = |revision| ExperimentSelection {
        experiment_id: "experiment".into(),
        variant_id: "edited".into(),
        revision,
    };
    change(ExperimentChange::Activate {
        selection: selection(1),
        acknowledge_overlaps: false,
    });
    let listener = start(&application, &environment).await;
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/variant/first"),
        &[],
    )
    .await;
    assert_success(&response);
    assert_eq!(response.body, b"first variant");
    let mut edited = original.clone();
    edited.revision = 2;
    edited.request.response_asset = Some(second.asset_ref());
    change(ExperimentChange::Save {
        id: "experiment".into(),
        variant_id: "edited".into(),
        expected_revision: 1,
        rules: vec![edited.clone()],
    });
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/variant/saved"),
        &[],
    )
    .await;
    assert_eq!(response.body, b"first variant");
    change(ExperimentChange::Activate {
        selection: selection(2),
        acknowledge_overlaps: false,
    });
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/variant/second"),
        &[],
    )
    .await;
    assert_success(&response);
    assert_eq!(response.body, b"second variant");
    let first_detail = wait_for_session(&application, "/echo/variant/first", "completed").await;
    let second_detail = wait_for_session(&application, "/echo/variant/second", "completed").await;
    assert_eq!(
        first_detail.auto_response.as_ref().unwrap().rule_revision,
        1
    );
    assert_eq!(
        second_detail.auto_response.as_ref().unwrap().rule_revision,
        2
    );
    assert!(
        first_detail
            .hook_effects
            .iter()
            .any(|effect| effect.contains("variant-rule@1"))
    );
    assert!(
        second_detail
            .hook_effects
            .iter()
            .any(|effect| effect.contains("variant-rule@2"))
    );
    let before_pause = application.automation_status();
    change(ExperimentChange::Pause {
        id: "experiment".into(),
        paused: true,
    });
    assert_eq!(application.automation_status().rules, before_pause.rules);
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/variant/paused"),
        &["User-Agent: Baseline-Agent"],
    )
    .await;
    assert_success(&response);
    assert_eq!(response.body, b"Baseline-Agent\n");
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/unrelated"),
        &[],
    )
    .await;
    assert_eq!(response.body, b"first variant");
    change(ExperimentChange::Pause {
        id: "experiment".into(),
        paused: false,
    });
    let baseline = ExperimentSelection {
        experiment_id: "experiment".into(),
        variant_id: "baseline".into(),
        revision: 1,
    };
    change(ExperimentChange::Activate {
        selection: baseline.clone(),
        acknowledge_overlaps: false,
    });
    assert_eq!(
        application.automation_status().rules,
        vec![unrelated.clone()]
    );
    change(ExperimentChange::UndoActivation {
        id: "experiment".into(),
    });
    assert_eq!(
        application.automation_status().rules,
        vec![unrelated.clone(), edited]
    );
    application
        .set_autoresponses_enabled(false, application.automation_status().generation)
        .unwrap();
    change(ExperimentChange::Activate {
        selection: selection(1),
        acknowledge_overlaps: false,
    });
    assert!(!application.automation_status().autoresponses_enabled);
    application
        .set_autoresponses_enabled(true, application.automation_status().generation)
        .unwrap();
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/variant/historical"),
        &[],
    )
    .await;
    assert_eq!(response.body, b"first variant");
    change(ExperimentChange::Activate {
        selection: baseline,
        acknowledge_overlaps: false,
    });
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/variant/baseline"),
        &["User-Agent: Baseline-Agent"],
    )
    .await;
    assert_eq!(response.body, b"Baseline-Agent\n");
    let saved = application.automation_status().experiments;
    application.shutdown().await.unwrap();
    drop(application);
    let restarted = Application::new(workspace.config(&environment)).unwrap();
    assert_eq!(restarted.automation_status().experiments, saved);
    assert_eq!(restarted.automation_status().rules, vec![unrelated]);
    restarted
        .change_experiment(
            restarted.automation_status().generation,
            ExperimentChange::Activate {
                selection: selection(2),
                acknowledge_overlaps: false,
            },
        )
        .unwrap();
    let listener = start(&restarted, &environment).await;
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("echo/variant/restarted"),
        &[],
    )
    .await;
    assert_success(&response);
    assert_eq!(response.body, b"second variant");
    restarted.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through the interoperability driver with packaged helpers"]
#[allow(clippy::too_many_lines)] // One complete acquisition/recording/save/import workflow.
async fn linked_snapshot_history_survives_external_edits_and_native_save() {
    use std::sync::Arc;
    use transmog_app::comparison::{ComparisonOptions, ComparisonRequest, ComparisonSubject};
    use transmog_app::{TraceImportRequest, TraceSaveOptions};
    let environment = Environment::from_process();
    let workspace = TestWorkspace::new();
    let path = workspace.root.join("iteration.css");
    std::fs::write(&path, b"first version").unwrap();
    let application = Application::new(workspace.config(&environment)).unwrap();
    let asset = application
        .import_response_asset(ImportResponseAsset {
            id: "iteration".into(),
            revision: 1,
            status: 200,
            headers: HeaderBlock::new(),
            body_path: path.clone(),
            body_file_mode: transmog_app::ResponseBodyFileMode::Linked,
            media_type: Some("text/css".into()),
        })
        .unwrap();
    let candidate = application
        .validate_automation(AutomationRuleSet {
            rules: vec![autoresponse_rule(
                "iteration-rule",
                "/auto/history/",
                &asset.asset_ref(),
            )],
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
    let listener = start(&application, &environment).await;
    let recorded = workspace.root.join("recorded-iterations.tmcap");
    application
        .start_capture(CaptureStartRequest {
            redact_sensitive_headers: false,
            password: None,
            include_network_context: false,
            path: recorded.clone(),
            max_file_bytes: 64 * 1024 * 1024,
            retain_body_samples: true,
        })
        .await
        .unwrap();
    let mut results = Vec::new();
    for (index, body) in [b"first version".as_slice(), b"other version".as_slice()]
        .into_iter()
        .enumerate()
    {
        let replacement = workspace.root.join("atomic-save.css");
        std::fs::write(&replacement, body).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        let suffix = format!("auto/history/{index}");
        let response = curl(
            &environment,
            &workspace.root,
            &listener,
            &environment.origin(&suffix),
            &[],
        )
        .await;
        assert_success(&response);
        assert_eq!(response.body, body);
        let detail = wait_for_session(&application, &format!("/{suffix}"), "completed").await;
        let matched = detail.auto_response.as_ref().unwrap();
        assert_eq!(matched.asset_reference, asset.asset_ref());
        let snapshot = matched.snapshot.as_ref().unwrap();
        let captured = detail
            .stored_bodies
            .iter()
            .find(|b| b.boundary == "client-response")
            .unwrap();
        assert_eq!(captured.sha256.as_deref(), Some(snapshot.sha256.as_str()));
        assert_eq!(snapshot.body_bytes, body.len() as u64);
        assert!(detail.route_attempts.is_empty());
        results.push(detail);
    }
    assert_ne!(
        results[0].auto_response.as_ref().unwrap().snapshot,
        results[1].auto_response.as_ref().unwrap().snapshot
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        application
            .inspect_body(inspection(&results[0].id, "client-response"))
            .await
            .unwrap()
            .display,
        "first version"
    );
    let subject = |id: &str| ComparisonSubject::Exchange {
        id: id.into(),
        request_boundary: "client-request".into(),
        response_boundary: "client-response".into(),
    };
    let comparison = application
        .compare_evidence(
            ComparisonRequest {
                operation_id: "historical-files".into(),
                left: subject(&results[0].id),
                right: subject(&results[1].id),
                options: ComparisonOptions::default(),
            },
            [None, None],
        )
        .await
        .unwrap();
    assert!(comparison.complete && comparison.different);
    assert!(
        comparison
            .left
            .notes
            .iter()
            .any(|note| note.contains("external file is never reopened"))
    );
    application.stop_capture().await.unwrap();
    let saved = workspace.root.join("iterations.tmcap");
    application
        .save_traffic_trace(saved.clone(), TraceSaveOptions::default())
        .await
        .unwrap();
    application.shutdown().await.unwrap();
    for saved in [saved, recorded] {
        let imported = Application::new(workspace.config(&environment)).unwrap();
        imported
            .import_trace(
                TraceImportRequest {
                    path: saved,
                    password: None,
                    operation_id: "iterations-import".into(),
                    max_file_bytes: u64::MAX,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        let entries = imported
            .query_sessions(SessionQueryInput {
                limit: Some(100),
                ..SessionQueryInput::default()
            })
            .unwrap();
        for original in &results {
            let old_path = original.requests[0].target.as_ref().unwrap();
            let row = entries
                .sessions
                .iter()
                .find(|row| old_path.ends_with(&row.path))
                .unwrap();
            let detail = imported.session_detail(&row.id).unwrap();
            assert_eq!(
                detail.auto_response.as_ref().unwrap().snapshot,
                original.auto_response.as_ref().unwrap().snapshot
            );
            assert_eq!(
                imported
                    .inspect_body(inspection(&row.id, "client-response"))
                    .await
                    .unwrap()
                    .display,
                application
                    .inspect_body(inspection(&original.id, "client-response"))
                    .await
                    .unwrap()
                    .display
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-interop.ps1 with Docker and packaged helpers"]
#[allow(clippy::too_many_lines)]
async fn linked_response_streams_compression_and_recovers_after_a_missing_file() {
    let environment = Environment::from_process();
    let workspace = TestWorkspace::new();
    let path = workspace.root.join("linked-response.txt");
    std::fs::write(&path, b"small linked response").unwrap();
    let application = Application::new(workspace.config(&environment)).unwrap();
    let mut encoder = transmog_content::ContentEncoder::new(
        transmog_content::ContentCoding::Gzip,
        transmog_content::ContentLimits::default(),
    )
    .unwrap();
    let mut frames = encoder
        .on_frame(BodyFrame::Data(b"original".to_vec().into()))
        .await
        .unwrap();
    frames.extend(encoder.finish().await.unwrap());
    let compressed_body = frames
        .into_iter()
        .filter_map(|frame| match frame {
            BodyFrame::Data(bytes) => Some(bytes.to_vec()),
            BodyFrame::Trailers(_) => None,
        })
        .flatten()
        .collect();
    let source = application
        .create_response_asset(AuthoredResponseAsset {
            id: "linked-source".into(),
            revision: 1,
            status: 200,
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("content-encoding", "gzip").unwrap(),
            ]),
            body: compressed_body,
            media_type: Some("text/plain; charset=utf-8".into()),
        })
        .unwrap();
    let linked = application
        .edit_response_asset(transmog_app::ResponseAssetEdit {
            asset_reference: source.asset_ref(),
            status: 200,
            headers: HeaderBlock::new(),
            media_type: source.media_type,
            decoded_body: None,
            body_path: Some(path.clone()),
            body_file_mode: transmog_app::ResponseBodyFileMode::Linked,
            preserve_content_encoding: true,
        })
        .await
        .unwrap();
    assert_eq!(linked.linked_body.as_ref().unwrap().path, path);
    let candidate = application
        .validate_automation(AutomationRuleSet {
            rules: vec![autoresponse_rule(
                "linked-rule",
                "/auto/linked/",
                &linked.asset_ref(),
            )],
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
    std::fs::write(&path, "First external edit").unwrap();
    let listener = start(&application, &environment).await;
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/linked/one"),
        &[],
    )
    .await;
    assert_success(&response);
    assert_header(&response, "content-encoding", "gzip");
    assert_header(&response, "transfer-encoding", "chunked");
    assert!(
        !response
            .headers
            .to_ascii_lowercase()
            .contains("content-length:")
    );
    let detail = wait_for_session(&application, "/auto/linked/one", "completed").await;
    assert_eq!(
        application
            .inspect_body(inspection(&detail.id, "client-response"))
            .await
            .unwrap()
            .display,
        "First external edit"
    );
    let selected = detail
        .auto_response
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap();
    let captured = detail
        .stored_bodies
        .iter()
        .find(|body| body.boundary == "client-response")
        .unwrap();
    assert_eq!(captured.sha256.as_deref(), Some(selected.sha256.as_str()));
    assert_eq!(selected.content_codings, ["gzip"]);
    assert!(detail.route_attempts.is_empty());
    application.shutdown().await.unwrap();
    drop(application);
    std::fs::remove_file(&path).unwrap();
    let restarted = Application::new(workspace.config(&environment)).unwrap();
    assert_eq!(restarted.automation_status().rules.len(), 1);
    let listener = start(&restarted, &environment).await;
    let missing = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/linked/missing"),
        &[],
    )
    .await;
    assert_ne!(missing.status, 200);
    let detail = wait_for_session(&restarted, "/auto/linked/missing", "failed").await;
    assert!(detail.route_attempts.is_empty());
    assert!(
        detail
            .responses
            .iter()
            .all(|head| head.boundary != "upstream-response")
    );
    assert!(
        restarted
            .response_body_file_info(&path, Some(&linked.asset_ref()))
            .is_err()
    );
    std::fs::write(&path, "Second external edit with a different size").unwrap();
    let recovered = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/linked/two"),
        &[],
    )
    .await;
    assert_success(&recovered);
    assert_header(&recovered, "content-encoding", "gzip");
    let detail = wait_for_session(&restarted, "/auto/linked/two", "completed").await;
    assert_eq!(
        restarted
            .inspect_body(inspection(&detail.id, "client-response"))
            .await
            .unwrap()
            .display,
        "Second external edit with a different size"
    );
    assert!(
        restarted
            .response_body_file_info(&path, Some(&linked.asset_ref()))
            .unwrap()
            .error
            .is_none()
    );
    restarted.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-response-file-modes.py with packaged helpers"]
async fn explicit_file_copy_above_old_cutoff_serves_saved_bytes() {
    let environment = Environment::from_process();
    let workspace = TestWorkspace::new();
    let path = workspace.root.join("copied-response.bin");
    let size = 25_000_001;
    std::fs::File::create(&path).unwrap().set_len(size).unwrap();
    let application = Application::new(workspace.config(&environment)).unwrap();
    let asset = application
        .import_response_asset(ImportResponseAsset {
            id: "explicit-copy".into(),
            revision: 1,
            status: 200,
            headers: HeaderBlock::new(),
            body_path: path.clone(),
            body_file_mode: transmog_app::ResponseBodyFileMode::Copy,
            media_type: Some("application/octet-stream".into()),
        })
        .unwrap();
    assert!(asset.linked_body.is_none());
    let candidate = application
        .validate_automation(AutomationRuleSet {
            rules: vec![autoresponse_rule(
                "copy-rule",
                "/auto/copied/",
                &asset.asset_ref(),
            )],
            ..AutomationRuleSet::default()
        })
        .unwrap();
    application
        .activate_automation(&candidate.candidate_id)
        .unwrap();
    std::fs::write(&path, b"external edit must not be served").unwrap();
    let listener = start(&application, &environment).await;
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/copied/one"),
        &[],
    )
    .await;
    assert_success(&response);
    assert_eq!(response.body.len() as u64, size);
    assert!(response.body.iter().all(|byte| *byte == 0));
    assert_header(&response, "content-length", &size.to_string());
    assert!(
        wait_for_session(&application, "/auto/copied/one", "completed")
            .await
            .route_attempts
            .is_empty()
    );
    application.shutdown().await.unwrap();
    drop(application);
    std::fs::remove_file(path).unwrap();
    let restarted = Application::new(workspace.config(&environment)).unwrap();
    let listener = start(&restarted, &environment).await;
    let response = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("auto/copied/two"),
        &[],
    )
    .await;
    assert_success(&response);
    assert_eq!(response.body.len() as u64, size);
    assert!(response.body.iter().all(|byte| *byte == 0));
    assert!(
        wait_for_session(&restarted, "/auto/copied/two", "completed")
            .await
            .route_attempts
            .is_empty()
    );
    restarted.shutdown().await.unwrap();
}

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
            body_file_mode: transmog_app::ResponseBodyFileMode::Copy,
            media_type: Some("application/octet-stream".to_owned()),
        })
        .unwrap();
    assert_eq!(large.body_bytes, LARGE_ASSET_BYTES as u64);

    activate_native_rules(&application, &small.asset_ref(), &large.asset_ref());
    activate_script(&application, &environment.script_host);
    let listener = start(&application, &environment).await;
    application
        .start_capture(CaptureStartRequest {
            redact_sensitive_headers: false,
            password: None,
            include_network_context: false,
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
    assert_complete_response_bodies(&changed_detail);
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
    let captured = application
        .create_response_asset_from_session(SessionResponseAsset {
            id: "phase9-captured".to_owned(),
            revision: 1,
            exchange_id: changed_detail.id.clone(),
            boundary: "client-response".to_owned(),
            decoded_body: None,
            body_path: None,
            body_file_mode: transmog_app::ResponseBodyFileMode::Copy,
            preserve_content_encoding: true,
        })
        .await
        .expect("a just-completed client response should be immediately reusable");
    assert_eq!(captured.status, changed.status);
    assert_eq!(captured.body_bytes, changed.body.len() as u64);

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
    assert_complete_response_bodies(&image_detail);
    let preview = application
        .inspect_body(BodyInspectionRequest {
            session_id: image_detail.id.clone(),
            boundary: "client-response".to_owned(),
            representation: BodyRepresentation::Auto,
            decode_content: true,
            offset: 0,
            max_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(preview.representation, "image");
    let handle = preview
        .preview_handle
        .expect("automatic image preview handle");
    let normalized = application.image_preview(&handle).expect("cached preview");
    assert_eq!(normalized.1, "image/png");
    assert!(normalized.0.starts_with(b"\x89PNG\r\n\x1a\n"));
    let image_file = application
        .prepare_response_file(&image_detail.id, "client-response")
        .unwrap();
    assert_eq!(image_file.suggested_name(), "image.png");
    let image_path = workspace.root.join("saved-image.png");
    image_file.save_to(image_path.clone()).await.unwrap();
    assert_eq!(std::fs::read(image_path).unwrap(), image.body);

    let encoded = curl(
        &environment,
        &workspace.root,
        &listener,
        &environment.origin("encoding/stacked"),
        &[],
    )
    .await;
    assert_success(&encoded);
    assert_header(&encoded, "content-encoding", "gzip, br, deflate, zstd");
    let encoded_detail = wait_for_session(&application, "/encoding/stacked", "completed").await;
    let decoded_file = application
        .prepare_response_file(&encoded_detail.id, "client-response")
        .unwrap();
    assert_eq!(decoded_file.suggested_name(), "stacked.html");
    let decoded_path = workspace.root.join("saved-decoded.html");
    decoded_file.save_to(decoded_path.clone()).await.unwrap();
    assert_eq!(
        std::fs::read(decoded_path).unwrap(),
        concat!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">",
            "<title>gzip-br-deflate-zstd interop fixture</title></head>",
            "<body><h1 data-origin=\"nginx\" data-coding=\"gzip-br-deflate-zstd\">",
            "gzip-br-deflate-zstd through Transmog</h1></body></html>"
        )
        .as_bytes()
    );

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
            password: None,
            source_password: None,
            redact_sensitive_headers: false,
            source: workspace.capture.clone(),
            destination: workspace.json_export.clone(),
            format: ExportFormat::JsonLines,
            max_source_bytes: 64 * 1024 * 1024,
        })
        .await
        .unwrap();
    let saz = application
        .export_capture(ExportRequest {
            password: None,
            source_password: None,
            redact_sensitive_headers: false,
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
    assert_eq!(restarted.response_assets().len(), 3);
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
            display_name: None,
            enabled: true,
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
            display_name: None,
            enabled: true,
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
        display_name: None,
        enabled: true,
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
    // Docker origins use the harness's private CA from the configured OS trust
    // source. The production default intentionally uses public Chromium roots.
    let catalog = application.product_state().preferences.network_profiles;
    if catalog.active_profile.settings.root_store != transmog_app::RootStore::Os {
        let mut profile = catalog.active_profile.clone();
        profile.id = "custom-interop-os".into();
        profile.name = "Interoperability OS roots".into();
        profile.settings.root_store = transmog_app::RootStore::Os;
        let mut profiles = catalog.custom_profiles;
        profiles.push(profile.clone());
        application
            .save_network_profiles(catalog.revision, profiles, Some(profile.id))
            .await
            .unwrap();
    }
    let status = application
        .start_proxy(
            ProxyStartRequest {
                recording_only: false,
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

fn assert_complete_response_bodies(detail: &SessionDetail) {
    assert!(
        detail
            .requests
            .iter()
            .any(|head| head.boundary == "client-request"),
        "completed session detail did not expose its client request: {detail:#?}"
    );
    assert!(
        detail
            .responses
            .iter()
            .any(|head| head.boundary == "client-response"),
        "completed session detail did not expose its client response: {detail:#?}"
    );
    for boundary in ["upstream-response", "client-response"] {
        assert!(
            detail.stored_bodies.iter().any(|body| {
                body.boundary == boundary && body.availability == BodyAvailability::Complete
            }),
            "completed session detail did not synchronize {boundary}: {detail:#?}"
        );
    }
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
        let sequence = WORKSPACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "transmog-phase9-{}-{nonce}-{sequence}",
            std::process::id()
        ));
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
                storage: transmog_app::BufferStorage::Memory,
                use_product_preferences: false,
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

use super::tests::{manifest, metadata, request};
use super::*;
use transmog_core::{
    BodyFrame, HttpLegVersion,
    intercept::{
        BodyPipelineLimits, BufferedHead, CompletedExchange, HookEffectAction, HookLimits,
        InterceptorChainFactory, RequestHeadOutcome,
    },
};

struct Runner<F>(F);
impl<F> ScriptRunner for Runner<F>
where
    F: Fn(ScriptInvocation) -> ScriptResult + Send + Sync,
{
    fn invoke(&self, event: ScriptInvocation) -> BoxScriptFuture<'_> {
        let result = (self.0)(event);
        Box::pin(async move { Ok(result) })
    }
}

#[test]
fn decisions_are_phase_specific_and_all_evidence_is_validated() {
    let mut manifest = manifest("", &[ScriptHandler::RequestHead]);
    manifest.capabilities.rewrite_request = true;
    manifest.capabilities.write_status = true;
    manifest.capabilities.write_body = true;
    manifest.capabilities.respond = true;
    manifest.capabilities.annotate = true;
    let edit = |edits| ScriptResult::from(ScriptAction::Edit { edits });
    let url = edit(ScriptEdits {
        url: Some("https://example.test/new?q=1".into()),
        ..Default::default()
    });
    assert!(validate_result(&manifest, ScriptHandler::RequestHead, &request(), &url).is_ok());
    assert!(validate_result(&manifest, ScriptHandler::RequestBody, &request(), &url).is_err());
    let cross_origin = edit(ScriptEdits {
        url: Some("https://other.test/".into()),
        ..Default::default()
    });
    assert!(
        validate_result(
            &manifest,
            ScriptHandler::RequestHead,
            &request(),
            &cross_origin
        )
        .is_err()
    );
    manifest.capabilities.reroute = true;
    assert!(
        validate_result(
            &manifest,
            ScriptHandler::RequestHead,
            &request(),
            &cross_origin
        )
        .is_ok()
    );
    for name in [
        "content-length",
        "transfer-encoding",
        "host",
        "content-encoding",
    ] {
        manifest.capabilities.write_headers.insert(name.into());
        let result = edit(ScriptEdits {
            headers: vec![ScriptHeaderOperation::Set {
                name: name.into(),
                value: "1".into(),
            }],
            ..Default::default()
        });
        assert!(
            validate_result(&manifest, ScriptHandler::ResponseBody, &request(), &result).is_err()
        );
    }
    let status = edit(ScriptEdits {
        status: Some(201),
        bytes: Some(vec![1]),
        ..Default::default()
    });
    assert!(validate_result(&manifest, ScriptHandler::ResponseBody, &request(), &status).is_ok());
    for phase in [
        ScriptHandler::RequestHead,
        ScriptHandler::RequestBody,
        ScriptHandler::ResponseHead,
        ScriptHandler::Completed,
        ScriptHandler::Failed,
    ] {
        assert!(validate_result(&manifest, phase, &request(), &status).is_err());
    }
    let mut report = ScriptResult::from(ScriptAction::Continue);
    report.annotations.push(ScriptAnnotation {
        note: "Finding".into(),
        tags: vec!["test".into()],
        severity: ScriptSeverity::Warning,
    });
    assert!(validate_result(&manifest, ScriptHandler::Completed, &request(), &report).is_ok());
    manifest.capabilities.annotate = false;
    assert!(validate_result(&manifest, ScriptHandler::Completed, &request(), &report).is_err());
    report.annotations.clear();
    report.state = Some(BTreeMap::from([(
        "large".into(),
        serde_json::Value::String("x".repeat(65_536)),
    )]));
    assert!(validate_result(&manifest, ScriptHandler::RequestHead, &request(), &report).is_err());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn state_parameters_body_edits_and_terminal_reports_share_one_exchange() {
    let mut manifest = manifest(
        "",
        &[
            ScriptHandler::RequestHead,
            ScriptHandler::ResponseBody,
            ScriptHandler::Completed,
        ],
    );
    manifest.prefilter.host = Some("example.test".into());
    manifest.capabilities.rewrite_request = true;
    manifest.capabilities.reroute = true;
    manifest.capabilities.read_bodies = true;
    manifest.capabilities.write_body = true;
    manifest.capabilities.write_status = true;
    manifest.capabilities.annotate = true;
    manifest
        .capabilities
        .write_headers
        .insert("content-type".into());
    manifest
        .response_body_filter
        .content_types
        .push("application/json".into());
    manifest.response_body_filter.statuses.push(200);
    manifest.parameters.insert(
        "enabled".into(),
        ScriptParameter::Boolean {
            label: "Enabled".into(),
            default: false,
            value: Some(true),
        },
    );
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_copy = seen.clone();
    let runner = Arc::new(Runner(move |event: ScriptInvocation| {
        seen_copy.lock().unwrap().push(event.context.handler);
        assert_eq!(event.parameters["enabled"], true);
        assert!(event.context.now_unix_ms > 0);
        match event.context.handler {
            ScriptHandler::RequestHead => {
                assert!(event.state.is_empty());
                ScriptResult {
                    state: Some(BTreeMap::from([("seen".into(), serde_json::json!(true))])),
                    ..ScriptAction::Edit {
                        edits: ScriptEdits {
                            url: Some("https://rerouted.test/changed".into()),
                            ..Default::default()
                        },
                    }
                    .into()
                }
            }
            ScriptHandler::ResponseBody => {
                assert_eq!(event.state["seen"], true);
                assert_eq!(event.request.host, "rerouted.test");
                assert_eq!(event.body.unwrap().bytes, b"{}");
                ScriptAction::Edit {
                    edits: ScriptEdits {
                        status: Some(202),
                        bytes: Some(b"changed".to_vec()),
                        headers: vec![ScriptHeaderOperation::Set {
                            name: "content-type".into(),
                            value: "text/plain".into(),
                        }],
                        ..Default::default()
                    },
                }
                .into()
            }
            ScriptHandler::Completed => {
                assert_eq!(event.state["seen"], true);
                assert_eq!(event.response.unwrap().status, 202);
                ScriptResult {
                    annotations: vec![ScriptAnnotation {
                        note: "Processed".into(),
                        tags: vec!["review".into()],
                        severity: ScriptSeverity::Info,
                    }],
                    ..ScriptAction::Continue.into()
                }
            }
            _ => panic!("unexpected invocation"),
        }
    }));
    let factory = InterceptorChainFactory::new(
        vec![registration(manifest, runner).unwrap()],
        HookLimits::default(),
    );
    for _ in 0..2 {
        let mut chain = factory.create_exchange(metadata()).unwrap();
        let RequestHeadOutcome::Continue { head, .. } =
            chain.request_head(request()).await.unwrap()
        else {
            panic!("head rejected")
        };
        let mut response = ResponseHead {
            status: 200,
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http2,
        };
        response
            .headers
            .push(HeaderField::try_new("content-type", "Application/JSON; charset=utf-8").unwrap());
        // Native admission bypasses both decoding and buffering for local or unmatched bodies.
        assert!(
            !chain
                .response_body_pipeline(&head, &response, true, BodyPipelineLimits::default())
                .await
                .unwrap()
                .buffers_head()
        );
        let mut other = response.clone();
        other.status = 404;
        assert!(
            !chain
                .response_body_pipeline(&head, &other, false, BodyPipelineLimits::default())
                .await
                .unwrap()
                .buffers_head()
        );
        let mut pipeline = chain
            .response_body_pipeline(&head, &response, false, BodyPipelineLimits::default())
            .await
            .unwrap();
        assert!(pipeline.buffers_head());
        assert_eq!(
            pipeline
                .process(BodyFrame::Data(Bytes::from_static(b"{}")))
                .await
                .unwrap(),
            [] as [transmog_core::BodyFrame; 0]
        );
        let output = pipeline.finish().await.unwrap();
        assert_eq!(
            output,
            vec![BodyFrame::Data(Bytes::from_static(b"changed"))]
        );
        let Some(BufferedHead::Response(head)) = pipeline.head() else {
            panic!("response head missing")
        };
        assert_eq!(head.status, 202);
        assert_eq!(
            head.headers.values("content-type").next().unwrap(),
            b"text/plain"
        );
        chain
            .completed(CompletedExchange {
                metadata: Arc::clone(chain.context().metadata()),
                request_head: request(),
                response_head: head.clone(),
            })
            .await;
        assert!(chain.hook_effects().iter().any(|e| matches!(&e.action, HookEffectAction::ScriptReport(report) if report.annotations[0].note == "Processed")));
        assert!(
            chain
                .hook_effects()
                .iter()
                .any(|e| matches!(e.action, HookEffectAction::ReplaceResponseHead(_)))
        );
    }
    assert_eq!(seen.lock().unwrap().len(), 6);
}

#[test]
fn tests_cannot_override_applied_parameters_or_header_read_authority() {
    let mut manifest = manifest("", &[ScriptHandler::RequestHead]);
    manifest.parameters.insert(
        "flag".into(),
        ScriptParameter::Boolean {
            label: "Flag".into(),
            default: true,
            value: None,
        },
    );
    let mut event = super::adapter::invocation(
        &manifest,
        "test",
        ScriptHandler::RequestHead,
        &request(),
        None,
        None,
    );
    event
        .parameters
        .insert("flag".into(), serde_json::json!(false));
    event.request.headers.push(ScriptHeader {
        name: "Authorization".into(),
        value: b"secret".to_vec(),
        sensitive: false,
        redacted: false,
    });
    prepare_invocation(&manifest, &mut event).unwrap();
    assert_eq!(event.parameters["flag"], true);
    assert_eq!(event.request.headers[0].value, [] as [u8; 0]);
    assert!(event.request.headers[0].redacted);
    event.body = Some(ScriptBody { bytes: vec![] });
    assert!(prepare_invocation(&manifest, &mut event).is_err());
}

#[tokio::test]
async fn failed_callback_receives_category_and_reports_validation_failures() {
    use transmog_core::intercept::{
        ExchangeFailure, ExchangeFailureKind, ExchangeStage, HookPhase,
    };
    for invalid in [false, true] {
        let manifest = manifest("", &[ScriptHandler::Failed]);
        let runner = Arc::new(Runner(move |event: ScriptInvocation| {
            assert_eq!(event.context.handler, ScriptHandler::Failed);
            assert_eq!(
                event.failure,
                Some(transmog_core::intercept::ExchangeFailureCategory::Upstream)
            );
            assert!(event.response.is_none());
            assert!(event.body.is_none());
            if invalid {
                ScriptAction::Edit {
                    edits: ScriptEdits {
                        bytes: Some(vec![]),
                        ..Default::default()
                    },
                }
                .into()
            } else {
                ScriptResult {
                    diagnostics: vec!["Upstream failed".into()],
                    ..ScriptAction::Continue.into()
                }
            }
        }));
        let factory = InterceptorChainFactory::new(
            vec![registration(manifest, runner).unwrap()],
            HookLimits::default(),
        );
        let mut chain = factory.create_exchange(metadata()).unwrap();
        chain.request_head(request()).await.unwrap();
        chain
            .failed(ExchangeFailure {
                metadata: Arc::clone(chain.context().metadata()),
                stage: ExchangeStage::Upstream,
                kind: ExchangeFailureKind::Upstream,
                request_committed: false,
                response_committed: false,
                message: "Native failure details must not enter script input".into(),
            })
            .await;
        let effects = chain.hook_effects();
        let report = effects
            .iter()
            .find_map(|effect| match &effect.action {
                HookEffectAction::ScriptReport(report) if effect.phase == HookPhase::Failed => {
                    Some(report)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(report.diagnostics.len(), 1);
        if invalid {
            assert!(report.diagnostics[0].contains("Terminal script callback failed"));
        } else {
            assert_eq!(report.diagnostics[0], "Upstream failed");
        }
    }
}

#[test]
fn informational_response_heads_are_valid_inputs_but_not_edit_targets() {
    let mut manifest = manifest("", &[ScriptHandler::ResponseHead]);
    manifest.capabilities.write_status = true;
    let response = ResponseHead {
        status: 101,
        headers: HeaderBlock::new(),
        source_version: HttpLegVersion::Http1,
    };
    let mut event = super::adapter::invocation(
        &manifest,
        "test",
        ScriptHandler::ResponseHead,
        &request(),
        Some(&response),
        None,
    );
    prepare_invocation(&manifest, &mut event).unwrap();
    let result = ScriptAction::Edit {
        edits: ScriptEdits {
            status: Some(101),
            ..Default::default()
        },
    }
    .into();
    assert!(validate_result(&manifest, ScriptHandler::ResponseHead, &request(), &result).is_err());
}

#[test]
fn help_failure_choices_and_types_match_the_runtime_contract() {
    use transmog_core::intercept::ExchangeFailureCategory;
    let help = crate::help();
    assert_eq!(help["apiRevision"], crate::SCRIPT_API_REVISION);
    let categories = ExchangeFailureCategory::ALL
        .iter()
        .map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert_eq!(help["failureCategories"], serde_json::json!(categories));
    for category in ExchangeFailureCategory::ALL {
        let encoded = serde_json::to_value(category).unwrap();
        assert_eq!(encoded, category.as_str());
        assert_eq!(
            serde_json::from_value::<ExchangeFailureCategory>(encoded).unwrap(),
            *category
        );
        assert!(crate::typescript_declarations().contains(&format!("| \"{}\"", category.as_str())));
    }
    assert!(serde_json::from_str::<ExchangeFailureCategory>("\"upstream\"").is_err());
}

fn scenario_fixture() -> ScriptScenario {
    ScriptScenario {
        request: script_request(&request(), true),
        response: Some(ScriptResponse {
            status: 200,
            headers: vec![ScriptHeader {
                name: "content-type".into(),
                value: b"application/json".to_vec(),
                sensitive: false,
                redacted: false,
            }],
        }),
        request_body: b"upload".to_vec(),
        response_body: b"{}".to_vec(),
        local_response: false,
        failure: None,
        http_version: HttpLegVersion::Http2,
        started_unix_ms: 1_700_000_000_000,
        duration_ms: 100,
    }
}

#[tokio::test]
async fn scenario_applies_edits_state_filters_and_controlled_context_through_completion() {
    let handlers = [
        ScriptHandler::RequestHead,
        ScriptHandler::RequestBody,
        ScriptHandler::ResponseHead,
        ScriptHandler::ResponseBody,
        ScriptHandler::Completed,
    ];
    let mut manifest = manifest("", &handlers);
    manifest.capabilities.rewrite_request = true;
    manifest.capabilities.reroute = true;
    manifest.capabilities.read_bodies = true;
    manifest.capabilities.write_body = true;
    manifest.capabilities.write_status = true;
    manifest.capabilities.write_headers.insert("x-seen".into());
    let result = test_scenario(
        manifest,
        Arc::new(Runner(|event: ScriptInvocation| {
            assert_eq!(event.context.source, ScriptSource::Sandbox);
            assert_eq!(event.context.http_version, HttpLegVersion::Http2);
            assert_eq!(
                event.context.now_unix_ms,
                event.context.started_unix_ms + event.context.elapsed_ms
            );
            if event.context.handler == ScriptHandler::RequestHead {
                return ScriptResult {
                    state: Some(BTreeMap::from([("seen".into(), serde_json::json!(true))])),
                    ..ScriptAction::Edit {
                        edits: ScriptEdits {
                            url: Some("https://other.test/changed".into()),
                            ..Default::default()
                        },
                    }
                    .into()
                };
            }
            assert_eq!(event.state["seen"], true);
            assert_eq!(event.request.host, "other.test");
            match event.context.handler {
                ScriptHandler::RequestBody => ScriptAction::Edit {
                    edits: ScriptEdits {
                        bytes: Some(b"changed upload".to_vec()),
                        headers: vec![ScriptHeaderOperation::Set {
                            name: "x-seen".into(),
                            value: "yes".into(),
                        }],
                        ..Default::default()
                    },
                }
                .into(),
                ScriptHandler::ResponseBody => ScriptAction::Edit {
                    edits: ScriptEdits {
                        bytes: Some(vec![]),
                        status: Some(202),
                        ..Default::default()
                    },
                }
                .into(),
                ScriptHandler::Completed => {
                    assert_eq!(event.context.elapsed_ms, 100);
                    assert_eq!(event.response.unwrap().status, 202);
                    assert!(event.request.headers.iter().any(|h| h.name == "x-seen"));
                    ScriptAction::Continue.into()
                }
                _ => ScriptAction::Continue.into(),
            }
        })),
        scenario_fixture(),
    )
    .await
    .unwrap();
    assert_eq!(result.outcome, "completed");
    assert_eq!(result.verdict, ScriptScenarioVerdict::Passed);
    assert!(result.admitted);
    assert_eq!(result.request_body, b"changed upload");
    assert_eq!(result.response_body, [] as [u8; 0]);
    assert!(
        result.steps[..5]
            .iter()
            .all(|s| s.selection == ScriptScenarioSelection::Invoked)
    );
}

#[tokio::test]
async fn scenario_explains_native_selection_and_body_forbidden_responses() {
    let mut manifest = manifest("", &[ScriptHandler::ResponseBody]);
    manifest.capabilities.read_bodies = true;
    let runner = Arc::new(Runner(|_: ScriptInvocation| ScriptAction::Continue.into()));
    let mut fixture = scenario_fixture();
    fixture.request.host = "unmatched.test".into();
    let result = test_scenario(manifest.clone(), runner.clone(), fixture)
        .await
        .unwrap();
    assert!(!result.admitted);
    assert_eq!(
        result.steps[3].selection,
        ScriptScenarioSelection::RequestFilter
    );
    let mut fixture = scenario_fixture();
    fixture.local_response = true;
    let result = test_scenario(manifest.clone(), runner.clone(), fixture)
        .await
        .unwrap();
    assert_eq!(
        result.steps[3].selection,
        ScriptScenarioSelection::BodyFilter
    );
    for status in [204, 304] {
        let mut fixture = scenario_fixture();
        fixture.response.as_mut().unwrap().status = status;
        let result = test_scenario(manifest.clone(), runner.clone(), fixture)
            .await
            .unwrap();
        assert_eq!(
            result.steps[3].selection,
            ScriptScenarioSelection::BodyForbidden
        );
        assert_eq!(result.response_body, [] as [u8; 0]);
    }
}

#[tokio::test]
async fn scenario_handles_inline_responses_failures_and_unresolved_assets() {
    let mut manifest = manifest(
        "",
        &[
            ScriptHandler::RequestHead,
            ScriptHandler::Completed,
            ScriptHandler::Failed,
        ],
    );
    manifest.capabilities.respond = true;
    let runner = Arc::new(Runner(|event: ScriptInvocation| {
        if event.context.handler == ScriptHandler::RequestHead {
            ScriptAction::Respond {
                response: ScriptResponseSource::Inline(ScriptInlineResponse {
                    status: 201,
                    headers: vec![],
                    bytes: b"local".to_vec(),
                }),
            }
            .into()
        } else {
            assert!(event.local_response);
            ScriptAction::Continue.into()
        }
    }));
    let mut fixture = scenario_fixture();
    fixture.response = None;
    fixture.failure = Some(transmog_core::intercept::ExchangeFailureCategory::Upstream);
    let result = test_scenario(manifest.clone(), runner, fixture)
        .await
        .unwrap();
    assert_eq!(result.outcome, "completed");
    assert_eq!(result.verdict, ScriptScenarioVerdict::Passed);
    assert_eq!(result.response_body, b"local");
    assert_eq!(
        result.steps[1].selection,
        ScriptScenarioSelection::NotExported
    );
    let runner = Arc::new(Runner(|event: ScriptInvocation| {
        if event.context.handler == ScriptHandler::RequestHead {
            ScriptAction::Respond {
                response: ScriptResponseSource::Asset {
                    asset_ref: "saved@1".into(),
                },
            }
            .into()
        } else {
            ScriptAction::Continue.into()
        }
    }));
    let result = test_scenario(manifest, runner, scenario_fixture())
        .await
        .unwrap();
    assert_eq!(result.outcome, "asset-unresolved");
    assert_eq!(result.verdict, ScriptScenarioVerdict::Incomplete);
    let manifest = super::tests::manifest("", &[ScriptHandler::RequestHead, ScriptHandler::Failed]);
    let runner = Arc::new(Runner(|event: ScriptInvocation| {
        if event.context.handler == ScriptHandler::Failed {
            assert_eq!(
                event.failure,
                Some(transmog_core::intercept::ExchangeFailureCategory::Route)
            );
        }
        ScriptAction::Continue.into()
    }));
    let mut fixture = scenario_fixture();
    fixture.response = None;
    fixture.failure = Some(transmog_core::intercept::ExchangeFailureCategory::Route);
    let result = test_scenario(manifest, runner, fixture).await.unwrap();
    assert_eq!(result.outcome, "failed");
    assert_eq!(result.verdict, ScriptScenarioVerdict::Passed);
    assert_eq!(result.steps[5].selection, ScriptScenarioSelection::Invoked);
}

#[test]
fn result_vocabulary_rejects_removed_aliases_and_ambiguous_response_sources() {
    for value in [
        serde_json::json!({"action":"headers","operations":[]}),
        serde_json::json!({"action":"replace-body","bytes":[]}),
        serde_json::json!({"action":"discard-body"}),
        serde_json::json!({"action":"response","response":{"status":200,"headers":[],"bytes":[]}}),
        serde_json::json!({"action":"respond","response":{"status":200,"headers":[],"bytes":[],"assetRef":"asset@1"}}),
    ] {
        assert!(serde_json::from_value::<ScriptAction>(value).is_err());
    }
}

#[tokio::test]
async fn scenario_verdict_reports_terminal_errors_independently_of_exchange_outcome() {
    for terminal in [ScriptHandler::Completed, ScriptHandler::Failed] {
        let manifest = manifest("", &[terminal]);
        let runner = Arc::new(Runner(|_: ScriptInvocation| {
            ScriptAction::Edit {
                edits: ScriptEdits {
                    status: Some(202),
                    ..Default::default()
                },
            }
            .into()
        }));
        let mut fixture = scenario_fixture();
        if terminal == ScriptHandler::Failed {
            fixture.response = None;
            fixture.failure = Some(transmog_core::intercept::ExchangeFailureCategory::Upstream);
        }
        let result = test_scenario(manifest, runner, fixture).await.unwrap();
        assert_eq!(
            result.outcome,
            if terminal == ScriptHandler::Completed {
                "completed"
            } else {
                "failed"
            }
        );
        assert_eq!(result.verdict, ScriptScenarioVerdict::Failed);
        assert!(result.error.is_some());
        let step = result
            .steps
            .iter()
            .find(|step| step.handler == terminal)
            .unwrap();
        assert_eq!(step.selection, ScriptScenarioSelection::Failed);
        assert_eq!(
            step.error.as_ref().unwrap().category,
            ScriptFailureCategory::InvalidAction
        );
        assert!(
            result
                .steps
                .iter()
                .filter(|step| step.handler != terminal)
                .all(|step| step.result.is_none())
        );
    }
}

#[tokio::test]
async fn scenario_body_input_limits_explain_the_selected_phase_without_invoking_it() {
    for handler in [ScriptHandler::RequestBody, ScriptHandler::ResponseBody] {
        let mut manifest = manifest("", &[handler, ScriptHandler::Failed]);
        manifest.capabilities.read_bodies = true;
        manifest.limits.max_input_body_bytes = 4;
        let runner = Arc::new(Runner(|event: ScriptInvocation| {
            assert_eq!(event.context.handler, ScriptHandler::Failed);
            assert_eq!(
                event.failure,
                Some(transmog_core::intercept::ExchangeFailureCategory::Body)
            );
            ScriptAction::Continue.into()
        }));
        let mut fixture = scenario_fixture();
        fixture.request_body = b"upload".to_vec();
        fixture.response_body = b"response".to_vec();
        let result = test_scenario(manifest, runner, fixture).await.unwrap();
        assert_eq!(result.outcome, "failed");
        assert_eq!(result.verdict, ScriptScenarioVerdict::Failed);
        let step = result
            .steps
            .iter()
            .find(|step| step.handler == handler)
            .unwrap();
        assert_eq!(step.selection, ScriptScenarioSelection::InputLimit);
        assert!(step.result.is_none());
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("body buffer limit 4")
        );
        assert_eq!(
            serde_json::to_value(step).unwrap()["selection"],
            "input-limit"
        );
    }
}

#[tokio::test]
async fn scenario_preserves_callback_evidence_when_body_output_processing_fails() {
    let mut manifest = manifest("", &[ScriptHandler::ResponseBody]);
    manifest.capabilities.read_bodies = true;
    manifest.capabilities.write_body = true;
    manifest.limits.max_output_bytes = 16 * 1024 * 1024;
    let runner = Arc::new(Runner(|_: ScriptInvocation| {
        ScriptAction::Edit {
            edits: ScriptEdits {
                bytes: Some(vec![0; 8 * 1024 * 1024 + 1]),
                ..Default::default()
            },
        }
        .into()
    }));
    let result = test_scenario(manifest, runner, scenario_fixture())
        .await
        .unwrap();
    assert_eq!(result.verdict, ScriptScenarioVerdict::Failed);
    assert_eq!(
        result.steps[3].selection,
        ScriptScenarioSelection::PipelineFailed
    );
    assert!(result.steps[3].result.is_some());
    assert!(result.steps[3].error.is_none());
    assert!(result.error.as_ref().unwrap().contains("body hook emitted"));
}

#[tokio::test]
async fn scenario_reports_terminal_gate_panics_and_timeouts() {
    struct GateFailure(bool);
    impl ScriptRunner for GateFailure {
        fn invoke(&self, _: ScriptInvocation) -> BoxScriptFuture<'_> {
            Box::pin(async move {
                assert!(!self.0, "contained terminal panic");
                std::future::pending().await
            })
        }
    }
    for (panic, category) in [
        (true, ScriptFailureCategory::Crash),
        (false, ScriptFailureCategory::Timeout),
    ] {
        let result = test_scenario(
            manifest("", &[ScriptHandler::Completed]),
            Arc::new(GateFailure(panic)),
            scenario_fixture(),
        )
        .await
        .unwrap();
        assert_eq!(result.outcome, "completed");
        assert_eq!(result.verdict, ScriptScenarioVerdict::Failed);
        assert_eq!(result.steps[4].selection, ScriptScenarioSelection::Failed);
        assert_eq!(result.steps[4].error.as_ref().unwrap().category, category);
        assert!(result.error.is_some());
    }
}

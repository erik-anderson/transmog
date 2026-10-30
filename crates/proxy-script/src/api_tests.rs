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
            assert_eq!(event.failure.as_deref(), Some("Upstream"));
            assert!(event.response.is_none());
            assert!(event.body.is_none());
            if invalid {
                ScriptAction::DiscardBody.into()
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

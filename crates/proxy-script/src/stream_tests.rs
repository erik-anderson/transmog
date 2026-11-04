use super::tests::metadata;
use super::tests::{manifest, request};
use super::*;
use transmog_core::{
    BodyFrame, HttpLegVersion,
    intercept::{
        BodyPipeline, BodyPipelineLimits, BufferedHead, HookLimits, InterceptorChainFactory,
    },
};

struct Runner<F>(F);
impl<F: Fn(ScriptInvocation) -> ScriptResult + Send + Sync> ScriptRunner for Runner<F> {
    fn invoke(&self, event: ScriptInvocation) -> BoxScriptFuture<'_> {
        let result = (self.0)(event);
        Box::pin(async move { Ok(result) })
    }
}

async fn response_pipeline(
    manifest: ScriptManifest,
    runner: Arc<dyn ScriptRunner>,
) -> BodyPipeline {
    response_pipeline_chain(vec![registration(manifest, runner).unwrap()]).await
}

async fn response_pipeline_chain(
    registrations: Vec<transmog_core::intercept::InterceptorRegistration>,
) -> BodyPipeline {
    response_pipeline_with_head(
        registrations,
        request(),
        ResponseHead {
            status: 200,
            headers: HeaderBlock::new(),
            source_version: request().source_version,
        },
    )
    .await
}

async fn response_pipeline_with_head(
    registrations: Vec<transmog_core::intercept::InterceptorRegistration>,
    request: RequestHead,
    head: ResponseHead,
) -> BodyPipeline {
    let factory = InterceptorChainFactory::new(registrations, HookLimits::default());
    let mut chain = factory.create_exchange(metadata()).unwrap();
    chain.request_head(request.clone()).await.unwrap();
    let transmog_core::intercept::ResponseHeadOutcome::Continue { head, .. } = chain
        .response_head(&request, head, None, false)
        .await
        .unwrap()
    else {
        panic!("head aborted")
    };
    chain
        .response_body_pipeline(&request, &head, false, BodyPipelineLimits::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn bodyless_responses_skip_body_callbacks_but_empty_streams_finish() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for handler in [ScriptHandler::ResponseBody, ScriptHandler::ResponseChunk] {
        for (method, status, process_body) in [
            ("HEAD", 200, false),
            ("head", 200, false),
            ("GET", 101, false),
            ("GET", 204, false),
            ("GET", 205, false),
            ("GET", 304, false),
            ("GET", 200, true),
        ] {
            let mut manifest = manifest("", &[ScriptHandler::ResponseHead, handler]);
            manifest.capabilities.read_bodies = true;
            let calls = Arc::new(AtomicUsize::new(0));
            let body_calls = calls.clone();
            let runner = Arc::new(Runner(move |event: ScriptInvocation| {
                if event.context.handler == ScriptHandler::ResponseHead {
                    return ScriptResult {
                        defer_headers: handler == ScriptHandler::ResponseChunk,
                        ..ScriptAction::Continue.into()
                    };
                }
                body_calls.fetch_add(1, Ordering::Relaxed);
                ScriptAction::Continue.into()
            }));
            let mut request = request();
            request.method = method.into();
            let mut pipeline = response_pipeline_with_head(
                vec![registration(manifest, runner).unwrap()],
                request,
                ResponseHead {
                    status,
                    headers: HeaderBlock::new(),
                    source_version: HttpLegVersion::Http1,
                },
            )
            .await;
            pipeline.finish().await.unwrap();
            assert_eq!(
                calls.load(Ordering::Relaxed),
                usize::from(process_body),
                "{method} {status} {handler:?}"
            );
            assert_eq!(pipeline.modifies_body(), process_body);
        }
    }
}

#[tokio::test]
async fn scenarios_discard_bodyless_edits_without_reporting_output_bytes() {
    for handler in [ScriptHandler::ResponseBody, ScriptHandler::ResponseChunk] {
        for status in [204, 205, 304] {
            let mut manifest = manifest("", &[handler]);
            manifest.capabilities.read_bodies = true;
            manifest.capabilities.write_status = true;
            if handler == ScriptHandler::ResponseChunk {
                manifest.handlers.insert(ScriptHandler::ResponseHead);
            }
            let runner = Arc::new(Runner(move |event: ScriptInvocation| {
                if event.context.handler == ScriptHandler::ResponseHead {
                    return ScriptResult {
                        defer_headers: true,
                        ..ScriptAction::Continue.into()
                    };
                }
                if handler == ScriptHandler::ResponseChunk {
                    assert!(
                        event.can_edit_headers,
                        "callback ran after body was discarded"
                    );
                }
                ScriptAction::Edit {
                    edits: ScriptEdits {
                        status: Some(status),
                        ..Default::default()
                    },
                }
                .into()
            }));
            let mut fixture = super::api_tests::scenario_fixture();
            fixture.response_body = b"firstlater".to_vec();
            fixture.response_chunks = Some(vec![
                ScriptScenarioChunk {
                    bytes: b"first".to_vec(),
                    delay_ms: 0,
                },
                ScriptScenarioChunk {
                    bytes: b"later".to_vec(),
                    delay_ms: 5,
                },
            ]);
            let result = test_scenario(manifest, runner, fixture).await.unwrap();
            assert_eq!(
                result.verdict,
                ScriptScenarioVerdict::Passed,
                "{:?}",
                result.error
            );
            assert_eq!(result.response_body, [] as [u8; 0]);
            assert!(result.body_steps.iter().all(|step| step.output_bytes == 0));
            if handler == ScriptHandler::ResponseChunk {
                assert_eq!(result.stream_calls.len(), 1);
            }
            let head = result.response.unwrap();
            assert_eq!(head.status, status);
            assert_eq!(
                head.headers
                    .iter()
                    .find(|header| header.name == "content-length")
                    .map(|header| header.value.as_slice()),
                (status == 205).then_some(b"0".as_slice())
            );
        }
    }
}

#[tokio::test]
async fn streaming_and_complete_body_stages_preserve_order_headers_and_trailers() {
    for stream_first in [false, true] {
        let mut stream = manifest(
            "",
            &[ScriptHandler::ResponseHead, ScriptHandler::ResponseChunk],
        );
        stream.id = "stream".into();
        stream.capabilities.read_bodies = true;
        stream.capabilities.write_body = true;
        stream.capabilities.write_headers.insert("x-stream".into());
        let stream_runner = Arc::new(Runner(|event: ScriptInvocation| {
            if event.context.handler == ScriptHandler::ResponseHead {
                return ScriptResult {
                    body: Some(ScriptBodyMode::Stream),
                    defer_headers: true,
                    ..ScriptAction::Continue.into()
                };
            }
            let headers = if event.can_edit_headers {
                vec![ScriptHeaderOperation::Set {
                    name: "x-stream".into(),
                    value: "yes".into(),
                }]
            } else {
                vec![]
            };
            ScriptAction::Emit {
                bytes: event.bytes.unwrap().to_ascii_uppercase(),
                consumed: None,
                headers,
                status: None,
            }
            .into()
        }));
        let mut buffer = manifest("", &[ScriptHandler::ResponseBody]);
        buffer.id = "buffer".into();
        buffer.capabilities.read_bodies = true;
        buffer.capabilities.write_headers.insert("x-buffer".into());
        let buffer_runner = Arc::new(Runner(move |event: ScriptInvocation| {
            let bytes = event.body.unwrap().bytes;
            assert_eq!(bytes, if stream_first { b"ABCD" } else { b"abcd" });
            ScriptAction::Edit {
                edits: ScriptEdits {
                    headers: vec![ScriptHeaderOperation::Set {
                        name: "x-buffer".into(),
                        value: "yes".into(),
                    }],
                    ..Default::default()
                },
            }
            .into()
        }));
        let mut registrations = vec![
            registration(stream, stream_runner).unwrap(),
            registration(buffer, buffer_runner).unwrap(),
        ];
        // Native response stages traverse the supplied registration order in reverse.
        if stream_first {
            registrations.reverse();
        }
        let mut pipeline = response_pipeline_chain(registrations).await;
        assert!(pipeline.head_pending());
        assert_eq!(
            pipeline.process(data(b"ab")).await.unwrap(),
            [] as [BodyFrame; 0]
        );
        assert_eq!(
            pipeline.process(data(b"cd")).await.unwrap(),
            [] as [BodyFrame; 0]
        );
        let trailers =
            HeaderBlock::from_fields(vec![HeaderField::try_new("x-tail", "yes").unwrap()]);
        assert_eq!(
            pipeline
                .process(BodyFrame::Trailers(trailers.clone()))
                .await
                .unwrap(),
            [] as [BodyFrame; 0]
        );
        assert!(pipeline.head_pending());
        assert_eq!(
            pipeline.finish().await.unwrap(),
            vec![data(b"ABCD"), BodyFrame::Trailers(trailers)]
        );
        assert!(!pipeline.head_pending());
        let Some(BufferedHead::Response(head)) = pipeline.head() else {
            panic!("response missing")
        };
        assert_eq!(head.headers.values("x-stream").next().unwrap(), b"yes");
        assert_eq!(head.headers.values("x-buffer").next().unwrap(), b"yes");
    }
}

fn data(bytes: &'static [u8]) -> BodyFrame {
    BodyFrame::Data(Bytes::from_static(bytes))
}

#[tokio::test]
async fn unchanged_prefixes_need_no_write_permission_and_release_deferred_headers() {
    for deferred in [false, true] {
        let mut manifest = manifest(
            "",
            &[ScriptHandler::ResponseHead, ScriptHandler::ResponseChunk],
        );
        manifest.capabilities.read_bodies = true;
        let runner = Arc::new(Runner(move |event: ScriptInvocation| {
            if event.context.handler == ScriptHandler::ResponseHead {
                return ScriptResult {
                    defer_headers: deferred,
                    ..ScriptAction::Continue.into()
                };
            }
            let bytes = event.bytes.unwrap();
            if bytes.is_empty() {
                return ScriptAction::Continue.into();
            }
            if bytes.len() < 3 && !event.end {
                return ScriptAction::NeedMore.into();
            }
            ScriptAction::Forward {
                consumed: bytes.len().min(2),
            }
            .into()
        }));
        let mut pipeline = response_pipeline(manifest, runner).await;
        assert_eq!(pipeline.head_pending(), deferred);
        assert_eq!(
            pipeline.process(data(b"ab")).await.unwrap(),
            [] as [BodyFrame; 0]
        );
        assert_eq!(pipeline.head_pending(), deferred);
        assert_eq!(
            pipeline.process(data(b"c\nz")).await.unwrap(),
            vec![data(b"ab"), data(b"c\n")]
        );
        assert!(!pipeline.head_pending());
        let trailers =
            HeaderBlock::from_fields(vec![HeaderField::try_new("x-tail", "yes").unwrap()]);
        assert_eq!(
            pipeline
                .process(BodyFrame::Trailers(trailers.clone()))
                .await
                .unwrap(),
            [] as [BodyFrame; 0]
        );
        assert_eq!(
            pipeline.finish().await.unwrap(),
            vec![data(b"z"), BodyFrame::Trailers(trailers)]
        );
    }
}

#[tokio::test]
async fn incremental_callbacks_retain_only_pending_input_and_flush_at_eof() {
    let mut manifest = manifest("", &[ScriptHandler::ResponseChunk]);
    manifest.capabilities.read_bodies = true;
    manifest.capabilities.write_body = true;
    let runner = Arc::new(Runner(|event: ScriptInvocation| {
        assert!(!event.can_edit_headers);
        let bytes = event.bytes.unwrap();
        if bytes.is_empty() {
            return ScriptAction::Continue.into();
        }
        let line = bytes.iter().position(|b| *b == b'\n').map(|n| n + 1);
        if line.is_none() && !event.end {
            return ScriptAction::NeedMore.into();
        }
        let consumed = line.unwrap_or(bytes.len());
        ScriptAction::Emit {
            bytes: String::from_utf8(bytes[..consumed].to_vec())
                .unwrap()
                .replace("before", "after")
                .into_bytes(),
            consumed: Some(consumed),
            headers: vec![],
            status: None,
        }
        .into()
    }));
    let mut first = response_pipeline(manifest.clone(), runner.clone()).await;
    let mut second = response_pipeline(manifest, runner).await;
    assert!(!first.head_pending());
    assert_eq!(
        first.process(data(b"be")).await.unwrap(),
        [] as [BodyFrame; 0]
    );
    assert_eq!(
        second.process(data(b"separate")).await.unwrap(),
        [] as [BodyFrame; 0]
    );
    assert_eq!(
        first.process(data(b"fore\nlast")).await.unwrap(),
        vec![data(b"after\n")]
    );
    assert_eq!(second.finish().await.unwrap(), vec![data(b"separate")]);
    assert_eq!(first.finish().await.unwrap(), vec![data(b"last")]);
}

#[tokio::test]
async fn callback_defaults_selection_and_deferred_head_authority_are_enforced() {
    for mode in [
        None,
        Some(ScriptBodyMode::Skip),
        Some(ScriptBodyMode::Buffer),
        Some(ScriptBodyMode::Stream),
    ] {
        let mut manifest = manifest(
            "",
            &[
                ScriptHandler::ResponseHead,
                ScriptHandler::ResponseBody,
                ScriptHandler::ResponseChunk,
            ],
        );
        manifest.capabilities.read_bodies = true;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = seen.clone();
        let runner = Arc::new(Runner(move |event: ScriptInvocation| {
            capture.lock().unwrap().push(event.context.handler);
            if event.context.handler == ScriptHandler::ResponseHead {
                ScriptResult {
                    body: mode,
                    ..ScriptAction::Continue.into()
                }
            } else {
                ScriptAction::Continue.into()
            }
        }));
        let mut pipeline = response_pipeline(manifest, runner).await;
        let output = pipeline.process(data(b"hello")).await.unwrap();
        if mode.is_none() || mode == Some(ScriptBodyMode::Buffer) {
            assert!(pipeline.buffers_head());
            assert_eq!(output, [] as [BodyFrame; 0]);
            assert_eq!(pipeline.finish().await.unwrap(), vec![data(b"hello")]);
            assert!(!seen.lock().unwrap().contains(&ScriptHandler::ResponseChunk));
        } else {
            assert!(!pipeline.buffers_head());
            assert_eq!(output, vec![data(b"hello")]);
            pipeline.finish().await.unwrap();
            assert!(!seen.lock().unwrap().contains(&ScriptHandler::ResponseBody));
        }
    }
    let mut manifest = manifest(
        "",
        &[ScriptHandler::ResponseHead, ScriptHandler::ResponseChunk],
    );
    manifest.capabilities.read_bodies = true;
    manifest
        .capabilities
        .write_headers
        .insert("x-sniffed".into());
    let runner = Arc::new(Runner(|event: ScriptInvocation| {
        if event.context.handler == ScriptHandler::ResponseHead {
            return ScriptResult {
                body: Some(ScriptBodyMode::Stream),
                defer_headers: true,
                ..ScriptAction::Continue.into()
            };
        }
        if event.bytes.as_ref().unwrap().len() < 4 && !event.end {
            return ScriptAction::NeedMore.into();
        }
        ScriptAction::Edit {
            edits: ScriptEdits {
                headers: vec![ScriptHeaderOperation::Set {
                    name: "x-sniffed".into(),
                    value: "yes".into(),
                }],
                ..Default::default()
            },
        }
        .into()
    }));
    let mut pipeline = response_pipeline(manifest, runner).await;
    assert!(pipeline.head_pending());
    assert_eq!(
        pipeline.process(data(b"ab")).await.unwrap(),
        [] as [BodyFrame; 0]
    );
    assert!(pipeline.head_pending());
    assert_eq!(
        pipeline.process(data(b"cd")).await.unwrap(),
        vec![data(b"abcd")]
    );
    assert!(!pipeline.head_pending());
    let Some(BufferedHead::Response(head)) = pipeline.head() else {
        panic!("response missing")
    };
    assert_eq!(head.headers.values("x-sniffed").next().unwrap(), b"yes");
    assert!(pipeline.process(data(b"later")).await.is_err());
}

#[tokio::test]
async fn streaming_queue_limits_eof_and_zero_progress_fail_without_spinning() {
    for (input, finish) in [(b"ab".as_slice(), true), (b"abcdef".as_slice(), false)] {
        let mut manifest = manifest("", &[ScriptHandler::ResponseChunk]);
        manifest.capabilities.read_bodies = true;
        manifest.limits.max_input_body_bytes = 4;
        let mut pipeline = response_pipeline(
            manifest,
            Arc::new(Runner(|_| ScriptAction::NeedMore.into())),
        )
        .await;
        let result = pipeline
            .process(BodyFrame::Data(Bytes::copy_from_slice(input)))
            .await;
        if finish {
            assert!(result.is_ok());
            assert!(pipeline.finish().await.is_err());
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn response_mode_and_stream_results_are_phase_and_revision_scoped() {
    let mut manifest = manifest(
        "",
        &[
            ScriptHandler::ResponseHead,
            ScriptHandler::ResponseBody,
            ScriptHandler::ResponseChunk,
        ],
    );
    manifest.capabilities.read_bodies = true;
    manifest.capabilities.write_body = true;
    assert_eq!(
        adapter::default_response_mode(&manifest),
        ScriptBodyMode::Buffer
    );
    let mut result: ScriptResult = ScriptAction::Continue.into();
    result.body = Some(ScriptBodyMode::Stream);
    result.defer_headers = true;
    assert!(validate_result(&manifest, ScriptHandler::ResponseHead, &request(), &result).is_ok());
    manifest.capabilities.respond = true;
    let local = ScriptResult {
        body: Some(ScriptBodyMode::Stream),
        defer_headers: true,
        ..ScriptAction::Respond {
            response: ScriptResponseSource::Inline(ScriptInlineResponse {
                status: 200,
                headers: vec![],
                bytes: b"local".to_vec(),
            }),
        }
        .into()
    };
    assert!(validate_result(&manifest, ScriptHandler::ResponseHead, &request(), &local).is_ok());
    assert!(validate_result(&manifest, ScriptHandler::ResponseBody, &request(), &result).is_err());
    result.body = Some(ScriptBodyMode::Buffer);
    assert!(validate_result(&manifest, ScriptHandler::ResponseHead, &request(), &result).is_err());
    result.defer_headers = false;
    manifest.api_revision = 3;
    assert!(validate_result(&manifest, ScriptHandler::ResponseHead, &request(), &result).is_err());
    manifest.handlers.remove(&ScriptHandler::ResponseChunk);
    let source = "export function onResponseHead() { return { decision: { action: 'continue' } }; } export function onResponseBody() { return { decision: { action: 'continue' } }; }";
    manifest.source_hash = source_hash(source.as_bytes());
    assert!(compile_typescript(manifest.clone(), source).is_ok());
    manifest.api_revision = 4;
    manifest.handlers.remove(&ScriptHandler::ResponseBody);
    result.body = Some(ScriptBodyMode::Buffer);
    assert!(validate_result(&manifest, ScriptHandler::ResponseHead, &request(), &result).is_err());
    result.body = Some(ScriptBodyMode::Skip);
    assert!(validate_result(&manifest, ScriptHandler::ResponseHead, &request(), &result).is_ok());
}

#[test]
fn streaming_results_require_progress_and_an_unreleased_head() {
    let mut manifest = manifest("", &[ScriptHandler::ResponseChunk]);
    manifest.capabilities.read_bodies = true;
    manifest.capabilities.write_body = true;
    manifest.capabilities.write_headers.insert("x-test".into());
    let response = ResponseHead {
        status: 200,
        headers: HeaderBlock::new(),
        source_version: request().source_version,
    };
    let mut event = adapter::invocation(
        &manifest,
        "stream",
        ScriptHandler::ResponseChunk,
        &request(),
        Some(&response),
        None,
    );
    event.bytes = Some(vec![1, 2]);
    assert!(prepare_invocation(&manifest, &mut event).is_ok());
    let wait: ScriptResult = ScriptAction::NeedMore.into();
    assert!(validate_stream_result(&manifest, &event, &wait).is_ok());
    event.end = true;
    assert!(validate_stream_result(&manifest, &event, &wait).is_err());
    event.end = false;
    let emitted = |consumed| {
        ScriptResult::from(ScriptAction::Emit {
            bytes: vec![],
            consumed: Some(consumed),
            headers: vec![],
            status: None,
        })
    };
    assert!(validate_stream_result(&manifest, &event, &emitted(0)).is_err());
    assert!(validate_stream_result(&manifest, &event, &emitted(3)).is_err());
    assert!(validate_stream_result(&manifest, &event, &emitted(1)).is_ok());
    let edit: ScriptResult = ScriptAction::Edit {
        edits: ScriptEdits {
            headers: vec![ScriptHeaderOperation::Remove {
                name: "x-test".into(),
            }],
            ..Default::default()
        },
    }
    .into();
    assert!(validate_result(&manifest, ScriptHandler::ResponseChunk, &request(), &edit).is_ok());
    assert!(validate_stream_result(&manifest, &event, &edit).is_err());
    event.can_edit_headers = true;
    assert!(validate_stream_result(&manifest, &event, &edit).is_ok());
    manifest.limits.max_input_body_bytes = 2;
    assert!(validate_stream_result(&manifest, &event, &wait).is_err());
}

#[test]
fn forwarding_is_bounded_read_only_and_scoped_to_streaming() {
    let mut manifest = manifest("", &[ScriptHandler::ResponseChunk]);
    manifest.capabilities.read_bodies = true;
    let response = ResponseHead {
        status: 200,
        headers: HeaderBlock::new(),
        source_version: request().source_version,
    };
    let mut event = adapter::invocation(
        &manifest,
        "stream",
        ScriptHandler::ResponseChunk,
        &request(),
        Some(&response),
        None,
    );
    event.bytes = Some(vec![1, 2]);
    for consumed in [0, 1, 2, 3] {
        let forwarded: ScriptResult = ScriptAction::Forward { consumed }.into();
        assert!(
            validate_result(
                &manifest,
                ScriptHandler::ResponseChunk,
                &request(),
                &forwarded
            )
            .is_ok()
        );
        assert_eq!(
            validate_stream_result(&manifest, &event, &forwarded).is_ok(),
            (1..=2).contains(&consumed)
        );
        for handler in [
            ScriptHandler::RequestHead,
            ScriptHandler::RequestBody,
            ScriptHandler::ResponseHead,
            ScriptHandler::ResponseBody,
            ScriptHandler::Completed,
            ScriptHandler::Failed,
        ] {
            assert!(validate_result(&manifest, handler, &request(), &forwarded).is_err());
        }
        manifest.api_revision = 3;
        assert!(
            validate_result(
                &manifest,
                ScriptHandler::ResponseChunk,
                &request(),
                &forwarded
            )
            .is_err()
        );
        manifest.api_revision = 4;
    }
    event.bytes = Some(vec![]);
    event.end = true;
    assert!(
        validate_stream_result(
            &manifest,
            &event,
            &ScriptAction::Forward { consumed: 0 }.into()
        )
        .is_ok()
    );
}

#[tokio::test]
async fn scenarios_report_chunk_arrival_header_release_and_terminal_flush() {
    for deferred in [false, true] {
        let mut manifest = manifest(
            "",
            &[ScriptHandler::ResponseHead, ScriptHandler::ResponseChunk],
        );
        manifest.capabilities.read_bodies = true;
        let runner = Arc::new(Runner(move |event: ScriptInvocation| {
            if event.context.handler == ScriptHandler::ResponseHead {
                return ScriptResult {
                    defer_headers: deferred,
                    ..ScriptAction::Continue.into()
                };
            }
            if event.can_edit_headers && event.bytes.as_ref().unwrap().len() < 4 && !event.end {
                ScriptAction::NeedMore.into()
            } else {
                ScriptAction::Continue.into()
            }
        }));
        let mut fixture = super::api_tests::scenario_fixture();
        fixture.response_body = b"before-after".to_vec();
        fixture.response_chunks = Some(vec![
            ScriptScenarioChunk {
                bytes: b"be".to_vec(),
                delay_ms: 0,
            },
            ScriptScenarioChunk {
                bytes: b"fore-".to_vec(),
                delay_ms: 5,
            },
            ScriptScenarioChunk {
                bytes: b"after".to_vec(),
                delay_ms: 5,
            },
        ]);
        let result = test_scenario(manifest, runner, fixture).await.unwrap();
        assert_eq!(result.verdict, ScriptScenarioVerdict::Passed);
        assert_eq!(result.response_body, b"before-after");
        assert_eq!(
            result.headers_released_at_ms,
            Some(if deferred { 55 } else { 50 })
        );
        assert_eq!(result.stream_calls[0].input_bytes, 2);
        assert_eq!(result.stream_calls[0].can_edit_headers, deferred);
        assert!(result.stream_calls.last().unwrap().end);
        assert_eq!(result.body_steps[0].headers_released, !deferred);
        assert!(
            result
                .response
                .unwrap()
                .headers
                .iter()
                .all(|header| !header.name.eq_ignore_ascii_case("content-length"))
        );
    }
}

#[tokio::test]
async fn scenarios_keep_only_accepted_output_when_a_later_chunk_fails() {
    for panic in [false, true] {
        let mut manifest = manifest("", &[ScriptHandler::ResponseChunk]);
        manifest.capabilities.read_bodies = true;
        manifest.capabilities.write_body = true;
        manifest.capabilities.abort = true;
        let runner = Arc::new(Runner(move |event: ScriptInvocation| {
            if event.bytes.as_deref() == Some(b"b") {
                assert!(!panic, "contained panic after earlier output");
                ScriptAction::Abort {
                    reason: "stop after the first chunk".into(),
                }
                .into()
            } else {
                ScriptResult {
                    diagnostics: vec!["first chunk processed".into()],
                    ..ScriptAction::Emit {
                        bytes: b"accepted".to_vec(),
                        consumed: None,
                        headers: vec![],
                        status: None,
                    }
                    .into()
                }
            }
        }));
        let mut fixture = super::api_tests::scenario_fixture();
        fixture.response_body = b"abc".to_vec();
        fixture.response_chunks = Some(
            b"abc"
                .iter()
                .map(|byte| ScriptScenarioChunk {
                    bytes: vec![*byte],
                    delay_ms: 0,
                })
                .collect(),
        );
        let result = test_scenario(manifest, runner, fixture).await.unwrap();
        assert_eq!(result.verdict, ScriptScenarioVerdict::Failed);
        assert_eq!(result.headers_released_at_ms, Some(50));
        assert_eq!(result.response_body, b"accepted");
        let step = result
            .steps
            .iter()
            .find(|step| step.handler == ScriptHandler::ResponseChunk)
            .unwrap();
        assert_eq!(
            step.result.is_none(),
            panic,
            "a cancelled callback must not reuse an earlier result"
        );
        assert_eq!(step.reports[0].diagnostics, ["first chunk processed"]);
        assert_eq!(
            result
                .body_steps
                .iter()
                .map(|step| step.output_bytes)
                .sum::<usize>(),
            result.response_body.len()
        );
    }
}

#[tokio::test]
async fn scenarios_retain_earlier_stream_findings_with_live_budgets_on_success_and_abort() {
    for abort in [false, true] {
        for count in [2, 200] {
            let mut manifest = manifest("", &[ScriptHandler::ResponseChunk]);
            manifest.capabilities.read_bodies = true;
            manifest.capabilities.annotate = true;
            manifest.capabilities.abort = abort;
            let runner = Arc::new(Runner(move |event: ScriptInvocation| {
                if event.end {
                    return if abort {
                        ScriptAction::Abort {
                            reason: "sample failure after findings".into(),
                        }
                        .into()
                    } else {
                        ScriptAction::Continue.into()
                    };
                }
                ScriptResult {
                    diagnostics: vec!["inspected a chunk".into()],
                    annotations: vec![ScriptAnnotation {
                        note: "chunk finding".into(),
                        tags: vec![],
                        severity: ScriptSeverity::Info,
                    }],
                    ..ScriptAction::Continue.into()
                }
            }));
            let mut fixture = super::api_tests::scenario_fixture();
            fixture.response_body = vec![b'x'; count];
            fixture.response_chunks = Some(
                (0..count)
                    .map(|_| ScriptScenarioChunk {
                        bytes: vec![b'x'],
                        delay_ms: 0,
                    })
                    .collect(),
            );
            let result = test_scenario(manifest, runner, fixture).await.unwrap();
            assert_eq!(
                result.verdict,
                if abort {
                    ScriptScenarioVerdict::Failed
                } else {
                    ScriptScenarioVerdict::Passed
                }
            );
            let step = result
                .steps
                .iter()
                .find(|step| step.handler == ScriptHandler::ResponseChunk)
                .unwrap();
            assert!(
                step.result.as_ref().unwrap().diagnostics.is_empty(),
                "last decision remains unchanged"
            );
            assert_eq!(
                step.reports.len(),
                count.min(128) + usize::from(count > 128)
            );
            assert_eq!(step.reports[0].annotations[0].note, "chunk finding");
            assert_eq!(step.reports[0].diagnostics, ["inspected a chunk"]);
            assert_eq!(
                step.reports
                    .iter()
                    .filter(|report| report.details_omitted)
                    .count(),
                usize::from(count > 128)
            );
        }
    }
}

#[tokio::test]
async fn long_streams_bound_diagnostic_retention_and_report_omission() {
    let mut manifest = manifest("", &[ScriptHandler::ResponseChunk]);
    manifest.capabilities.read_bodies = true;
    let runner = Arc::new(Runner(|_| ScriptResult {
        diagnostics: vec!["chunk observed".into()],
        ..ScriptAction::Continue.into()
    }));
    let factory = InterceptorChainFactory::new(
        vec![registration(manifest, runner).unwrap()],
        HookLimits::default(),
    );
    let mut chain = factory.create_exchange(metadata()).unwrap();
    chain.request_head(request()).await.unwrap();
    let head = ResponseHead {
        status: 200,
        headers: HeaderBlock::new(),
        source_version: request().source_version,
    };
    let mut pipeline = chain
        .response_body_pipeline(&request(), &head, false, BodyPipelineLimits::default())
        .await
        .unwrap();
    for _ in 0..400 {
        pipeline.process(data(b"x")).await.unwrap();
    }
    pipeline.finish().await.unwrap();
    let reports = chain
        .hook_effects()
        .into_iter()
        .filter_map(|effect| {
            if let transmog_core::intercept::HookEffectAction::ScriptReport(report) = effect.action
            {
                Some(report)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 129);
    assert_eq!(
        reports
            .iter()
            .filter(|report| report.details_omitted)
            .count(),
        1
    );
}

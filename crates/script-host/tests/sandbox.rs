//! Windows `AppContainer` integration tests for the script host.

#![cfg(windows)]

use std::collections::BTreeSet;

use transmog_script::{
    ScriptAction, ScriptCapabilities, ScriptContext, ScriptHandler, ScriptInvocation, ScriptLimits,
    ScriptManifest, ScriptPrefilter, ScriptRequest, ScriptRunner, compile_typescript, source_hash,
};
use transmog_script_supervisor::{IsolationPolicy, ScriptHostConfig, SupervisedScriptRunner};
#[path = "../../extension-host/tests/support/debugger.rs"]
mod debugger;

#[tokio::test]
async fn debugger_logpoints_complete_callbacks_through_the_authenticated_host_pipe() {
    use transmog_script::debugger::{DebugAction, DebugEventKind, SourceBreakpoint};
    let compiled = compile(
        "export function onRequestHead(){\n const local=7;\n return {decision:{action:'continue'}};\n}",
        1000,
    );
    let line = u32::try_from(
        compiled
            .javascript
            .lines()
            .position(|line| line.contains("return"))
            .unwrap()
            + 1,
    )
    .unwrap();
    let session = debugger::session('1');
    let owner = std::sync::Arc::clone(&session);
    let runner = tokio::task::spawn_blocking(move || {
        SupervisedScriptRunner::start_debug(&config(), compiled, Some(owner))
    })
    .await
    .unwrap()
    .unwrap();
    session
        .command(DebugAction::Breakpoints {
            values: vec![SourceBreakpoint {
                id: 77,
                condition: None,
                log_expression: Some("'value ' + local".into()),
                location: transmog_script::SourceLocation {
                    source: "file:///transmog-script/main.js".into(),
                    line,
                    column: 1,
                },
            }],
        })
        .unwrap();
    debugger::wait(&session, |event| matches!(event, DebugEventKind::Sources { breakpoints, .. } if breakpoints.iter().any(|point| point.id == 77 && point.resolved.is_some()))).await;
    runner.invoke(invocation()).await.unwrap();
    debugger::wait(&session, |event| matches!(event, DebugEventKind::Logpoint { text, thrown: false, .. } if text == "value 7")).await;
    assert!(session.page(0).paused.is_none());
    assert!(!runner.has_failed());
    session.stop("Test ended");
}

#[tokio::test]
async fn live_reservation_preserves_actual_heap_and_rejects_competing_and_pinned_exchanges() {
    use std::sync::Arc;
    use transmog_script::debugger::DebugEventKind;
    let source = "globalThis.starts=0;export function onRequestHead(event){const local=7;debugger;console.log('starts',++starts);return {decision:{action:'continue'},state:{token:starts}}}export function onRequestBody(event){const local=7;debugger;console.log('body',event.state.token);return {decision:{action:'continue'}}}export function onCompleted(event){const local=7;debugger;console.log('terminal',event.state.token);return {decision:{action:'continue'}}}";
    let mut manifest = compile(
        "export function onRequestHead(){return {decision:{action:'continue'}}}",
        1000,
    )
    .manifest;
    manifest.handlers.insert(ScriptHandler::Completed);
    manifest.handlers.insert(ScriptHandler::RequestBody);
    manifest.capabilities.read_bodies = true;
    manifest.source_hash = source_hash(source.as_bytes());
    let compiled = compile_typescript(manifest.clone(), source).unwrap();
    let runner = SupervisedScriptRunner::start(&config(), compiled).unwrap();
    runner.invoke(invocation()).await.unwrap();
    runner.exchange_open().unwrap();
    let blocked = debugger::session('d');
    assert!(
        runner
            .arm_debugger(&blocked, ScriptPrefilter::default())
            .is_err()
    );
    blocked.stop("Busy arming test ended");
    assert!(!runner.has_failed());
    runner.exchange_closed("old", true);
    let session = debugger::session('e');
    runner
        .arm_debugger(
            &session,
            ScriptPrefilter {
                method: Some("GET".into()),
                host: Some("example.test".into()),
                path_prefix: Some("/".into()),
            },
        )
        .unwrap();
    assert!(
        runner
            .debug_admit("mismatch", "POST", "example.test", "/")
            .is_err()
    );
    let request = invocation().request;
    let scenario:transmog_script::ScriptScenario=serde_json::from_value(serde_json::json!({"request":request,"response":{"status":200,"headers":[]},"requestBody":[97,98,99],"responseBody":[],"localResponse":false,"failure":null,"httpVersion":"Http1","startedUnixMs":0,"durationMs":100})).unwrap();
    let worker = Arc::clone(&runner);
    let call =
        tokio::spawn(
            async move { transmog_script::test_scenario(manifest, worker, scenario).await },
        );
    let pause = debugger::pause(&session, 0).await;
    assert!(session.page(0).exchange_id.is_some());
    assert!(
        runner
            .debug_admit("competitor", "GET", "example.test", "/")
            .is_err()
    );
    // Submission is eager in this runner. Reject before creating its cancellation
    // guard, including when the competing future is never polled.
    drop(runner.invoke(invocation()));
    assert!(!runner.has_failed());
    assert!(runner.invoke(invocation()).await.is_err());
    assert!(!runner.has_failed());
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    debugger::local_seven(&session, pause).await;
    debugger::resume(&session, pause);
    let mut previous = pause;
    for _ in 0..2 {
        let next = debugger::pause(&session, previous).await;
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        debugger::local_seven(&session, next).await;
        debugger::resume(&session, next);
        previous = next;
    }
    let result = call.await.unwrap().unwrap();
    assert_eq!(
        result.verdict,
        transmog_script::ScriptScenarioVerdict::Passed,
        "{result:#?}"
    );
    debugger::wait(&session, |event| matches!(event, DebugEventKind::Detached)).await;
    assert!(!runner.has_failed());
    let logs = runner.logs();
    assert!(
        result.steps[0]
            .result
            .as_ref()
            .unwrap()
            .logs
            .iter()
            .flat_map(|run| &run.records)
            .any(|record| record.text == "starts 2")
    );
    assert!(
        logs.iter()
            .flat_map(|run| &run.records)
            .any(|record| record.text == "terminal 2")
    );
    assert!(
        runner
            .invoke(invocation())
            .await
            .unwrap()
            .logs
            .iter()
            .flat_map(|run| &run.records)
            .any(|record| record.text == "starts 3")
    );
    let stop = debugger::session('f');
    runner
        .arm_debugger(&stop, ScriptPrefilter::default())
        .unwrap();
    stop.stop("Owner window disappeared");
    assert!(runner.has_failed());
    assert!(runner.invoke(invocation()).await.is_err());
}

#[tokio::test]
async fn debugger_preserves_initializer_heap_locals_and_idle_detach_in_the_real_sandbox() {
    use transmog_script::debugger::{DebugAction, DebugEventKind};
    let session = debugger::session('a');
    let owner = std::sync::Arc::clone(&session);
    let compiled = compile(
        "globalThis.starts=1; debugger;\nexport function onRequestHead(event){const local=7;debugger;console.log('starts',globalThis.starts++);return {decision:{action:'continue'}}}",
        1000,
    );
    let startup = tokio::task::spawn_blocking(move || {
        SupervisedScriptRunner::start_debug(&config(), compiled, Some(owner))
    });
    let init = debugger::pause(&session, 0).await;
    tokio::time::sleep(std::time::Duration::from_millis(5200)).await;
    debugger::resume(&session, init);
    let runner = startup.await.unwrap().unwrap();
    let worker = std::sync::Arc::clone(&runner);
    let call = tokio::spawn(async move { worker.invoke(invocation()).await });
    let pause = debugger::pause(&session, init).await;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    debugger::local_seven(&session, pause).await;
    debugger::resume(&session, pause);
    assert!(call.await.unwrap().is_ok());
    session.command(DebugAction::Detach).unwrap();
    debugger::wait(&session, |event| matches!(event, DebugEventKind::Detached)).await;
    let next = runner.invoke(invocation()).await.unwrap();
    assert!(
        next.logs
            .iter()
            .flat_map(|run| &run.records)
            .any(|record| record.text == "starts 2")
    );
    let session = debugger::session('b');
    runner
        .attach_debugger(std::sync::Arc::clone(&session))
        .unwrap();
    let worker = std::sync::Arc::clone(&runner);
    let call = tokio::spawn(async move { worker.invoke(invocation()).await });
    let pause = debugger::pause(&session, 0).await;
    debugger::local_seven(&session, pause).await;
    debugger::resume(&session, pause);
    assert!(
        call.await
            .unwrap()
            .unwrap()
            .logs
            .iter()
            .flat_map(|run| &run.records)
            .any(|record| record.text == "starts 3")
    );
    session.stop("Test ended");
    assert!(runner.has_failed());
}

#[tokio::test]
async fn native_stop_terminates_a_debugged_infinite_loop_without_an_engine_reply() {
    let session = debugger::session('c');
    let owner = std::sync::Arc::clone(&session);
    let compiled = compile(
        "export function onRequestHead(event):never{debugger;while(true){}}",
        5000,
    );
    let runner = tokio::task::spawn_blocking(move || {
        SupervisedScriptRunner::start_debug(&config(), compiled, Some(owner))
    })
    .await
    .unwrap()
    .unwrap();
    let worker = std::sync::Arc::clone(&runner);
    let call = tokio::spawn(async move { worker.invoke(invocation()).await });
    let pause = debugger::pause(&session, 0).await;
    debugger::resume(&session, pause);
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    session.stop("Test Stop");
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(runner.has_failed());
}

fn compile(source: &str, max_duration_ms: u64) -> transmog_script::CompiledScript {
    compile_typescript(
        ScriptManifest {
            parameters: std::collections::BTreeMap::new(),
            request_body_filter: transmog_script::ScriptBodyFilter::default(),
            response_body_filter: transmog_script::ScriptBodyFilter::default(),
            id: "sandbox-test".to_owned(),
            revision: 1,
            api_revision: transmog_script::SCRIPT_API_REVISION,
            source_hash: source_hash(source.as_bytes()),
            handlers: BTreeSet::from([ScriptHandler::RequestHead]),
            prefilter: ScriptPrefilter::default(),
            capabilities: ScriptCapabilities {
                abort: true,
                ..ScriptCapabilities::default()
            },
            limits: ScriptLimits {
                max_duration_ms,
                ..ScriptLimits::default()
            },
            priority: 0,
        },
        source,
    )
    .unwrap()
}

fn invocation() -> ScriptInvocation {
    ScriptInvocation {
        bytes: None,
        end: false,
        can_edit_headers: false,
        parameters: std::collections::BTreeMap::new(),
        state: std::collections::BTreeMap::new(),
        local_response: false,
        failure: None,
        context: ScriptContext {
            exchange_id: "1".to_owned(),
            now_unix_ms: 0,
            handler: ScriptHandler::RequestHead,
            ..ScriptContext::default()
        },
        request: ScriptRequest {
            method: "GET".to_owned(),
            scheme: "https".to_owned(),
            host: "example.test".to_owned(),
            port: 443,
            path: "/".to_owned(),
            query: None,
            headers: Vec::new(),
        },
        response: None,
        body: None,
    }
}

fn config() -> ScriptHostConfig {
    ScriptHostConfig {
        executable: std::path::PathBuf::from(env!("CARGO_BIN_EXE_transmog-script-host")),
        isolation: IsolationPolicy::RequireSandbox,
    }
}

#[tokio::test]
async fn console_streams_survive_throw_validation_and_timeout_without_changing_decisions() {
    let source = r"
        console.info('initializer');
        export function onRequestHead(event) {
            console.log('before', event.context.nowUnixMs);
            console.trace('trace');
            if(event.context.nowUnixMs===1) throw new Error('failure');
            if(event.context.nowUnixMs===2) return {bad:true};
            if(event.context.nowUnixMs===3) while(true) {}
            if(event.context.nowUnixMs===4) for(let i=0;i<1000;i++) console.warn('flood'.repeat(512));
            return {decision:{action:'continue'}};
        }
    ";
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 1000)).unwrap();
    assert_eq!(runner.startup_logs()[0].records[0].text, "initializer");
    for operation in [0, 1, 2, 4, 0, 3] {
        let mut input = invocation();
        input.context.now_unix_ms = operation;
        let result = runner.invoke(input).await;
        let logs = if matches!(operation, 1..=3) {
            let error = result.unwrap_err();
            if operation == 3 {
                assert_eq!(
                    error.category,
                    transmog_script::ScriptFailureCategory::Timeout
                );
            }
            error.logs
        } else {
            let result = result.unwrap();
            assert_eq!(result.decision, ScriptAction::Continue);
            result.logs
        };
        let call = logs.last().unwrap();
        assert!(call.is_bounded());
        assert_eq!(call.context.callback, "onRequestHead");
        assert_eq!(call.context.exchange_id.as_deref(), Some("1"));
        assert_eq!(call.records[0].text, format!("before {operation}"));
        assert!(call.records[1].frames[0].original.is_some());
        assert_eq!(call.incomplete, operation == 3);
        if operation == 4 {
            assert!(call.dropped > 0);
        }
    }
}

#[test]
fn console_records_preceding_initializer_failure_remain_available() {
    let source = "console.error('startup detail'); throw new Error('failed'); export function onRequestHead() { return {decision:{action:'continue'}}; }";
    let error = SupervisedScriptRunner::start(&config(), compile(source, 1000))
        .err()
        .unwrap();
    assert_eq!(error.logs[0].context.callback, "initializer");
    assert_eq!(error.logs[0].records[0].text, "startup detail");
    assert!(!error.logs[0].incomplete);
    assert_eq!(error.logs[0].context.worker.len(), 64);
    let another = SupervisedScriptRunner::start(&config(), compile(source, 1000))
        .err()
        .unwrap();
    assert_eq!(
        error.logs[0].context.revision,
        another.logs[0].context.revision
    );
    assert_ne!(error.logs[0].context.worker, another.logs[0].context.worker);
    let diagnostic = error.exception.as_ref().unwrap();
    let artifact = diagnostic.artifact.as_ref().unwrap();
    assert_eq!(Some(artifact), error.logs[0].context.artifact.as_ref());
    let generated = transmog_script::diagnostic_sources::read(
        artifact,
        "file:///transmog-script/main.js",
        1,
        1,
    )
    .unwrap();
    assert!(
        generated
            .lines
            .iter()
            .any(|line| line.text.contains("startup detail"))
    );
    let original = diagnostic
        .exception
        .frames
        .iter()
        .find_map(|frame| frame.original.as_ref())
        .unwrap();
    let page = transmog_script::diagnostic_sources::read(
        artifact,
        &original.source,
        original.line,
        original.column,
    )
    .unwrap();
    assert!(
        page.lines
            .iter()
            .any(|line| line.text.contains("startup detail"))
    );
    assert!(transmog_script::diagnostic_sources::read(artifact, "transmog:invoke", 1, 1).is_none());
}

#[test]
fn identical_javascript_with_different_original_types_keeps_distinct_source_snapshots() {
    let original = "const marker: number = 1; throw new Error('same generated code'); export function onRequestHead(){return {decision:{action:'continue'}};}";
    let edited = original.replace("marker: number", "marker: 1");
    let first = compile(original, 1000);
    let second = compile(&edited, 1000);
    assert_eq!(first.javascript, second.javascript);
    let first = SupervisedScriptRunner::start(&config(), first)
        .err()
        .unwrap();
    let second = SupervisedScriptRunner::start(&config(), second)
        .err()
        .unwrap();
    let first = first.exception.unwrap();
    let second = second.exception.unwrap();
    assert_ne!(first.artifact, second.artifact);
    let location = first
        .exception
        .frames
        .iter()
        .find_map(|frame| frame.original.as_ref())
        .unwrap();
    let page = transmog_script::diagnostic_sources::read(
        first.artifact.as_ref().unwrap(),
        &location.source,
        location.line,
        location.column,
    )
    .unwrap();
    assert!(page.lines[0].text.contains("marker: number"));
    let page = transmog_script::diagnostic_sources::read(
        second.artifact.as_ref().unwrap(),
        &location.source,
        1,
        1,
    )
    .unwrap();
    assert!(page.lines[0].text.contains("marker: 1"));
}

#[tokio::test]
async fn abandoning_a_logging_callback_keeps_accepted_output_in_worker_history() {
    let runner=SupervisedScriptRunner::start(&config(),compile("export function onRequestHead(){console.error('accepted before cancellation');while(true){}}",1000)).unwrap();
    let owned = runner.clone();
    let task = tokio::spawn(async move { owned.invoke(invocation()).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if runner.logs().iter().any(|run| {
                run.records
                    .iter()
                    .any(|record| record.text == "accepted before cancellation")
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    let logs = runner.logs();
    assert_eq!(
        logs.last().unwrap().records[0].text,
        "accepted before cancellation"
    );
    assert!(logs.last().unwrap().incomplete);
    assert!(runner.has_failed());
    runner.clear_logs();
    assert!(runner.logs().iter().all(|run| run.records.is_empty()));
}

#[tokio::test]
async fn every_callback_preserves_structured_exception_evidence() {
    let handlers = [
        ScriptHandler::RequestHead,
        ScriptHandler::RequestBody,
        ScriptHandler::ResponseHead,
        ScriptHandler::ResponseBody,
        ScriptHandler::Completed,
        ScriptHandler::Failed,
    ];
    let source = handlers
        .iter()
        .map(|handler| {
            format!(
                "export function {}(){{throw new Error('{}');}}\n",
                handler.export_name(),
                handler.export_name()
            )
        })
        .collect::<String>();
    let mut manifest = compile(
        "export function onRequestHead(){return {decision:{action:'continue'}};}",
        1000,
    )
    .manifest;
    manifest.handlers = BTreeSet::from(handlers);
    manifest.capabilities.read_bodies = true;
    manifest.source_hash = source_hash(source.as_bytes());
    let runner =
        SupervisedScriptRunner::start(&config(), compile_typescript(manifest, &source).unwrap())
            .unwrap();
    for handler in handlers {
        let mut event = invocation();
        event.context.handler = handler;
        if matches!(
            handler,
            ScriptHandler::ResponseHead | ScriptHandler::ResponseBody | ScriptHandler::Completed
        ) {
            event.response = Some(transmog_script::ScriptResponse {
                status: 200,
                headers: vec![],
            });
        }
        if matches!(
            handler,
            ScriptHandler::RequestBody | ScriptHandler::ResponseBody
        ) {
            event.body = Some(transmog_script::ScriptBody { bytes: vec![] });
        }
        if handler == ScriptHandler::Failed {
            event.failure = Some(serde_json::from_str("\"Cancelled\"").unwrap());
        }
        let failure = runner.invoke(event).await.unwrap_err();
        let diagnostic = failure.exception.unwrap();
        assert!(diagnostic.is_bounded());
        assert_eq!(diagnostic.exception.message, handler.export_name());
        assert_eq!(
            diagnostic.exception.frames[0].function,
            handler.export_name()
        );
        assert!(diagnostic.exception.frames[0].original.is_some());
        assert!(!runner.has_failed());
    }
}

#[tokio::test]
async fn complete_scenario_uses_native_helpers_state_selection_and_timing() {
    let source = r#"
      import {pass, readCookies, readForm, replaceForm, replaceJson} from 'transmog:api';
      export function onRequestHead(event) { return pass({state:{cookies:readCookies(event.request.headers)}}); }
      export function onRequestBody(event) {
        const form=readForm(event.body,event.request.headers);
        return {...replaceForm([...form,['extra','yes']]),state:{...event.state,form}};
      }
      export function onResponseBody(event) {
        const name=event.state.form.find(([key])=>key==='name')[1].normalize();
        return replaceJson({...event.state,name,letters:/^\p{L}+$/u.test(name),localizedPrice:new Intl.NumberFormat('de-DE').format(1234.5)});
      }
      export function onCompleted(event) {
        if(event.context.source !== 'sandbox' || event.context.elapsedMs !== 400 || event.context.nowUnixMs !== 1400) throw new Error('wrong context');
        return pass({diagnostics:['scenario complete']});
      }
    "#;
    let mut manifest = compile(
        "export function onRequestHead() { return {decision:{action:'continue'}}; }",
        1000,
    )
    .manifest;
    manifest.source_hash = source_hash(source.as_bytes());
    manifest.handlers = BTreeSet::from([
        ScriptHandler::RequestHead,
        ScriptHandler::RequestBody,
        ScriptHandler::ResponseBody,
        ScriptHandler::Completed,
    ]);
    manifest.capabilities.read_bodies = true;
    manifest.capabilities.read_sensitive_headers = true;
    manifest.capabilities.write_body = true;
    manifest
        .capabilities
        .write_headers
        .insert("content-type".into());
    manifest.request_body_filter.content_types = vec!["application/x-www-form-urlencoded".into()];
    manifest.response_body_filter.content_types = vec!["application/json".into()];
    let compiled = compile_typescript(manifest.clone(), source).unwrap();
    let runner = SupervisedScriptRunner::start(&config(), compiled).unwrap();
    let mut request = invocation().request;
    request.headers = vec![
        transmog_script::ScriptHeader {
            name: "cookie".into(),
            value: b"a=1; a=2".to_vec(),
            sensitive: false,
            redacted: false,
        },
        transmog_script::ScriptHeader {
            name: "content-type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
            sensitive: false,
            redacted: false,
        },
    ];
    let result = transmog_script::test_scenario(
        manifest,
        runner,
        transmog_script::ScriptScenario {
            response_chunks: None,
            request,
            response: Some(transmog_script::ScriptResponse {
                status: 200,
                headers: vec![transmog_script::ScriptHeader {
                    name: "content-type".into(),
                    value: b"application/json".to_vec(),
                    sensitive: false,
                    redacted: false,
                }],
            }),
            request_body: b"x=one&x=two&name=E%CC%81lodie".to_vec(),
            response_body: b"{}".to_vec(),
            local_response: false,
            failure: None,
            http_version: invocation().context.http_version,
            started_unix_ms: 1000,
            duration_ms: 400,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.outcome, "completed");
    assert_eq!(
        result.request_body,
        b"x=one&x=two&name=E%CC%81lodie&extra=yes"
    );
    let output: serde_json::Value = serde_json::from_slice(&result.response_body).unwrap();
    assert_eq!(
        output["cookies"],
        serde_json::json!([["a", "1"], ["a", "2"]])
    );
    assert_eq!(
        output["form"],
        serde_json::json!([["x", "one"], ["x", "two"], ["name", "E\u{301}lodie"]])
    );
    assert_eq!(output["name"], "Élodie");
    assert_eq!(output["letters"], true);
    assert_eq!(output["localizedPrice"], "1.234,5");
    assert_eq!(
        result.steps[5].result.as_ref().unwrap().diagnostics,
        ["scenario complete"]
    );
}

#[tokio::test]
async fn streaming_scenario_uses_isolated_helpers_and_deferred_header_authority() {
    let source = r#"
      import {pass, edit, needMore, deferResponseHeaders, replaceTextChunk, setHeader} from 'transmog:api';
      export function onResponseHead(event) { return event.parameters.defer ? deferResponseHeaders() : pass(); }
      export function onResponseChunk(event) {
        if (!Object.isFrozen(event.bytes) || typeof event.end !== 'boolean' || typeof event.canEditHeaders !== 'boolean') throw new Error('invalid streaming input');
        console.info('chunk window', event.bytes.length, event.end);
        if (event.end && event.parameters.failAtEnd) throw new Error('stream failed at EOF');
        if (event.canEditHeaders) {
          if (event.bytes.length < 4 && !event.end) return needMore();
          return edit({headers:[setHeader('x-inspected','yes')]});
        }
        return replaceTextChunk(event,'before','after');
      }
    "#;
    for (deferred, fail_at_end) in [(false, false), (true, false), (true, true)] {
        let mut manifest = compile(
            "export function onRequestHead() { return {decision:{action:'continue'}}; }",
            1000,
        )
        .manifest;
        manifest.source_hash = source_hash(source.as_bytes());
        manifest.handlers =
            BTreeSet::from([ScriptHandler::ResponseHead, ScriptHandler::ResponseChunk]);
        manifest.capabilities.read_bodies = true;
        manifest.capabilities.write_body = true;
        manifest
            .capabilities
            .write_headers
            .insert("x-inspected".into());
        manifest.parameters.insert(
            "defer".into(),
            transmog_script::ScriptParameter::Boolean {
                label: "Defer headers".into(),
                default: deferred,
                value: None,
            },
        );
        manifest.parameters.insert(
            "failAtEnd".into(),
            transmog_script::ScriptParameter::Boolean {
                label: "Fail at EOF".into(),
                default: fail_at_end,
                value: None,
            },
        );
        let runner = SupervisedScriptRunner::start(
            &config(),
            compile_typescript(manifest.clone(), source).unwrap(),
        )
        .unwrap();
        let result = transmog_script::test_scenario(
            manifest,
            runner,
            transmog_script::ScriptScenario {
                response_chunks: Some(vec![
                    transmog_script::ScriptScenarioChunk {
                        bytes: b"be".to_vec(),
                        delay_ms: 0,
                    },
                    transmog_script::ScriptScenarioChunk {
                        bytes: b"fore-".to_vec(),
                        delay_ms: 5,
                    },
                    transmog_script::ScriptScenarioChunk {
                        bytes: b"be".to_vec(),
                        delay_ms: 5,
                    },
                    transmog_script::ScriptScenarioChunk {
                        bytes: b"fore".to_vec(),
                        delay_ms: 5,
                    },
                ]),
                request: invocation().request,
                response: Some(transmog_script::ScriptResponse {
                    status: 200,
                    headers: vec![],
                }),
                request_body: vec![],
                response_body: b"before-before".to_vec(),
                local_response: false,
                failure: None,
                http_version: invocation().context.http_version,
                started_unix_ms: 1000,
                duration_ms: 100,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            result.verdict,
            if fail_at_end {
                transmog_script::ScriptScenarioVerdict::Failed
            } else {
                transmog_script::ScriptScenarioVerdict::Passed
            },
            "{:?}",
            result.error
        );
        assert_eq!(
            result.response_body,
            if deferred {
                b"before-after".as_slice()
            } else {
                b"after-after".as_slice()
            }
        );
        assert_eq!(
            result.headers_released_at_ms,
            Some(if deferred { 55 } else { 50 })
        );
        assert!(result.stream_calls.last().unwrap().end);
        let reports = &result
            .steps
            .iter()
            .find(|step| step.handler == ScriptHandler::ResponseChunk)
            .unwrap()
            .reports;
        let logs: Vec<_> = reports
            .iter()
            .flat_map(|report| &report.logs)
            .flat_map(|run| &run.records)
            .collect();
        assert!(
            logs.iter()
                .any(|record| record.text == "chunk window 2 false")
        );
        assert!(
            logs.iter()
                .any(|record| record.text == "chunk window 0 true")
        );
        assert_eq!(logs.len(), result.stream_calls.len());
        assert_eq!(
            reports
                .iter()
                .filter(|report| report.exception.is_some())
                .count(),
            usize::from(fail_at_end)
        );
        assert_eq!(
            result
                .response
                .unwrap()
                .headers
                .iter()
                .any(|header| header.name == "x-inspected"),
            deferred
        );
    }
}

#[tokio::test]
async fn complete_body_observation_and_header_edits_use_the_input_limit() {
    for handler in [ScriptHandler::RequestBody, ScriptHandler::ResponseBody] {
        for (decision, replaces) in [
            ("pass()", false),
            ("edit({headers:[setHeader('x-inspected','yes')]})", false),
            ("edit({bytes:[1,2]})", true),
        ] {
            let source = format!(
                "import {{pass, edit, setHeader}} from 'transmog:api'; export function {}() {{ return {decision}; }}",
                handler.export_name()
            );
            let mut manifest = compile(
                "export function onRequestHead() { return {decision:{action:'continue'}}; }",
                1000,
            )
            .manifest;
            manifest.source_hash = source_hash(source.as_bytes());
            manifest.handlers = BTreeSet::from([handler]);
            manifest.capabilities.read_bodies = true;
            manifest.capabilities.write_body = replaces;
            manifest
                .capabilities
                .write_headers
                .insert("x-inspected".into());
            manifest.limits.max_input_body_bytes = 8;
            manifest.limits.max_output_bytes = 1;
            let runner = SupervisedScriptRunner::start(
                &config(),
                compile_typescript(manifest.clone(), &source).unwrap(),
            )
            .unwrap();
            let result = transmog_script::test_scenario(
                manifest,
                runner,
                transmog_script::ScriptScenario {
                    response_chunks: None,
                    request: invocation().request,
                    response: Some(transmog_script::ScriptResponse {
                        status: 200,
                        headers: vec![],
                    }),
                    request_body: b"upload".to_vec(),
                    response_body: b"reply".to_vec(),
                    local_response: false,
                    failure: None,
                    http_version: invocation().context.http_version,
                    started_unix_ms: 1000,
                    duration_ms: 100,
                },
            )
            .await
            .unwrap();
            assert_eq!(
                result.verdict,
                if replaces {
                    transmog_script::ScriptScenarioVerdict::Failed
                } else {
                    transmog_script::ScriptScenarioVerdict::Passed
                },
                "{handler:?} {decision}: {:?}",
                result.error
            );
            if replaces {
                assert_eq!(
                    result
                        .steps
                        .iter()
                        .find(|step| step.handler == handler)
                        .unwrap()
                        .error
                        .as_ref()
                        .unwrap()
                        .category,
                    transmog_script::ScriptFailureCategory::InvalidAction
                );
            } else {
                assert_eq!(result.request_body, b"upload");
                assert_eq!(result.response_body, b"reply");
                let headers = if handler == ScriptHandler::RequestBody {
                    &result.request.headers
                } else {
                    &result.response.as_ref().unwrap().headers
                };
                assert_eq!(
                    headers.iter().any(|header| header.name == "x-inspected"),
                    decision.starts_with("edit(")
                );
            }
        }
    }
}

#[tokio::test]
async fn bundled_helpers_work_inside_the_native_sandbox() {
    let source = r#"
      import { respondJson, requestUrl, utf8Decode, base64Decode } from "transmog:api";
      export function onRequestHead(event) {
        return { ...respondJson(201, { url: requestUrl(event.request), text: utf8Decode(base64Decode("aMOpbGxv")) }), diagnostics: ["constructed response"] };
      }
    "#;
    let mut compiled = compile(source, 500);
    compiled.manifest.capabilities.respond = true;
    let runner = SupervisedScriptRunner::start(&config(), compiled).unwrap();
    let result = runner.invoke(invocation()).await.unwrap();
    let ScriptAction::Respond {
        response: transmog_script::ScriptResponseSource::Inline(response),
    } = result.decision
    else {
        panic!("inline response missing")
    };
    assert_eq!(response.status, 201);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&response.bytes).unwrap(),
        serde_json::json!({"url":"https://example.test/","text":"héllo"})
    );
    assert_eq!(result.diagnostics, ["constructed response"]);
}

#[tokio::test]
async fn appcontainer_host_executes_without_ambient_authority() {
    let source = r#"
      export function onRequestHead() {
        if (typeof Deno !== "undefined" || typeof fetch !== "undefined" ||
            typeof WebSocket !== "undefined" || typeof WebAssembly !== "undefined") {
          throw new Error("ambient capability exposed");
        }
        return { decision: { action: "abort", reason: "sandboxed" } };
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    assert_eq!(
        runner.invoke(invocation()).await.unwrap().decision,
        ScriptAction::Abort {
            reason: "sandboxed".to_owned()
        }
    );
}

#[tokio::test]
async fn infinite_loop_is_killed_and_host_stays_failed_closed() {
    let source = r#"
      export function onRequestHead() {
        while (true) {}
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 50)).unwrap();
    let first = runner.invoke(invocation()).await.unwrap_err();
    assert_eq!(
        first.category,
        transmog_script::ScriptFailureCategory::Timeout
    );
    let second = runner.invoke(invocation()).await.unwrap_err();
    assert_eq!(
        second.category,
        transmog_script::ScriptFailureCategory::Unavailable
    );
}

#[tokio::test]
async fn allocation_bomb_cannot_take_the_desktop_or_proxy_with_it() {
    let source = r#"
      export function onRequestHead() {
        const values = [];
        while (true) values.push(new Array(1_000_000).fill("bounded"));
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 2_000)).unwrap();
    assert!(runner.invoke(invocation()).await.is_err());
    assert!(runner.has_failed());
}

#[tokio::test]
async fn native_arraybuffer_memory_is_bounded_without_affecting_another_script() {
    let healthy_source = "export function onRequestHead() { return { decision: {action:'abort',reason:'healthy'} }; }";
    let healthy = SupervisedScriptRunner::start(&config(), compile(healthy_source, 500)).unwrap();
    let source = r"
export function onRequestHead() {
  const bytes = new Uint8Array(256 * 1024 * 1024);
  bytes.fill(165);
  return { decision: {action:'abort',reason:String(bytes[bytes.length - 1])} };
}
";
    let mut compiled = compile(source, 2_000);
    // The native process cap is 3 * heap + 128 MiB. A 256 MiB backing store
    // exceeds this 152 MiB cap while requiring almost no JavaScript heap.
    compiled.manifest.limits.max_heap_bytes = 8 * 1024 * 1024;
    let runner = SupervisedScriptRunner::start(&config(), compiled).unwrap();
    let failure = runner.invoke(invocation()).await.unwrap_err();
    assert_ne!(
        failure.category,
        transmog_script::ScriptFailureCategory::Timeout,
        "the allocation must hit the native limit before its deadline"
    );
    assert_eq!(
        healthy.invoke(invocation()).await.unwrap().decision,
        ScriptAction::Abort {
            reason: "healthy".to_owned()
        }
    );
}

#[test]
fn recursively_queued_native_microtasks_are_contained_by_the_startup_deadline() {
    let source = r"
function repeat() { queueMicrotask(repeat); }
queueMicrotask(repeat);
export function onRequestHead() { return { decision: {action:'continue'} }; }
";
    let started = std::time::Instant::now();
    let Err(failure) = SupervisedScriptRunner::start(&config(), compile(source, 500)) else {
        panic!("an endlessly draining microtask queue must not become ready");
    };
    assert_eq!(
        failure.category,
        transmog_script::ScriptFailureCategory::Timeout
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
}

#[tokio::test]
async fn hostile_output_and_exception_getters_do_not_poison_later_script_calls() {
    for hostile in [
        "return {get then() {throw new Error('then getter');}};",
        "return new Proxy({}, {get() {throw new Error('proxy getter');}});",
        "return {toJSON() {throw new Proxy({}, {get() {throw new Error('diagnostic getter');}});}};",
    ] {
        let source = format!(
            "let calls=0; export function onRequestHead() {{ \
             if(++calls===1) {{ {hostile} }} return {{ decision: {{action:'abort',reason:'recovered'}} }}; }}"
        );
        let runner = SupervisedScriptRunner::start(&config(), compile(&source, 500)).unwrap();
        let failure = runner.invoke(invocation()).await.unwrap_err();
        assert_eq!(
            failure.category,
            transmog_script::ScriptFailureCategory::Exception,
            "hostile case: {hostile}"
        );
        assert!(failure.message.len() <= 512);
        assert_eq!(
            runner.invoke(invocation()).await.unwrap().decision,
            ScriptAction::Abort {
                reason: "recovered".to_owned()
            }
        );
    }
}

#[tokio::test]
async fn serialization_and_exception_diagnostics_remain_under_the_callback_deadline() {
    for hostile in [
        "return {get then() {while(true) {}}};",
        "return {toJSON() {while(true) {}}};",
    ] {
        let source = format!("export function onRequestHead() {{ {hostile} }}");
        let runner = SupervisedScriptRunner::start(&config(), compile(&source, 75)).unwrap();
        assert_eq!(
            runner.invoke(invocation()).await.unwrap_err().category,
            transmog_script::ScriptFailureCategory::Timeout,
            "hostile case: {hostile}"
        );
        assert!(runner.has_failed());
        assert_eq!(
            runner.invoke(invocation()).await.unwrap_err().category,
            transmog_script::ScriptFailureCategory::Unavailable
        );
    }
}

#[tokio::test]
async fn exception_accessors_are_not_executed_and_do_not_kill_the_worker() {
    let source = "export function onRequestHead() {throw {get stack(){while(true){}}, get message(){while(true){}}};}";
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 100)).unwrap();
    for _ in 0..2 {
        let failure = runner.invoke(invocation()).await.unwrap_err();
        assert_eq!(
            failure.category,
            transmog_script::ScriptFailureCategory::Exception
        );
        assert!(failure.exception.unwrap().exception.unavailable);
        assert!(!runner.has_failed());
    }
}

#[tokio::test]
async fn abandoning_a_polled_invocation_keeps_the_script_watchdog_active() {
    let source = "export function onRequestHead() {while(true) {}}";
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 75)).unwrap();
    let mut pending = runner.invoke(invocation());
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(pending.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(pending);
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        runner.has_failed(),
        "canceling the caller must retain containment"
    );
    assert_eq!(
        runner.invoke(invocation()).await.unwrap_err().category,
        transmog_script::ScriptFailureCategory::Unavailable
    );
}

#[tokio::test]
async fn dropping_an_unpolled_script_invocation_cannot_strand_work() {
    let source = r"
export function onRequestHead({ context }) {
  if(context.exchangeId === 'abandoned') while(true) {}
  return { decision: {action:'abort',reason:'healthy'} };
}
";
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 75)).unwrap();
    let mut abandoned = invocation();
    abandoned.context.exchange_id = "abandoned".to_owned();
    drop(runner.invoke(abandoned));
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(runner.has_failed());
    assert_eq!(
        runner.invoke(invocation()).await.unwrap_err().category,
        transmog_script::ScriptFailureCategory::Unavailable
    );
}

#[tokio::test]
async fn module_initialization_awaits_promises_with_string_codegen_already_disabled() {
    let source = r#"
      for (const constructor of [
        (() => 0).constructor,
        (async () => 0).constructor,
        (function* () {}).constructor,
        (async function* () {}).constructor,
      ]) {
        let denied = false;
        try { constructor("return 1"); } catch { denied = true; }
        if (!denied) throw new Error("top-level string generation available");
      }
      let queued = false;
      queueMicrotask(() => { queued = true; });
      const initialized = await Promise.resolve("ready");
      export function onRequestHead() {
        if (!queued) throw new Error("initialization microtask did not run");
        return { decision: {action: "abort", reason: initialized} };
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    assert_eq!(
        runner.invoke(invocation()).await.unwrap().decision,
        ScriptAction::Abort {
            reason: "ready".to_owned()
        }
    );
}

#[test]
fn rejected_top_level_await_fails_script_initialization() {
    let source = r#"
      await Promise.reject(new Error("initialization rejected"));
      export function onRequestHead() { return { decision: {action: "abort", reason: "unreachable"} }; }
    "#;
    let Err(failure) = SupervisedScriptRunner::start(&config(), compile(source, 500)) else {
        panic!("a rejected module must not become ready");
    };
    assert_eq!(
        failure.category,
        transmog_script::ScriptFailureCategory::Exception
    );
}

#[test]
fn throwing_native_microtasks_fail_script_initialization() {
    let source = "queueMicrotask(() => { throw new Error('native microtask failed'); }); export function onRequestHead() { return { decision: {action:'continue'} }; }";
    let Err(failure) = SupervisedScriptRunner::start(&config(), compile(source, 500)) else {
        panic!("an uncaught initialization microtask must not become ready");
    };
    assert_eq!(
        failure.category,
        transmog_script::ScriptFailureCategory::Exception
    );
    assert!(failure.message.contains("native microtask failed"));
}

#[test]
fn initialization_errors_map_to_original_typescript_positions() {
    let source = concat!(
        "type Marker = {message: string};\n",
        "// This comment remains while the declarations disappear.\n",
        "interface Removed { value: number; }\n",
        "const marker: Marker = {message: 'initialization failure'};\n",
        "throw new Error(marker.message);\n",
        "export function onRequestHead() { return { decision: {action: 'continue'} }; }\n",
    );
    let Err(failure) = SupervisedScriptRunner::start(&config(), compile(source, 500)) else {
        panic!("the top-level exception must fail initialization");
    };
    assert_eq!(
        failure.category,
        transmog_script::ScriptFailureCategory::Exception
    );
    assert!(failure.message.contains("initialization failure"));
    assert_eq!((failure.line, failure.column), (Some(5), Some(7)));
}

#[tokio::test]
async fn handler_errors_map_to_original_typescript_positions() {
    let source = concat!(
        "import type { RequestHeadResult } from 'transmog:api';\n",
        "interface Removed { value: string; }\n",
        "// Preserve this comment without losing original positions.\n",
        "export function onRequestHead(): RequestHeadResult {\n",
        "  const value: string = 'handler failure';\n",
        "  throw new Error(value);\n",
        "}\n",
    );
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    let failure = runner.invoke(invocation()).await.unwrap_err();
    assert_eq!(
        failure.category,
        transmog_script::ScriptFailureCategory::Exception
    );
    assert!(failure.message.contains("handler failure"));
    assert_eq!((failure.line, failure.column), (Some(6), Some(9)));
    let diagnostic = failure.exception.unwrap();
    assert!(diagnostic.is_bounded());
    assert_eq!(
        diagnostic.exception.frames[0]
            .original
            .as_ref()
            .unwrap()
            .line,
        6
    );
}

#[tokio::test]
async fn every_nested_exception_frame_maps_to_the_executing_typescript() {
    let source = concat!(
        "interface Removed { value: string; }\n",
        "type Marker = string;\n",
        "function inner() {\n",
        "  const message: Marker = 'nested failure';\n",
        "  throw new Error(message, {cause: new TypeError('cause')});\n",
        "}\n",
        "export function onRequestHead() {\n",
        "  return inner();\n",
        "}\n",
    );
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    let failure = runner.invoke(invocation()).await.unwrap_err();
    let diagnostic = failure.exception.unwrap();
    assert!(diagnostic.is_bounded());
    assert_eq!(
        diagnostic.exception.frames[0]
            .original
            .as_ref()
            .unwrap()
            .line,
        5
    );
    assert_eq!(
        diagnostic.exception.frames[1]
            .original
            .as_ref()
            .unwrap()
            .line,
        8
    );
    assert_eq!(
        diagnostic.causes[0].frames[0]
            .original
            .as_ref()
            .unwrap()
            .line,
        5
    );
    assert_eq!(diagnostic.causes[0].message, "cause");
}

#[tokio::test]
async fn exception_source_columns_preserve_utf16_after_astral_characters() {
    let source =
        "export function onRequestHead() { const face: string = \"😀\"; throw new Error(face); }";
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    let failure = runner.invoke(invocation()).await.unwrap_err();
    assert_eq!((failure.line, failure.column), (Some(1), Some(68)));
    let diagnostic = failure.exception.unwrap();
    assert_eq!(
        diagnostic.exception.frames[0]
            .original
            .as_ref()
            .unwrap()
            .column,
        68
    );
}

#[tokio::test]
async fn callbacks_keep_inputs_frozen_and_do_not_run_queued_microtasks() {
    let source = r#"
      let calls = 0;
      let microtasks = 0;
      export function onRequestHead({ context, request }) {
        if (!Object.isFrozen(context) || !Object.isFrozen(request) ||
            !Object.isFrozen(request.headers)) throw new Error("mutable callback input");
        if (microtasks !== 0) throw new Error("callback microtasks were executed");
        Promise.resolve().then(() => microtasks++);
        queueMicrotask(() => microtasks++);
        return { decision: {action: "abort", reason: String(++calls)} };
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    for calls in 1..=3 {
        assert_eq!(
            runner.invoke(invocation()).await.unwrap().decision,
            ScriptAction::Abort {
                reason: calls.to_string()
            }
        );
    }
}

#[tokio::test]
async fn promises_and_thenables_are_rejected_without_poisoning_the_script() {
    for result in [
        "Promise.resolve({action: 'abort', reason: 'async'})",
        "{then() {}, action: 'abort', reason: 'thenable'}",
    ] {
        let source = format!(
            "let calls = 0; export function onRequestHead() {{ \
             if (++calls === 1) return {result}; \
             return {{ decision: {{action: 'abort', reason: 'recovered'}} }}; }}"
        );
        let runner = SupervisedScriptRunner::start(&config(), compile(&source, 500)).unwrap();
        assert_eq!(
            runner.invoke(invocation()).await.unwrap_err().category,
            transmog_script::ScriptFailureCategory::Exception
        );
        assert_eq!(
            runner.invoke(invocation()).await.unwrap().decision,
            ScriptAction::Abort {
                reason: "recovered".to_owned()
            }
        );
    }
}

#[tokio::test]
async fn body_sample_clear_switch_does_not_decode_headers_or_require_header_writes() {
    let source = help_example("Body helpers: text, JSON, forms and raw bytes").replace(
        "const shouldClearBody = false;",
        "const shouldClearBody = true;",
    );
    let mut manifest = compile(
        "export function onRequestHead() { return { decision: { action: 'continue' } }; }",
        500,
    )
    .manifest;
    manifest.handlers = BTreeSet::from([ScriptHandler::RequestBody, ScriptHandler::ResponseBody]);
    manifest.capabilities = ScriptCapabilities {
        read_bodies: true,
        write_body: true,
        ..ScriptCapabilities::default()
    };
    manifest.source_hash = source_hash(source.as_bytes());
    let runner =
        SupervisedScriptRunner::start(&config(), compile_typescript(manifest, &source).unwrap())
            .unwrap();
    let mut input = invocation();
    input.context.handler = ScriptHandler::ResponseBody;
    input.body = Some(transmog_script::ScriptBody {
        bytes: vec![255, 0],
    });
    input.response = Some(transmog_script::ScriptResponse {
        status: 200,
        headers: vec![transmog_script::ScriptHeader {
            name: "content-type".into(),
            value: b"text/plain; note=\xff".to_vec(),
            sensitive: false,
            redacted: false,
        }],
    });
    let result = runner.invoke(input).await.unwrap();
    let ScriptAction::Edit { edits } = result.decision else {
        panic!("The clear switch did not edit the body")
    };
    assert_eq!(edits.bytes, Some(Vec::new()));
    assert!(edits.headers.is_empty());
}

fn help_example(title: &str) -> String {
    let help = transmog_script::help();
    help["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["id"] == "examples")
        .unwrap()["examples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|example| example["title"] == title)
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .into()
}

async fn streaming_help_scenario(
    title: &str,
    toggle: &str,
    chunks: &[&[u8]],
) -> transmog_script::ScriptScenarioResult {
    let source = help_example(title);
    let switch = format!("const {toggle} = false;");
    assert!(source.contains(&switch));
    let source = source.replace(&switch, &format!("const {toggle} = true;"));
    let inspect = toggle == "shouldInspectInitialBytes";
    let mut manifest = compile(
        "export function onRequestHead() { return {decision:{action:'continue'}}; }",
        500,
    )
    .manifest;
    manifest.handlers = BTreeSet::from([ScriptHandler::ResponseChunk]);
    manifest.capabilities = ScriptCapabilities {
        read_bodies: true,
        write_body: toggle == "shouldApplySampleChange",
        ..ScriptCapabilities::default()
    };
    if inspect {
        manifest.handlers.insert(ScriptHandler::ResponseHead);
        manifest
            .capabilities
            .write_headers
            .insert("x-sample-prefix".into());
    }
    manifest.response_body_filter.content_types = vec!["text/plain".into()];
    manifest.source_hash = source_hash(source.as_bytes());
    let runner = SupervisedScriptRunner::start(
        &config(),
        compile_typescript(manifest.clone(), &source).unwrap(),
    )
    .unwrap();
    let result = transmog_script::test_scenario(
        manifest,
        runner,
        transmog_script::ScriptScenario {
            response_chunks: Some(
                chunks
                    .iter()
                    .enumerate()
                    .map(|(index, bytes)| transmog_script::ScriptScenarioChunk {
                        bytes: bytes.to_vec(),
                        delay_ms: if index == 0 { 0 } else { 5 },
                    })
                    .collect(),
            ),
            request: invocation().request,
            response: Some(transmog_script::ScriptResponse {
                status: 200,
                headers: vec![transmog_script::ScriptHeader {
                    name: "content-type".into(),
                    value: b"text/plain; charset=utf-8".to_vec(),
                    sensitive: false,
                    redacted: false,
                }],
            }),
            request_body: vec![],
            response_body: chunks
                .iter()
                .flat_map(|bytes| bytes.iter().copied())
                .collect(),
            local_response: false,
            failure: None,
            http_version: invocation().context.http_version,
            started_unix_ms: 1000,
            duration_ms: 100,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        result.verdict,
        transmog_script::ScriptScenarioVerdict::Passed,
        "{:?}",
        result.error
    );
    assert!(result.stream_calls.last().unwrap().end);
    result
}

#[tokio::test]
async fn streaming_help_sample_switches_work_independently() {
    for (title, toggle, expected) in [
        (
            "Queue complete lines before processing",
            "shouldQueueCompleteLines",
            b"before\nlast".as_slice(),
        ),
        (
            "Queue complete lines before processing",
            "shouldApplySampleChange",
            b"after\nlast".as_slice(),
        ),
        (
            "Streaming text replacement",
            "shouldApplySampleChange",
            b"after\nlast".as_slice(),
        ),
    ] {
        let result = streaming_help_scenario(title, toggle, &[b"be", b"fore\nla", b"st"]).await;
        assert_eq!(result.response_body, expected);
        assert!(result.stream_calls.iter().any(|call| call.action
            == if toggle == "shouldQueueCompleteLines" {
                "forward"
            } else {
                "emit"
            }));
        assert!(
            result
                .stream_calls
                .iter()
                .any(|call| call.action == "need-more")
        );
    }
}

#[tokio::test]
async fn header_inspection_help_sample_handles_split_utf8_and_empty_bodies() {
    for (chunks, expected) in [
        (vec![b"be".as_slice(), b"fore\nlast".as_slice()], "YmVmbw=="),
        (
            vec![b"\xf0\x9f".as_slice(), b"\x8c\x8dtext".as_slice()],
            "8J+MjQ==",
        ),
        (vec![], ""),
    ] {
        let result = streaming_help_scenario(
            "Inspect initial bytes before releasing headers",
            "shouldInspectInitialBytes",
            &chunks,
        )
        .await;
        let original: Vec<u8> = chunks
            .iter()
            .flat_map(|bytes| bytes.iter().copied())
            .collect();
        assert_eq!(result.response_body, original);
        assert_eq!(
            result.headers_released_at_ms,
            Some(if chunks.is_empty() { 75 } else { 55 })
        );
        let response = result.response.unwrap();
        let header = response
            .headers
            .iter()
            .find(|header| header.name == "x-sample-prefix")
            .unwrap();
        assert_eq!(header.value, expected.as_bytes());
    }
}

#[tokio::test]
async fn bundled_help_examples_execute_with_the_documented_permissions() {
    let help = transmog_script::help();
    let examples = help["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["id"] == "examples")
        .unwrap()["examples"]
        .as_array()
        .unwrap();
    for (index, example) in examples.iter().enumerate() {
        let source = example["code"].as_str().unwrap();
        let mut manifest = compile(
            "export function onRequestHead() { return { decision: { action: 'continue' } }; }",
            500,
        )
        .manifest;
        manifest.source_hash = source_hash(source.as_bytes());
        manifest.capabilities = ScriptCapabilities::default();
        manifest.prefilter.host = Some("example.test".into());
        let mut input = invocation();
        match index {
            0 => {
                manifest.handlers = BTreeSet::from([
                    ScriptHandler::RequestHead,
                    ScriptHandler::ResponseHead,
                    ScriptHandler::Completed,
                    ScriptHandler::Failed,
                ]);
            }
            1 => {
                manifest.handlers =
                    BTreeSet::from([ScriptHandler::RequestBody, ScriptHandler::ResponseBody]);
                manifest.capabilities.read_bodies = true;
                input.context.handler = ScriptHandler::RequestBody;
                input.body = Some(transmog_script::ScriptBody {
                    bytes: b"arbitrary non-JSON bytes".to_vec(),
                });
            }
            2 => {
                manifest
                    .capabilities
                    .write_headers
                    .insert("user-agent".into());
            }
            3 => {
                manifest.handlers = BTreeSet::from([ScriptHandler::ResponseBody]);
                manifest.capabilities.read_bodies = true;
                manifest.capabilities.write_body = true;
                manifest
                    .capabilities
                    .write_headers
                    .insert("content-type".into());
                manifest.response_body_filter.content_types = vec!["application/json".into()];
                input.context.handler = ScriptHandler::ResponseBody;
                input.response = Some(transmog_script::ScriptResponse {
                    status: 200,
                    headers: vec![],
                });
                input.body = Some(transmog_script::ScriptBody {
                    bytes: br#"{"value":42}"#.to_vec(),
                });
            }
            4 => {
                manifest.capabilities.respond = true;
                manifest.prefilter.path_prefix = Some("/fixture".into());
                input.request.path = "/fixture".into();
            }
            5 => {
                manifest.handlers.insert(ScriptHandler::Completed);
                manifest.capabilities.annotate = true;
                input.context.now_unix_ms = 100;
            }
            6..=8 => {
                manifest.handlers = BTreeSet::from([ScriptHandler::ResponseChunk]);
                if index == 8 {
                    manifest.handlers.insert(ScriptHandler::ResponseHead);
                    manifest
                        .capabilities
                        .write_headers
                        .insert("x-sample-prefix".into());
                }
                manifest.capabilities.read_bodies = true;
                manifest.capabilities.write_body = false;
                input.context.handler = ScriptHandler::ResponseChunk;
                input.bytes = Some(b"arbitrary streaming bytes".to_vec());
                input.end = true;
                input.response = Some(transmog_script::ScriptResponse {
                    status: 200,
                    headers: vec![],
                });
            }
            _ => panic!("Add execution coverage for the new help example"),
        }
        let handlers = manifest.handlers.clone();
        let compiled = compile_typescript(manifest, source).unwrap();
        let runner = SupervisedScriptRunner::start(&config(), compiled).unwrap();
        let result = runner.invoke(input.clone()).await.unwrap();
        match index {
            0 | 1 | 6..=8 => {
                for handler in handlers {
                    input.context.handler = handler;
                    input.bytes = (handler == ScriptHandler::ResponseChunk)
                        .then(|| b"arbitrary streaming bytes".to_vec());
                    input.end = handler == ScriptHandler::ResponseChunk;
                    input.response = matches!(
                        handler,
                        ScriptHandler::ResponseHead
                            | ScriptHandler::ResponseBody
                            | ScriptHandler::ResponseChunk
                            | ScriptHandler::Completed
                    )
                    .then(|| transmog_script::ScriptResponse {
                        status: 200,
                        headers: vec![],
                    });
                    input.failure = (handler == ScriptHandler::Failed)
                        .then(|| serde_json::from_str("\"Upstream\"").unwrap());
                    let result = runner.invoke(input.clone()).await.unwrap();
                    assert_eq!(result.decision, ScriptAction::Continue);
                    assert!(result.annotations.is_empty());
                    assert!(result.diagnostics.is_empty());
                    assert!(result.state.is_none());
                }
            }
            2 => assert!(matches!(result.decision, ScriptAction::Edit { .. })),
            3 => {
                let ScriptAction::Edit { edits } = result.decision else {
                    panic!("JSON edit")
                };
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&edits.bytes.unwrap()).unwrap(),
                    serde_json::json!({"original":{"value":42},"inspected":true})
                );
            }
            4 => {
                let ScriptAction::Respond {
                    response: transmog_script::ScriptResponseSource::Inline(response),
                } = result.decision
                else {
                    panic!("local response")
                };
                assert_eq!(response.status, 200);
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&response.bytes).unwrap(),
                    serde_json::json!({"ready":true})
                );
            }
            5 => {
                input.state = result.state.unwrap();
                input.context.handler = ScriptHandler::Completed;
                input.context.now_unix_ms = 125;
                input.context.elapsed_ms = 25;
                input.response = Some(transmog_script::ScriptResponse {
                    status: 200,
                    headers: vec![],
                });
                let completed = runner.invoke(input).await.unwrap();
                assert_eq!(completed.annotations[0].note, "Completed after 25 ms");
            }
            _ => unreachable!(),
        }
    }
}

//! Windows `AppContainer` integration tests for the script host.

#![cfg(windows)]

use std::collections::BTreeSet;

use transmog_script::{
    ScriptAction, ScriptCapabilities, ScriptContext, ScriptHandler, ScriptInvocation, ScriptLimits,
    ScriptManifest, ScriptPrefilter, ScriptRequest, ScriptRunner, compile_typescript, source_hash,
};
use transmog_script_supervisor::{IsolationPolicy, ScriptHostConfig, SupervisedScriptRunner};

fn compile(source: &str, max_duration_ms: u64) -> transmog_script::CompiledScript {
    compile_typescript(
        ScriptManifest {
            id: "sandbox-test".to_owned(),
            revision: 1,
            api_revision: 1,
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
        context: ScriptContext {
            exchange_id: "1".to_owned(),
            now_unix_ms: 0,
            handler: ScriptHandler::RequestHead,
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
async fn appcontainer_host_executes_without_ambient_authority() {
    let source = r#"
      export function onRequestHead() {
        if (typeof Deno !== "undefined" || typeof fetch !== "undefined" ||
            typeof WebSocket !== "undefined" || typeof WebAssembly !== "undefined") {
          throw new Error("ambient capability exposed");
        }
        return { action: "abort", reason: "sandboxed" };
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    assert_eq!(
        runner.invoke(invocation()).await.unwrap(),
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
        return {action: "abort", reason: initialized};
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    assert_eq!(
        runner.invoke(invocation()).await.unwrap(),
        ScriptAction::Abort {
            reason: "ready".to_owned()
        }
    );
}

#[test]
fn rejected_top_level_await_fails_script_initialization() {
    let source = r#"
      await Promise.reject(new Error("initialization rejected"));
      export function onRequestHead() { return {action: "abort", reason: "unreachable"}; }
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
    let source = "queueMicrotask(() => { throw new Error('native microtask failed'); }); export function onRequestHead() { return {action:'continue'}; }";
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
        "export function onRequestHead() { return {action: 'continue'}; }\n",
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
        "import type { Action } from 'transmog:api/v1';\n",
        "interface Removed { value: string; }\n",
        "// Preserve this comment without losing original positions.\n",
        "export function onRequestHead(): Action {\n",
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
}

#[tokio::test]
async fn callbacks_keep_inputs_frozen_and_do_not_run_queued_microtasks() {
    let source = r#"
      let calls = 0;
      let microtasks = 0;
      export function onRequestHead(context, request) {
        if (!Object.isFrozen(context) || !Object.isFrozen(request) ||
            !Object.isFrozen(request.headers)) throw new Error("mutable callback input");
        if (microtasks !== 0) throw new Error("callback microtasks were executed");
        Promise.resolve().then(() => microtasks++);
        queueMicrotask(() => microtasks++);
        return {action: "abort", reason: String(++calls)};
      }
    "#;
    let runner = SupervisedScriptRunner::start(&config(), compile(source, 500)).unwrap();
    for calls in 1..=3 {
        assert_eq!(
            runner.invoke(invocation()).await.unwrap(),
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
             return {{action: 'abort', reason: 'recovered'}}; }}"
        );
        let runner = SupervisedScriptRunner::start(&config(), compile(&source, 500)).unwrap();
        assert_eq!(
            runner.invoke(invocation()).await.unwrap_err().category,
            transmog_script::ScriptFailureCategory::Exception
        );
        assert_eq!(
            runner.invoke(invocation()).await.unwrap(),
            ScriptAction::Abort {
                reason: "recovered".to_owned()
            }
        );
    }
}

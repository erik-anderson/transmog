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

//! Production `AppContainer` coverage for capability-free extension execution and lifecycle.
#![cfg(windows)]
use std::{collections::BTreeMap, sync::Arc};
use transmog_extension_supervisor::{HostConfig, Runner};
use transmog_extensions::*;

fn snapshot(source: &str) -> Snapshot {
    let helper = "class Target { static title; static { this.title = 'Structured view'; } } export const title = Target.title;";
    let manifest:Manifest=serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.worker","version":"1.0.0","name":"Example worker",
        "visualizers":[{"id":"view","name":"View","entry":"src/main.js","surfaces":["body"],
            "inputs":{"request":{"headers":["x-kind"],"body":"none"},"response":{"headers":["content-type"],"body":"optional"}},
            "matches":[{"direction":"response","contentTypes":["application/example"]}],
            "limits":{"inputBytes":1_048_576,"outputBytes":1_048_576,"heapBytes":67_108_864,"durationMs":1000,"nodes":1000}}],
        "files":[{"path":"src/main.js","bytes":source.len()},{"path":"src/helper.mjs","bytes":helper.len()}]
    })).unwrap();
    Snapshot::new(
        manifest,
        BTreeMap::from([
            ("src/main.js".into(), Arc::<[u8]>::from(source.as_bytes())),
            (
                "src/helper.mjs".into(),
                Arc::<[u8]>::from(helper.as_bytes()),
            ),
        ]),
    )
    .unwrap()
}
fn start(snapshot: Snapshot) -> Result<Arc<Runner>, Failure> {
    Runner::start(
        &HostConfig {
            executable: env!("CARGO_BIN_EXE_transmog-extension-host").into(),
        },
        snapshot,
        "view",
    )
}
fn input() -> VisualizationInput {
    VisualizationInput {
        instance_id: "instance-one".into(),
        generation: "1".into(),
        direction: Direction::Response,
        request: Some(MessageInput {
            evidence_id: "request-one".into(),
            headers: vec![Header {
                source_id: None,
                index: 0,
                name: "Cookie".into(),
                value: Some(b"must-not-cross".to_vec()),
                redacted: false,
                original_bytes: Some("14".into()),
            }],
            body: Some(BodyReference {
                source_id: "forbidden-request".into(),
                length: "4".into(),
                availability: "complete".into(),
            }),
            method: Some("POST".into()),
            path: Some("/example".into()),
            status: None,
        }),
        response: Some(MessageInput {
            evidence_id: "response-one".into(),
            headers: vec![],
            body: Some(BodyReference {
                source_id: "response-body".into(),
                length: "4".into(),
                availability: "complete".into(),
            }),
            method: None,
            path: None,
            status: Some(200),
        }),
    }
}
const DOCUMENT: &str =
    "{kind:'document',document:{title:'View',status:'complete',presentation:'tree',roots:[]}}";

#[tokio::test]
async fn relative_javascript_modules_execute_after_capability_restrictions_and_projection() {
    let source = r"
import {title} from './helper.mjs';
if (typeof Deno !== 'undefined' || typeof fetch !== 'undefined' || typeof WebSocket !== 'undefined' || typeof WebAssembly !== 'undefined' || typeof setTimeout !== 'undefined') throw new Error('ambient authority');
export function visualize(input) {
  const value = input;
  if (value.request.headers.length || value.request.body != null) throw new Error('undeclared input');
  let denied = false;
  try { (()=>0).constructor('return 1')(); } catch { denied = true; }
  if (!denied) throw new Error('string generation available');
  return {kind:'document',document:{title,status:'complete',presentation:'tree',roots:[]}};
}
";
    let runner = start(snapshot(source)).unwrap();
    let step = runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap();
    let VisualizationStep::Document { document } = step else {
        panic!("document required");
    };
    assert_eq!(document.title, "Structured view");
    runner.stop();
    assert!(runner.is_stopped());
}
#[tokio::test]
async fn byte_continuations_are_authorized_and_their_results_are_host_validated() {
    let source = r"
export function visualize(input) {return {kind:'needInput',token:'body',reads:[{sourceId:'response-body',offset:'0',maxBytes:4}]};}
export function resume(token,pages) { return {kind:'document',document:{title:'Bytes',status:'complete',presentation:'tree',roots:[{id:'sum',name:'Sum',value:{kind:'integer',value:String(pages[0].bytes.reduce((a,b)=>a+b,0))},spans:[{sourceId:'response-body',offset:'0',length:'4'}]}]}}; }
";
    let runner = start(snapshot(source)).unwrap();
    assert!(matches!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap(),
        VisualizationStep::NeedInput { .. }
    ));
    let step = runner
        .invoke(Invocation::Resume {
            token: "body".into(),
            pages: vec![BytePage {
                source_id: "response-body".into(),
                offset: "0".into(),
                bytes: vec![1, 2, 3, 4],
            }],
        })
        .await
        .unwrap();
    let VisualizationStep::Document { document } = step else {
        panic!("document required");
    };
    assert_eq!(document.roots[0].value, Some(Value::Integer("10".into())));
}
#[tokio::test]
async fn undeclared_source_read_and_forged_document_source_are_rejected() {
    let source = "export function visualize() {return {kind:'needInput',token:'bad',reads:[{sourceId:'forbidden-request',offset:'0',maxBytes:1}]};}";
    let runner = start(snapshot(source)).unwrap();
    assert_eq!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap_err()
            .kind,
        FailureKind::Validation
    );
    let source = "export function visualize(){return {kind:'document',document:{title:'Bad',status:'complete',presentation:'tree',roots:[{id:'bad',name:'Bad',spans:[{sourceId:'not-granted',offset:'0',length:'1'}]}]}};}";
    let runner = start(snapshot(source)).unwrap();
    assert_eq!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap_err()
            .kind,
        FailureKind::Validation
    );
}
#[test]
fn imports_cannot_resolve_network_filesystem_or_another_package() {
    for dependency in [
        "https://example.invalid/script.js",
        "file:///C:/outside.js",
        "../../outside.js",
        "node:fs",
    ] {
        let source = format!(
            "import {{value}} from '{dependency}'; export function visualize(){{return {DOCUMENT};}}"
        );
        let Err(error) = start(snapshot(&source)) else {
            panic!("external import accepted");
        };
        assert_eq!(error.kind, FailureKind::Validation);
    }
    let source = "export function visualize(){return import('https://example.invalid/code');}";
    let Err(error) = start(snapshot(source)) else {
        panic!("dynamic import accepted");
    };
    assert_eq!(error.kind, FailureKind::Validation);
}

#[test]
fn typescript_and_non_javascript_dependencies_are_not_transpiled() {
    let typed = format!("export function visualize(input: unknown){{return {DOCUMENT};}}");
    assert_eq!(
        start(snapshot(&typed)).err().unwrap().kind,
        FailureKind::Validation
    );
    let source = format!(
        "import {{title}} from './author.ts'; export function visualize(){{return {DOCUMENT};}}"
    );
    let mut candidate = snapshot(&source);
    let bytes = b"export const title = 'This is inert authoring source';";
    candidate.manifest.files.push(Payload {
        path: "src/author.ts".into(),
        bytes: bytes.len() as u64,
        sha256: None,
    });
    candidate
        .files
        .insert("src/author.ts".into(), Arc::<[u8]>::from(bytes.as_slice()));
    let candidate = Snapshot::new(candidate.manifest, candidate.files).unwrap();
    assert_eq!(
        start(candidate).err().unwrap().kind,
        FailureKind::Validation
    );
}
#[tokio::test]
async fn infinite_loop_times_out_and_unload_is_independent_between_instances() {
    let source = "export function visualize(){while(true){}}";
    let runner = start(snapshot(source)).unwrap();
    assert_eq!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap_err()
            .kind,
        FailureKind::Timeout
    );
    assert!(runner.is_stopped());
    let good = format!("export function visualize(){{return {DOCUMENT};}}");
    let first = start(snapshot(&good)).unwrap();
    let second = start(snapshot(&good)).unwrap();
    first.stop();
    assert!(
        first
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .is_err()
    );
    assert!(
        second
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .is_ok()
    );
}
#[test]
fn helper_refuses_an_unsandboxed_launch() {
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_transmog-extension-host"))
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(72));
}

#[test]
fn stopping_an_idle_worker_releases_slots_even_while_its_ui_handle_is_retained() {
    let source = format!("export function visualize(){{return {DOCUMENT};}}");
    let mut retained = Vec::new();
    for _ in 0..20 {
        let runner = start(snapshot(&source)).unwrap();
        runner.stop();
        retained.push(runner);
    }
    assert!(retained.iter().all(|r| r.is_stopped()));
}

#[tokio::test]
async fn forged_continuation_tokens_cannot_supply_unsolicited_body_pages() {
    let source = "export function visualize(){return {kind:'needInput',token:'right-token',reads:[{sourceId:'response-body',offset:'0',maxBytes:4}]};} export function resume(){throw new Error('must not run');}";
    let runner = start(snapshot(source)).unwrap();
    runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap();
    let error = runner
        .invoke(Invocation::Resume {
            token: "wrong-token".into(),
            pages: vec![BytePage {
                source_id: "response-body".into(),
                offset: "0".into(),
                bytes: vec![1, 2, 3, 4],
            }],
        })
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::Protocol);
    assert!(runner.is_stopped());
}

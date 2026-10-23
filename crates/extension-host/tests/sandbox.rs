//! Production `AppContainer` coverage for capability-free extension execution and lifecycle.
#![cfg(windows)]
mod support;
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

fn snapshot_with_modules(source: &str, modules: &[(&str, &str)]) -> Snapshot {
    let mut candidate = snapshot(source);
    for &(path, source) in modules {
        candidate.manifest.files.push(Payload {
            path: path.into(),
            bytes: source.len() as u64,
            sha256: None,
        });
        candidate
            .files
            .insert(path.into(), Arc::<[u8]>::from(source.as_bytes()));
    }
    Snapshot::new(candidate.manifest, candidate.files).unwrap()
}
fn start(snapshot: Snapshot) -> Result<Arc<Runner>, Failure> {
    Runner::start(
        &HostConfig {
            executable: support::host_executable().into(),
        },
        snapshot,
        "view",
    )
}
fn input() -> VisualizationInput {
    VisualizationInput {
        instance_id: "instance-one".into(),
        exchange_id: "exchange-one".into(),
        request_number: "1".into(),
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
async fn live_batches_use_frozen_projected_inputs_and_bounded_source_continuations() {
    let source = r"
function render() {return {kind:'document',document:{title:'Snapshot',status:'complete',presentation:'tree',roots:[]}};}
function traffic(events,info) {
  if (!Object.isFrozen(events) || !Object.isFrozen(events[0].input) || !Object.isFrozen(info) || info.missedHints !== '3') throw new Error('unbounded live input');
  const event=events[0];
  if(event.exchangeId !== event.input.exchangeId || event.phase !== 'bodyCompleted' || event.input.request.headers.length || event.input.request.body) throw new Error('invalid live projection');
  return {kind:'needInput',token:'live-bytes',reads:[{sourceId:event.input.response.body.sourceId,offset:'0',maxBytes:4}]};
}
function finish(token,pages) {return {kind:'document',document:{title:'Live bytes',status:'complete',presentation:'tree',roots:[{id:'live',name:'Live value',value:{kind:'integer',value:String(pages[0].bytes[0])},spans:[{sourceId:pages[0].sourceId,offset:'0',length:'4'}]}]}};}
// Private names must not shadow the public aliases used for dispatch.
function visualize() {throw new Error('private visualize');}
function onTraffic() {throw new Error('private onTraffic');}
function resume() {throw new Error('private resume');}
export {render as visualize, traffic as onTraffic, finish as resume};
";
    let original = snapshot(source);
    let mut manifest = original.manifest.clone();
    manifest.visualizers[0].live_updates = Some(LiveUpdates {
        events: vec![Direction::Response],
        body_progress: false,
    });
    let runner = start(Snapshot::new(manifest, original.files).unwrap()).unwrap();
    runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap();
    let mut update = input();
    update.exchange_id = "exchange-two".into();
    update.request_number = "2".into();
    update.generation = "2".into();
    update
        .response
        .as_mut()
        .unwrap()
        .body
        .as_mut()
        .unwrap()
        .source_id = "live-body".into();
    let event = TrafficEvent {
        sequence: "1".into(),
        exchange_id: "exchange-two".into(),
        direction: Direction::Response,
        phase: TrafficPhase::BodyCompleted,
        revision: "2".into(),
        input: update,
    };
    assert!(matches!(
        runner
            .invoke(Invocation::Traffic {
                events: vec![event],
                missed_hints: "3".into()
            })
            .await
            .unwrap(),
        VisualizationStep::NeedInput { .. }
    ));
    let output = runner
        .invoke(Invocation::Resume {
            token: "live-bytes".into(),
            pages: vec![BytePage {
                source_id: "live-body".into(),
                offset: "0".into(),
                bytes: vec![7, 8, 9, 10],
            }],
        })
        .await
        .unwrap();
    let VisualizationStep::Document { document } = output else {
        panic!("document expected")
    };
    assert_eq!(document.roots[0].value, Some(Value::Integer("7".into())));
    runner.stop();
}

#[tokio::test]
async fn declared_context_continuations_receive_only_projected_inputs_and_scoped_sources() {
    let source = r"
export function visualize() {return {kind:'needContext',token:'history',request:{position:'before',cursor:null,limit:1}};}
export function resumeContext(token,page) {
  if (token !== 'history' || page.anchorOrder !== '2' || page.exchanges.length !== 1 || !Object.isFrozen(page.exchanges[0].input)) throw new Error('bad context');
  const input = page.exchanges[0].input;
  if (input.request.headers.length || input.request.body != null) throw new Error('undeclared related input');
  return {kind:'needInput',token:'related-bytes',reads:[{sourceId:input.response.body.sourceId,offset:'0',maxBytes:4}]};
}
export function resume(token,pages) { return {kind:'document',document:{title:'Related bytes',status:'complete',presentation:'tree',roots:[{id:'value',name:'Value',value:{kind:'integer',value:String(pages[0].bytes[0])},spans:[{sourceId:pages[0].sourceId,offset:'0',length:'4'}]}]}}; }
";
    let original = snapshot(source);
    let mut manifest = original.manifest.clone();
    manifest.visualizers[0].context = Some(Context {
        headers: vec![HeaderPredicate {
            direction: Direction::Request,
            name: "x-kind".into(),
            test: HeaderTest::Present,
        }],
        max_exchanges: 2,
        look_ahead: false,
    });
    let runner = start(Snapshot::new(manifest, original.files).unwrap()).unwrap();
    assert!(matches!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap(),
        VisualizationStep::NeedContext { .. }
    ));
    let mut related = input();
    related
        .response
        .as_mut()
        .unwrap()
        .body
        .as_mut()
        .unwrap()
        .source_id = "related-body".into();
    assert!(matches!(
        runner
            .invoke(Invocation::ResumeContext {
                token: "history".into(),
                page: ContextPage {
                    anchor_order: "2".into(),
                    exchanges: vec![ContextExchange {
                        order: "1".into(),
                        input: related
                    }],
                    next_cursor: None,
                    status: ContextStatus::Complete
                }
            })
            .await
            .unwrap(),
        VisualizationStep::NeedInput { .. }
    ));
    let step = runner
        .invoke(Invocation::Resume {
            token: "related-bytes".into(),
            pages: vec![BytePage {
                source_id: "related-body".into(),
                offset: "0".into(),
                bytes: vec![7, 8, 9, 10],
            }],
        })
        .await
        .unwrap();
    let VisualizationStep::Document { document } = step else {
        panic!("document required")
    };
    assert_eq!(document.roots[0].value, Some(Value::Integer("7".into())));
    runner.stop();
    assert_eq!(
        start(snapshot(source))
            .unwrap()
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap_err()
            .kind,
        FailureKind::Validation
    );
}

#[tokio::test]
async fn relative_javascript_modules_execute_after_capability_restrictions_and_projection() {
    let source = r"
import {title} from './helper.mjs';
if (typeof Deno !== 'undefined' || typeof fetch !== 'undefined' || typeof WebSocket !== 'undefined' || typeof WebAssembly !== 'undefined' || typeof setTimeout !== 'undefined') throw new Error('ambient authority');
export {visualize};
function visualize(input) {
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
async fn cyclic_static_graphs_preserve_module_identity_live_bindings_and_evaluation_order() {
    let shared = r"
globalThis.order = ['shared'];
export const identity = {};
export let counter = 0;
export function next() { return ++counter; }
";
    let left = r"
import {identity, next} from './shared.mjs';
import {rightName} from './right.mjs';
globalThis.order.push('left');
export {identity, next};
export function leftName() { return 'left'; }
export function readRight() { return rightName(); }
";
    let right = r"
import {leftName} from './left.mjs';
import {identity} from './shared.mjs';
globalThis.order.push('right');
export {identity};
export function rightName() { return leftName() + '-right'; }
";
    let source = r"
import * as left from './left.mjs';
import * as right from './right.mjs';
import {identity, counter} from './nested/../shared.mjs';
globalThis.order.push('main');
export function visualize() {
  if (identity !== left.identity || identity !== right.identity) throw new Error('module instantiated twice');
  const next = left.next();
  if (counter !== next) throw new Error('export lost its live binding');
  return {kind:'document',document:{title:globalThis.order.join(',') + ':' + counter + ':' + left.readRight(),status:'complete',presentation:'tree',roots:[]}};
}
";
    let runner = start(snapshot_with_modules(
        source,
        &[
            ("src/shared.mjs", shared),
            ("src/left.mjs", left),
            ("src/right.mjs", right),
        ],
    ))
    .unwrap();
    for counter in 1..=2 {
        let VisualizationStep::Document { document } = runner
            .invoke(Invocation::Visualize {
                input: Box::new(input()),
            })
            .await
            .unwrap()
        else {
            panic!("document required");
        };
        assert_eq!(
            document.title,
            format!("shared,right,left,main:{counter}:left-right")
        );
    }
}

#[tokio::test]
async fn dependency_initializers_cannot_generate_code_and_can_await_resolved_promises() {
    let dependency = r"
for (const name of ['Function', 'eval', 'fetch', 'Deno', 'process', 'require', 'WebAssembly', 'setTimeout', 'queueMicrotask']) {
  if (globalThis[name] !== undefined) throw new Error('ambient authority before entry');
}
for (const constructor of [
  (() => 0).constructor,
  (async () => 0).constructor,
  (function* () {}).constructor,
  (async function* () {}).constructor,
]) {
  let denied = false;
  try { constructor('return 1'); } catch { denied = true; }
  if (!denied) throw new Error('dependency string generation available');
}
export const initialized = await Promise.resolve('ready');
";
    let source = r"
import {initialized} from './restricted.mjs';
export function visualize() {
  return {kind:'document',document:{title:initialized,status:'complete',presentation:'tree',roots:[]}};
}
";
    let runner = start(snapshot_with_modules(
        source,
        &[("src/restricted.mjs", dependency)],
    ))
    .unwrap();
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap()
    else {
        panic!("document required");
    };
    assert_eq!(document.title, "ready");
}

#[test]
fn rejected_top_level_await_fails_extension_initialization() {
    let source = format!(
        "await Promise.reject(new Error('initialization rejected')); \
         export function visualize(){{return {DOCUMENT};}}"
    );
    assert_eq!(
        start(snapshot(&source))
            .err()
            .expect("a rejected module must not become ready")
            .kind,
        FailureKind::Exception
    );
}

#[test]
fn unsettled_top_level_await_never_becomes_ready_and_remains_cancelable() {
    let source =
        format!("await new Promise(() => {{}}); export function visualize(){{return {DOCUMENT};}}");
    let started = std::time::Instant::now();
    let result = Runner::start_cancellable(
        &HostConfig {
            executable: support::host_executable().into(),
        },
        snapshot(&source),
        "view",
        || started.elapsed() >= std::time::Duration::from_millis(150),
    );
    assert!(result.is_err(), "an unsettled module must not become ready");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    // A failed or canceled module must leave the next activation usable.
    let runner = start(snapshot(&format!(
        "export function visualize(){{return {DOCUMENT};}}"
    )))
    .unwrap();
    runner.stop();
}

#[tokio::test]
async fn queued_microtasks_run_before_the_next_extension_invocation() {
    let source = r"
let calls = 0;
let completed = 0;
export function visualize() {
  if (completed !== calls) throw new Error('queued microtask was not processed');
  calls++;
  Promise.resolve().then(() => completed++);
  return {kind:'document',document:{title:calls + ':' + completed,status:'complete',presentation:'tree',roots:[]}};
}
";
    let runner = start(snapshot(source)).unwrap();
    for calls in 1..=3 {
        let VisualizationStep::Document { document } = runner
            .invoke(Invocation::Visualize {
                input: Box::new(input()),
            })
            .await
            .unwrap()
        else {
            panic!("document required");
        };
        assert_eq!(document.title, format!("{calls}:{}", calls - 1));
    }
}

#[test]
fn unhandled_promise_rejections_fail_extension_initialization() {
    let source = format!(
        "Promise.reject(new Error('unhandled initialization')); \
         export function visualize(){{return {DOCUMENT};}}"
    );
    assert_eq!(
        start(snapshot(&source))
            .err()
            .expect("an unhandled rejection must not become ready")
            .kind,
        FailureKind::Exception
    );
}

#[tokio::test]
async fn handled_rejections_are_allowed_and_unhandled_rejections_surface_before_the_next_call() {
    let source = r"
let calls = 0;
let handled = 0;
Promise.reject(new Error('handled initialization')).catch(() => handled++);
export function visualize() {
  calls++;
  if (calls === 1) Promise.reject(new Error('handled callback')).catch(() => handled++);
  if (calls === 2) Promise.reject(new Error('unhandled callback'));
  return {kind:'document',document:{title:calls + ':' + handled,status:'complete',presentation:'tree',roots:[]}};
}
";
    let runner = start(snapshot(source)).unwrap();
    for expected in ["1:1", "2:2"] {
        let VisualizationStep::Document { document } = runner
            .invoke(Invocation::Visualize {
                input: Box::new(input()),
            })
            .await
            .unwrap()
        else {
            panic!("document required");
        };
        assert_eq!(document.title, expected);
    }
    assert_eq!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input()),
            })
            .await
            .unwrap_err()
            .kind,
        FailureKind::Exception
    );
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap()
    else {
        panic!("document required");
    };
    assert_eq!(document.title, "3:2");
}

#[tokio::test]
async fn promises_and_thenables_are_rejected_without_poisoning_the_extension() {
    for result in [
        format!("Promise.resolve({DOCUMENT})"),
        "{then() {}}".to_owned(),
    ] {
        let source = format!(
            "let calls = 0; export function visualize(){{ \
             if (++calls === 1) return {result}; return {DOCUMENT}; }}"
        );
        let runner = start(snapshot(&source)).unwrap();
        assert_eq!(
            runner
                .invoke(Invocation::Visualize {
                    input: Box::new(input()),
                })
                .await
                .unwrap_err()
                .kind,
            FailureKind::Exception
        );
        assert!(matches!(
            runner
                .invoke(Invocation::Visualize {
                    input: Box::new(input()),
                })
                .await
                .unwrap(),
            VisualizationStep::Document { .. }
        ));
    }
}

#[tokio::test]
async fn large_modules_and_repeated_allocations_keep_the_engine_available() {
    let source = format!(
        "const payload = '{}'; let calls = 0; \
         export function visualize(){{ \
           const scratch = new Array(400_000).fill(payload.length); \
           if(scratch[399_999] !== 262_144) throw new Error('corrupt allocation'); \
           return {{kind:'document',document:{{title:String(++calls),status:'complete',presentation:'tree',roots:[]}}}}; \
         }}",
        "x".repeat(262_144)
    );
    let runner = start(snapshot(&source)).unwrap();
    for calls in 1..=24 {
        let VisualizationStep::Document { document } = runner
            .invoke(Invocation::Visualize {
                input: Box::new(input()),
            })
            .await
            .unwrap()
        else {
            panic!("document required");
        };
        assert_eq!(document.title, calls.to_string());
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[test]
fn commonjs_and_non_synchronous_named_handlers_are_rejected() {
    for source in [
        "module.exports = {visualize() {}};",
        "exports.visualize = function() {};",
        "function visualize() {}",
        "async function render() {} export {render as visualize};",
        "function* render() {} export {render as visualize};",
        "const render = () => ({}); export {render as visualize};",
        "function visualize() {} async function next() {} export {visualize, next as resume};",
        "export function visualize() {} function* next() {} export {next as expand};",
        "export function visualize() {} async function next() {} export {next as onTraffic};",
        "export {title as visualize} from './helper.mjs';",
    ] {
        assert_eq!(
            start(snapshot(source)).err().unwrap().kind,
            FailureKind::Validation,
            "accepted unsupported handlers: {source}"
        );
    }
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
    for args in [
        vec!["--extension"],
        vec!["--extension", "--sandboxed"],
        vec!["--extension", "--sandboxed", "--development"],
    ] {
        let result = std::process::Command::new(support::host_executable())
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(72), "arguments: {args:?}");
    }
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

#[test]
fn canceling_initialization_terminates_top_level_code_without_waiting_for_startup_deadline() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let canceled = Arc::new(AtomicBool::new(false));
    let task_canceled = Arc::clone(&canceled);
    let source = format!("while(true){{}} export function visualize(){{return {DOCUMENT};}}");
    let started = std::time::Instant::now();
    let worker = std::thread::spawn(move || {
        Runner::start_cancellable(
            &HostConfig {
                executable: support::host_executable().into(),
            },
            snapshot(&source),
            "view",
            || task_canceled.load(Ordering::Acquire),
        )
    });
    std::thread::sleep(std::time::Duration::from_millis(100));
    canceled.store(true, Ordering::Release);
    assert!(worker.join().unwrap().is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
fn cancellation_closes_an_already_ready_workers_idle_queue() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = AtomicUsize::new(0);
    let started = std::time::Instant::now();
    let result = Runner::start_cancellable(
        &HostConfig {
            executable: support::host_executable().into(),
        },
        snapshot(&format!(
            "export function visualize(){{return {DOCUMENT};}}"
        )),
        "view",
        || {
            if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                return false;
            }
            // Give the initialized helper time to wait on its work queue before
            // cancellation reaches the supervisor's readiness check.
            std::thread::sleep(std::time::Duration::from_millis(500));
            true
        },
    );
    assert!(result.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

const HTTP_SOURCE: &str = r"
let decodedId;
export function visualize(input) { return {kind:'needDecoded',sourceId:input.response.body.sourceId,token:'decode'}; }
export function resumeDecoded(token,result) {
  if(token!=='decode'||result.kind!=='ready'||!Object.isFrozen(result)||!Object.isFrozen(result.decoded.body))throw new Error('Invalid decode metadata');
  decodedId=result.decoded.body.sourceId;
  return {kind:'needInput',token:'decoded-bytes',reads:[{sourceId:decodedId,offset:'0',maxBytes:3}]};
}
export function resume(token,pages) {
  if(token!=='decoded-bytes'||!Object.isFrozen(pages[0].bytes))throw new Error('Invalid decoded page');
  return {kind:'document',document:{title:'HTTP decoded',status:'complete',presentation:'tree',roots:[{id:'text',name:'Decoded text',value:{kind:'string',value:String.fromCharCode(...pages[0].bytes)},spans:[{sourceId:decodedId,offset:'0',length:'3'}]}]}};
}
";
#[tokio::test]
async fn http_decoding_resumes_frozen_metadata_and_grants_only_requested_decoded_pages() {
    let denied = start(snapshot(HTTP_SOURCE)).unwrap();
    assert_eq!(
        denied
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap_err()
            .kind,
        FailureKind::Validation
    );
    denied.stop();
    let snapshot = snapshot(HTTP_SOURCE);
    let mut manifest = snapshot.manifest;
    let access = &mut manifest.visualizers[0].inputs.response;
    access.headers.push("content-encoding".into());
    access.decode_http_content = true;
    let runner = start(Snapshot::new(manifest, snapshot.files).unwrap()).unwrap();
    let step = runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap();
    assert!(
        matches!(step,VisualizationStep::NeedDecoded { source_id,.. } if source_id=="response-body")
    );
    let decoded = DecodedBody {
        body: BodyReference {
            source_id: "decoded-native".into(),
            length: "3".into(),
            availability: "complete".into(),
        },
        parent: Span {
            source_id: "response-body".into(),
            offset: "0".into(),
            length: "4".into(),
        },
        codings: vec!["gzip".into()],
    };
    let step = runner
        .invoke(Invocation::ResumeDecoded {
            token: "decode".into(),
            result: HttpDecodeResult::Ready { decoded },
        })
        .await
        .unwrap();
    assert!(matches!(step, VisualizationStep::NeedInput { .. }));
    let step = runner
        .invoke(Invocation::Resume {
            token: "decoded-bytes".into(),
            pages: vec![BytePage {
                source_id: "decoded-native".into(),
                offset: "0".into(),
                bytes: b"ABC".to_vec(),
            }],
        })
        .await
        .unwrap();
    let VisualizationStep::Document { document } = step else {
        panic!("document required");
    };
    assert_eq!(document.roots[0].value, Some(Value::String("ABC".into())));
}

//! Fixed same-machine sandbox round-trip benchmark; no captured data is accepted.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Instant,
};
use transmog_script::{
    ScriptBodyFilter, ScriptCapabilities, ScriptContext, ScriptHandler, ScriptInvocation,
    ScriptLimits, ScriptManifest, ScriptPrefilter, ScriptRequest, ScriptRunner, compile_typescript,
    source_hash,
};
use transmog_script_supervisor::{IsolationPolicy, ScriptHostConfig, SupervisedScriptRunner};
fn main() {
    let executable = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("matching host executable"),
    );
    let attached = std::env::args().any(|arg| arg == "--attached");
    let source = "import {pass} from 'transmog:api';export function onRequestHead(){return pass()}";
    let invocation = ScriptInvocation {
        parameters: BTreeMap::new(),
        state: BTreeMap::new(),
        local_response: false,
        failure: None,
        context: ScriptContext {
            exchange_id: "benchmark".into(),
            handler: ScriptHandler::RequestHead,
            ..ScriptContext::default()
        },
        request: ScriptRequest {
            method: "GET".into(),
            scheme: "https".into(),
            host: "example.test".into(),
            port: 443,
            path: "/".into(),
            query: None,
            headers: vec![],
        },
        response: None,
        body: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let manifest = ScriptManifest {
                parameters: BTreeMap::new(),
                request_body_filter: ScriptBodyFilter::default(),
                response_body_filter: ScriptBodyFilter::default(),
                id: "roundtrip-benchmark".into(),
                revision: 1,
                api_revision: transmog_script::SCRIPT_API_REVISION,
                source_hash: source_hash(source.as_bytes()),
                handlers: BTreeSet::from([ScriptHandler::RequestHead]),
                prefilter: ScriptPrefilter::default(),
                capabilities: ScriptCapabilities::default(),
                limits: ScriptLimits::default(),
                priority: 0,
            };
            let compiled = compile_typescript(manifest, source).unwrap();
            let config = ScriptHostConfig {
                executable: executable.clone(),
                isolation: IsolationPolicy::RequireSandbox,
            };
            // BENCHMARK_ATTACHMENT_BEGIN: omitted only in the archived pre-feature baseline.
            let session = attached.then(|| {
                transmog_script_supervisor::DebugSession::new(
                    transmog_script::debugger::DebugOptions {
                        session: "a".repeat(32),
                        exceptions: transmog_script::debugger::ExceptionPolicy::None,
                        breakpoints: vec![],
                    },
                )
                .unwrap()
            });
            let runner =
                SupervisedScriptRunner::start_debug(&config, compiled, session.clone()).unwrap();
            // BENCHMARK_ATTACHMENT_END
            for _ in 0..1000 {
                runner.invoke(invocation.clone()).await.unwrap();
            }
            let mut batches = Vec::new();
            let mut timings = Vec::new();
            for _ in 0..11 {
                let batch = Instant::now();
                for _ in 0..1000 {
                    let start = Instant::now();
                    runner.invoke(invocation.clone()).await.unwrap();
                    timings.push(start.elapsed().as_nanos());
                }
                batches.push(batch.elapsed().as_nanos() / 1000);
            }
            timings.sort_unstable();
            batches.sort_unstable();
            samples.push(
                serde_json::json!({"medianNs":batches[5],"p95Ns":timings[timings.len()*95/100]}),
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"attached":attached,"samples":samples})
            )
            .unwrap()
        );
    });
}

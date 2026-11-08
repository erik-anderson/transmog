#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Single-script V8 host with no application, filesystem, or network ops.

use std::io::{Read, Write};

use std::collections::BTreeMap;
use transmog_js_runtime::debugger::{DebugAttachment, HostInput, Input, Routed};
use transmog_js_runtime::{ErrorKind, Runtime};
use transmog_script::{
    SCRIPT_HOST_PROTOCOL_VERSION, ScriptFailure, ScriptFailureCategory, ScriptHostReply,
    ScriptHostRequest, ScriptInvocation, ScriptManifest, ScriptResult, write_host_reply_frame,
};
use url::Url;

const BOOTSTRAP: &str = r#"
const __transmogDeepFreeze = (value, seen = new WeakSet()) => {
  if (value === null || (typeof value !== "object" && typeof value !== "function") || seen.has(value)) return value;
  seen.add(value);
  for (const key of Reflect.ownKeys(value)) __transmogDeepFreeze(value[key], seen);
  return Object.freeze(value);
};
globalThis.__transmogInvoke = (name, inputJson) => {
  const handler = globalThis.__transmogHandlers[name];
  if (typeof handler !== "function") throw new Error("declared handler is unavailable");
  const input = JSON.parse(inputJson);
  if (name !== "onResponseChunk") {
    delete input.end;
    delete input.canEditHeaders;
  }
  __transmogDeepFreeze(input);
  const result = handler(input);
  if (result !== null && (typeof result === "object" || typeof result === "function") && typeof result.then === "function") {
    throw new Error("handlers must return synchronously");
  }
  const output = JSON.stringify(result);
  if (typeof output !== "string") throw new Error("handler result is not JSON serializable");
  return output;
};
delete globalThis.Deno;
delete globalThis.fetch;
delete globalThis.WebSocket;
delete globalThis.Worker;
delete globalThis.SharedWorker;
delete globalThis.XMLHttpRequest;
delete globalThis.WebAssembly;
delete globalThis.eval;
delete globalThis.Function;
"#;

/// Runs the framed host protocol until shutdown or input EOF.
///
/// # Errors
/// Returns only process-level framing or output failures. Script failures are
/// sent as protocol values and do not expose script inputs.
pub fn run(
    input: impl Read + Send + 'static,
    mut output: impl Write + Send + 'static,
) -> Result<(), String> {
    let writer = transmog_script::stream::Output::new(move |reply: &ScriptHostReply| {
        write_host_reply_frame(&mut output, reply).is_ok()
    });
    let result = run_stream(input, &writer.sender());
    if !writer.finish() {
        return Err("script output transport failed".into());
    }
    result
}

fn send(
    output: &transmog_script::stream::OutputSender<ScriptHostReply>,
    reply: ScriptHostReply,
) -> Result<(), String> {
    output
        .control(reply)
        .then_some(())
        .ok_or_else(|| "script output transport failed".into())
}

fn run_stream(
    input: impl Read + Send + 'static,
    output: &transmog_script::stream::OutputSender<ScriptHostReply>,
) -> Result<(), String> {
    let mut input = HostInput::new(input, |input| {
        transmog_script::read_host_request_frame(input).map_err(|_| "Invalid host input".into())
    });
    let Input::Message(first) = input.read_input()? else {
        return Err("Initialization required".into());
    };
    let (protocol_version, token, manifest, javascript, debug) = match first {
        ScriptHostRequest::Initialize {
            protocol_version,
            token,
            manifest,
            javascript,
            debug,
        } => (protocol_version, token, Some(manifest), javascript, debug),
        ScriptHostRequest::InitializeScratch {
            protocol_version,
            token,
            debug,
        } => (protocol_version, token, None, String::new(), Some(debug)),
        _ => return Err("script host must be initialized first".to_owned()),
    };
    if protocol_version != SCRIPT_HOST_PROTOCOL_VERSION {
        return Err("script host protocol version is unsupported".to_owned());
    }
    let attachment = if debug.is_some() {
        input.enable(debug, debug_sink(output, token), move |request| {
            route_debug(request, token)
        })?
    } else {
        None
    };
    let logs = output.clone();
    let sink = move |request_id, event| {
        let control = matches!(event, transmog_script::ConsoleEvent::End { .. });
        let reply = ScriptHostReply::Console {
            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
            token,
            request_id,
            event,
        };
        if control {
            logs.event(reply)
        } else {
            logs.log(reply)
        }
    };
    let initialized = if let Some(manifest) = manifest {
        ScriptRuntime::new_debug(&manifest, javascript, sink, attachment)
    } else {
        ScriptRuntime::scratch(sink, attachment)
    };
    let mut runtime = match initialized {
        Ok(runtime) => runtime,
        Err(failure) => {
            let reply = ScriptHostReply::Fatal {
                protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
                token: Some(token),
                failure,
            };
            send(output, reply)?;
            return Ok(());
        }
    };
    send(
        output,
        ScriptHostReply::Ready {
            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
            token,
        },
    )?;

    loop {
        let request = match input.read_input() {
            Ok(Input::Message(request)) => request,
            Ok(Input::Wake) => {
                runtime.runtime.poll_debugger();
                if runtime.runtime.debugger_stopped() {
                    return Ok(());
                }
                continue;
            }
            Ok(Input::Attach(attachment)) => {
                attach(&mut runtime.runtime, attachment)?;
                continue;
            }
            Err(_) => return Ok(()),
        };
        match request {
            ScriptHostRequest::AttachDebugger {
                protocol_version,
                token: supplied,
                request_id: _,
                options,
            } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
                let attachment = input
                    .enable(Some(options), debug_sink(output, token), move |request| {
                        route_debug(request, token)
                    })?
                    .ok_or("Debugger attachment missing")?;
                attach(&mut runtime.runtime, attachment)?;
            }
            ScriptHostRequest::Invoke {
                protocol_version,
                token: supplied,
                request_id,
                invocation,
            } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
                runtime.runtime.begin_console(request_id);
                runtime.runtime.begin_debug_operation(request_id);
                let result = runtime.invoke(invocation);
                runtime.runtime.finish_debug_operation();
                runtime.runtime.finish_console();
                send(
                    output,
                    ScriptHostReply::Result {
                        protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
                        token,
                        request_id,
                        result,
                    },
                )?;
            }
            ScriptHostRequest::Shutdown {
                protocol_version,
                token: supplied,
            } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
                return Ok(());
            }
            _ => return Err("script host authentication or message order failed".to_owned()),
        }
    }
}

fn debug_sink(
    output: &transmog_script::stream::OutputSender<ScriptHostReply>,
    token: transmog_script::ScriptHostToken,
) -> impl Fn(transmog_script::debugger::DebugEvent) -> bool + Send + Sync + 'static {
    let output = output.clone();
    move |event| {
        output.event(ScriptHostReply::Debugger {
            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
            token,
            event,
        })
    }
}
fn route_debug(
    request: ScriptHostRequest,
    token: transmog_script::ScriptHostToken,
) -> Result<Routed<ScriptHostRequest>, String> {
    Ok(match request {
        ScriptHostRequest::AttachDebugger {
            protocol_version,
            token: supplied,
            request_id,
            options,
        } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
            Routed::Attach {
                options,
                request: request_id,
            }
        }
        ScriptHostRequest::ControlDebugger {
            protocol_version,
            token: supplied,
            command,
        } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
            Routed::Control(command)
        }
        value @ ScriptHostRequest::Invoke {
            protocol_version,
            token: supplied,
            ..
        } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
            Routed::Message(value)
        }
        value @ ScriptHostRequest::Shutdown {
            protocol_version,
            token: supplied,
        } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
            Routed::Last(value)
        }
        _ => return Err("Invalid debug input authentication or order".into()),
    })
}
fn attach(runtime: &mut Runtime, attachment: DebugAttachment) -> Result<(), String> {
    let (options, transport, _) = attachment.into_parts();
    runtime
        .enable_debugger(options, transport)
        .map_err(|_| "Debugger unavailable".into())
}

struct ScriptRuntime {
    runtime: Runtime,
    manifest: Option<ScriptManifest>,
}

impl ScriptRuntime {
    fn scratch(
        sink: impl Fn(u64, transmog_script::ConsoleEvent) -> bool + 'static,
        attachment: Option<DebugAttachment>,
    ) -> Result<Self, ScriptFailure> {
        let mut runtime = Runtime::new(1024 * 1024, 64 * 1024 * 1024)
            .map_err(|error| failure(ScriptFailureCategory::Unavailable, &error.to_string()))?;
        runtime
            .enable_console(sink)
            .map_err(|error| failure(ScriptFailureCategory::Unavailable, &error.to_string()))?;
        if let Some(attachment) = attachment {
            attach(&mut runtime, attachment)
                .map_err(|_| failure(ScriptFailureCategory::Unavailable, "Scratch unavailable"))?;
        }
        Ok(Self {
            runtime,
            manifest: None,
        })
    }
    #[cfg(test)]
    fn new(
        manifest: &ScriptManifest,
        javascript: String,
        sink: impl Fn(u64, transmog_script::ConsoleEvent) -> bool + 'static,
    ) -> Result<Self, ScriptFailure> {
        Self::new_debug(manifest, javascript, sink, None)
    }
    fn new_debug(
        manifest: &ScriptManifest,
        javascript: String,
        sink: impl Fn(u64, transmog_script::ConsoleEvent) -> bool + 'static,
        attachment: Option<DebugAttachment>,
    ) -> Result<Self, ScriptFailure> {
        let mut runtime = Runtime::new(
            (manifest.limits.max_heap_bytes / 8).max(1024 * 1024),
            manifest.limits.max_heap_bytes,
        )
        .map_err(|error| failure(ScriptFailureCategory::Unavailable, &error.to_string()))?;
        runtime
            .enable_console(sink)
            .map_err(|error| failure(ScriptFailureCategory::Unavailable, &error.to_string()))?;
        runtime.begin_console(0);
        if let Some(attachment) = attachment {
            attach(&mut runtime, attachment)
                .map_err(|_| failure(ScriptFailureCategory::Unavailable, "Debugger unavailable"))?;
        }
        runtime.begin_debug_operation(0);
        let initialized = (|| -> Result<(), ScriptFailure> {
            // Restrictions apply before any user top-level code executes.
            runtime
                .execute_script("transmog:script-bootstrap", BOOTSTRAP)
                .map_err(|error| failure(ScriptFailureCategory::Unavailable, &error.to_string()))?;
            let handlers = manifest
                .handlers
                .iter()
                .map(|handler| {
                    let name = handler.export_name();
                    format!("{name}: typeof {name} === 'function' ? {name} : undefined")
                })
                .collect::<Vec<_>>()
                .join(",");
            let module = format!(
                "{javascript}\nglobalThis.__transmogHandlers = Object.freeze({{{handlers}}});"
            );
            let specifier = Url::parse("file:///transmog-script/main.js")
                .expect("constant module specifier is valid");
            runtime
                .evaluate_modules(
                    &specifier,
                    BTreeMap::from([
                        (specifier.clone(), module),
                        (
                            Url::parse("transmog:api").expect("constant API URL"),
                            include_str!("api.mjs").to_owned(),
                        ),
                    ]),
                )
                .map_err(|error| engine_failure(ScriptFailureCategory::Exception, error))?;
            Ok(())
        })();
        runtime.finish_debug_operation();
        runtime.finish_console();
        initialized?;
        Ok(Self {
            runtime,
            manifest: Some(manifest.clone()),
        })
    }

    fn invoke(&mut self, mut invocation: ScriptInvocation) -> Result<ScriptResult, ScriptFailure> {
        let manifest = self.manifest.as_ref().ok_or_else(|| {
            failure(
                ScriptFailureCategory::Protocol,
                "Scratch has no traffic callbacks",
            )
        })?;
        if !manifest.handlers.contains(&invocation.context.handler) {
            return Err(failure(
                ScriptFailureCategory::Protocol,
                "invocation requested an undeclared handler",
            ));
        }
        let request = transmog_script::prepare_invocation(manifest, &mut invocation)?;
        let phase = invocation.context.handler;
        let handler = phase.export_name();
        let json = serde_json::to_string(&invocation)
            .map_err(|error| failure(ScriptFailureCategory::Protocol, &error.to_string()))?;
        let json_string = serde_json::to_string(&json)
            .map_err(|error| failure(ScriptFailureCategory::Protocol, &error.to_string()))?;
        let source = format!("globalThis.__transmogInvoke({handler:?}, {json_string})");
        let output = self
            .runtime
            .execute_string(
                "transmog:invoke",
                &source,
                manifest.limits.max_output_bytes * 4 + manifest.limits.max_log_bytes + 256 * 1024,
            )
            .map_err(|error| {
                let category = match error.kind {
                    ErrorKind::OutputLimit => ScriptFailureCategory::ResourceLimit,
                    ErrorKind::NotString => ScriptFailureCategory::InvalidAction,
                    _ => ScriptFailureCategory::Exception,
                };
                engine_failure(category, error)
            })?;
        let result = serde_json::from_str(&output)
            .map_err(|error| failure(ScriptFailureCategory::InvalidAction, &error.to_string()))?;
        transmog_script::validate_result(manifest, phase, &request, &result)?;
        transmog_script::validate_stream_result(manifest, &invocation, &result)?;
        Ok(result)
    }
}

fn failure(category: ScriptFailureCategory, message: &str) -> ScriptFailure {
    ScriptFailure {
        category,
        logs: Vec::new(),
        message: message.chars().take(512).collect(),
        line: None,
        column: None,
        exception: None,
    }
}

fn engine_failure(
    category: ScriptFailureCategory,
    error: transmog_js_runtime::Error,
) -> ScriptFailure {
    let mut failure = failure(category, &error.to_string());
    failure.exception = error.exception;
    if let Some(location) = failure.exception.as_ref().and_then(|diagnostic| {
        diagnostic
            .exception
            .frames
            .iter()
            .filter_map(|frame| frame.generated.as_ref())
            .find(|location| location.source == "file:///transmog-script/main.js")
    }) {
        failure.line = Some(location.line);
        failure.column = Some(location.column);
    }
    failure
}

#[cfg(test)]
mod location_tests {
    use super::*;

    #[test]
    fn authored_message_cannot_forge_a_location() {
        let failure = failure(
            ScriptFailureCategory::Exception,
            "Error: file:///transmog-script/main.js:17:9",
        );
        assert_eq!((failure.line, failure.column), (None, None));
    }
}

/// Applies process-global V8 flags before the first isolate is created.
pub fn configure_v8() {
    transmog_js_runtime::initialize();
}

/// Launches the actual host inside a zero-capability Windows AppContainer.
///
/// # Errors
/// Returns a redacted bootstrap failure. This function is available on every
/// platform so argument handling never silently falls back to no sandbox.
pub fn sandbox_bootstrap(max_heap_bytes: usize) -> Result<i32, String> {
    let process_memory_bytes = max_heap_bytes
        .saturating_mul(3)
        .saturating_add(128 * 1024 * 1024);
    transmog_process_sandbox::sandbox_bootstrap(
        transmog_process_sandbox::SandboxIdentity {
            profile_namespace: "Script",
            display_name: "Transmog isolated traffic script",
            description: "Ephemeral zero-capability Transmog script host",
        },
        process_memory_bytes,
    )
}

/// Verifies the current host has both AppContainer and Job Object isolation.
///
/// # Errors
/// Returns an error if a production host was launched without either boundary.
pub fn verify_sandbox() -> Result<(), String> {
    transmog_process_sandbox::verify_current_process()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use transmog_script::{
        ScriptCapabilities, ScriptContext, ScriptHandler, ScriptLimits, ScriptPrefilter,
        ScriptRequest,
    };

    use super::*;
    use transmog_script::ScriptAction;

    fn manifest() -> ScriptManifest {
        ScriptManifest {
            parameters: std::collections::BTreeMap::new(),
            request_body_filter: transmog_script::ScriptBodyFilter::default(),
            response_body_filter: transmog_script::ScriptBodyFilter::default(),
            id: "test".to_owned(),
            revision: 1,
            api_revision: transmog_script::SCRIPT_API_REVISION,
            source_hash: "0".repeat(64),
            handlers: BTreeSet::from([ScriptHandler::RequestHead]),
            prefilter: ScriptPrefilter::default(),
            capabilities: ScriptCapabilities {
                abort: true,
                ..ScriptCapabilities::default()
            },
            limits: ScriptLimits::default(),
            priority: 0,
        }
    }

    #[test]
    fn v8_executes_a_synchronous_typed_action_without_ambient_deno() {
        configure_v8();
        let source = r#"
          export function onRequestHead({ context, request }) {
            if (typeof Deno !== "undefined" || typeof fetch !== "undefined") throw new Error("ambient API");
            return { decision: { action: "abort", reason: `${request.host}:${context.handler}` } };
          }
        "#;
        let mut runtime = ScriptRuntime::new(&manifest(), source.to_owned(), |_, _| true).unwrap();
        let result = runtime
            .invoke(ScriptInvocation {
                bytes: None,
                end: false,
                can_edit_headers: false,
                parameters: std::collections::BTreeMap::new(),
                state: std::collections::BTreeMap::new(),
                local_response: false,
                failure: None,
                context: ScriptContext {
                    exchange_id: "1".to_owned(),
                    now_unix_ms: 7,
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
            })
            .unwrap();
        assert_eq!(
            result.decision,
            ScriptAction::Abort {
                reason: "example.test:onRequestHead".to_owned()
            }
        );
    }
}

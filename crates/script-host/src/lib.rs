#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Single-script V8 host with no application, filesystem, or network ops.

use std::io::{Read, Write};

use std::collections::BTreeMap;
use transmog_js_runtime::{ErrorKind, Runtime};
use transmog_script::{
    SCRIPT_HOST_PROTOCOL_VERSION, ScriptFailure, ScriptFailureCategory, ScriptHostReply,
    ScriptHostRequest, ScriptInvocation, ScriptManifest, ScriptResult, read_host_frame,
    write_host_frame,
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
  const input = __transmogDeepFreeze(JSON.parse(inputJson));
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
pub fn run(mut input: impl Read, mut output: impl Write) -> Result<(), String> {
    let first: ScriptHostRequest =
        read_host_frame(&mut input).map_err(|error| error.to_string())?;
    let ScriptHostRequest::Initialize {
        protocol_version,
        token,
        manifest,
        javascript,
    } = first
    else {
        return Err("script host must be initialized first".to_owned());
    };
    if protocol_version != SCRIPT_HOST_PROTOCOL_VERSION {
        return Err("script host protocol version is unsupported".to_owned());
    }
    let mut runtime = match ScriptRuntime::new(&manifest, javascript) {
        Ok(runtime) => runtime,
        Err(failure) => {
            let reply = ScriptHostReply::Fatal {
                protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
                token: Some(token),
                failure,
            };
            write_host_frame(&mut output, &reply).map_err(|error| error.to_string())?;
            return Ok(());
        }
    };
    write_host_frame(
        &mut output,
        &ScriptHostReply::Ready {
            protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
            token,
        },
    )
    .map_err(|error| error.to_string())?;

    loop {
        let request: ScriptHostRequest = match read_host_frame(&mut input) {
            Ok(request) => request,
            Err(error) if is_eof(&error) => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        match request {
            ScriptHostRequest::Invoke {
                protocol_version,
                token: supplied,
                request_id,
                invocation,
            } if protocol_version == SCRIPT_HOST_PROTOCOL_VERSION && supplied == token => {
                let result = runtime.invoke(invocation);
                write_host_frame(
                    &mut output,
                    &ScriptHostReply::Result {
                        protocol_version: SCRIPT_HOST_PROTOCOL_VERSION,
                        token,
                        request_id,
                        result,
                    },
                )
                .map_err(|error| error.to_string())?;
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

fn is_eof(error: &transmog_script::ScriptProtocolError) -> bool {
    matches!(error, transmog_script::ScriptProtocolError::Io(source) if source.kind() == std::io::ErrorKind::UnexpectedEof)
}

struct ScriptRuntime {
    runtime: Runtime,
    manifest: ScriptManifest,
}

impl ScriptRuntime {
    fn new(manifest: &ScriptManifest, javascript: String) -> Result<Self, ScriptFailure> {
        let mut runtime = Runtime::new(
            (manifest.limits.max_heap_bytes / 8).max(1024 * 1024),
            manifest.limits.max_heap_bytes,
        )
        .map_err(|error| failure(ScriptFailureCategory::Unavailable, &error.to_string()))?;
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
        let module =
            format!("{javascript}\nglobalThis.__transmogHandlers = Object.freeze({{{handlers}}});");
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
            .map_err(|error| failure(ScriptFailureCategory::Exception, &error.to_string()))?;
        Ok(Self {
            runtime,
            manifest: manifest.clone(),
        })
    }

    fn invoke(&mut self, mut invocation: ScriptInvocation) -> Result<ScriptResult, ScriptFailure> {
        if !self.manifest.handlers.contains(&invocation.context.handler) {
            return Err(failure(
                ScriptFailureCategory::Protocol,
                "invocation requested an undeclared handler",
            ));
        }
        let request = transmog_script::prepare_invocation(&self.manifest, &mut invocation)?;
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
                self.manifest.limits.max_output_bytes * 4
                    + self.manifest.limits.max_log_bytes
                    + 256 * 1024,
            )
            .map_err(|error| {
                let category = match error.kind {
                    ErrorKind::OutputLimit => ScriptFailureCategory::ResourceLimit,
                    ErrorKind::NotString => ScriptFailureCategory::InvalidAction,
                    _ => ScriptFailureCategory::Exception,
                };
                failure(category, &error.to_string())
            })?;
        let result = serde_json::from_str(&output)
            .map_err(|error| failure(ScriptFailureCategory::InvalidAction, &error.to_string()))?;
        transmog_script::validate_result(&self.manifest, phase, &request, &result)?;
        Ok(result)
    }
}

fn failure(category: ScriptFailureCategory, message: &str) -> ScriptFailure {
    let (line, column) = generated_location(message).unwrap_or((None, None));
    ScriptFailure {
        category,
        message: message.chars().take(512).collect(),
        line,
        column,
    }
}

fn generated_location(message: &str) -> Option<(Option<u32>, Option<u32>)> {
    let suffix = message.split("main.js:").nth(1)?;
    let mut parts = suffix.split(|character: char| !character.is_ascii_digit());
    let line = parts.next()?.parse::<u32>().ok()?;
    let column = parts.next()?.parse::<u32>().ok()?;
    Some((Some(line), Some(column)))
}

#[cfg(test)]
mod location_tests {
    use super::generated_location;

    #[test]
    fn extracts_v8_module_stack_location() {
        assert_eq!(
            generated_location(
                "Error: failed\n    at onRequestHead (file:///transmog-script/main.js:17:9)"
            ),
            Some((Some(17), Some(9)))
        );
        assert_eq!(generated_location("bounded failure"), None);
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
        let mut runtime = ScriptRuntime::new(&manifest(), source.to_owned()).unwrap();
        let result = runtime
            .invoke(ScriptInvocation {
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

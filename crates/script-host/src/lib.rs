#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Single-script V8 host with no application, filesystem, or network ops.

use std::io::{Read, Write};

use deno_core::{JsRuntime, ModuleSpecifier, RuntimeOptions, v8};
use transmog_script::{
    SCRIPT_HOST_PROTOCOL_VERSION, ScriptAction, ScriptFailure, ScriptFailureCategory,
    ScriptHostReply, ScriptHostRequest, ScriptInvocation, ScriptManifest, read_host_frame,
    write_host_frame,
};

#[cfg(windows)]
mod windows_sandbox;

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
  const result = handler(input.context, input.request, input.response, input.body);
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
    runtime: JsRuntime,
    manifest: ScriptManifest,
}

impl ScriptRuntime {
    fn new(manifest: &ScriptManifest, javascript: String) -> Result<Self, ScriptFailure> {
        let create_params = v8::CreateParams::default().heap_limits(
            (manifest.limits.max_heap_bytes / 8).max(1024 * 1024),
            manifest.limits.max_heap_bytes,
        );
        let mut runtime = JsRuntime::try_new(RuntimeOptions {
            create_params: Some(create_params),
            ..RuntimeOptions::default()
        })
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
            "{javascript}\nglobalThis.__transmogHandlers = Object.freeze({{{handlers}}});\n{BOOTSTRAP}"
        );
        let specifier = ModuleSpecifier::parse("file:///transmog-script/main.js")
            .expect("constant module specifier is valid");
        let local = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|error| failure(ScriptFailureCategory::Unavailable, &error.to_string()))?;
        local.block_on(async {
            let id = runtime
                .load_main_es_module_from_code(&specifier, module)
                .await
                .map_err(|error| failure(ScriptFailureCategory::Exception, &error.to_string()))?;
            let evaluation = runtime.mod_evaluate(id);
            runtime
                .run_event_loop(Default::default())
                .await
                .map_err(|error| failure(ScriptFailureCategory::Exception, &error.to_string()))?;
            evaluation
                .await
                .map_err(|error| failure(ScriptFailureCategory::Exception, &error.to_string()))
        })?;
        Ok(Self {
            runtime,
            manifest: manifest.clone(),
        })
    }

    fn invoke(&mut self, invocation: ScriptInvocation) -> Result<ScriptAction, ScriptFailure> {
        if !self.manifest.handlers.contains(&invocation.context.handler) {
            return Err(failure(
                ScriptFailureCategory::Protocol,
                "invocation requested an undeclared handler",
            ));
        }
        let handler = invocation.context.handler.export_name();
        let json = serde_json::to_string(&invocation)
            .map_err(|error| failure(ScriptFailureCategory::Protocol, &error.to_string()))?;
        let json_string = serde_json::to_string(&json)
            .map_err(|error| failure(ScriptFailureCategory::Protocol, &error.to_string()))?;
        let source = format!("globalThis.__transmogInvoke({handler:?}, {json_string})");
        let value = self
            .runtime
            .execute_script("transmog:invoke", source)
            .map_err(|error| failure(ScriptFailureCategory::Exception, &error.to_string()))?;
        deno_core::scope!(scope, &mut self.runtime);
        let value = v8::Local::new(scope, value);
        let output = value
            .to_string(scope)
            .ok_or_else(|| {
                failure(
                    ScriptFailureCategory::InvalidAction,
                    "handler result is not a string",
                )
            })?
            .to_rust_string_lossy(scope);
        if output.len() > self.manifest.limits.max_output_bytes {
            return Err(failure(
                ScriptFailureCategory::ResourceLimit,
                "handler output exceeded its declared byte limit",
            ));
        }
        serde_json::from_str(&output)
            .map_err(|error| failure(ScriptFailureCategory::InvalidAction, &error.to_string()))
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
    v8::V8::set_flags_from_string("--disallow-code-generation-from-strings");
}

/// Launches the actual host inside a zero-capability Windows AppContainer.
///
/// # Errors
/// Returns a redacted bootstrap failure. This function is available on every
/// platform so argument handling never silently falls back to no sandbox.
pub fn sandbox_bootstrap(max_heap_bytes: usize) -> Result<i32, String> {
    #[cfg(windows)]
    {
        windows_sandbox::bootstrap(max_heap_bytes)
    }
    #[cfg(not(windows))]
    {
        let _ = max_heap_bytes;
        Err("the script sandbox is supported only on Windows".to_owned())
    }
}

/// Verifies the current host has both AppContainer and Job Object isolation.
///
/// # Errors
/// Returns an error if a production host was launched without either boundary.
pub fn verify_sandbox() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_sandbox::verify_current_process()
    }
    #[cfg(not(windows))]
    {
        Err("the script sandbox is supported only on Windows".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use transmog_script::{
        ScriptCapabilities, ScriptContext, ScriptHandler, ScriptLimits, ScriptPrefilter,
        ScriptRequest,
    };

    use super::*;

    fn manifest() -> ScriptManifest {
        ScriptManifest {
            id: "test".to_owned(),
            revision: 1,
            api_revision: 1,
            source_hash: "0".repeat(64),
            handlers: BTreeSet::from([ScriptHandler::RequestHead]),
            prefilter: ScriptPrefilter::default(),
            capabilities: ScriptCapabilities::default(),
            limits: ScriptLimits::default(),
            priority: 0,
        }
    }

    #[test]
    fn v8_executes_a_synchronous_typed_action_without_ambient_deno() {
        configure_v8();
        let source = r#"
          export function onRequestHead(context, request) {
            if (typeof Deno !== "undefined" || typeof fetch !== "undefined") throw new Error("ambient API");
            return { action: "abort", reason: `${request.host}:${context.handler}` };
          }
        "#;
        let mut runtime = ScriptRuntime::new(&manifest(), source.to_owned()).unwrap();
        let result = runtime
            .invoke(ScriptInvocation {
                context: ScriptContext {
                    exchange_id: "1".to_owned(),
                    now_unix_ms: 7,
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
            })
            .unwrap();
        assert_eq!(
            result,
            ScriptAction::Abort {
                reason: "example.test:onRequestHead".to_owned()
            }
        );
    }
}

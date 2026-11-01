//! Isolated JavaScript modules and synchronous pure invocation, without transpilation.
mod runtime;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::Arc,
};
use transmog_extensions::{
    Failure, FailureKind, HOST_PROTOCOL_VERSION, HostReply, HostRequest, PAYLOAD_CHUNK_BYTES,
    Snapshot, decimal, write_reply,
};
use transmog_js_runtime::debugger::{HostInput, Input, Routed};

/// Disables alternate JavaScript string code-generation paths before creating isolates.
pub fn configure_v8() {
    transmog_js_runtime::initialize();
}

/// Creates the same zero-capability AppContainer/Job Object boundary for every extension.
/// # Errors
/// Returns a safe error if the native sandbox or its requested limit is unavailable.
pub fn sandbox_bootstrap(heap: usize) -> Result<i32, String> {
    bootstrap(heap, transmog_process_sandbox::SandboxMode::Default)
}

fn bootstrap(heap: usize, mode: transmog_process_sandbox::SandboxMode) -> Result<i32, String> {
    if !(16 * 1024 * 1024..=256 * 1024 * 1024).contains(&heap) {
        return Err("Invalid worker heap limit".into());
    }
    transmog_process_sandbox::sandbox_bootstrap_with_mode(
        transmog_process_sandbox::SandboxIdentity {
            profile_namespace: "Extension",
            display_name: "Transmog isolated visualizer",
            description: "Ephemeral zero-capability protocol visualizer",
        },
        heap.saturating_mul(3).saturating_add(128 * 1024 * 1024),
        mode,
    )
}

/// Runs the extension entry point of the shared JavaScript helper.
/// Returns a process exit code without logging source or captured input.
pub fn run_process(args: &[std::ffi::OsString]) -> i32 {
    if args.first().is_some_and(|arg| arg == "--sandbox-bootstrap") {
        let heap = args
            .get(1)
            .and_then(|value| value.to_string_lossy().parse::<usize>().ok())
            .unwrap_or(128 * 1024 * 1024);
        return bootstrap(heap, transmog_process_sandbox::SandboxMode::Extension).unwrap_or(71);
    }
    if args.len() != 1
        || args[0] != "--sandboxed"
        || transmog_process_sandbox::verify_current_process().is_err()
    {
        return 72;
    }
    configure_v8();
    if run(std::io::stdin(), std::io::stdout()).is_err() {
        return 70;
    }
    0
}

/// Serves framed IPC after the executable has verified containment.
/// # Errors
/// Returns framing/authentication/transport failures; never logs source or captured input.
pub fn run(
    input: impl Read + Send + 'static,
    mut output: impl Write + Send + 'static,
) -> Result<(), String> {
    let writer = transmog_extensions::stream::Output::new(move |reply: &HostReply| {
        write_reply(&mut output, reply).is_ok()
    });
    let result = run_stream(input, &writer.sender());
    if !writer.finish() {
        return Err("Worker output transport failed".into());
    }
    result
}

fn send(
    output: &transmog_extensions::stream::OutputSender<HostReply>,
    reply: HostReply,
) -> Result<(), String> {
    output
        .control(reply)
        .then_some(())
        .ok_or_else(|| "Worker output transport failed".into())
}
#[allow(clippy::too_many_lines)] // Authenticated setup and invocation framing remain sequential.
fn run_stream(
    input: impl Read + Send + 'static,
    output: &transmog_extensions::stream::OutputSender<HostReply>,
) -> Result<(), String> {
    transmog_process_sandbox::verify_current_process()
        .map_err(|_| "Worker containment is required")?;
    let mut input = HostInput::new(input, |input| {
        transmog_extensions::read_request(input).map_err(|_| "Invalid worker input".into())
    });
    let Input::Message(first) = input.read_input()? else {
        return Err("Initialization required".into());
    };
    let HostRequest::Initialize {
        protocol_version,
        token,
        manifest,
        visualizer_id,
        debug,
    } = first
    else {
        return Err("Initialization required".into());
    };
    if protocol_version != HOST_PROTOCOL_VERSION {
        return Err("Unsupported worker protocol".into());
    }
    let initialized = (|| -> Result<runtime::Runtime, Failure> {
        manifest
            .validate(false)
            .map_err(|_| Failure::new(FailureKind::Validation, "Invalid extension manifest"))?;
        let mut collected: BTreeMap<String, Vec<u8>> = manifest
            .files
            .iter()
            .map(|f| (f.path.clone(), Vec::new()))
            .collect();
        loop {
            let Input::Message(request) = input.read_input().map_err(|_| {
                Failure::new(FailureKind::Protocol, "Invalid source transfer frame")
            })?
            else {
                return Err(Failure::new(
                    FailureKind::Protocol,
                    "Invalid source transfer order",
                ));
            };
            match request {
                HostRequest::Payload {
                    token: supplied,
                    path,
                    offset,
                    bytes_base64,
                } if supplied == token => {
                    let declared =
                        manifest
                            .files
                            .iter()
                            .find(|f| f.path == path)
                            .ok_or_else(|| {
                                Failure::new(FailureKind::Protocol, "Undeclared source transfer")
                            })?;
                    let bytes = collected.get_mut(&path).ok_or_else(|| {
                        Failure::new(FailureKind::Protocol, "Invalid source path")
                    })?;
                    if decimal(&offset).ok() != Some(bytes.len() as u64)
                        || bytes_base64.len() > PAYLOAD_CHUNK_BYTES * 2
                    {
                        return Err(Failure::new(
                            FailureKind::Limit,
                            "Invalid source chunk extent",
                        ));
                    }
                    let next = STANDARD.decode(&bytes_base64).map_err(|_| {
                        Failure::new(FailureKind::Protocol, "Invalid source encoding")
                    })?;
                    if next.len() > PAYLOAD_CHUNK_BYTES
                        || bytes.len().saturating_add(next.len()) as u64 > declared.bytes
                    {
                        return Err(Failure::new(
                            FailureKind::Limit,
                            "Source transfer exceeds declaration",
                        ));
                    }
                    bytes.extend_from_slice(&next);
                }
                HostRequest::Start { token: supplied } if supplied == token => break,
                _ => {
                    return Err(Failure::new(
                        FailureKind::Protocol,
                        "Invalid initialization order or token",
                    ));
                }
            }
        }
        let files = collected
            .into_iter()
            .map(|(p, b)| (p, Arc::<[u8]>::from(b)))
            .collect();
        let snapshot = Snapshot::new(manifest.clone(), files).map_err(|_| {
            Failure::new(
                FailureKind::Validation,
                "Source revision verification failed",
            )
        })?;
        let logs = output.clone();
        let attachment = if debug.is_some() {
            input
                .enable(debug, debug_sink(output, token), move |request| {
                    route_debug(request, token)
                })
                .map_err(|_| Failure::new(FailureKind::Unavailable, "Debugger input unavailable"))?
        } else {
            None
        };
        runtime::Runtime::new(
            &snapshot,
            &visualizer_id,
            move |request_id, event| {
                let control = matches!(event, transmog_extensions::ConsoleEvent::End { .. });
                let reply = HostReply::Console {
                    token,
                    request_id,
                    event,
                };
                if control {
                    logs.event(reply)
                } else {
                    logs.log(reply)
                }
            },
            attachment,
        )
    })();
    let mut runtime = match initialized {
        Ok(value) => value,
        Err(failure) => {
            send(
                output,
                HostReply::Fatal {
                    token: Some(token),
                    failure,
                },
            )
            .map_err(|_| "Initialization reply failed")?;
            return Ok(());
        }
    };
    send(output, HostReply::Ready { token })?;
    let mut last = 0_u64;
    loop {
        let request = match input.read_input() {
            Ok(Input::Message(value)) => value,
            Ok(Input::Wake) => {
                runtime.engine.poll_debugger();
                if runtime.engine.debugger_stopped() {
                    return Ok(());
                }
                continue;
            }
            Ok(Input::Attach(attachment)) => {
                let (options, transport, _) = attachment.into_parts();
                runtime
                    .engine
                    .enable_debugger(options, transport)
                    .map_err(|_| "Debugger unavailable")?;
                continue;
            }
            Err(_) => return Ok(()),
        };
        if let HostRequest::AttachDebugger {
            protocol_version,
            token: supplied,
            request_id: _,
            options,
        } = request
        {
            if protocol_version != HOST_PROTOCOL_VERSION || supplied != token {
                return Err("Invalid debugger authentication".into());
            }
            let attachment = input
                .enable(Some(options), debug_sink(output, token), move |request| {
                    route_debug(request, token)
                })?
                .ok_or("Debugger attachment missing")?;
            let (options, transport, _) = attachment.into_parts();
            runtime
                .engine
                .enable_debugger(options, transport)
                .map_err(|_| "Debugger unavailable")?;
            continue;
        }
        let HostRequest::Invoke {
            token: supplied,
            request_id,
            invocation,
        } = request
        else {
            return Err("Invocation required".into());
        };
        if supplied != token || request_id <= last {
            return Err("Invalid invocation token or sequence".into());
        }
        last = request_id;
        runtime.begin_console(request_id);
        let result = runtime.invoke(&invocation);
        runtime.finish_console();
        send(
            output,
            HostReply::Result {
                token,
                request_id,
                result,
            },
        )
        .map_err(|_| "Result reply failed")?;
    }
}

fn debug_sink(
    output: &transmog_extensions::stream::OutputSender<HostReply>,
    token: transmog_extensions::HostToken,
) -> impl Fn(transmog_extensions::debugger::DebugEvent) -> bool + Send + Sync + 'static {
    let output = output.clone();
    move |event| output.event(HostReply::Debugger { token, event })
}
fn route_debug(
    request: HostRequest,
    token: transmog_extensions::HostToken,
) -> Result<Routed<HostRequest>, String> {
    Ok(match request {
        HostRequest::AttachDebugger {
            protocol_version,
            token: supplied,
            request_id,
            options,
        } if protocol_version == HOST_PROTOCOL_VERSION && supplied == token => Routed::Attach {
            options,
            request: request_id,
        },
        HostRequest::ControlDebugger {
            protocol_version,
            token: supplied,
            command,
        } if protocol_version == HOST_PROTOCOL_VERSION && supplied == token => {
            Routed::Control(command)
        }
        value @ HostRequest::Invoke {
            token: supplied, ..
        } if supplied == token => Routed::Message(value),
        _ => return Err("Invalid debugger authentication or order".into()),
    })
}

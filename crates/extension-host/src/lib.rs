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
    Snapshot, decimal, read_frame, write_frame,
};

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
    if run(std::io::stdin().lock(), std::io::stdout().lock()).is_err() {
        return 70;
    }
    0
}

/// Serves framed IPC after the executable has verified containment.
/// # Errors
/// Returns framing/authentication/transport failures; never logs source or captured input.
#[allow(clippy::too_many_lines)] // Linear authenticated protocol setup avoids partial initialization states.
pub fn run(mut input: impl Read, mut output: impl Write) -> Result<(), String> {
    transmog_process_sandbox::verify_current_process()
        .map_err(|_| "Worker containment is required")?;
    let first =
        read_frame::<HostRequest>(&mut input).map_err(|_| "Invalid initialization frame")?;
    let HostRequest::Initialize {
        protocol_version,
        token,
        manifest,
        visualizer_id,
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
            match read_frame::<HostRequest>(&mut input)
                .map_err(|_| Failure::new(FailureKind::Protocol, "Invalid source transfer frame"))?
            {
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
        runtime::Runtime::new(&snapshot, &visualizer_id)
    })();
    let mut runtime = match initialized {
        Ok(value) => value,
        Err(failure) => {
            write_frame(
                &mut output,
                &HostReply::Fatal {
                    token: Some(token),
                    failure,
                },
            )
            .map_err(|_| "Initialization reply failed")?;
            return Ok(());
        }
    };
    write_frame(&mut output, &HostReply::Ready { token }).map_err(|_| "Ready reply failed")?;
    let mut last = 0_u64;
    loop {
        let request = match read_frame::<HostRequest>(&mut input) {
            Ok(value) => value,
            Err(transmog_extensions::Error::Io(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }
            Err(_) => return Err("Invalid invocation frame".into()),
        };
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
        let result = runtime.invoke(&invocation);
        write_frame(
            &mut output,
            &HostReply::Result {
                token,
                request_id,
                result,
            },
        )
        .map_err(|_| "Result reply failed")?;
    }
}

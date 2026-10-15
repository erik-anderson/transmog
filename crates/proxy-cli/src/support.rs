//! Guided support capture, with durable cleanup identities and streaming compression.
use crate::roots::{RootLedger, RootTrust, SystemRootTrust, state_directory};
use std::{
    env,
    error::Error,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use transmog_capture::{CaptureLimits, CapturePolicy};
use transmog_core::intercept::NoopInterceptorFactory;
use transmog_runtime::{ListenerConfig, ProxyConfig, ProxyServer};
use transmog_session::{
    ApplicationSessionService, CaptureStart, CaptureStatus, ServiceConfig, ServiceStatus,
};

#[allow(clippy::too_many_lines)] // Keep cleanup paired with every setup/record result.
pub(crate) async fn record(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let profile = crate::option(arguments, "--profile")
        .map(|p| crate::support_profile::Loaded::read(Path::new(p)))
        .transpose()?;
    let effective = profile
        .as_ref()
        .map_or_else(|| arguments.to_vec(), |p| p.arguments(arguments));
    validate_options(&effective)?;
    let original = arguments;
    let arguments = effective.as_slice();
    let output = output_path(arguments)?;
    // Probe the parent before trust/routing setup; publishing still uses create-new semantics.
    let prepared = crate::output::PendingOutput::new(&output)?;
    let circular_output = if crate::option(arguments, "--circular-buffer").is_some() {
        Some(prepared)
    } else {
        drop(prepared);
        None
    };
    if let Some(profile) = &profile {
        profile.preflight();
    }
    let encoding = transmog_capture::CaptureEncoding {
        password: crate::passwords::output(arguments)?,
    };
    let circular_recording = circular_output.is_some();
    let ledger = RootLedger::open(state_directory()?)?;
    let trust = SystemRootTrust;
    let (redact, request_body_limit) = recording_preferences(original, &ledger, profile.as_ref())?;
    let support_context = profile
        .as_ref()
        .map(|p| p.context(original, redact, request_body_limit));
    if profile.is_some() {
        println!(
            "Resolved settings and origins: {}",
            support_context.as_ref().unwrap()["resolvedSettings"]
        );
    }
    println!(
        "Preflight output: {} · encryption {} · network context {} · duration {}",
        output.display(),
        if encoding.password.is_some() {
            "on"
        } else {
            "off"
        },
        arguments.iter().any(|a| a == "--include-network-context"),
        crate::option(arguments, "--duration").unwrap_or("manual stop")
    );
    #[cfg(windows)]
    recover_proxy(&ledger)?;
    if ledger.cleanup_ephemeral(&trust)? > 0 {
        eprintln!(
            "Previous roots still need removal. Their identities are preserved; this capture will use a new root."
        );
    }
    let persistent = arguments.iter().any(|arg| arg == "--persistent-root");
    let (ca, root) = ledger.prepare(persistent)?;
    println!("Capture file: {}", output.display());
    println!(
        "Root mode: {}",
        if persistent {
            "persistent; reused on future --persistent-root runs"
        } else {
            "ephemeral; its private key stays in memory and its trust is removed afterward"
        }
    );
    let result = run_capture(
        arguments,
        &output,
        &ledger,
        &root,
        ca,
        RecordingOptions {
            redact,
            request_body_limit,
            encoding,
            circular_output,
            profile,
            support_context,
        },
    )
    .await;
    if let Err(error) = ledger.finish_run(&root, result.is_ok()) {
        eprintln!(
            "CLI lifecycle metadata could not be updated: {error}. Continuing certificate cleanup."
        );
    }
    // Always attempt removal after setup/start/record errors too. Metadata stays
    // on disk until OS removal succeeds; ephemeral private keys never do.
    let cleanup = if persistent {
        println!(
            "Persistent root retained. Reuse it with --persistent-root; remove it with transmog-cli roots cleanup --include-persistent."
        );
        Ok(())
    } else {
        println!("Removing this capture's trusted root. Approve the OS removal prompt if shown.");
        ledger.cleanup(&root, &trust)
    };
    if let Err(error) = &cleanup {
        eprintln!(
            "Root cleanup is pending: {error}. The next CLI run will retry; ownership metadata is kept at {}.",
            ledger.directory().display()
        );
    }
    if ledger.cleanup_pending(&trust, Some(&root.sha256))? > 0 {
        eprintln!(
            "Previous or rotated roots still need cleanup. Run transmog-cli roots cleanup; public recovery records are retained."
        );
    }
    if result.is_err() && !circular_recording && output.is_file() {
        eprintln!(
            "Capture did not finish cleanly. The native evidence remains at {} for recovery.",
            output.display()
        );
    }
    result?;
    println!("Trace saved: {}", output.display());
    println!("Open it in Transmog to review it, then share this file with your support contact.");
    cleanup?;
    println!("Certificate cleanup: completed (or explicitly retained persistent trust).");
    Ok(())
}

#[cfg(windows)]
fn recover_proxy(ledger: &RootLedger) -> Result<(), Box<dyn Error>> {
    let host = transmog_host_windows::WindowsProxyIntegration::system(
        ledger.directory().join("proxy-recovery.json"),
    );
    if host.recover_pending()? {
        println!("Restored Windows proxy settings from an interrupted CLI capture.");
    }
    Ok(())
}

fn recording_preferences(
    arguments: &[String],
    ledger: &RootLedger,
    profile: Option<&crate::support_profile::Loaded>,
) -> Result<(bool, Option<u64>), Box<dyn Error>> {
    let body_choice = if arguments
        .iter()
        .any(|arg| arg == "--unlimited-request-bodies")
    {
        Some(None)
    } else {
        crate::option(arguments, "--request-body-limit")
            .map(|value| value.parse::<u64>().map(Some))
            .transpose()?
    };
    let (mut redact, mut request_body_limit) = ledger.preferences(
        if arguments.iter().any(|arg| arg == "--redact") {
            Some(true)
        } else if arguments.iter().any(|arg| arg == "--retain-sensitive") {
            Some(false)
        } else {
            None
        },
        body_choice,
    )?;
    if let Some(profile) = profile {
        if !arguments
            .iter()
            .any(|a| matches!(a.as_str(), "--redact" | "--retain-sensitive"))
            && let Some(value) = profile.profile.recording.redact_sensitive_headers
        {
            redact = value;
        }
        if !arguments.iter().any(|a| {
            matches!(
                a.as_str(),
                "--request-body-limit" | "--unlimited-request-bodies"
            )
        }) && let Some(value) = &profile.profile.recording.request_body_limit
        {
            request_body_limit = match value {
                transmog_capture::support::BodyLimit::Limited { bytes } => Some(*bytes),
                transmog_capture::support::BodyLimit::Unlimited => None,
            };
        }
    }
    println!(
        "Request body retention: {}. Trace files have no size ceiling. A request retention limit keeps byte counts and marks bodies incomplete.",
        request_body_limit.map_or_else(
            || "Unlimited per request".into(),
            |limit| format!("{limit} bytes per request")
        )
    );
    Ok((redact, request_body_limit))
}

async fn original_trace_context(arguments: &[String]) -> serde_json::Value {
    let network = if arguments
        .iter()
        .any(|arg| arg == "--include-network-context")
    {
        println!("Collecting network configuration for the trace…");
        let mut network = transmog_network::context::collect().await;
        network.collector = format!("Transmog CLI {}", env!("CARGO_PKG_VERSION"));
        Some(network)
    } else {
        None
    };
    serde_json::json!({"application":"Transmog CLI", "version":env!("CARGO_PKG_VERSION"), "networkContext":network})
}

fn capture_policy(redact: bool, request_body_limit: Option<u64>) -> CapturePolicy {
    let mut policy = if redact {
        CapturePolicy::default()
    } else {
        CapturePolicy::default().retain_sensitive_headers()
    };
    policy.retain_body_samples = true;
    policy.request_body_limit = request_body_limit;
    policy
}

async fn capture_metadata(
    arguments: &[String],
    ledger: &RootLedger,
    root: &crate::roots::RootRecord,
    request_body_limit: Option<u64>,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let mut metadata = original_trace_context(arguments).await;
    metadata["requestBodyLimit"] = serde_json::json!(request_body_limit);
    metadata["certificateContext"] = serde_json::to_value(
        ledger
            .records()?
            .into_iter()
            .find(|record| record.sha256 == root.sha256)
            .ok_or_else(|| io::Error::other("CLI root recovery metadata is unavailable"))?,
    )?;
    Ok(metadata)
}

struct RecordingOptions {
    redact: bool,
    request_body_limit: Option<u64>,
    encoding: transmog_capture::CaptureEncoding,
    circular_output: Option<crate::output::PendingOutput>,
    profile: Option<crate::support_profile::Loaded>,
    support_context: Option<serde_json::Value>,
}

#[allow(clippy::too_many_lines)] // One owned recorder lifecycle and outcome report.
async fn run_capture(
    arguments: &[String],
    native: &Path,
    ledger: &RootLedger,
    root: &crate::roots::RootRecord,
    ca: transmog_tls::ProxyCa,
    options: RecordingOptions,
) -> Result<(), Box<dyn Error>> {
    let RecordingOptions {
        redact,
        request_body_limit,
        encoding,
        circular_output,
        profile,
        support_context,
    } = options;
    setup_root(arguments, ledger, root)?;
    println!(
        "Captured headers: {}. Captured bodies may contain private data; review the trace before sharing.",
        if redact {
            "Authorization, Proxy-Authorization, Cookie and Set-Cookie values are redacted"
        } else {
            "complete, including credentials and cookies"
        }
    );
    // The CLI has no traffic viewer. Capture observers retain all evidence independently;
    // its internal catalog needs only active entries and the most recent completed one.
    let service = ApplicationSessionService::new(ServiceConfig {
        sessions: transmog_session::SessionLimits {
            max_sessions: std::num::NonZeroUsize::new(1),
            ..Default::default()
        },
        ..ServiceConfig::default()
    })?;
    service.set_redact_sensitive_headers(redact);
    let config = ProxyConfig {
        listener: ListenerConfig {
            listen_addr: crate::option(arguments, "--listen")
                .unwrap_or("127.0.0.1:0")
                .parse()?,
            allow_remote_clients: arguments.iter().any(|arg| arg == "--allow-remote"),
        },
        route_policy: crate::parse_route(crate::option(arguments, "--route").unwrap_or("auto"))?,
        limits: transmog_runtime::RuntimeLimits {
            max_request_body_bytes: usize::MAX,
            ..transmog_runtime::RuntimeLimits::default()
        },
        ..ProxyConfig::default()
    };
    let policy = capture_policy(redact, request_body_limit);
    let mut metadata = capture_metadata(arguments, ledger, root, request_body_limit).await?;
    if let Some(context) = support_context {
        metadata["supportCapture"] = context;
    }
    let run_id = transmog_capture::support::new_id()?;
    metadata["captureRunId"] = run_id.clone().into();
    let health = Arc::new(crate::support_recording::Health::new(
        profile.as_ref().map(|p| p.profile.clone()),
    ));
    let circular = crate::option(arguments, "--circular-buffer")
        .map(|value| {
            crate::circular::CircularObserver::new(
                value,
                ledger.directory().join("circular"),
                encoding.clone(),
                policy.clone(),
                redact,
            )
            .map(Arc::new)
        })
        .transpose()?;
    if let Some(circular) = &circular {
        circular.metadata(metadata.clone())?;
    }
    let (mut components, _) = crate::build_components(
        &config,
        ca,
        Arc::new(NoopInterceptorFactory),
        false,
        None,
        false,
    )
    .await?;
    components =
        components.with_observer(health.clone(), crate::support_recording::health_config());
    if let Some(circular) = &circular {
        components = components.with_observer(circular.clone(), circular.config());
    }
    let proxy = ProxyServer::bind_with_components(
        config,
        crate::load_upstream_trust(arguments)?,
        service.prepare_components(components),
    )
    .await?;
    if circular.is_none() {
        service
            .start_capture(CaptureStart {
                encoding,
                metadata: Some(metadata),
                path: native.to_path_buf(),
                limits: CaptureLimits::default(),
                policy,
            })
            .await?;
    }
    let endpoint = start_owned_proxy(arguments, ledger, &service, proxy).await?;
    println!("Proxy address: {endpoint}");
    #[cfg(not(windows))]
    println!(
        "Set your application's HTTP and HTTPS proxy to this address. Remove that setting after recording."
    );
    println!("Recording. Reproduce the issue, then press Ctrl+C once to stop and save the trace.");
    println!(
        "Recording started: UTC {} ms since the Unix epoch. Health updates every 10 seconds.",
        transmog_capture::support::unix_millis()
    );
    let result = finish_capture(
        &service,
        circular.as_deref(),
        circular_output,
        native,
        health.as_ref(),
        run_id,
        crate::option(arguments, "--duration").and_then(|v| v.parse::<u64>().ok()),
    )
    .await;
    health.print_summary(circular.as_ref().map_or(0, |c| c.evicted()));
    let sealed = matches!(service.capture().status(), CaptureStatus::Sealed(_))
        || circular.as_ref().is_some_and(|c| c.published());
    println!("File sealed: {sealed}. Destination: {}", native.display());
    println!(
        "Routing restoration: {}. Recording stop: {}.",
        if service.has_pending_host_restore() {
            "pending; see recovery details"
        } else {
            "complete (or no owned change)"
        },
        if result.is_ok() {
            "completed"
        } else {
            "failed; see recovery details"
        }
    );
    result
}

async fn finish_capture(
    service: &ApplicationSessionService,
    circular: Option<&crate::circular::CircularObserver>,
    circular_output: Option<crate::output::PendingOutput>,
    native: &Path,
    health: &crate::support_recording::Health,
    run_id: String,
    duration: Option<u64>,
) -> Result<(), Box<dyn Error>> {
    let running = wait_for_stop(service, circular, health, run_id, duration).await;
    // Ensure failures still flush the recoverable prefix and restore owned host state.
    let stopped = service.stop().await;
    if let Some(circular) = circular {
        // Flush the available prefix even after a runtime/observer failure.
        let saved = circular.save(
            circular_output
                .ok_or_else(|| io::Error::other("Circular destination was not prepared"))?,
        );
        if (running.is_err() || stopped.is_err()) && saved.is_ok() {
            eprintln!(
                "Recording did not finish cleanly. Retained circular evidence was saved at {}",
                native.display()
            );
        }
        // A save error includes the recovery path and must not be obscured by
        // an earlier observer/runtime error.
        saved?;
        running?;
        stopped?;
        return Ok(());
    }
    running?;
    stopped?;
    verify_sealed(service, native)
}

fn verify_sealed(service: &ApplicationSessionService, native: &Path) -> Result<(), Box<dyn Error>> {
    match service.capture().status() {
        CaptureStatus::Sealed(_) => {}
        CaptureStatus::Failed(failure) => {
            return Err(io::Error::other(format!(
                "Capture failed: {}. A recoverable native prefix remains at {}",
                failure.message,
                native.display()
            ))
            .into());
        }
        _ => return Err(io::Error::other("The capture did not seal successfully").into()),
    }
    Ok::<(), Box<dyn Error>>(())
}

async fn start_owned_proxy(
    arguments: &[String],
    ledger: &RootLedger,
    service: &ApplicationSessionService,
    proxy: ProxyServer,
) -> Result<std::net::SocketAddr, Box<dyn Error>> {
    #[cfg(not(windows))]
    let _ = (arguments, ledger);
    let endpoint;
    #[cfg(windows)]
    {
        if arguments.iter().any(|arg| arg == "--no-system-proxy") {
            endpoint = service.start(proxy).await?;
        } else {
            let host = Arc::new(transmog_host_windows::WindowsProxyIntegration::system(
                ledger.directory().join("proxy-recovery.json"),
            ));
            endpoint = service
                .start_with_host(
                    proxy,
                    transmog_session::HostIntegrationPlan { integration: host },
                )
                .await?;
            println!("Windows proxy settings are active. They will be restored when you stop.");
        }
    }
    #[cfg(not(windows))]
    {
        endpoint = service.start(proxy).await?;
    }

    Ok(endpoint)
}

fn setup_root(
    arguments: &[String],
    ledger: &RootLedger,
    root: &crate::roots::RootRecord,
) -> io::Result<()> {
    let trust = SystemRootTrust;
    let install = if arguments.iter().any(|arg| arg == "--no-install-root") {
        false
    } else if arguments.iter().any(|arg| arg == "--install-root") {
        true
    } else {
        confirm(
            "Install this capture's public root certificate to inspect HTTPS? Approve any OS consent prompt",
            true,
        )?
    };
    if install {
        ledger.mark(
            root,
            crate::root_lifecycle::RootLifecycle::InstallationRequested,
        )?;
        loop {
            match trust.install(&ledger.certificate(root), &root.sha256) {
                Ok(()) => {
                    ledger.mark(root, crate::root_lifecycle::RootLifecycle::Installed)?;
                    println!("HTTPS certificate setup completed. No relaunch is needed.");
                    break;
                }
                Err(error) => {
                    eprintln!("Certificate setup failed: {error}");
                    if arguments.iter().any(|arg| arg == "--install-root")
                        || !confirm("Try certificate installation again", false)?
                    {
                        return Err(io::Error::other(
                            "HTTPS certificate setup was not completed",
                        ));
                    }
                }
            }
        }
    } else {
        ledger.mark(root, crate::root_lifecycle::RootLifecycle::Manual)?;
        println!(
            "HTTPS clients must trust this public root manually: {}",
            ledger.certificate(root).display()
        );
        println!("Root SHA-256: {}", root.sha256);
    }
    Ok(())
}

async fn wait_for_stop(
    service: &ApplicationSessionService,
    circular: Option<&crate::circular::CircularObserver>,
    health: &crate::support_recording::Health,
    run_id: String,
    duration: Option<u64>,
) -> io::Result<()> {
    use transmog_capture::support::{IncidentMarker, MAX_MARKERS, MarkerOrigin};
    let mut input = crate::support_recording::MarkerInput::start();
    let began = std::time::Instant::now();
    let mut marker_count = 0usize;
    let mut stopping = false;
    let mut pending: Option<
        std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<IncidentMarker>> + '_>>,
    > = None;
    let mut last_health = std::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        if stopping && service.status() == ServiceStatus::Stopped && pending.is_none() {
            println!("Incident markers recorded: {marker_count}");
            return Ok(());
        }
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal?;
                input.stop();
                if stopping {
                    service.stop_now().await.map_err(io::Error::other)?;
                } else {
                    println!("Stopping capture: restoring proxy settings, then finishing active requests. Press Ctrl+C again to stop waiting (unfinished requests will be incomplete).");
                    stopping = true;
                    service.begin_drain().await.map_err(io::Error::other)?;
                }
            }
            result = async { pending.as_mut().unwrap().await }, if pending.is_some() => {
                pending = None;
                match result {
                    Ok(marker) => { marker_count += 1; println!("Issue marker {marker_count} recorded: {} (UTC {} ms).", marker.id, marker.unix_millis); },
                    Err(error) => eprintln!("Issue marker was not recorded: {error}"),
                }
            }
            _ = tick.tick() => {
                check_failure(service)?;
                if let Some(circular) = circular { circular.check()?; }
                if last_health.elapsed() >= Duration::from_secs(10) {
                    health.print_health(began.elapsed()); last_health = std::time::Instant::now();
                }
                if !stopping {
                    if let Some(line) = input.poll() {
                        if let Some(note) = crate::support_recording::marker_note(&line) {
                            if pending.is_some() { eprintln!("Previous marker is still being saved; this marker was not recorded. Retry after acknowledgement."); }
                            else if marker_count >= MAX_MARKERS { eprintln!("Incident marker limit reached (256); this marker was not recorded."); }
                            else {
                                let run_id = run_id.clone();
                                let offset = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
                                pending = Some(Box::pin(async move {
                                    if let Some(circular) = circular {
                                        let marker = IncidentMarker::new(run_id, Some(offset), MarkerOrigin::Cli, note)?;
                                        circular.mark(&marker).await?;
                                        Ok(marker)
                                    } else { service.capture().mark(note, MarkerOrigin::Cli).await.map_err(io::Error::other) }
                                }));
                            }
                        } else { eprintln!("Use m then Enter, or m followed by a short incident note."); }
                    }
                    if duration.is_some_and(|seconds| began.elapsed() >= Duration::from_secs(seconds)) {
                        println!("Requested duration reached. Stopping capture: restoring proxy settings, then finishing active requests. Press Ctrl+C again to stop waiting.");
                        input.stop(); stopping = true;
                        service.begin_drain().await.map_err(io::Error::other)?;
                    }
                }
            }
        }
    }
}

fn check_failure(service: &ApplicationSessionService) -> io::Result<()> {
    if let ServiceStatus::Failed { message, .. } = service.status() {
        return Err(io::Error::other(message));
    }
    if let CaptureStatus::Failed(failure) = service.capture().status() {
        return Err(io::Error::other(failure.message));
    }
    Ok(())
}
fn confirm(question: &str, default: bool) -> io::Result<bool> {
    if !io::stdin().is_terminal() {
        return Err(crate::invalid_input(
            "Interactive setup needs a console. Use --install-root to authorize installation or --no-install-root for manual setup",
        ));
    }
    loop {
        print!("{question} [{}] ", if default { "Y/n" } else { "y/N" });
        io::stdout().flush()?;
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer)? == 0 {
            return Err(io::Error::other("Console input closed during setup"));
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Enter yes or no."),
        }
    }
}
fn output_path(arguments: &[String]) -> io::Result<PathBuf> {
    let path = crate::option(arguments, "--output").map_or_else(
        || {
            PathBuf::from(format!(
                "Transmog-{}.tmcap",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        },
        PathBuf::from,
    );
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tmcap"))
    {
        return Err(crate::invalid_input(
            "--output must end in .tmcap (chunk-compressed)",
        ));
    }
    #[cfg(windows)]
    {
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| crate::invalid_input("--output requires a valid Windows filename"))?;
        let device = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
            || ["COM", "LPT"].iter().any(|prefix| {
                device.strip_prefix(prefix).is_some_and(|suffix| {
                    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&suffix)
                })
            });
        if reserved
            || name.encode_utf16().count() > 255
            || name.ends_with([' ', '.'])
            || name
                .chars()
                .any(|ch| ch.is_control() || "<>:\"|?*".contains(ch))
        {
            return Err(crate::invalid_input(
                "--output requires an ordinary Windows filename, without reserved devices, streams or invalid characters",
            ));
        }
    }
    Ok(if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    })
}
#[allow(clippy::too_many_lines)] // Validate all recorder flags before any setup.
fn validate_options(arguments: &[String]) -> io::Result<()> {
    crate::arguments::validate(
        arguments,
        &[
            "--output",
            "--profile",
            "--duration",
            "--client-version",
            "--expected",
            "--actual",
            "--listen",
            "--route",
            "--upstream-ca-cert",
            "--request-body-limit",
            "--password-file",
            "--circular-buffer",
        ],
        &[
            "--encrypt",
            "--persistent-root",
            "--install-root",
            "--no-install-root",
            "--no-system-proxy",
            "--redact",
            "--retain-sensitive",
            "--allow-remote",
            "--include-network-context",
            "--unlimited-request-bodies",
        ],
    )?;
    if arguments.iter().any(|arg| arg == "--install-root")
        && arguments.iter().any(|arg| arg == "--no-install-root")
    {
        return Err(crate::invalid_input(
            "Choose only one root installation option",
        ));
    }
    if arguments.iter().any(|arg| arg == "--redact")
        && arguments.iter().any(|arg| arg == "--retain-sensitive")
    {
        return Err(crate::invalid_input(
            "Choose only one header redaction option",
        ));
    }
    if arguments
        .iter()
        .any(|arg| arg == "--unlimited-request-bodies")
        && crate::option(arguments, "--request-body-limit").is_some()
    {
        return Err(crate::invalid_input(
            "Choose only one request body retention limit",
        ));
    }
    if let Some(value) = crate::option(arguments, "--request-body-limit")
        && value.parse::<u64>().map_or(true, |limit| limit == 0)
    {
        return Err(crate::invalid_input(
            "--request-body-limit requires a positive byte count",
        ));
    }
    if crate::option(arguments, "--password-file").is_some()
        && !arguments.iter().any(|arg| arg == "--encrypt")
    {
        return Err(crate::invalid_input(
            "--password-file requires --encrypt when recording",
        ));
    }
    if let Some(value) = crate::option(arguments, "--circular-buffer")
        && !["auto", "unlimited"].contains(&value)
    {
        crate::circular::parse_size(value)?;
    }
    if let Some(value) = crate::option(arguments, "--duration")
        && value.parse::<u64>().map_or(true, |v| v == 0 || v > 86400)
    {
        return Err(crate::invalid_input("--duration requires 1–86400 seconds"));
    }
    for flag in ["--client-version", "--expected", "--actual"] {
        if let Some(value) = crate::option(arguments, flag) {
            transmog_capture::support::text(flag, value, 2048, false)?;
        }
    }
    let listener = ListenerConfig {
        listen_addr: crate::option(arguments, "--listen")
            .unwrap_or("127.0.0.1:0")
            .parse()
            .map_err(|_| crate::invalid_input("--listen must be an IP address and port"))?,
        allow_remote_clients: arguments.iter().any(|arg| arg == "--allow-remote"),
    };
    listener
        .validate()
        .map_err(|error| crate::invalid_input(error.to_string()))?;
    crate::parse_route(crate::option(arguments, "--route").unwrap_or("auto"))?;
    #[cfg(windows)]
    if !listener.listen_addr.ip().is_loopback()
        && !arguments.iter().any(|arg| arg == "--no-system-proxy")
    {
        return Err(crate::invalid_input(
            "Remote listeners need --no-system-proxy; configure that device with the printed proxy address",
        ));
    }
    Ok(())
}
pub(crate) fn cleanup_roots(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.first().is_none_or(|arg| arg != "cleanup")
        || arguments
            .iter()
            .skip(1)
            .any(|arg| arg != "--include-persistent")
    {
        return Err(
            crate::invalid_input("Use transmog-cli roots cleanup [--include-persistent]").into(),
        );
    }
    let ledger = RootLedger::open(state_directory()?)?;
    #[cfg(windows)]
    recover_proxy(&ledger)?;
    let all = arguments.iter().any(|arg| arg == "--include-persistent");
    let mut pending = 0;
    for root in ledger
        .records()?
        .iter()
        .filter(|root| all || ledger.pending_cleanup(root))
    {
        if let Err(error) = ledger.cleanup(root, &SystemRootTrust) {
            eprintln!("Root {} still needs cleanup: {error}", root.sha256);
            pending += 1;
        }
    }
    if pending > 0 {
        return Err(io::Error::other(format!(
            "{pending} root(s) still need removal; their metadata is retained"
        ))
        .into());
    }
    println!("CLI root cleanup completed.");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_privacy_is_run_scoped_but_explicit_flags_keep_their_preference() {
        let root = tempfile::tempdir().unwrap();
        let ledger = RootLedger::open(root.path().into()).unwrap();
        let profile=transmog_capture::support::SupportProfile::parse(br#"{"schemaVersion":1,"id":"fixture","revision":1,"title":"Issue","recording":{"redactSensitiveHeaders":true,"requestBodyLimit":{"kind":"limited","bytes":1234},"encryption":"required"}}"#).unwrap();
        let loaded = crate::support_profile::Loaded {
            digest: profile.digest().unwrap(),
            profile,
        };
        assert_eq!(
            recording_preferences(&[], &ledger, Some(&loaded)).unwrap(),
            (true, Some(1234))
        );
        assert_eq!(
            ledger.preferences(None, None).unwrap(),
            (false, Some(25_000_000))
        );
        assert_eq!(
            recording_preferences(
                &[
                    "--retain-sensitive".into(),
                    "--unlimited-request-bodies".into()
                ],
                &ledger,
                Some(&loaded)
            )
            .unwrap(),
            (false, None)
        );
        assert_eq!(ledger.preferences(None, None).unwrap(), (false, None));
        let args = loaded.arguments(&["--password-file".into(), "secret.txt".into()]);
        assert!(args.iter().any(|a| a == "--encrypt"));
        validate_options(&args).unwrap();
        assert!(crate::support_recording::marker_note("m issue happened").is_some());
        assert!(crate::support_recording::marker_note("unexpected command").is_none());
    }
    #[test]
    fn output_requires_native_extension_and_preserves_relative_paths() {
        let output = output_path(&["--output".into(), "my trace.TMCAP".into()]).unwrap();
        assert!(output.is_absolute());
        assert_eq!(output.file_name().unwrap(), "my trace.TMCAP");
        assert!(output_path(&["--output".into(), "trace.bin".into()]).is_err());
        #[cfg(windows)]
        for name in [
            "CON.tmcap",
            "a?.tmcap",
            "a.tmcap:stream.tmcap",
            "LPT1.extra.tmcap",
        ] {
            assert!(output_path(&["--output".into(), name.into()]).is_err());
        }
    }

    #[test]
    fn guided_options_reject_ambiguous_or_missing_values() {
        for args in [
            vec!["--output"],
            vec!["--output", "one.tmcap", "--output", "two.tmcap"],
            vec!["--install-root", "--no-install-root"],
            vec!["--unknown"],
            vec!["--request-body-limit", "0"],
            vec!["--request-body-limit", "invalid"],
            vec![
                "--request-body-limit",
                "25000000",
                "--unlimited-request-bodies",
            ],
        ] {
            assert!(
                validate_options(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
        assert!(
            validate_options(&[
                "--persistent-root".into(),
                "--include-network-context".into(),
                "--output".into(),
                "my trace.tmcap".into()
            ])
            .is_ok()
        );
    }
}

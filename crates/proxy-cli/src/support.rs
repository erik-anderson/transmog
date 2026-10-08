//! Guided support capture, with durable cleanup identities and streaming compression.
use crate::roots::{RootLedger, RootTrust, SystemRootTrust, state_directory};
use flate2::{Compression, write::GzEncoder};
use std::{
    env,
    error::Error,
    fs::{self, File, OpenOptions},
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

pub(crate) async fn record(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    validate_options(arguments)?;
    let output = output_path(arguments)?;
    let compressed = output
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".tmcap.gz");
    let native = if compressed {
        output.with_extension("")
    } else {
        output.clone()
    };
    if output.exists() || native.exists() {
        return Err(crate::invalid_input(
            "The capture destination already exists; choose a new filename",
        )
        .into());
    }
    let ledger = RootLedger::open(state_directory()?)?;
    let trust = SystemRootTrust;
    let redact = ledger.redaction(if arguments.iter().any(|arg| arg == "--redact") {
        Some(true)
    } else if arguments.iter().any(|arg| arg == "--retain-sensitive") {
        Some(false)
    } else {
        None
    })?;
    if ledger.cleanup_ephemeral(&trust)? > 0 {
        eprintln!(
            "Previous roots still need removal. Their identities are preserved; this capture will use a new root."
        );
    }
    #[cfg(windows)]
    {
        let host = transmog_host_windows::WindowsProxyIntegration::system(
            ledger.directory().join("proxy-recovery.json"),
        );
        if host.recover_pending()? {
            println!("Restored Windows proxy settings from an interrupted CLI capture.");
        }
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
    let result = run_capture(arguments, &native, &ledger, &root, ca, redact).await;
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
    if result.is_err() && native.is_file() {
        eprintln!(
            "Capture did not finish cleanly. The native evidence remains at {} for recovery.",
            native.display()
        );
    }
    result?;
    if compressed {
        println!("Compressing the saved trace…");
        compress_capture(&native, &output)?;
        fs::remove_file(&native)?;
    }
    println!("Trace saved: {}", output.display());
    println!("Open it in Transmog to review it, then share this file with your support contact.");
    cleanup?;
    Ok(())
}

async fn run_capture(
    arguments: &[String],
    native: &Path,
    ledger: &RootLedger,
    root: &crate::roots::RootRecord,
    ca: transmog_tls::ProxyCa,
    redact: bool,
) -> Result<(), Box<dyn Error>> {
    setup_root(arguments, ledger, root)?;
    println!(
        "Captured headers: {}. Captured bodies may contain private data; review the trace before sharing.",
        if redact {
            "Authorization, Proxy-Authorization, Cookie and Set-Cookie values are redacted"
        } else {
            "complete, including credentials and cookies"
        }
    );
    let service = ApplicationSessionService::new(ServiceConfig::default())?;
    service.set_redact_sensitive_headers(redact);
    let config = ProxyConfig {
        listener: ListenerConfig {
            listen_addr: crate::option(arguments, "--listen")
                .unwrap_or("127.0.0.1:0")
                .parse()?,
            allow_remote_clients: arguments.iter().any(|arg| arg == "--allow-remote"),
        },
        route_policy: crate::parse_route(crate::option(arguments, "--route").unwrap_or("auto"))?,
        ..ProxyConfig::default()
    };
    let (components, _) = crate::build_components(
        &config,
        ca,
        Arc::new(NoopInterceptorFactory),
        false,
        None,
        false,
    )?;
    let proxy = ProxyServer::bind_with_components(
        config,
        crate::load_upstream_trust(arguments)?,
        service.prepare_components(components),
    )
    .await?;
    let mut policy = if redact {
        CapturePolicy::default()
    } else {
        CapturePolicy::default().retain_sensitive_headers()
    };
    policy.retain_body_samples = true;
    service
        .start_capture(CaptureStart {
            path: native.to_path_buf(),
            limits: CaptureLimits::default(),
            policy,
        })
        .await?;
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
    println!("Proxy address: {endpoint}");
    #[cfg(not(windows))]
    println!(
        "Set your application's HTTP and HTTPS proxy to this address. Remove that setting after recording."
    );
    println!("Recording. Reproduce the issue, then press Ctrl+C once to stop and save the trace.");
    let running = wait_for_stop(&service).await;
    // Ensure failures still flush the recoverable prefix and restore owned host state.
    let stopped = service.stop().await;
    running?;
    stopped?;
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
        loop {
            match trust.install(&ledger.certificate(root), &root.sha256) {
                Ok(()) => {
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
        println!(
            "HTTPS clients must trust this public root manually: {}",
            ledger.certificate(root).display()
        );
        println!("Root SHA-256: {}", root.sha256);
    }
    Ok(())
}

async fn wait_for_stop(service: &ApplicationSessionService) -> io::Result<()> {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal?;
                println!("Stopping capture: restoring proxy settings, then finishing active requests. Press Ctrl+C again to stop waiting (unfinished requests will be incomplete).");
                service.begin_drain().await.map_err(io::Error::other)?;
                break;
            }
            _ = tick.tick() => { check_failure(service)?; }
        }
    }
    loop {
        if service.status() == ServiceStatus::Stopped {
            return Ok(());
        }
        check_failure(service)?;
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal?;
                // The legacy bounded stop intentionally permits forced completion.
                service.stop_now().await.map_err(io::Error::other)?;
                return Ok(());
            }
            _ = tick.tick() => {}
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
                "Transmog-{}.tmcap.gz",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        },
        PathBuf::from,
    );
    let name = path.to_string_lossy().to_ascii_lowercase();
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tmcap"))
        && !name.ends_with(".tmcap.gz")
    {
        return Err(crate::invalid_input(
            "--output must end in .tmcap.gz (compressed) or .tmcap",
        ));
    }
    Ok(if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    })
}
fn validate_options(arguments: &[String]) -> io::Result<()> {
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--output" | "--listen" | "--route" | "--upstream-ca-cert" => {
                if arguments
                    .get(index + 1)
                    .is_none_or(|arg| arg.starts_with("--"))
                {
                    return Err(crate::invalid_input(format!(
                        "{} requires a value",
                        arguments[index]
                    )));
                }
                index += 2;
            }
            "--persistent-root" | "--install-root" | "--no-install-root" | "--no-system-proxy"
            | "--redact" | "--retain-sensitive" | "--allow-remote" => index += 1,
            unknown => {
                return Err(crate::invalid_input(format!(
                    "Unknown record option {unknown}"
                )));
            }
        }
    }
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
    let all = arguments.iter().any(|arg| arg == "--include-persistent");
    let mut pending = 0;
    for root in ledger
        .records()?
        .iter()
        .filter(|root| all || !root.persistent)
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
pub(crate) fn compress_capture(source: &Path, destination: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = (|| {
        let mut writer = GzEncoder::new(file, Compression::default());
        io::copy(&mut File::open(source)?, &mut writer)?;
        writer.finish()?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn compression_is_streaming_exact_and_refuses_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("trace.tmcap");
        let destination = directory.path().join("trace.tmcap.gz");
        let bytes = vec![42; 1024 * 1024];
        fs::write(&source, &bytes).unwrap();
        compress_capture(&source, &destination).unwrap();
        let mut decoded = Vec::new();
        flate2::read::MultiGzDecoder::new(File::open(&destination).unwrap())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, bytes);
        assert!(compress_capture(&source, &destination).is_err());
    }
    #[test]
    fn guided_options_reject_ambiguous_or_missing_values() {
        for args in [
            vec!["--output"],
            vec!["--install-root", "--no-install-root"],
            vec!["--unknown"],
        ] {
            assert!(
                validate_options(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
        assert!(
            validate_options(&[
                "--persistent-root".into(),
                "--output".into(),
                "my trace.tmcap.gz".into()
            ])
            .is_ok()
        );
    }
}

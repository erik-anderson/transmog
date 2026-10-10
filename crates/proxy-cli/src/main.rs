//! Operator entry point for the runnable explicit proxy and CA generation.

mod arguments;
mod circular;
mod help;
mod output;
mod passwords;
mod root_lifecycle;
mod roots;
mod support;

use std::{
    collections::{HashMap, HashSet},
    env,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::SocketAddr,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use bytes::Bytes;
use transmog_capture::{
    CaptureBodyRetention, CaptureExporter, CaptureLimits, CapturePolicy, CaptureReader,
    CaptureSummary, CaptureWriter, JsonLinesExporter, RecoveredCapture, loss_record,
    record_from_observer,
};
use transmog_content::{ContentLimits, ContentPolicy};
use transmog_core::{
    HeaderField, RoutePolicy,
    intercept::{
        BodyHookError, BodyPlan, BoxBodyFuture, BoxHookFuture, BufferedBody, BufferedBodyHandler,
        ExchangeInterceptor, ExchangeMetadata, HookInitError, InterceptorChainFactory,
        InterceptorFactory, InterceptorRegistration, InterceptorRequirement,
        NoopInterceptorFactory, RequestHeadAction, RequestHeadEvent, ResponseBodyAction,
        ResponseBodyEvent, ResponseHeadAction, ResponseHeadEvent,
    },
    observe::{
        BodyObservation, BoxObserverFuture, ObservationInterest, Observer, ObserverConfig,
        ObserverError, ObserverEvent, ObserverHub,
    },
};
use transmog_key_protection::SystemKeyProtection;
use transmog_runtime::{
    ExchangeEvidence, ListenerConfig, ProxyComponents, ProxyConfig, ProxyServer,
    WebSocketSessionEvidence, WebSocketSessionOutcome,
};
use transmog_saz::{SazExporter, SazLimits, SazMode};
use transmog_tls::{
    CachedMitmCertificateResolver, CompositeTrustSource, EndpointIdentity, PemTrustSource, ProxyCa,
    SystemTrustSource, TrustSnapshot, TrustSource,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "transmog=info".into()),
        )
        .with_target(false)
        .try_init()
        .ok();
    if let Err(error) = run().await {
        eprintln!("transmog-cli: {error}");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if let Some(help) = help::requested(&arguments)? {
        println!("{help}");
        return Ok(());
    }
    match arguments.first().map(String::as_str) {
        Some("record") => support::record(&arguments[1..]).await,
        Some("roots") => support::cleanup_roots(&arguments[1..]),
        Some("--version" | "-V") => {
            println!("transmog-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("serve") => serve(&arguments[1..]).await,
        Some("ca") if arguments.get(1).map(String::as_str) == Some("generate") => {
            generate_ca(&arguments[2..])
        }
        Some("ca") if arguments.get(1).map(String::as_str) == Some("issue") => {
            issue_certificate(&arguments[2..])
        }
        Some("ca") if arguments.get(1).map(String::as_str) == Some("protect") => {
            protect_ca(&arguments[2..])
        }
        Some("capture") => capture_command(&arguments[1..]),
        _ => Err(invalid_input("unknown command; run `transmog-cli help`").into()),
    }
}

async fn serve(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(
        arguments,
        &[
            "--ca-cert",
            "--ca-key",
            "--upstream-ca-cert",
            "--listen",
            "--route",
            "--proof-id",
            "--capture",
        ],
        &["--capture-bodies", "--allow-remote"],
    )?;
    let certificate_path = required_option(arguments, "--ca-cert")?;
    let key_path = required_option(arguments, "--ca-key")?;
    let listen_addr = option(arguments, "--listen")
        .unwrap_or("127.0.0.1:0")
        .parse::<SocketAddr>()?;
    let route_policy = parse_route(option(arguments, "--route").unwrap_or("auto"))?;
    let proof_id = option(arguments, "--proof-id");
    let proof = proof_id
        .map(|id| ProofFactory::new(id.to_owned(), 16 * 1024 * 1024))
        .transpose()?;
    let capture_path = option(arguments, "--capture").map(PathBuf::from);
    let capture_bodies = arguments.iter().any(|value| value == "--capture-bodies");
    if capture_bodies && capture_path.is_none() {
        return Err(invalid_input("--capture-bodies requires --capture PATH").into());
    }
    let allow_remote_clients = arguments.iter().any(|value| value == "--allow-remote");

    let certificate_pem = fs::read(certificate_path)?;
    let ca = load_ca(&certificate_pem, Path::new(key_path))?;
    let thumbprint = ca.sha256_thumbprint()?;
    let trust = load_upstream_trust(arguments)?;
    let interceptor: Arc<dyn InterceptorFactory> = proof.map_or_else(
        || Arc::new(NoopInterceptorFactory) as Arc<dyn InterceptorFactory>,
        |proof| Arc::new(proof),
    );
    let config = ProxyConfig {
        listener: ListenerConfig {
            listen_addr,
            allow_remote_clients,
        },
        route_policy,
        limits: transmog_runtime::RuntimeLimits {
            max_request_body_bytes: usize::MAX,
            ..transmog_runtime::RuntimeLimits::default()
        },
        ..ProxyConfig::default()
    };
    let (components, capture) = build_components(
        &config,
        ca,
        interceptor,
        proof_id.is_some(),
        capture_path.as_deref(),
        capture_bodies,
    )?;
    let proxy = ProxyServer::bind_with_components(config, trust, components).await?;
    let actual_addr = proxy.local_addr()?;
    let mut evidence = proxy.subscribe_evidence();
    let mut websocket_evidence = proxy.subscribe_websocket_evidence();
    tokio::spawn(async move {
        while let Ok(event) = evidence.recv().await {
            print_evidence(&event);
            tracing::info!(
                session_id = %event.session_id.0,
                ingress = ?event.ingress_version,
                egress = ?event.egress_version,
                trust_generation = event.trust_generation,
                "intercepted exchange completed"
            );
        }
    });
    tokio::spawn(async move {
        while let Ok(event) = websocket_evidence.recv().await {
            print_websocket_evidence(&event);
        }
    });

    println!("LISTEN_ADDR={actual_addr}");
    println!("CA_SHA256={thumbprint}");
    println!("ROUTE_POLICY={route_policy:?}");
    println!("Press Ctrl+C to stop the proxy and save any active capture.");
    let result = proxy
        .serve(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "failed to install Ctrl-C handler");
            }
        })
        .await;
    if result.is_ok()
        && let Some(capture) = capture
    {
        seal_live_capture(&capture).await?;
    }
    result?;
    Ok(())
}

fn load_upstream_trust(arguments: &[String]) -> Result<Arc<TrustSnapshot>, Box<dyn Error>> {
    let Some(path) = option(arguments, "--upstream-ca-cert") else {
        return Ok(Arc::new(TrustSnapshot::load(&SystemTrustSource, 1)?));
    };
    let additional = Arc::new(PemTrustSource::from_pem(
        &fs::read(path)?,
        format!("operator PEM bundle {path}"),
    )?) as Arc<dyn TrustSource>;
    let sources = vec![
        Arc::new(SystemTrustSource) as Arc<dyn TrustSource>,
        additional,
    ];
    let composite = CompositeTrustSource::new("operating system plus operator PEM bundle", sources);
    Ok(Arc::new(TrustSnapshot::load(&composite, 1)?))
}

fn build_components(
    config: &ProxyConfig,
    ca: ProxyCa,
    interceptor: Arc<dyn InterceptorFactory>,
    enable_content_processing: bool,
    capture_path: Option<&Path>,
    capture_bodies: bool,
) -> Result<(ProxyComponents, Option<LiveCapture>), Box<dyn Error>> {
    let hooks = InterceptorChainFactory::new(
        vec![InterceptorRegistration::new(
            "application",
            interceptor,
            InterceptorRequirement::Required,
        )],
        config.limits.hooks,
    );
    let certificates = Arc::new(CachedMitmCertificateResolver::new(
        ca,
        config.limits.leaf_cache_capacity,
        config.limits.leaf_validity_days,
    )?);
    let mut components = ProxyComponents::new(hooks, certificates);
    if enable_content_processing {
        components = components.with_content_policy(ContentPolicy::preserve_original_output(
            ContentLimits::default(),
        ));
    }
    let capture = if let Some(path) = capture_path {
        let state = open_live_capture(path)?;
        let mut policy = CapturePolicy::default();
        policy.retain_body_samples = capture_bodies;
        let observer = Arc::new(FileCaptureObserver {
            state: Arc::clone(&state),
            policy,
        });
        components = components.with_observers(ObserverHub::new(vec![(
            observer,
            ObserverConfig {
                interest: ObservationInterest {
                    lifecycle: true,
                    sensitive_headers: false,
                    request_body: if capture_bodies {
                        BodyObservation::Full
                    } else {
                        BodyObservation::MetadataOnly
                    },
                    response_body: if capture_bodies {
                        BodyObservation::Full
                    } else {
                        BodyObservation::MetadataOnly
                    },
                },
                ..ObserverConfig::default()
            },
        )]));
        Some(state)
    } else {
        None
    };
    Ok((components, capture))
}

struct LiveCaptureState {
    writer: CaptureWriter<File>,
    retention: CaptureBodyRetention,
    last_sequences: HashMap<u128, u64>,
}

type LiveCapture = Arc<Mutex<LiveCaptureState>>;

struct FileCaptureObserver {
    state: LiveCapture,
    policy: CapturePolicy,
}

impl Observer for FileCaptureObserver {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        let state = Arc::clone(&self.state);
        let policy = self.policy.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || append_observer_event(&state, &policy, &event))
                .await
                .map_err(|_| ObserverError::new("capture writer task failed"))?
                .map_err(|error| ObserverError::new(error.to_string()))
        })
    }
}

fn open_live_capture(path: &Path) -> Result<LiveCapture, Box<dyn Error>> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let writer = CaptureWriter::with_encoding(
        file,
        CaptureLimits::default(),
        &transmog_capture::CaptureEncoding::default(),
    )?;
    Ok(Arc::new(Mutex::new(LiveCaptureState {
        writer,
        retention: CaptureBodyRetention::default(),
        last_sequences: HashMap::new(),
    })))
}

fn append_observer_event(
    state: &Mutex<LiveCaptureState>,
    policy: &CapturePolicy,
    event: &ObserverEvent,
) -> Result<(), transmog_capture::CaptureError> {
    let mut state = state
        .lock()
        .map_err(|_| io::Error::other("capture state is unavailable"))?;
    let exchange_id = event.exchange_id.0;
    let previous = state.last_sequences.get(&exchange_id).copied().unwrap_or(0);
    if event.sequence > previous.saturating_add(1) {
        let first_missing = previous.saturating_add(1);
        let missing = event.sequence.saturating_sub(first_missing);
        state.writer.append(&loss_record(
            exchange_id,
            first_missing,
            missing,
            "observer-delivery-gap",
        ))?;
    } else if event.sequence <= previous {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "capture observer sequence moved backward",
        )
        .into());
    }
    if let Some(mut record) = record_from_observer(event, policy) {
        state.retention.apply(&mut record, policy);
        state.writer.append(&record)?;
    }
    if matches!(
        event.kind,
        transmog_core::observe::ObserverEventKind::Completed(_)
            | transmog_core::observe::ObserverEventKind::Failed(_)
    ) {
        state.last_sequences.remove(&exchange_id);
    } else {
        state.last_sequences.insert(exchange_id, event.sequence);
    }
    Ok(())
}

async fn seal_live_capture(state: &LiveCapture) -> Result<(), Box<dyn Error>> {
    let state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        state
            .lock()
            .map_err(|_| io::Error::other("capture state is unavailable"))?
            .writer
            .seal()
            .map_err(io::Error::other)
    })
    .await
    .map_err(|_| io::Error::other("capture sealing task failed"))??;
    Ok(())
}

fn capture_command(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    match arguments.first().map(String::as_str) {
        Some("inspect") => capture_inspect(&arguments[1..]),
        Some("validate") => capture_validate(&arguments[1..]),
        Some("seal") => capture_seal(&arguments[1..]),
        Some("export") => capture_export(&arguments[1..]),
        _ => {
            Err(invalid_input("capture command must be inspect, validate, seal, or export").into())
        }
    }
}

fn recover_file(arguments: &[String]) -> Result<RecoveredCapture, Box<dyn Error>> {
    Ok(recover_file_with_password(arguments)?.0)
}
fn recover_file_with_password(
    arguments: &[String],
) -> Result<(RecoveredCapture, Option<transmog_capture::CapturePassword>), Box<dyn Error>> {
    with_capture_password(arguments, |file, password| {
        transmog_capture::recover_with_password(file, CaptureLimits::default(), password)
    })
}

fn with_capture_password<T>(
    arguments: &[String],
    mut read: impl FnMut(
        File,
        Option<&transmog_capture::CapturePassword>,
    ) -> Result<T, transmog_capture::CaptureError>,
) -> Result<(T, Option<transmog_capture::CapturePassword>), Box<dyn Error>> {
    let path = required_option(arguments, "--input")?;
    let password_path = if option(arguments, "--source-password-file").is_some() {
        option(arguments, "--source-password-file")
    } else if !arguments.iter().any(|arg| arg == "--encrypt") {
        option(arguments, "--password-file")
    } else {
        None
    };
    let mut password = password_path.map(passwords::read_file).transpose()?;
    loop {
        let file = File::open(path)?;
        match read(file, password.as_ref()) {
            Ok(capture) => return Ok((capture, password)),
            Err(transmog_capture::CaptureError::PasswordRequired) => {
                password = Some(passwords::prompt(false)?);
            }
            Err(transmog_capture::CaptureError::InvalidPassword) if password_path.is_none() => {
                eprintln!("The capture password was not accepted.");
                password = Some(passwords::prompt(false)?);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn scan_capture(
    input: impl Read,
    password: Option<&transmog_capture::CapturePassword>,
    count_exchanges: bool,
) -> Result<(CaptureSummary, u64), transmog_capture::CaptureError> {
    let mut reader = CaptureReader::with_password(input, CaptureLimits::default(), password)?;
    let mut exchanges = HashSet::new();
    let mut summary = CaptureSummary {
        records: 0,
        exchanges: 0,
        loss_markers: 0,
        retained_body_bytes: 0,
        sealed: false,
        truncated_tail: false,
    };
    // Fully verify each payload, then release it before reading the next frame.
    while let Some(frame) = reader.read_next()? {
        summary.records += 1;
        if count_exchanges && frame.record.exchange_id != 0 {
            exchanges.insert(frame.record.exchange_id);
        }
        match frame.record.kind {
            transmog_capture::CaptureRecordKind::Loss { .. } => summary.loss_markers += 1,
            transmog_capture::CaptureRecordKind::BodySegment {
                bytes: Some(bytes), ..
            } => {
                summary.retained_body_bytes = summary
                    .retained_body_bytes
                    .saturating_add(bytes.len() as u64);
            }
            _ => {}
        }
    }
    summary.exchanges = exchanges.len();
    summary.sealed = reader.sealed();
    summary.truncated_tail = reader.truncated_tail();
    Ok((summary, reader.valid_bytes()))
}

fn capture_inspect(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(
        arguments,
        &["--input", "--password-file", "--source-password-file"],
        &[],
    )?;
    let ((summary, valid_bytes), _) = with_capture_password(arguments, |file, password| {
        scan_capture(file, password, true)
    })?;
    println!(
        "FORMAT_REVISION={}",
        transmog_capture::CAPTURE_FORMAT_REVISION
    );
    println!("RECORDS={}", summary.records);
    println!("EXCHANGES={}", summary.exchanges);
    println!("LOSS_MARKERS={}", summary.loss_markers);
    println!("RETAINED_BODY_BYTES={}", summary.retained_body_bytes);
    println!("SEALED={}", summary.sealed);
    println!("TRUNCATED_TAIL={}", summary.truncated_tail);
    println!("VALID_BYTES={valid_bytes}");
    Ok(())
}

fn capture_validate(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(
        arguments,
        &["--input", "--password-file", "--source-password-file"],
        &[],
    )?;
    let ((summary, _), _) = with_capture_password(arguments, |file, password| {
        scan_capture(file, password, false)
    })?;
    if summary.truncated_tail {
        return Err(
            invalid_input("capture has a truncated tail; seal a recovered copy first").into(),
        );
    }
    if !summary.sealed {
        return Err(invalid_input("capture is not sealed").into());
    }
    println!("VALID=true");
    println!("RECORDS={}", summary.records);
    Ok(())
}

fn capture_seal(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(
        arguments,
        &[
            "--input",
            "--output",
            "--password-file",
            "--source-password-file",
        ],
        &[],
    )?;
    let (capture, password) = recover_file_with_password(arguments)?;
    if capture.sealed {
        return Err(invalid_input("capture is already sealed").into());
    }
    let output = PathBuf::from(required_option(arguments, "--output")?);
    output::write_new(&output, |file| {
        Ok(write_sealed_capture_encoded(
            file,
            &capture,
            &transmog_capture::CaptureEncoding { password },
        )?)
    })?;
    println!("CAPTURE={}", output.display());
    println!("RECOVERED_TAIL={}", capture.truncated_tail);
    Ok(())
}

#[cfg(test)]
fn write_sealed_capture<W: Write>(
    output: W,
    capture: &RecoveredCapture,
) -> Result<(), transmog_capture::CaptureError> {
    write_sealed_capture_encoded(
        output,
        capture,
        &transmog_capture::CaptureEncoding::default(),
    )
}
fn write_sealed_capture_encoded<W: Write>(
    output: W,
    capture: &RecoveredCapture,
    encoding: &transmog_capture::CaptureEncoding,
) -> Result<(), transmog_capture::CaptureError> {
    let mut writer = CaptureWriter::with_encoding(output, CaptureLimits::default(), encoding)?;
    for record in &capture.records {
        if !matches!(
            record.kind,
            transmog_capture::CaptureRecordKind::Seal { .. }
        ) {
            writer.append(record)?;
        }
    }
    writer.seal()
}

fn capture_export(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(
        arguments,
        &[
            "--input",
            "--output",
            "--format",
            "--password-file",
            "--source-password-file",
        ],
        &["--encrypt"],
    )?;
    let format = option(arguments, "--format").unwrap_or("jsonl");
    if !["jsonl", "saz", "saz-extended"].contains(&format) {
        return Err(invalid_input("--format must be jsonl, saz, or saz-extended").into());
    }
    if arguments.iter().any(|argument| argument == "--encrypt") && format == "jsonl" {
        return Err(invalid_input("JSONL does not support password encryption; choose SAZ").into());
    }
    let output = option(arguments, "--output").unwrap_or("-");
    if format != "jsonl" && output == "-" {
        return Err(invalid_input("SAZ output must be a seekable file, not stdout").into());
    }
    let password = passwords::output(arguments)?;
    let capture = recover_file(arguments)?;
    let report = match format {
        "jsonl" if output == "-" => {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            JsonLinesExporter::new(&mut lock).export(&capture)?
        }
        "jsonl" => output::write_new(Path::new(output), |file| {
            Ok(JsonLinesExporter::new(file).export(&capture)?)
        })?,
        "saz" | "saz-extended" => {
            let mode = if format == "saz" {
                SazMode::Strict
            } else {
                SazMode::Extended
            };
            output::write_new(Path::new(output), |file| {
                let mut exporter = SazExporter::new(file, mode, SazLimits::default())?;
                if let Some(password) = password {
                    exporter = exporter.encrypted(password)?;
                }
                let report = exporter.export(&capture)?;
                let detail = exporter
                    .report()
                    .ok_or_else(|| io::Error::other("SAZ export report is unavailable"))?;
                if detail.skipped_incomplete > 0 {
                    return Err(invalid_input(format!(
                        "SAZ cannot represent {} exchange(s) missing request or response headers. Keep the TMCap input to preserve those entries; no SAZ file was saved",
                        detail.skipped_incomplete)).into());
                }
                if detail.incomplete_bodies > 0 {
                    eprintln!(
                        "INCOMPLETE_BODIES={}. The SAZ contains retained body prefixes; missing bytes cannot be recovered",
                        detail.incomplete_bodies
                    );
                }
                Ok(report)
            })?
        }
        _ => return Err(invalid_input("--format must be jsonl, saz, or saz-extended").into()),
    };
    eprintln!("EXPORTED_RECORDS={}", report.records);
    eprintln!("EXPORTED_BYTES={}", report.bytes);
    eprintln!("SOURCE_SEALED={}", capture.sealed);
    eprintln!("SOURCE_TRUNCATED_TAIL={}", capture.truncated_tail);
    Ok(())
}

fn print_evidence(event: &ExchangeEvidence) {
    let h3_alpn = event
        .h3
        .as_ref()
        .map_or("-", |telemetry| telemetry.alpn.as_str());
    let ingress_alpn = protocol_alpn(event.ingress_version);
    let egress_alpn = event.h3.as_ref().map_or_else(
        || protocol_alpn(event.egress_version),
        |telemetry| telemetry.alpn.as_str(),
    );
    let adapter = if event.h3.is_some() {
        "quiche"
    } else {
        "hyper"
    };
    let upstream_peer = event.h3.as_ref().map_or_else(
        || "-".to_owned(),
        |telemetry| telemetry.peer_addr.to_string(),
    );
    let peer_chain_sha256 = event.h3.as_ref().map_or_else(
        || "-".to_owned(),
        |telemetry| telemetry.peer_chain_sha256.join(","),
    );
    let upstream_verification = if event.target_scheme == "https" {
        "verified"
    } else {
        "not-applicable"
    };
    let route_attempts = event
        .route_attempts
        .iter()
        .map(|attempt| format!("{:?}:{}", attempt.protocol, attempt.outcome))
        .collect::<Vec<_>>()
        .join(",");
    let fallback_count = event.route_attempts.len().saturating_sub(1);
    println!(
        "EVIDENCE session_id={} connection_id={} stream_id={} scheme={} host={} path={} ingress={:?} ingress_alpn={} egress={:?} egress_alpn={} adapter={} upstream_verification={} request_breakpoint={} response_breakpoint={} request_event_id={}:request response_event_id={}:response trust_generation={} h3_alpn={} h3_peer={} peer_chain_sha256={} route_attempts={} fallback_count={}",
        event.session_id.0,
        event.downstream_connection_id.0,
        event.stream_id.0,
        event.target_scheme,
        event.target_host,
        event.target_path,
        event.ingress_version,
        ingress_alpn,
        event.egress_version,
        egress_alpn,
        adapter,
        upstream_verification,
        event.request_breakpoint_fired,
        event.response_breakpoint_fired,
        event.session_id.0,
        event.session_id.0,
        event.trust_generation,
        h3_alpn,
        upstream_peer,
        peer_chain_sha256,
        route_attempts,
        fallback_count,
    );
}

fn print_websocket_evidence(event: &WebSocketSessionEvidence) {
    match &event.outcome {
        WebSocketSessionOutcome::Completed(report) => println!(
            "WS_EVIDENCE session_id={} target={} outcome=completed client_bytes={} server_bytes={} clean_close={} messages={} controls={} effects={}",
            event.session_id.0,
            event.target,
            report.client_to_server_wire_bytes,
            report.server_to_client_wire_bytes,
            report.clean_close,
            report.messages,
            report.control_frames,
            report.effects.len(),
        ),
        WebSocketSessionOutcome::Failed(_) => println!(
            "WS_EVIDENCE session_id={} target={} outcome=failed",
            event.session_id.0, event.target,
        ),
    }
}

fn protocol_alpn(version: transmog_core::HttpLegVersion) -> &'static str {
    match version {
        transmog_core::HttpLegVersion::Http1 => "http/1.1",
        transmog_core::HttpLegVersion::Http2 => "h2",
        transmog_core::HttpLegVersion::Http3 => "h3",
    }
}

fn generate_ca(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(arguments, &["--cert", "--key", "--name"], &[])?;
    let certificate_path = PathBuf::from(required_option(arguments, "--cert")?);
    let key_path = PathBuf::from(required_option(arguments, "--key")?);
    let common_name = option(arguments, "--name").unwrap_or("Transmog local interception CA");
    if certificate_path.exists() || key_path.exists() {
        return Err(invalid_input("refusing to overwrite an existing certificate or key").into());
    }
    let ca = ProxyCa::generate(common_name, 365)?;
    let key = zeroize::Zeroizing::new(ca.private_key_pem_pkcs8()?);
    transmog_key_protection::write_new(&key_path, &key, &SystemKeyProtection)?;
    if let Err(error) = write_new(&certificate_path, &ca.certificate_pem()?) {
        let _ = transmog_key_protection::remove(&key_path, &SystemKeyProtection);
        return Err(error.into());
    }
    println!("CA_CERT={}", certificate_path.display());
    println!("CA_KEY={}", key_path.display());
    println!("CA_SHA256={}", ca.sha256_thumbprint()?);
    eprintln!(
        "The private key is protected for this OS user. Install only the public certificate."
    );
    Ok(())
}

fn load_ca(certificate: &[u8], key_path: &Path) -> Result<ProxyCa, Box<dyn Error>> {
    let key = transmog_key_protection::read(key_path, &SystemKeyProtection)?;
    let ca = ProxyCa::from_pem(certificate, &key)?;
    transmog_key_protection::protect_existing(key_path, &SystemKeyProtection)?;
    Ok(ca)
}

fn protect_ca(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(arguments, &["--ca-cert", "--ca-key"], &[])?;
    let certificate = required_option(arguments, "--ca-cert")?;
    let key = required_option(arguments, "--ca-key")?;
    let ca = load_ca(&fs::read(certificate)?, Path::new(key))?;
    println!("CA_SHA256={}", ca.sha256_thumbprint()?);
    println!("The interception CA private key is protected for this OS user.");
    Ok(())
}

fn issue_certificate(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    arguments::validate(
        arguments,
        &[
            "--ca-cert",
            "--ca-key",
            "--identity",
            "--cert",
            "--key",
            "--days",
        ],
        &[],
    )?;
    let ca_certificate_path = required_option(arguments, "--ca-cert")?;
    let ca_key_path = required_option(arguments, "--ca-key")?;
    let identity = EndpointIdentity::parse(required_option(arguments, "--identity")?)?;
    let certificate_path = PathBuf::from(required_option(arguments, "--cert")?);
    let key_path = PathBuf::from(required_option(arguments, "--key")?);
    let validity_days = option(arguments, "--days").unwrap_or("7").parse::<u32>()?;
    let ca = load_ca(&fs::read(ca_certificate_path)?, Path::new(ca_key_path))?;
    let leaf = ca.issue(identity, validity_days)?;
    write_new(&key_path, &leaf.private_key.private_key_to_pem_pkcs8()?)?;
    if let Err(error) = write_new(&certificate_path, &leaf.certificate.to_pem()?) {
        let _ = fs::remove_file(&key_path);
        return Err(error.into());
    }
    println!("LEAF_CERT={}", certificate_path.display());
    println!("LEAF_KEY={}", key_path.display());
    println!("LEAF_IDENTITY={}", leaf.identity.as_text());
    Ok(())
}

fn write_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

fn option<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .filter(|pair| !pair[1].is_empty() && !pair[1].starts_with("--"))
        .map(|pair| pair[1].as_str())
}

fn required_option<'a>(arguments: &'a [String], name: &str) -> io::Result<&'a str> {
    option(arguments, name).ok_or_else(|| invalid_input(format!("missing required {name}")))
}

fn parse_route(value: &str) -> io::Result<RoutePolicy> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Ok(RoutePolicy::Auto),
        "h1" | "http1" => Ok(RoutePolicy::Http1Only),
        "h2" | "http2" => Ok(RoutePolicy::Http2Only),
        "h3" | "http3" => Ok(RoutePolicy::Http3Only),
        _ => Err(invalid_input("--route must be auto, h1, h2, or h3")),
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

struct ProofFactory {
    id: Arc<str>,
    request_marker: HeaderField,
    response_marker: HeaderField,
    max_body_bytes: usize,
}

impl ProofFactory {
    fn new(id: String, max_body_bytes: usize) -> io::Result<Self> {
        let invalid = |_| {
            invalid_input("--proof-id must be a valid HTTP header value without control characters")
        };
        let request_marker =
            HeaderField::try_new("x-intercept-test", id.as_bytes()).map_err(invalid)?;
        let response_marker =
            HeaderField::try_new("x-intercepted-by", id.as_bytes()).map_err(invalid)?;
        Ok(Self {
            id: id.into(),
            request_marker,
            response_marker,
            max_body_bytes,
        })
    }
}

impl InterceptorFactory for ProofFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(ProofInterceptor {
            id: Arc::clone(&self.id),
            request_marker: self.request_marker.clone(),
            response_marker: self.response_marker.clone(),
            max_body_bytes: self.max_body_bytes,
            html: AtomicBool::new(false),
        }))
    }
}

struct ProofInterceptor {
    id: Arc<str>,
    request_marker: HeaderField,
    response_marker: HeaderField,
    max_body_bytes: usize,
    html: AtomicBool,
}

impl ExchangeInterceptor for ProofInterceptor {
    fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        Box::pin(async move {
            let mut head = event.head;
            head.headers.replace_all(self.request_marker.clone());
            head.headers.replace_all(
                HeaderField::try_new("accept-encoding", "identity")
                    .expect("static identity header is valid"),
            );
            RequestHeadAction::Replace(head)
        })
    }

    fn on_response_head(&self, event: ResponseHeadEvent) -> BoxHookFuture<'_, ResponseHeadAction> {
        Box::pin(async move {
            let mut head = event.head;
            let is_html = head.headers.values("content-type").any(|value| {
                std::str::from_utf8(value)
                    .is_ok_and(|value| value.to_ascii_lowercase().contains("text/html"))
            });
            self.html.store(is_html, Ordering::Release);
            if !is_html {
                return ResponseHeadAction::Continue;
            }
            head.headers.replace_all(self.response_marker.clone());
            ResponseHeadAction::Replace(head)
        })
    }

    fn on_response_body(&self, _event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        let action = if self.html.load(Ordering::Acquire) {
            let marker = format!(
                "<meta name=\"intercept-proxy-proof\" content=\"{}\">",
                escape_html_attribute(&self.id)
            )
            .into_bytes();
            ResponseBodyAction::decoded(BodyPlan::Buffer {
                limit: NonZeroUsize::new(self.max_body_bytes).expect("proof body bound is nonzero"),
                handler: Box::new(HtmlProofEditor {
                    max_output_body_bytes: self.max_body_bytes.saturating_add(marker.len()),
                    marker,
                }),
            })
        } else {
            ResponseBodyAction::pass_through()
        };
        Box::pin(async move { action })
    }
}

fn escape_html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&#39;")
}

struct HtmlProofEditor {
    marker: Vec<u8>,
    max_output_body_bytes: usize,
}

impl BufferedBodyHandler for HtmlProofEditor {
    fn on_body(
        &mut self,
        body: BufferedBody,
    ) -> BoxBodyFuture<'_, Result<BufferedBody, BodyHookError>> {
        Box::pin(async move {
            let trailers = body.trailers().cloned();
            let mut bytes = body.data().to_vec();
            let position = find_ascii_case_insensitive(&bytes, b"</head>")
                .or_else(|| find_ascii_case_insensitive(&bytes, b"<body"))
                .unwrap_or(0);
            bytes.splice(position..position, self.marker.iter().copied());
            BufferedBody::try_new(self.max_output_body_bytes, Bytes::from(bytes), trailers)
                .map_err(BodyHookError::from)
        })
    }
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_values_are_validated_before_files_or_traffic_and_html_is_escaped() {
        for value in ["bad\r\nvalue", "bad\0value"] {
            assert!(ProofFactory::new(value.into(), 1024).is_err());
        }
        assert!(ProofFactory::new("test-id".into(), 1024).is_ok());
        assert_eq!(escape_html_attribute("\"<&>'"), "&quot;&lt;&amp;&gt;&#39;");
    }

    #[test]
    fn streamed_summary_matches_recovery_and_checks_body_corruption() {
        use transmog_capture::{CaptureRecord, CaptureRecordKind};
        use transmog_core::observe::ExchangeBoundary;
        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        for index in 0..16 {
            writer
                .append(&CaptureRecord {
                    exchange_id: 7,
                    sequence: index + 1,
                    kind: CaptureRecordKind::BodySegment {
                        boundary: ExchangeBoundary::ClientResponse,
                        byte_count: 256 * 1024,
                        bytes: Some(vec![123; 256 * 1024]),
                        truncated: false,
                    },
                })
                .unwrap();
        }
        writer.append(&loss_record(8, 1, 1, "fixture")).unwrap();
        writer.seal().unwrap();
        let bytes = writer.into_inner();
        let recovered =
            transmog_capture::recover(bytes.as_slice(), CaptureLimits::default()).unwrap();
        let (summary, valid_bytes) = scan_capture(bytes.as_slice(), None, true).unwrap();
        assert_eq!(summary, recovered.summary());
        assert_eq!(valid_bytes, bytes.len() as u64);

        let mut reader = CaptureReader::new(bytes.as_slice(), CaptureLimits::default()).unwrap();
        let frame = reader.read_next().unwrap().unwrap();
        let mut damaged = bytes;
        damaged[usize::try_from(frame.offset + frame.frame_bytes - 1).unwrap()] ^= 1;
        assert!(scan_capture(damaged.as_slice(), None, false).is_err());
    }

    #[test]
    fn saz_missing_heads_is_rejected_without_publishing_either_mode() {
        use transmog_capture::{CaptureRecord, CaptureRecordKind};
        use transmog_core::observe::ExchangeBoundary;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.tmcap");
        let mut writer =
            CaptureWriter::new(File::create(&source).unwrap(), CaptureLimits::default()).unwrap();
        writer
            .append(&CaptureRecord {
                exchange_id: 1,
                sequence: 1,
                kind: CaptureRecordKind::RequestHead {
                    boundary: ExchangeBoundary::ClientRequest,
                    method: "GET".into(),
                    target: "http://example.test/".into(),
                    headers: vec![],
                },
            })
            .unwrap();
        writer.seal().unwrap();
        drop(writer);
        for format in ["saz", "saz-extended"] {
            let output = directory.path().join(format!("{format}.saz"));
            let arguments = vec![
                "--input".into(),
                source.to_string_lossy().into_owned(),
                "--format".into(),
                format.into(),
                "--output".into(),
                output.to_string_lossy().into_owned(),
            ];
            let error = capture_export(&arguments).unwrap_err().to_string();
            assert!(error.contains("1 exchange(s)"), "{error}");
            assert!(error.contains("TMCap"));
            assert!(!output.exists());
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn export_rejects_invalid_formats_and_encryption_before_prompting_or_reading() {
        let arguments = ["--input", "missing.tmcap", "--encrypt"].map(str::to_owned);
        assert!(
            capture_export(&arguments)
                .unwrap_err()
                .to_string()
                .contains("JSONL")
        );
        let arguments = ["--input", "missing.tmcap", "--format", "unknown"].map(str::to_owned);
        assert!(
            capture_export(&arguments)
                .unwrap_err()
                .to_string()
                .contains("--format")
        );
    }

    #[test]
    fn required_values_cannot_consume_another_option() {
        let arguments = vec!["--input".into(), "--output".into(), "copy.tmcap".into()];
        assert!(required_option(&arguments, "--input").is_err());
    }

    #[test]
    fn capture_options_are_validated_before_opening_files() {
        let arguments = vec![
            "--input".into(),
            "missing.tmcap".into(),
            "--encrypton".into(),
        ];
        let error = capture_export(&arguments).unwrap_err().to_string();
        assert!(error.contains("--encrypton"), "unexpected error: {error}");
    }

    #[test]
    fn html_proof_prefers_head_but_supports_minimal_html() {
        let marker = b"<meta name=proof>";
        for (input, expected) in [
            (b"<html><head></head><body>x</body></html>".as_slice(), 12),
            (b"<html><body>x</body></html>".as_slice(), 6),
            (b"plain html".as_slice(), 0),
        ] {
            let position = find_ascii_case_insensitive(input, b"</head>")
                .or_else(|| find_ascii_case_insensitive(input, b"<body"))
                .unwrap_or(0);
            let mut output = input.to_vec();
            output.splice(position..position, marker.iter().copied());
            assert_eq!(&output[position..position + marker.len()], marker);
            assert_eq!(position, expected);
        }
    }

    #[test]
    fn recovered_native_capture_can_be_resealed_without_mutating_records() {
        let source_record = transmog_capture::loss_record(7, 2, 1, "test-gap");
        let mut source = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        source.append(&source_record).unwrap();
        let recovered =
            transmog_capture::recover(&source.into_inner()[..], CaptureLimits::default()).unwrap();
        assert!(!recovered.sealed);

        let mut output = Vec::new();
        write_sealed_capture(&mut output, &recovered).unwrap();
        let sealed = transmog_capture::recover(&output[..], CaptureLimits::default()).unwrap();
        assert!(sealed.sealed);
        assert_eq!(sealed.records.first(), Some(&source_record));
    }
}

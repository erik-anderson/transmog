//! Operator entry point for the runnable explicit proxy and CA generation.

use std::{
    collections::HashSet,
    env,
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use rustymiddle_core::{
    AbortReason, BodyFrame, BodyTransform, BoxBreakpointFuture, BreakpointDecision,
    BreakpointEvent, BreakpointHandler, BreakpointPhase, ContinueHandler, HeaderField, RoutePolicy,
    SessionId,
};
use rustymiddle_runtime::{ExchangeEvidence, ListenerConfig, ProxyConfig, ProxyServer};
use rustymiddle_tls::{ProxyCa, SystemTrustSource, TrustSnapshot};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rustymiddle=info".into()),
        )
        .with_target(false)
        .try_init()
        .ok();
    if let Err(error) = run().await {
        eprintln!("rustymiddle: {error}");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("serve") => serve(&arguments[1..]).await,
        Some("ca") if arguments.get(1).map(String::as_str) == Some("generate") => {
            generate_ca(&arguments[2..])
        }
        Some("help" | "--help" | "-h") | None => {
            print_usage();
            Ok(())
        }
        _ => Err(invalid_input("unknown command; run `rustymiddle help`").into()),
    }
}

async fn serve(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let certificate_path = required_option(arguments, "--ca-cert")?;
    let key_path = required_option(arguments, "--ca-key")?;
    let listen_addr = option(arguments, "--listen")
        .unwrap_or("127.0.0.1:0")
        .parse::<SocketAddr>()?;
    let route_policy = parse_route(option(arguments, "--route").unwrap_or("auto"))?;
    let proof_id = option(arguments, "--proof-id");
    let allow_remote_clients = arguments.iter().any(|value| value == "--allow-remote");

    let certificate_pem = fs::read(certificate_path)?;
    let private_key_pem = fs::read(key_path)?;
    let ca = ProxyCa::from_pem(&certificate_pem, &private_key_pem)?;
    let thumbprint = ca.sha256_thumbprint()?;
    let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1)?);
    let handler: Arc<dyn BreakpointHandler> = proof_id.map_or_else(
        || Arc::new(ContinueHandler) as Arc<dyn BreakpointHandler>,
        |id| Arc::new(ProofHandler::new(id.to_owned(), 16 * 1024 * 1024)),
    );
    let proxy = ProxyServer::bind(
        ProxyConfig {
            listener: ListenerConfig {
                listen_addr,
                allow_remote_clients,
            },
            route_policy,
            ..ProxyConfig::default()
        },
        ca,
        trust,
        handler,
    )
    .await?;
    let actual_addr = proxy.local_addr()?;
    let mut evidence = proxy.subscribe_evidence();
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

    println!("LISTEN_ADDR={actual_addr}");
    println!("CA_SHA256={thumbprint}");
    println!("ROUTE_POLICY={route_policy:?}");
    proxy
        .serve(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "failed to install Ctrl-C handler");
            }
        })
        .await?;
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

fn protocol_alpn(version: rustymiddle_core::HttpLegVersion) -> &'static str {
    match version {
        rustymiddle_core::HttpLegVersion::Http1 => "http/1.1",
        rustymiddle_core::HttpLegVersion::Http2 => "h2",
        rustymiddle_core::HttpLegVersion::Http3 => "h3",
    }
}

fn generate_ca(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let certificate_path = PathBuf::from(required_option(arguments, "--cert")?);
    let key_path = PathBuf::from(required_option(arguments, "--key")?);
    let common_name = option(arguments, "--name").unwrap_or("rustymiddle local interception CA");
    if certificate_path.exists() || key_path.exists() {
        return Err(invalid_input("refusing to overwrite an existing certificate or key").into());
    }
    let ca = ProxyCa::generate(common_name, 365)?;
    write_new(&key_path, &ca.private_key_pem_pkcs8()?)?;
    if let Err(error) = write_new(&certificate_path, &ca.certificate_pem()?) {
        let _ = fs::remove_file(&key_path);
        return Err(error.into());
    }
    println!("CA_CERT={}", certificate_path.display());
    println!("CA_KEY={}", key_path.display());
    println!("CA_SHA256={}", ca.sha256_thumbprint()?);
    eprintln!(
        "Protect the private key with a user-only ACL and install only the public certificate."
    );
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

fn print_usage() {
    println!(
        "rustymiddle\n\n\
         Generate a CA (files must not already exist):\n  \
         rustymiddle ca generate --cert ca.pem --key ca.key [--name NAME]\n\n\
         Run the explicit proxy:\n  \
         rustymiddle serve --ca-cert ca.pem --ca-key ca.key [--listen 127.0.0.1:0] [--route auto|h1|h2|h3] [--proof-id ID]\n\n\
         Non-loopback listening additionally requires --allow-remote."
    );
}

struct ProofHandler {
    id: String,
    max_body_bytes: usize,
    html_sessions: Mutex<HashSet<SessionId>>,
}

impl ProofHandler {
    fn new(id: String, max_body_bytes: usize) -> Self {
        Self {
            id,
            max_body_bytes,
            html_sessions: Mutex::new(HashSet::new()),
        }
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashSet<SessionId>> {
        self.html_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl BreakpointHandler for ProofHandler {
    fn on_breakpoint(&self, event: BreakpointEvent) -> BoxBreakpointFuture<'_> {
        Box::pin(async move {
            match event.phase {
                BreakpointPhase::BeforeRequestHeaders => {
                    let Some(mut head) = event.request_head else {
                        return BreakpointDecision::Continue;
                    };
                    let proof = HeaderField::try_new("x-intercept-test", self.id.as_bytes())
                        .expect("proof identifiers are valid header values");
                    head.headers.replace_all(proof);
                    head.headers.replace_all(
                        HeaderField::try_new("accept-encoding", "identity")
                            .expect("static identity header is valid"),
                    );
                    BreakpointDecision::ReplaceRequestHead(head)
                }
                BreakpointPhase::BeforeResponseHeaders => {
                    let Some(mut head) = event.response_head else {
                        return BreakpointDecision::Continue;
                    };
                    let is_html = head.headers.values("content-type").any(|value| {
                        std::str::from_utf8(value)
                            .is_ok_and(|value| value.to_ascii_lowercase().contains("text/html"))
                    });
                    if !is_html {
                        return BreakpointDecision::Continue;
                    }
                    self.lock_sessions().insert(event.session.session_id);
                    head.headers.replace_all(
                        HeaderField::try_new("x-intercepted-by", self.id.as_bytes())
                            .expect("proof identifiers are valid header values"),
                    );
                    BreakpointDecision::ReplaceResponseHead(head)
                }
                BreakpointPhase::ResponseBody
                    if self.lock_sessions().remove(&event.session.session_id) =>
                {
                    BreakpointDecision::TransformBodyStream(Box::new(HtmlProofTransform::new(
                        &self.id,
                        self.max_body_bytes,
                    )))
                }
                BreakpointPhase::Completed | BreakpointPhase::Failed => {
                    self.lock_sessions().remove(&event.session.session_id);
                    BreakpointDecision::Continue
                }
                _ => BreakpointDecision::Continue,
            }
        })
    }
}

struct HtmlProofTransform {
    marker: Vec<u8>,
    max_body_bytes: usize,
    body: Vec<u8>,
    trailers: Option<rustymiddle_core::HeaderBlock>,
}

impl HtmlProofTransform {
    fn new(id: &str, max_body_bytes: usize) -> Self {
        Self {
            marker: format!("<meta name=\"intercept-proxy-proof\" content=\"{id}\">").into_bytes(),
            max_body_bytes,
            body: Vec::new(),
            trailers: None,
        }
    }
}

impl BodyTransform for HtmlProofTransform {
    fn transform(&mut self, frame: BodyFrame) -> Result<Vec<BodyFrame>, AbortReason> {
        match frame {
            BodyFrame::Data(data) => {
                let attempted = self.body.len().saturating_add(data.len());
                if attempted > self.max_body_bytes || self.trailers.is_some() {
                    return Err(AbortReason::Policy(format!(
                        "HTML proof buffer exceeded {} bytes or received data after trailers",
                        self.max_body_bytes
                    )));
                }
                self.body.extend_from_slice(&data);
            }
            BodyFrame::Trailers(trailers) => {
                if self.trailers.replace(trailers).is_some() {
                    return Err(AbortReason::Policy(
                        "duplicate HTML response trailers".to_owned(),
                    ));
                }
            }
        }
        Ok(Vec::new())
    }

    fn finish(&mut self) -> Result<Vec<BodyFrame>, AbortReason> {
        let position = find_ascii_case_insensitive(&self.body, b"</head>")
            .or_else(|| find_ascii_case_insensitive(&self.body, b"<body"))
            .unwrap_or(0);
        self.body
            .splice(position..position, self.marker.iter().copied());
        let mut frames = vec![BodyFrame::Data(Bytes::from(std::mem::take(&mut self.body)))];
        if let Some(trailers) = self.trailers.take() {
            frames.push(BodyFrame::Trailers(trailers));
        }
        Ok(frames)
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
}

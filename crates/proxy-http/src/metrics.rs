//! Physical connection instrumentation underneath Hyper's existing pools.
use crate::{HappyEyeballsResolver, HyperEgressMode};
use http::Uri;
use hyper_boring::{HttpsConnector, MaybeHttpsStream};
use hyper_util::{
    client::legacy::connect::{Connected, Connection, HttpConnector},
    rt::TokioIo,
};
use std::{
    error::Error,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Instant,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tower_service::Service;
use transmog_core::performance::{
    ConnectionSetupTime, PerformanceRecorder, TransportObservation, TransportOutcome,
};
use transmog_core::{
    HttpLegVersion,
    connection_policy::{AllowedHttpProtocols, OutboundProtocolPolicy},
};
use transmog_network::{
    HappyEyeballsConfig,
    metrics::{ConnectionMetrics, MeteredIo},
};
use transmog_tls::{TlsCapture, TrustError, UpstreamTlsContextFactory};

type BoxError = Box<dyn Error + Send + Sync>;
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);
tokio::task_local! {static SETUP: Arc<ConnectionSetup>;}

#[derive(Debug)]
struct ConnectionSetup {
    allowed: AllowedHttpProtocols,
    id: u64,
    values: Mutex<SetupValues>,
    bytes: Arc<ConnectionMetrics>,
    uses: AtomicU64,
}
#[derive(Debug, Default)]
struct SetupValues {
    metadata_factory: Option<UpstreamTlsContextFactory>,
    prior_failure: Option<ConnectionObservation>,
    no_reuse: bool,
    facts: TransportObservation,
    tls_capture: Option<TlsCapture>,
    tls_begin: Option<Instant>,
    dns_begin: Option<Instant>,
    dns_done: Option<Instant>,
    tcp_begin: Option<Instant>,
    tcp_done: Option<Instant>,
    tls_done: Option<Instant>,
    dns_failed: bool,
    attempt: Option<transmog_tls::client_authentication::ClientAuthenticationAttempt>,
    waker: Option<std::task::Waker>,
    tcp: Option<std::sync::Weak<Mutex<TcpState>>>,
}
impl ConnectionSetup {
    fn new(allowed: AllowedHttpProtocols) -> Arc<Self> {
        Arc::new(Self {
            allowed,
            id: NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed),
            values: Mutex::new(SetupValues::default()),
            bytes: Arc::new(ConnectionMetrics::default()),
            uses: AtomicU64::new(0),
        })
    }
    fn values(&self) -> std::sync::MutexGuard<'_, SetupValues> {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Extra metadata attached by Hyper to each response from this same connection.
#[derive(Clone, Debug)]
pub(crate) struct ConnectionObservation(Arc<ConnectionSetup>);
impl ConnectionObservation {
    pub(crate) fn permits_reuse(&self) -> bool {
        !self.0.values().no_reuse
    }
    pub(crate) fn snapshot(
        &self,
        shared: bool,
        performance: Option<&PerformanceRecorder>,
    ) -> TransportObservation {
        let values = self.0.values();
        if let (Some(previous), Some(performance)) = (&values.prior_failure, performance) {
            performance.transport(previous.snapshot(false, Some(performance)));
        }
        let mut facts = values.facts.clone();
        facts.tls = values.tls_capture.as_ref().map(TlsCapture::snapshot);
        if let Some(performance) = performance {
            let phases = [
                ("dns", values.dns_begin, values.dns_done),
                ("tcp", values.tcp_begin, values.tcp_done),
                ("tls", values.tls_begin, values.tls_done),
            ]
            .into_iter()
            .filter_map(|(phase, begin, end)| {
                Some(ConnectionSetupTime {
                    phase,
                    began: begin?,
                    ended: end?,
                })
            })
            .collect::<Vec<_>>();
            performance.project_connection_setup(&mut facts, &phases);
        }
        if let Some(performance) = performance {
            self.0.bytes.observe(&mut facts, performance);
        }
        facts.leg = "upstream".into();
        facts.connection_id = format!("upstream-{}", self.0.id);
        facts.shared = shared;
        facts.bytes_read = Some(self.0.bytes.bytes_read());
        facts.bytes_written = Some(self.0.bytes.bytes_written());
        facts
    }
    pub(crate) fn claim(&self) -> bool {
        self.0.uses.fetch_add(1, Ordering::Relaxed) > 0
    }
}

#[derive(Debug)]
struct ConnectionFailure {
    source: BoxError,
    observation: ConnectionObservation,
}
impl std::fmt::Display for ConnectionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.source)
    }
}
impl Error for ConnectionFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}
pub(crate) fn failed_observation(
    mut error: &(dyn Error + 'static),
    performance: &PerformanceRecorder,
) -> Option<TransportObservation> {
    for _ in 0..16 {
        if let Some(failure) = error.downcast_ref::<ConnectionFailure>() {
            return Some(failure.observation.snapshot(false, Some(performance)));
        }
        error = error.source()?;
    }
    None
}

pub(crate) fn dns_finished(started: Instant, succeeded: bool) {
    let _ = SETUP.try_with(|setup| {
        let mut values = setup.values();
        let done = Instant::now();
        values.dns_begin = Some(started);
        values.dns_done = Some(done);
        values.facts.dns_micros = Some(micros(done.duration_since(started)));
        values.dns_failed = !succeeded;
    });
}
fn micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[derive(Clone)]
struct MeasuredTcpConnector(HttpConnector<HappyEyeballsResolver>);
impl Service<Uri> for MeasuredTcpConnector {
    type Response = TokioIo<MeasuredTcp>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx).map_err(Into::into)
    }
    fn call(&mut self, uri: Uri) -> Self::Future {
        let connection = self.0.call(uri);
        Box::pin(async move {
            let setup = SETUP.with(Arc::clone);
            let started = Instant::now();
            let result = connection.await;
            let overall = micros(started.elapsed());
            {
                let mut values = setup.values();
                if !values.dns_failed {
                    values.tcp_begin = Some(values.dns_done.unwrap_or(started));
                    values.tcp_done = Some(Instant::now());
                    values.facts.tcp_micros =
                        Some(overall.saturating_sub(values.facts.dns_micros.unwrap_or(0)));
                }
            }
            let socket = result?.into_inner();
            {
                let mut values = setup.values();
                values.facts.peer = socket.peer_addr().ok().map(|address| address.to_string());
                values.facts.local = socket.local_addr().ok().map(|address| address.to_string());
            }
            let io = Arc::new(Mutex::new(TcpState {
                io: MeteredIo::new_tcp(socket, setup.bytes.clone()),
                buffered: std::collections::VecDeque::new(),
                eof: false,
            }));
            setup.values().tcp = Some(Arc::downgrade(&io));
            Ok(TokioIo::new(MeasuredTcp {
                io,
                observation: ConnectionObservation(setup),
            }))
        })
    }
}

#[derive(Debug)]
pub(crate) struct MeasuredTcp {
    io: Arc<Mutex<TcpState>>,
    observation: ConnectionObservation,
}
#[derive(Debug)]
struct TcpState {
    io: MeteredIo<TcpStream>,
    buffered: std::collections::VecDeque<u8>,
    eof: bool,
}
impl Connection for MeasuredTcp {
    fn connected(&self) -> Connected {
        let connected = self
            .io
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .io
            .get_ref()
            .connected()
            .extra(self.observation.clone());
        if self.observation.0.values().no_reuse {
            connected.poison();
        }
        connected
    }
}
impl AsyncRead for MeasuredTcp {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = self
            .io
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.buffered.is_empty() {
            let length = state.buffered.len().min(buffer.remaining());
            for _ in 0..length {
                buffer.put_slice(&[state.buffered.pop_front().expect("buffered ciphertext")]);
            }
            return Poll::Ready(Ok(()));
        }
        if state.eof {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut state.io).poll_read(cx, buffer)
    }
}
impl AsyncWrite for MeasuredTcp {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(
            &mut self
                .io
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .io,
        )
        .poll_write(cx, bytes)
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(
            &mut self
                .io
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .io,
        )
        .poll_write_vectored(cx, bytes)
    }
    fn is_write_vectored(&self) -> bool {
        self.io
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .io
            .is_write_vectored()
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(
            &mut self
                .io
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .io,
        )
        .poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(
            &mut self
                .io
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .io,
        )
        .poll_shutdown(cx)
    }
}

#[derive(Clone)]
pub(crate) struct MeasuredHttpsConnector {
    authorization: UpstreamTlsContextFactory,
    inner: HttpsConnector<MeasuredTcpConnector>,
    mode: HyperEgressMode,
    protocols: OutboundProtocolPolicy,
    happy: HappyEyeballsConfig,
    prior_failure: Option<ConnectionObservation>,
}
impl Service<Uri> for MeasuredHttpsConnector {
    type Response = crate::alps::AlpsStream<MaybeHttpsStream<MeasuredTcp>>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }
    fn call(&mut self, uri: Uri) -> Self::Future {
        let allowed = self.protocols.snapshot();
        let admitted = mode_admitted(self.mode, allowed, uri.scheme_str() == Some("https"));
        let mode = self.mode;
        let authorization = self.authorization.clone();
        let host = uri.host().unwrap_or("").trim_matches(['[', ']']).to_owned();
        let port = uri.port_u16().unwrap_or(443);
        let setup = ConnectionSetup::new(allowed);
        setup.values().prior_failure.clone_from(&self.prior_failure);
        let retry_uri = uri.clone();
        let protocols = self.protocols.clone();
        let happy = self.happy;
        if !admitted {
            setup.values().facts.outcome = TransportOutcome::PolicyRejected;
            return Box::pin(async move {
                Err(Box::new(ConnectionFailure {
                    source: Box::new(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "outbound HTTP protocol is disabled for new connections",
                    )),
                    observation: ConnectionObservation(setup),
                }) as BoxError)
            });
        }
        let connection = self.inner.call(uri);
        let scope = setup.clone();
        Box::pin(SETUP.scope(scope, async move {
            let _owner = HandshakeOwner(setup.clone());
            let mut connection = connection;
            let result = std::future::poll_fn(|cx| poll_handshake(connection.as_mut(), &setup, cx))
            .await;
            let stream = match result {
                Ok(stream) => stream,
                Err(source) => {
                    let recovery = metadata_recovery(&setup, mode, happy, protocols);
                    let failure = connection_failed(setup.clone(), source);
                    if let Some(retry) = recovery {
                        return retry?.call(retry_uri).await;
                    }
                    return Err(failure);
                }
            };
            setup.values().facts.outcome = TransportOutcome::Connected;
            if let MaybeHttpsStream::Https(tls) = &stream {
                let identity = setup.values().attempt.as_ref().and_then(transmog_tls::client_authentication::ClientAuthenticationAttempt::selected_identity);
                let reuse = authorization.permits_selected_client_certificate_reuse(&host, port, identity.as_deref());
                let mut values = setup.values();
                values.no_reuse = !reuse;
                if let Some(capture) = &values.tls_capture {
                    capture.completed(tls.ssl(), false);
                }
                values.tls_done = values.tls_begin.map(|_| Instant::now());
                values.facts.tls_micros = values.tls_begin.map(|begin| micros(begin.elapsed()));
                values.facts.tls_version = Some(tls.ssl().version_str().into());
                values.facts.tls_resumed = Some(tls.ssl().session_reused());
                values.facts.cipher = tls
                    .ssl()
                    .current_cipher()
                    .map(|cipher| cipher.name().into());
                values.facts.alpn = tls
                    .ssl()
                    .selected_alpn_protocol()
                    .map(|alpn| String::from_utf8_lossy(alpn).into_owned());
                let negotiated = if tls.ssl().selected_alpn_protocol() == Some(b"h2") {
                    HttpLegVersion::Http2
                } else {
                    HttpLegVersion::Http1
                };
                let mode_matches = match mode {
                    HyperEgressMode::Auto => true,
                    HyperEgressMode::Http1Only => negotiated == HttpLegVersion::Http1,
                    HyperEgressMode::Http2Only => negotiated == HttpLegVersion::Http2,
                };
                if !allowed.permits(negotiated) || !mode_matches {
                    values.facts.outcome = TransportOutcome::PolicyRejected;
                    drop(values);
                    return Err(Box::new(ConnectionFailure {
                        source: Box::new(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "origin did not negotiate an allowed HTTP protocol",
                        )),
                        observation: ConnectionObservation(setup),
                    }) as BoxError);
                }
            }
            let prefix = if let MaybeHttpsStream::Https(tls) = &stream {
                transmog_tls::peer_application_settings(tls.ssl())
                    .map(crate::alps::bootstrap).transpose()
                    .map_err(|error| connection_failed(setup.clone(), Box::new(error)))?
                    .flatten()
            } else { None };
            Ok(crate::alps::AlpsStream::new(stream, prefix))
        }))
    }
}

fn mode_admitted(mode: HyperEgressMode, allowed: AllowedHttpProtocols, https: bool) -> bool {
    match mode {
        HyperEgressMode::Http1Only => allowed.http1,
        HyperEgressMode::Http2Only => allowed.http2,
        HyperEgressMode::Auto => allowed.http1 || (https && allowed.http2),
    }
}

fn metadata_recovery(
    setup: &Arc<ConnectionSetup>,
    mode: HyperEgressMode,
    happy: HappyEyeballsConfig,
    protocols: OutboundProtocolPolicy,
) -> Option<Result<MeasuredHttpsConnector, BoxError>> {
    let factory = {
        let values = setup.values();
        values
            .metadata_factory
            .as_ref()
            .zip(values.tls_capture.as_ref())
            .and_then(|(factory, capture)| factory.trust_anchor_recovery(capture))?
    };
    Some(
        connector(&factory, mode, happy, protocols)
            .map(|mut retry| {
                retry.prior_failure = Some(ConnectionObservation(setup.clone()));
                retry
            })
            .map_err(|error| Box::new(error) as BoxError),
    )
}

fn connection_failed(setup: Arc<ConnectionSetup>, source: BoxError) -> BoxError {
    {
        let mut values = setup.values();
        if let Some(capture) = &values.tls_capture {
            capture.failure_chain(source.as_ref());
        }
        values.tls_done = values.tls_begin.map(|_| Instant::now());
        values.facts.tls_micros = values.tls_begin.map(|begin| micros(begin.elapsed()));
        values.facts.outcome = if values.tls_begin.is_some() {
            TransportOutcome::TlsFailed
        } else if values.dns_failed {
            TransportOutcome::DnsFailed
        } else {
            TransportOutcome::TcpFailed
        };
    }
    Box::new(ConnectionFailure {
        source,
        observation: ConnectionObservation(setup),
    })
}

fn poll_handshake(
    connection: Pin<
        &mut (dyn Future<Output = Result<MaybeHttpsStream<MeasuredTcp>, BoxError>> + Send),
    >,
    setup: &ConnectionSetup,
    cx: &mut Context<'_>,
) -> Poll<Result<MaybeHttpsStream<MeasuredTcp>, BoxError>> {
    let (previous, attempt) = {
        let mut values = setup.values();
        (
            values.waker.replace(cx.waker().clone()),
            values.attempt.clone(),
        )
    };
    drop(previous);
    if let Some(attempt) = attempt {
        attempt.register(cx.waker());
    }
    let result = connection.poll(cx);
    if result.is_pending() {
        observe_paused_peer(setup, cx);
    }
    result
}

pub(crate) fn connector(
    factory: &UpstreamTlsContextFactory,
    mode: HyperEgressMode,
    happy: HappyEyeballsConfig,
    protocols: OutboundProtocolPolicy,
) -> Result<MeasuredHttpsConnector, TrustError> {
    let mut tcp = HttpConnector::new_with_resolver(HappyEyeballsResolver::new(happy));
    tcp.enforce_http(false);
    tcp.set_nodelay(true);
    tcp.set_happy_eyeballs_timeout(Some(happy.attempt_delay()));
    let tls = factory.hyper_connector_builder(mode.alpn())?;
    let mut connector = HttpsConnector::with_connector(MeasuredTcpConnector(tcp), tls)?;
    let authorization = factory.clone();
    let factory = factory.clone();
    let generation = factory.client_certificate_admission_generation();
    connector.set_ssl_callback(move |ssl, uri| {
        let factory = factory.for_connection();
        let _ = SETUP.try_with(|setup| setup.values().metadata_factory = Some(factory.clone()));
        let _ = SETUP.try_with(|setup| setup.values().tls_begin = Some(Instant::now()));
        let allowed = SETUP.try_with(|setup| setup.allowed).unwrap_or_default();
        let alpn = match mode {
            HyperEgressMode::Http1Only => HyperEgressMode::Http1Only.alpn(),
            HyperEgressMode::Http2Only => HyperEgressMode::Http2Only.alpn(),
            HyperEgressMode::Auto => match (allowed.http1, allowed.http2) {
                (true, true) => HyperEgressMode::Auto.alpn(),
                (false, true) => HyperEgressMode::Http2Only.alpn(),
                _ => HyperEgressMode::Http1Only.alpn(),
            },
        };
        ssl.set_alpn_protos(alpn)?;
        if allowed.http2
            && mode != HyperEgressMode::Http1Only
            && factory
                .network_profile()
                .is_none_or(|profile| profile.settings.http2_alps)
        {
            transmog_tls::configure_h2_application_settings(ssl, &[])?;
        }
        let offered_alpn = match alpn {
            b"\x02h2\x08http/1.1" => vec!["h2".into(), "http/1.1".into()],
            b"\x02h2" => vec!["h2".into()],
            _ => vec!["http/1.1".into()],
        };
        let capture = factory.capture(uri.host().map(str::to_owned), offered_alpn);
        capture.attach_ssl(ssl)?;
        let _ = SETUP.try_with(|setup| setup.values().tls_capture = Some(capture.clone()));
        factory.configure_ssl_metadata(ssl).map_err(|error| {
            capture.failure(&error.to_string());
            boring::error::ErrorStack::get()
        })?;
        if let Some(attempt) = factory.client_certificate_attempt(
            uri.host().unwrap_or("").trim_matches(['[', ']']),
            uri.port_u16().unwrap_or(443),
            generation,
            capture.clone(),
        ) {
            // Hyper's own cache overwrites context cache settings. Clear any
            // cached session after it has been applied, before sending ClientHello.
            transmog_tls::client_connections::clear_cached_session(ssl)?;
            attempt.attach_ssl(ssl)?;
            let _ = SETUP.try_with(|setup| {
                let waker = setup.values().waker.clone();
                if let Some(waker) = waker {
                    attempt.register(&waker);
                }
                setup.values().attempt = Some(attempt);
            });
        }
        let _ = SETUP.try_with(|setup| setup.values().tls_capture = Some(capture));
        Ok(())
    });
    Ok(MeasuredHttpsConnector {
        authorization,
        inner: connector,
        mode,
        protocols,
        happy,
        prior_failure: None,
    })
}

struct HandshakeOwner(Arc<ConnectionSetup>);
impl Drop for HandshakeOwner {
    fn drop(&mut self) {
        let (attempt, waker) = {
            let mut values = self.0.values();
            (values.attempt.take(), values.waker.take())
        };
        if let Some(attempt) = attempt {
            attempt.cancel("physical connection handshake ended");
        }
        drop(waker);
    }
}
fn observe_paused_peer(setup: &ConnectionSetup, cx: &mut Context<'_>) {
    let (attempt, tcp) = {
        let values = setup.values();
        (values.attempt.clone(), values.tcp.clone())
    };
    let (Some(attempt), Some(tcp)) = (attempt, tcp.and_then(|tcp| tcp.upgrade())) else {
        return;
    };
    if !attempt.is_waiting() {
        return;
    }
    let mut state = tcp
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut bytes = [0; 16_384];
    let remaining = (1024 * 1024usize).saturating_sub(state.buffered.len());
    let capacity = remaining.min(bytes.len());
    let mut buffer = ReadBuf::new(&mut bytes[..capacity]);
    let reason = if state.buffered.len() >= 1024 * 1024 {
        Some("peer exceeded paused handshake buffer")
    } else {
        match Pin::new(&mut state.io).poll_read(cx, &mut buffer) {
            Poll::Ready(Ok(())) if buffer.filled().is_empty() => {
                state.eof = true;
                Some("peer closed while authentication was pending")
            }
            Poll::Ready(Ok(())) => {
                state.buffered.extend(buffer.filled());
                cx.waker().wake_by_ref();
                None
            }
            Poll::Ready(Err(_)) => Some("peer failed while authentication was pending"),
            Poll::Pending => None,
        }
    };
    drop(state);
    if let Some(reason) = reason {
        attempt.cancel(reason);
    }
}

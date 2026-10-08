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
use transmog_network::{
    HappyEyeballsConfig,
    metrics::{ConnectionMetrics, MeteredIo},
};
use transmog_tls::{TrustError, UpstreamTlsContextFactory};

type BoxError = Box<dyn Error + Send + Sync>;
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);
tokio::task_local! {static SETUP: Arc<ConnectionSetup>;}

#[derive(Debug)]
struct ConnectionSetup {
    id: u64,
    values: Mutex<SetupValues>,
    bytes: Arc<ConnectionMetrics>,
    uses: AtomicU64,
}
#[derive(Debug, Default)]
struct SetupValues {
    facts: TransportObservation,
    tls_begin: Option<Instant>,
    dns_begin: Option<Instant>,
    dns_done: Option<Instant>,
    tcp_begin: Option<Instant>,
    tcp_done: Option<Instant>,
    tls_done: Option<Instant>,
    dns_failed: bool,
}
impl ConnectionSetup {
    fn new() -> Arc<Self> {
        Arc::new(Self {
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
    pub(crate) fn snapshot(
        &self,
        shared: bool,
        performance: Option<&PerformanceRecorder>,
    ) -> TransportObservation {
        let values = self.0.values();
        let mut facts = values.facts.clone();
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
            Ok(TokioIo::new(MeasuredTcp {
                io: MeteredIo::new(socket, setup.bytes.clone()),
                observation: ConnectionObservation(setup),
            }))
        })
    }
}

#[derive(Debug)]
pub(crate) struct MeasuredTcp {
    io: MeteredIo<TcpStream>,
    observation: ConnectionObservation,
}
impl Connection for MeasuredTcp {
    fn connected(&self) -> Connected {
        self.io
            .get_ref()
            .connected()
            .extra(self.observation.clone())
    }
}
impl AsyncRead for MeasuredTcp {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buffer)
    }
}
impl AsyncWrite for MeasuredTcp {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, bytes)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bytes)
    }
    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[derive(Clone)]
pub(crate) struct MeasuredHttpsConnector(HttpsConnector<MeasuredTcpConnector>);
impl Service<Uri> for MeasuredHttpsConnector {
    type Response = MaybeHttpsStream<MeasuredTcp>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }
    fn call(&mut self, uri: Uri) -> Self::Future {
        let connection = self.0.call(uri);
        let setup = ConnectionSetup::new();
        let scope = setup.clone();
        Box::pin(SETUP.scope(scope, async move {
            let stream = match connection.await {
                Ok(stream) => stream,
                Err(source) => {
                    {
                        let mut values = setup.values();
                        values.tls_done = values.tls_begin.map(|_| Instant::now());
                        values.facts.tls_micros =
                            values.tls_begin.map(|begin| micros(begin.elapsed()));
                        values.facts.outcome = if values.tls_begin.is_some() {
                            TransportOutcome::TlsFailed
                        } else if values.dns_failed {
                            TransportOutcome::DnsFailed
                        } else {
                            TransportOutcome::TcpFailed
                        };
                    }
                    return Err(Box::new(ConnectionFailure {
                        source,
                        observation: ConnectionObservation(setup),
                    }) as BoxError);
                }
            };
            setup.values().facts.outcome = TransportOutcome::Connected;
            if let MaybeHttpsStream::Https(tls) = &stream {
                let mut values = setup.values();
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
            }
            Ok(stream)
        }))
    }
}

pub(crate) fn connector(
    factory: &UpstreamTlsContextFactory,
    mode: HyperEgressMode,
    happy: HappyEyeballsConfig,
) -> Result<MeasuredHttpsConnector, TrustError> {
    let mut tcp = HttpConnector::new_with_resolver(HappyEyeballsResolver::new(happy));
    tcp.enforce_http(false);
    tcp.set_happy_eyeballs_timeout(Some(happy.attempt_delay()));
    let tls = factory.hyper_connector_builder(mode.alpn())?;
    let mut connector = HttpsConnector::with_connector(MeasuredTcpConnector(tcp), tls)?;
    connector.set_ssl_callback(|_, _| {
        let _ = SETUP.try_with(|setup| setup.values().tls_begin = Some(Instant::now()));
        Ok(())
    });
    Ok(MeasuredHttpsConnector(connector))
}

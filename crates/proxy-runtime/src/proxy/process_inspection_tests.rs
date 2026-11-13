//! Network-level evidence for the inspection admission boundary.
use super::*;
use std::sync::{Mutex as StdMutex, atomic::AtomicUsize};
use tokio::{io::AsyncWriteExt, sync::oneshot, task::JoinHandle};
use transmog_core::{
    ClientIdentity,
    intercept::{ExchangeInterceptor, HookInitError, NoopInterceptorFactory},
    observe::{BoxObserverFuture, ObserverEvent},
};
use transmog_tls::SystemTrustSource;

struct Identity(ClientIdentity);
impl ClientIdentityResolver for Identity {
    fn resolve(&self, _: SocketAddr, _: SocketAddr) -> ClientIdentity {
        self.0.clone()
    }
}

#[derive(Default)]
struct Calls {
    hooks: AtomicUsize,
    observers: AtomicUsize,
    certificates: AtomicUsize,
    events: StdMutex<Vec<ObserverEvent>>,
}

struct CountingHooks(Arc<Calls>);
impl InterceptorFactory for CountingHooks {
    fn create(
        &self,
        metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        self.0.hooks.fetch_add(1, Ordering::Relaxed);
        NoopInterceptorFactory.create(metadata)
    }
}
struct CountingObserver(Arc<Calls>);
impl Observer for CountingObserver {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        self.0.observers.fetch_add(1, Ordering::Relaxed);
        self.0.events.lock().unwrap().push(event);
        Box::pin(async { Ok(()) })
    }
}
struct NoCertificates(Arc<Calls>);
impl DownstreamCertificateResolver for NoCertificates {
    fn resolve(
        &self,
        _: &EndpointIdentity,
        _: std::time::SystemTime,
    ) -> Result<transmog_tls::IssuedLeaf, CertificateResolverError> {
        self.0.certificates.fetch_add(1, Ordering::Relaxed);
        panic!("uninspected connection requested a MITM certificate");
    }
}

#[derive(Clone)]
struct HeaderDiagnostics(Arc<AtomicUsize>);
const DIAGNOSTIC_HEADER_LIMIT: usize = 128 * 1024 + 123;

pub(super) fn dependency_diagnostic_probe(header_limit: usize) {
    // Hyper's own diagnostics require its opt-in unstable tracing feature.
    // Probe the actual parser/driver task dispatchers under either build choice.
    if header_limit == DIAGNOSTIC_HEADER_LIMIT {
        tracing::trace!(target: "hyper::privacy_regression", private_header = "process-filter-private-header");
    }
}

impl tracing::Subscriber for HeaderDiagnostics {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target().starts_with("hyper")
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct FindPrivateHeader(bool);
        impl tracing::field::Visit for FindPrivateHeader {
            fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0 |= format!("{value:?}").contains("process-filter-private-header");
            }
        }
        let mut visitor = FindPrivateHeader(false);
        event.record(&mut visitor);
        if visitor.0 {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::TRACE)
    }
}

fn named(name: &str) -> ClientIdentity {
    ClientIdentity::LocalProcess {
        pid: 123,
        name: Some(name.into()),
    }
}
fn only(name: &str) -> ProcessInspectionPolicy {
    ProcessInspectionPolicy {
        mode: ProcessInspectionMode::Only,
        include: vec![name.into()],
        exclude: vec![],
    }
}
fn except(name: &str) -> ProcessInspectionPolicy {
    ProcessInspectionPolicy {
        mode: ProcessInspectionMode::Except,
        include: vec![],
        exclude: vec![name.into()],
    }
}

struct Running {
    address: SocketAddr,
    control: ProxyControl,
    calls: Arc<Calls>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), ProxyRuntimeError>>,
}
impl Running {
    async fn start(policy: ProcessInspectionPolicy, identity: ClientIdentity) -> Self {
        Self::with_limits(policy, identity, RuntimeLimits::default()).await
    }
    async fn with_limits(
        policy: ProcessInspectionPolicy,
        identity: ClientIdentity,
        limits: RuntimeLimits,
    ) -> Self {
        Self::with_resolver(policy, Arc::new(Identity(identity)), limits).await
    }
    async fn with_resolver(
        policy: ProcessInspectionPolicy,
        resolver: Arc<dyn ClientIdentityResolver>,
        limits: RuntimeLimits,
    ) -> Self {
        let calls = Arc::new(Calls::default());
        let config = ProxyConfig {
            process_inspection: policy,
            limits,
            ..Default::default()
        };
        let components = ProxyComponents::new(
            InterceptorChainFactory::new(
                vec![InterceptorRegistration::new(
                    "count",
                    Arc::new(CountingHooks(calls.clone())),
                    InterceptorRequirement::Required,
                )],
                config.limits.hooks,
            ),
            Arc::new(NoCertificates(calls.clone())),
        )
        .with_client_identity_resolver(resolver)
        .with_observer(
            Arc::new(CountingObserver(calls.clone())),
            ObserverConfig::default(),
        );
        let server = ProxyServer::bind_with_components(
            config,
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap()),
            components,
        )
        .await
        .unwrap();
        let address = server.local_addr().unwrap();
        let control = server.control();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(server.serve(async {
            let _ = stopped.await;
        }));
        Self {
            address,
            control,
            calls,
            stop,
            task,
        }
    }
    async fn finish(self, inspected: bool) {
        self.finish_observed(inspected, inspected).await;
    }
    async fn finish_observed(self, inspected: bool, observed: bool) {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(self.calls.certificates.load(Ordering::Relaxed), 0);
        if inspected {
            assert!(self.calls.hooks.load(Ordering::Relaxed) > 0);
        } else {
            assert_eq!(self.calls.hooks.load(Ordering::Relaxed), 0);
        }
        if observed {
            assert!(self.calls.observers.load(Ordering::Relaxed) > 0);
        } else {
            assert_eq!(self.calls.observers.load(Ordering::Relaxed), 0);
        }
    }
}

async fn head<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
    timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            bytes.push(stream.read_u8().await.unwrap());
        }
        bytes
    })
    .await
    .unwrap()
}

async fn tunnel(proxy: SocketAddr, destination: SocketAddr) -> TcpStream {
    let mut client = TcpStream::connect(proxy).await.unwrap();
    client.write_all(format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\nSecret: never-captured\r\n\r\n").as_bytes()).await.unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 200"));
    client
}

#[tokio::test]
async fn excluded_connect_preserves_bytes_and_records_only_connection_context() {
    for policy in [only("selected.exe"), except("excluded.exe")] {
        let proxy = Running::start(policy, named("excluded.exe")).await;
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = origin.local_addr().unwrap();
        let bytes = [0x16, 0x03, 0x03, 0, 0xff, 0x80, b'G', 0].repeat(16_384);
        let expected = bytes.clone();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let mut received = Vec::new();
            stream.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, expected);
            stream.write_all(&received).await.unwrap();
        });
        let mut client = tunnel(proxy.address, destination).await;
        client.write_all(&bytes).await.unwrap();
        client.shutdown().await.unwrap();
        let mut received = Vec::new();
        timeout(Duration::from_secs(3), client.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, bytes);
        origin.await.unwrap();
        assert_eq!(proxy.control.unresolved_process_connections(), 0);
        let calls = proxy.calls.clone();
        proxy.finish_observed(false, true).await;
        let events = calls.events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(&event.kind,
            ObserverEventKind::ExchangeStarted { metadata }
                if metadata.client_identity == named("excluded.exe")
        )));
        assert!(events.iter().any(|event| matches!(&event.kind,
            ObserverEventKind::RouteSelected { policy_id, .. }
                if policy_id.as_ref() == "process-bypass-tunnel"
        )));
        assert!(events.iter().any(|event| matches!(&event.kind,
            ObserverEventKind::Completed(completed)
                if completed.request_head.method == "CONNECT"
                    && completed.request_head.headers.iter().next().is_none()
        )));
        assert!(events.iter().any(|event| matches!(&event.kind,
            ObserverEventKind::Performance(performance) if performance.transports.iter().any(|transport|
                transport.tunnel_destination == Some(destination.to_string())
                    && transport.bytes_read == Some(bytes.len() as u64)
                    && transport.bytes_written == Some(bytes.len() as u64)
                    && transport.tls.is_none() && transport.tls_version.is_none())
        )));
        for event in events.iter() {
            match &event.kind {
                ObserverEventKind::RequestHeadObserved { head, .. }
                | ObserverEventKind::RequestHeadFinalized(head) => {
                    assert!(head.headers.iter().next().is_none());
                }
                ObserverEventKind::BodyChunk(_)
                | ObserverEventKind::BodyTrailers(_)
                | ObserverEventKind::HookEffect(_) => {
                    panic!("excluded tunnel retained message content")
                }
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn unresolved_nameless_and_remote_callers_bypass_both_filters() {
    for policy in [only("selected.exe"), except("excluded.exe")] {
        for identity in [
            ClientIdentity::LocalUnknown,
            ClientIdentity::Remote,
            ClientIdentity::LocalProcess {
                pid: 123,
                name: None,
            },
        ] {
            let proxy = Running::start(policy.clone(), identity).await;
            let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let destination = origin.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = origin.accept().await.unwrap();
                let mut bytes = [0; 4];
                stream.read_exact(&mut bytes).await.unwrap();
                assert_eq!(&bytes, b"ping");
                stream.write_all(b"pong").await.unwrap();
            });
            let mut client = tunnel(proxy.address, destination).await;
            client.write_all(b"ping").await.unwrap();
            let mut bytes = [0; 4];
            client.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"pong");
            drop(client);
            origin.await.unwrap();
            assert_eq!(proxy.control.unresolved_process_connections(), 1);
            proxy.finish_observed(false, true).await;
        }
    }
}

#[tokio::test]
async fn excluded_connect_failure_records_context_without_mitm() {
    let proxy = Running::start(except("excluded.exe"), named("excluded.exe")).await;
    let unavailable = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = unavailable.local_addr().unwrap();
    drop(unavailable);
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client
        .write_all(
            format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 502"));
    drop(client);
    let calls = proxy.calls.clone();
    proxy.finish_observed(false, true).await;
    assert!(
        calls
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(&event.kind, ObserverEventKind::Failed(_)))
    );
}

#[tokio::test]
async fn excluded_http_streams_body_and_preserves_connection_bound_authentication() {
    let proxy = Running::start(except("excluded.exe"), named("excluded.exe")).await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        let first = head(&mut stream).await;
        let first = String::from_utf8_lossy(&first).to_ascii_lowercase();
        assert!(first.starts_with("post /auth?secret=kept http/1.1\r\n"));
        assert!(first.contains("authorization: ntlm first"));
        assert!(!first.contains("proxy-authorization"));
        // Reply before the request ends to prove that its body is not buffered.
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"part");
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 5\r\nWWW-Authenticate: NTLM challenge\r\n\r\nfirst").await.unwrap();
        let mut suffix = [0; 4];
        stream.read_exact(&mut suffix).await.unwrap();
        assert_eq!(&suffix, b"done");
        let second = String::from_utf8_lossy(&head(&mut stream).await).to_ascii_lowercase();
        assert!(second.contains("authorization: ntlm second"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nsecond")
            .await
            .unwrap();
    });
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(format!("POST http://{destination}/auth?secret=kept HTTP/1.1\r\nHost: {destination}\r\nContent-Length: 8\r\nAuthorization: NTLM first\r\nProxy-Authorization: secret\r\n\r\npart").as_bytes()).await.unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 401"));
    let mut first = [0; 5];
    client.read_exact(&mut first).await.unwrap();
    assert_eq!(&first, b"first");
    client.write_all(format!("doneGET http://{destination}/auth HTTP/1.1\r\nHost: {destination}\r\nAuthorization: NTLM second\r\n\r\n").as_bytes()).await.unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 200"));
    let mut second = [0; 6];
    client.read_exact(&mut second).await.unwrap();
    assert_eq!(&second, b"second");
    drop(client);
    origin.await.unwrap();
    proxy.finish(false).await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Preserve the early-response, upload-backpressure and drain ordering in one network flow.
async fn excluded_early_empty_response_keeps_upload_active_until_origin_flush() {
    const SUFFIX_BYTES: usize = 32 * 1024 * 1024;
    let proxy = Running::start(except("excluded.exe"), named("excluded.exe")).await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let origin_release = release.clone();
    let origin_task = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        head(&mut stream).await;
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"part");
        stream
            .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        origin_release.notified().await;
        let mut remaining = SUFFIX_BYTES;
        let mut chunk = vec![0; 64 * 1024];
        while remaining != 0 {
            let count = stream
                .read(&mut chunk[..remaining.min(64 * 1024)])
                .await
                .unwrap();
            assert_ne!(
                count, 0,
                "drain truncated the admitted upload after its early response"
            );
            assert!(chunk[..count].iter().all(|byte| *byte == b'x'));
            remaining -= count;
        }
    });
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(format!(
        "POST http://{destination}/early HTTP/1.1\r\nHost: {destination}\r\nContent-Length: {}\r\n\r\npart", SUFFIX_BYTES + 4
    ).as_bytes()).await.unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 401"));
    let generation = proxy.control.begin_drain();
    assert!(
        proxy.control.activity().transports > 0,
        "an unfinished upload needs its own activity after the response ends"
    );
    assert!(!proxy.control.finish_drain(generation));
    let uploader = tokio::spawn(async move {
        client.write_all(&vec![b'x'; SUFFIX_BYTES]).await.unwrap();
        client.shutdown().await.unwrap();
        client
    });
    assert!(
        timeout(
            Duration::from_millis(100),
            proxy.control.wait_for_drain(generation)
        )
        .await
        .is_err(),
        "origin backpressure must keep drain pending rather than dropping buffered upload bytes"
    );
    release.notify_one();
    timeout(Duration::from_secs(10), origin_task)
        .await
        .unwrap()
        .unwrap();
    let client = timeout(Duration::from_secs(10), uploader)
        .await
        .unwrap()
        .unwrap();
    assert!(
        timeout(
            Duration::from_secs(3),
            proxy.control.wait_for_drain(generation)
        )
        .await
        .unwrap()
    );
    drop(client);
    proxy.finish(false).await;
}

#[tokio::test]
async fn excluded_early_response_upload_activity_releases_on_origin_error() {
    let proxy = Running::start(except("excluded.exe"), named("excluded.exe")).await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        head(&mut stream).await;
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        // This origin refuses to consume the remaining upload and closes.
    });
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(format!("POST http://{destination}/early-error HTTP/1.1\r\nHost: {destination}\r\nContent-Length: 1000000\r\n\r\npart").as_bytes()).await.unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 401"));
    origin_task.await.unwrap();
    let generation = proxy.control.begin_drain();
    assert!(
        timeout(
            Duration::from_secs(3),
            proxy.control.wait_for_drain(generation)
        )
        .await
        .unwrap(),
        "a failed origin upload must release its independent activity"
    );
    drop(client);
    proxy.finish(false).await;
}

#[tokio::test]
async fn excluded_pending_upload_activity_does_not_block_forced_shutdown() {
    let proxy = Running::with_limits(
        except("excluded.exe"),
        named("excluded.exe"),
        RuntimeLimits {
            shutdown_timeout: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let proxy_address = proxy.address;
    let client_task = tokio::spawn(async move {
        let mut client = TcpStream::connect(proxy_address).await.unwrap();
        client.write_all(format!("POST http://{destination}/early-stop HTTP/1.1\r\nHost: {destination}\r\nContent-Length: 1000000\r\n\r\npart").as_bytes()).await.unwrap();
        assert!(head(&mut client).await.starts_with(b"HTTP/1.1 401"));
        client
    });
    let (mut stream, _) = origin.accept().await.unwrap();
    head(&mut stream).await;
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).await.unwrap();
    stream
        .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    let client = client_task.await.unwrap();
    let control = proxy.control.clone();
    assert!(control.activity().transports > 0);
    proxy.finish(false).await;
    assert_eq!(
        control.activity().transports,
        0,
        "forced shutdown retained an upload monitor"
    );
    assert_eq!(
        timeout(Duration::from_secs(1), stream.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    drop(client);
}

#[tokio::test]
async fn excluded_http_keeps_healthy_uploads_past_the_response_header_deadline() {
    let proxy = Running::with_limits(
        only("selected.exe"),
        named("excluded.exe"),
        RuntimeLimits {
            header_read_timeout: Duration::from_millis(100),
            body_idle_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    )
    .await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        head(&mut stream).await;
        let mut body = [0; 6];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"abcdef");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(format!("POST http://{destination}/upload HTTP/1.1\r\nHost: {destination}\r\nContent-Length: 6\r\n\r\n").as_bytes()).await.unwrap();
    for byte in b"abcdef" {
        client.write_all(&[*byte]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 200"));
    drop(client);
    origin.await.unwrap();
    proxy.finish(false).await;
}

#[tokio::test]
async fn selected_and_default_unknown_callers_keep_normal_inspection() {
    for (policy, identity) in [
        (only("selected.exe"), named("selected.exe")),
        (except("excluded.exe"), named("selected.exe")),
        (
            ProcessInspectionPolicy::default(),
            ClientIdentity::LocalUnknown,
        ),
    ] {
        let proxy = Running::start(policy, identity).await;
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = origin.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            head(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let mut client = TcpStream::connect(proxy.address).await.unwrap();
        client
            .write_all(
                format!("GET http://{destination}/ HTTP/1.1\r\nHost: {destination}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        assert!(head(&mut client).await.starts_with(b"HTTP/1.1 200"));
        drop(client);
        origin.await.unwrap();
        assert_eq!(proxy.control.unresolved_process_connections(), 0);
        proxy.finish(true).await;
    }
}

#[tokio::test]
async fn excluded_http_preserves_arbitrary_upgrade_bytes() {
    let proxy = Running::start(only("selected.exe"), named("excluded.exe")).await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        let headers = String::from_utf8_lossy(&head(&mut stream).await).to_ascii_lowercase();
        assert!(headers.contains("upgrade: custom-protocol"));
        stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: custom-protocol\r\n\r\norigin-prefix").await.unwrap();
        let mut bytes = [0; 4];
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"\x00\xffab");
        stream.write_all(&bytes).await.unwrap();
    });
    let mut client = TcpStream::connect(proxy.address).await.unwrap();
    client.write_all(format!("GET http://{destination}/upgrade HTTP/1.1\r\nHost: {destination}\r\nConnection: Upgrade\r\nUpgrade: custom-protocol\r\n\r\n\x00").as_bytes()).await.unwrap();
    client.write_all(b"\xffab").await.unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 101"));
    let mut prefix = [0; 13];
    client.read_exact(&mut prefix).await.unwrap();
    assert_eq!(&prefix, b"origin-prefix");
    let mut bytes = [0; 4];
    client.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"\x00\xffab");
    drop(client);
    origin.await.unwrap();
    proxy.finish(false).await;
}

#[tokio::test]
async fn policy_updates_only_change_new_connections_and_invalid_updates_are_atomic() {
    let proxy = Running::start(except("excluded.exe"), named("excluded.exe")).await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        for responses in [2, 1] {
            let (mut stream, _) = origin.accept().await.unwrap();
            for _ in 0..responses {
                head(&mut stream).await;
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();
            }
        }
    });
    let mut existing = TcpStream::connect(proxy.address).await.unwrap();
    let request = format!("GET http://{destination}/ HTTP/1.1\r\nHost: {destination}\r\n\r\n");
    existing.write_all(request.as_bytes()).await.unwrap();
    head(&mut existing).await;
    assert_eq!(proxy.calls.hooks.load(Ordering::Relaxed), 0);
    let previous = proxy.control.process_inspection();
    assert!(
        proxy
            .control
            .set_process_inspection(ProcessInspectionPolicy {
                mode: ProcessInspectionMode::Only,
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(proxy.control.process_inspection(), previous);
    proxy
        .control
        .set_process_inspection(ProcessInspectionPolicy::default())
        .unwrap();
    existing.write_all(request.as_bytes()).await.unwrap();
    head(&mut existing).await;
    assert_eq!(proxy.calls.hooks.load(Ordering::Relaxed), 0);
    drop(existing);
    let mut new = TcpStream::connect(proxy.address).await.unwrap();
    new.write_all(request.as_bytes()).await.unwrap();
    head(&mut new).await;
    drop(new);
    origin.await.unwrap();
    proxy.finish(true).await;
}

#[tokio::test]
async fn excluded_tunnels_participate_in_drain_and_shutdown() {
    let proxy = Running::start(except("excluded.exe"), named("excluded.exe")).await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let mut client = tunnel(proxy.address, destination).await;
    let (mut server, _) = origin.accept().await.unwrap();
    let generation = proxy.control.begin_drain();
    assert!(proxy.control.activity().transports > 0);
    assert!(!proxy.control.finish_drain(generation));
    client.write_all(b"live").await.unwrap();
    let mut bytes = [0; 4];
    server.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"live");
    assert!(proxy.control.resume_drain(generation));
    proxy.finish_observed(false, true).await;
    assert_eq!(
        timeout(Duration::from_secs(1), client.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

#[tokio::test]
async fn excluded_malformed_headers_never_reach_verbose_dependency_logs() {
    let diagnostics = Arc::new(AtomicUsize::new(0));
    tracing::subscriber::set_global_default(HeaderDiagnostics(diagnostics.clone())).unwrap();
    let limits = RuntimeLimits {
        max_header_bytes: DIAGNOSTIC_HEADER_LIMIT,
        ..Default::default()
    };
    let header = format!("process-filter-private-header{}", "x".repeat(65_536));
    // Prove the collector receives the dependency-diagnostic probe for the same
    // malformed input on a selected connection. Native Hyper diagnostics are
    // included automatically when its unstable tracing feature is enabled.
    let selected = Running::with_limits(
        ProcessInspectionPolicy::default(),
        named("selected.exe"),
        limits.clone(),
    )
    .await;
    let mut client = TcpStream::connect(selected.address).await.unwrap();
    client
        .write_all(
            format!(
                "GET http://127.0.0.1/ HTTP/1.1\r\nHost: 127.0.0.1\r\n{header}: private\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 431"));
    drop(client);
    selected.finish(false).await;
    assert!(
        diagnostics.swap(0, Ordering::Relaxed) > 0,
        "control input must exercise the parser's verbose diagnostic scope"
    );

    let excluded = Running::with_limits(only("selected.exe"), named("excluded.exe"), limits).await;
    let mut client = TcpStream::connect(excluded.address).await.unwrap();
    client
        .write_all(
            format!(
                "GET http://127.0.0.1/ HTTP/1.1\r\nHost: 127.0.0.1\r\n{header}: private\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 431"));
    drop(client);
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        head(&mut stream).await;
        stream
            .write_all(
                format!("HTTP/1.1 200 OK\r\n{header}: private\r\nContent-Length: 0\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
    });
    let mut client = TcpStream::connect(excluded.address).await.unwrap();
    client
        .write_all(
            format!("GET http://{destination}/ HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    assert!(head(&mut client).await.starts_with(b"HTTP/1.1 502"));
    drop(client);
    origin.await.unwrap();
    excluded.finish(false).await;
    assert_eq!(
        diagnostics.load(Ordering::Relaxed),
        0,
        "excluded downstream and spawned origin parsers must suppress identifying header diagnostics"
    );
}

#[tokio::test]
async fn policy_snapshot_precedes_a_delayed_identity_lookup() {
    struct DelayedIdentity {
        entered: Arc<tokio::sync::Notify>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        resolutions: AtomicUsize,
    }
    impl ClientIdentityResolver for DelayedIdentity {
        fn resolve(&self, _: SocketAddr, _: SocketAddr) -> ClientIdentity {
            if self.resolutions.fetch_add(1, Ordering::Relaxed) == 0 {
                self.entered.notify_one();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            named("excluded.exe")
        }
    }
    let entered = Arc::new(tokio::sync::Notify::new());
    let (release, pending) = std::sync::mpsc::channel();
    let proxy = Running::with_resolver(
        except("excluded.exe"),
        Arc::new(DelayedIdentity {
            entered: entered.clone(),
            release: std::sync::Mutex::new(pending),
            resolutions: AtomicUsize::new(0),
        }),
        RuntimeLimits::default(),
    )
    .await;
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = origin.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = origin.accept().await.unwrap();
            head(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        }
    });
    let mut old = TcpStream::connect(proxy.address).await.unwrap();
    timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    proxy
        .control
        .set_process_inspection(ProcessInspectionPolicy::default())
        .unwrap();
    release.send(()).unwrap();
    let request = format!("GET http://{destination}/ HTTP/1.1\r\nHost: {destination}\r\n\r\n");
    old.write_all(request.as_bytes()).await.unwrap();
    assert!(head(&mut old).await.starts_with(b"HTTP/1.1 200"));
    drop(old);
    assert_eq!(proxy.calls.hooks.load(Ordering::Relaxed), 0);
    let mut new = TcpStream::connect(proxy.address).await.unwrap();
    new.write_all(request.as_bytes()).await.unwrap();
    assert!(head(&mut new).await.starts_with(b"HTTP/1.1 200"));
    drop(new);
    origin.await.unwrap();
    proxy.finish(true).await;
}

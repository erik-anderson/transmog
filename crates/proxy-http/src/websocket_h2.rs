//! RFC 8441 origin streams. Weak pool entries live only while streams are in use.
use super::{
    BodyFrame, BodyStream, BodyStreamError, Bytes, ConnectionObservation, Context, Duration,
    HttpLegVersion, HyperEgressMode, HyperOriginClient, HyperOriginError, HyperUpgradeResponse,
    MessageKind, Milestone, NonZeroUsize, Pin, Poll, ProtocolObservation, Request, RequestHead,
    ResponseHead, ResponseObservation, StreamingResponse, TranslationOptions, Version, WebSocketIo,
    append_headers, block_from_headers, prepare_headers, timeout,
};
use hyper_util::{client::legacy::connect::Connection, rt::TokioIo};
use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    task::ready,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tower_service::Service;

type Lease = Arc<tokio::sync::Mutex<Option<Peer>>>;
type Peers = HashMap<(String, u64), Weak<tokio::sync::Mutex<Option<Peer>>>>;

#[derive(Clone, Default)]
pub(super) struct Pool(Arc<Mutex<Peers>>);

struct Driver(Option<tokio::task::JoinHandle<()>>);
impl Drop for Driver {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}
struct Peer {
    sender: h2::client::SendRequest<Bytes>,
    observation: Option<ConnectionObservation>,
    reusable: bool,
    active: Arc<AtomicUsize>,
}

struct StreamSlot(Arc<AtomicUsize>);
impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Pool {
    fn lease(&self, client: &HyperOriginClient, uri: &http::Uri) -> Lease {
        let reuse = uri.scheme_str() != Some("https")
            || client.factory.permits_client_certificate_reuse(
                uri.host().unwrap_or("").trim_matches(['[', ']']),
                uri.port_u16().unwrap_or(443),
            );
        if !reuse {
            return Arc::new(tokio::sync::Mutex::new(None));
        }
        let key = (
            format!(
                "{}://{}",
                uri.scheme_str().unwrap_or("http"),
                uri.authority().map_or("", http::uri::Authority::as_str)
            ),
            client.factory.client_certificate_generation(),
        );
        let mut peers = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        peers.retain(|_, peer| peer.strong_count() != 0);
        if let Some(peer) = peers.get(&key).and_then(Weak::upgrade) {
            return peer;
        }
        let peer = Arc::new(tokio::sync::Mutex::new(None));
        peers.insert(key, Arc::downgrade(&peer));
        peer
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) async fn execute(
        &self,
        client: &HyperOriginClient,
        request: &RequestHead,
        mode: HyperEgressMode,
        max_response_bytes: usize,
        capacity: NonZeroUsize,
        idle: Duration,
    ) -> Result<HyperUpgradeResponse, HyperOriginError> {
        let uri: http::Uri = format!(
            "{}://{}{}{}",
            request.target.scheme,
            request.target.authority,
            request.target.path,
            request
                .target
                .query
                .as_ref()
                .map(|q| format!("?{q}"))
                .unwrap_or_default()
        )
        .parse()?;
        let mut lease = self.lease(client, &uri);
        let mut locked = lease.lock().await;
        // Authorization of the selected certificate can be more restrictive
        // than the origin-wide decision made before the TLS handshake.
        if locked.as_ref().is_some_and(|peer| {
            !peer.reusable
                || peer.active.load(Ordering::Acquire) >= peer.sender.current_max_send_streams()
        }) {
            // A WebSocket can occupy a peer's stream slot indefinitely. Open an
            // overflow connection rather than queueing its successor behind it.
            drop(locked);
            lease = Arc::new(tokio::sync::Mutex::new(None));
            locked = lease.lock().await;
        }
        if let Some(peer) = locked.as_mut()
            && std::future::poll_fn(|cx| peer.sender.poll_ready(cx))
                .await
                .is_err()
        {
            *locked = None;
        }
        if locked.is_none() {
            *locked = Some(connect(client, uri.clone(), mode).await?);
        }
        let peer = locked.as_mut().expect("connected peer");
        std::future::poll_fn(|cx| peer.sender.poll_ready(cx)).await?;
        let mut headers = prepare_headers(
            &request.headers,
            TranslationOptions {
                destination: HttpLegVersion::Http2,
                kind: MessageKind::Request,
                body_modified: false,
                force_identity_encoding: false,
            },
        )?;
        for name in [
            "host",
            "sec-websocket-key",
            "sec-websocket-accept",
            "content-length",
        ] {
            headers.remove_all(name);
        }
        let mut outgoing = Request::builder()
            .method("CONNECT")
            .uri(uri)
            .version(Version::HTTP_2)
            .body(())?;
        append_headers(outgoing.headers_mut(), &headers)?;
        outgoing
            .extensions_mut()
            .insert(h2::ext::Protocol::from_static("websocket"));
        let observation = peer.observation.clone().and_then(|connection| {
            let shared = connection.claim();
            client.performance.as_ref().map(|performance| {
                performance.transport(connection.snapshot(shared, Some(performance)));
                performance.mark(Milestone::UpstreamConnected);
                performance.protocol(ProtocolObservation {
                    boundary: "upstream-request".into(),
                    version: "HTTP/2".into(),
                    reason: Some("WebSocket extended CONNECT".into()),
                });
                ResponseObservation {
                    connection,
                    shared,
                    performance: performance.clone(),
                }
            })
        });
        let (response, send) = peer.sender.send_request(outgoing, false)?;
        peer.active.fetch_add(1, Ordering::AcqRel);
        let slot = StreamSlot(peer.active.clone());
        if let Some(performance) = &client.performance {
            performance.mark(Milestone::UpstreamRequestConsumed);
        }
        drop(locked);
        let response = response.await?;
        if let Some(performance) = &client.performance {
            performance.mark(Milestone::ResponseHeaders);
            performance.protocol(ProtocolObservation {
                boundary: "upstream-response".into(),
                version: "HTTP/2".into(),
                reason: Some("WebSocket extended CONNECT".into()),
            });
        }
        let head = ResponseHead {
            status: response.status().as_u16(),
            headers: block_from_headers(response.headers())?,
            source_version: HttpLegVersion::Http2,
        };
        let stream = Tunnel {
            send,
            recv: response.into_body(),
            buffered: Bytes::new(),
            send_closed: false,
            recv_closed: false,
            _lease: lease,
            _observation: observation,
            _slot: slot,
        };
        if (200..300).contains(&head.status) {
            return Ok(HyperUpgradeResponse::Switched {
                head,
                request: RequestHead {
                    method: "CONNECT".into(),
                    source_version: HttpLegVersion::Http2,
                    headers,
                    ..request.clone()
                },
                upgraded: Box::pin(async { Ok(Box::new(stream) as Box<dyn WebSocketIo>) }),
            });
        }
        let (sender, body) = BodyStream::channel(capacity);
        tokio::spawn(relay_rejection(stream, sender, max_response_bytes, idle));
        Ok(HyperUpgradeResponse::Rejected(
            StreamingResponse { head, body },
            RequestHead {
                method: "CONNECT".into(),
                source_version: HttpLegVersion::Http2,
                headers,
                ..request.clone()
            },
        ))
    }
}

async fn relay_rejection(
    mut stream: Tunnel,
    sender: transmog_core::BodyStreamSender,
    max_response_bytes: usize,
    idle: Duration,
) {
    // A rejected CONNECT is an ordinary bounded response, never a WebSocket.
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
    let result = async {
        let mut total = 0usize;
        while let Some(bytes) = timeout(idle, stream.recv.data())
            .await
            .map_err(|_| "rejected WebSocket response timed out".to_owned())?
        {
            let bytes = bytes.map_err(|e| e.to_string())?;
            let len = bytes.len();
            total = total.saturating_add(len);
            if total > max_response_bytes {
                return Err("rejected WebSocket response exceeds body limit".into());
            }
            stream
                .recv
                .flow_control()
                .release_capacity(len)
                .map_err(|e| e.to_string())?;
            sender
                .send(Ok(BodyFrame::Data(bytes)))
                .await
                .map_err(|e| e.to_string())?;
        }
        if let Some(trailers) = timeout(idle, stream.recv.trailers())
            .await
            .map_err(|_| "rejected WebSocket trailers timed out".to_owned())?
            .map_err(|e| e.to_string())?
        {
            sender
                .send(Ok(BodyFrame::Trailers(
                    block_from_headers(&trailers).map_err(|e| e.to_string())?,
                )))
                .await
                .map_err(|e| e.to_string())?;
        }
        stream.recv_closed = true;
        Ok::<_, String>(())
    }
    .await;
    if let Err(message) = result {
        let _ = sender.send(Err(BodyStreamError::Failed(message))).await;
    }
}

async fn connect(
    client: &HyperOriginClient,
    uri: http::Uri,
    mode: HyperEgressMode,
) -> Result<Peer, HyperOriginError> {
    let admitted = client
        .factory
        .clone()
        .for_client_certificate_epoch(client.factory.client_certificate_generation());
    let mut connector = crate::metrics::connector(
        &admitted,
        mode,
        client.happy_eyeballs,
        client.protocols.clone(),
    )?;
    let io = connector.call(uri.clone()).await.map_err(|error| {
        if let Some(performance) = &client.performance
            && let Some(facts) = crate::metrics::failed_observation(error.as_ref(), performance)
        {
            performance.transport(facts);
        }
        HyperOriginError::Transport(error)
    })?;
    let connected = io.connected();
    if uri.scheme_str() == Some("https") && !connected.is_negotiated_h2() {
        return Err(HyperOriginError::ExtendedConnectUnsupported);
    }
    let mut extras = http::Extensions::new();
    connected.get_extras(&mut extras);
    let observation = extras.get::<ConnectionObservation>().cloned();
    let (sender, mut connection) = h2::client::handshake(TokioIo::new(io)).await?;
    let mut ping = connection.ping_pong().expect("first ping handle");
    let mut driver = Driver(Some(tokio::spawn(async move {
        let _ = connection.await;
    })));
    // A PING acknowledgement follows the mandatory initial SETTINGS. This
    // establishes capability without sending an application request or guessing
    // how long to wait for SETTINGS. The caller bounds the entire handshake.
    ping.ping(h2::Ping::opaque()).await?;
    if !sender.is_extended_connect_protocol_enabled() {
        return Err(HyperOriginError::ExtendedConnectUnsupported);
    }
    drop(driver.0.take()); // Last stream/sender release lets h2 flush and close naturally.
    let reusable = observation
        .as_ref()
        .is_none_or(ConnectionObservation::permits_reuse);
    Ok(Peer {
        sender,
        observation,
        reusable,
        active: Arc::new(AtomicUsize::new(0)),
    })
}

struct Tunnel {
    send: h2::SendStream<Bytes>,
    recv: h2::RecvStream,
    buffered: Bytes,
    send_closed: bool,
    recv_closed: bool,
    _lease: Lease,
    _observation: Option<ResponseObservation>,
    _slot: StreamSlot,
}
impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        while this.buffered.is_empty() && !this.recv_closed {
            match ready!(this.recv.poll_data(cx)) {
                Some(Ok(bytes)) => this.buffered = bytes,
                Some(Err(error)) => return Poll::Ready(Err(io::Error::other(error))),
                None => this.recv_closed = true,
            }
        }
        let len = output.remaining().min(this.buffered.len());
        output.put_slice(&this.buffered.split_to(len));
        this.recv
            .flow_control()
            .release_capacity(len)
            .map_err(io::Error::other)?;
        Poll::Ready(Ok(()))
    }
}
impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if this.send_closed {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        this.send.reserve_capacity(bytes.len().min(16 * 1024));
        loop {
            match ready!(this.send.poll_capacity(cx)) {
                Some(Ok(0)) => {}
                Some(Ok(capacity)) => {
                    let len = capacity.min(bytes.len()).min(16 * 1024);
                    this.send
                        .send_data(Bytes::copy_from_slice(&bytes[..len]), false)
                        .map_err(io::Error::other)?;
                    return Poll::Ready(Ok(len));
                }
                Some(Err(error)) => return Poll::Ready(Err(io::Error::other(error))),
                None => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.send_closed {
            this.send
                .send_data(Bytes::new(), true)
                .map_err(io::Error::other)?;
            this.send_closed = true;
        }
        Poll::Ready(Ok(()))
    }
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        if !self.send_closed {
            self.send.send_reset(h2::Reason::CANCEL);
        }
    }
}

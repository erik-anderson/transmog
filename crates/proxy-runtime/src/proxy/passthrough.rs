//! Uninspected forwarding: protocol framing without inspection components.
//!
//! This HTTP path never constructs hook contexts, capture records or certificate
//! requests. CONNECT uses the separate metadata-only tunnel path. HTTP/1 senders
//! belong to one downstream connection so that connection-bound authentication
//! cannot cross callers or inspection decisions.

use super::{
    Arc, Body, BodyStreamError, Bytes, ConnectAuthority, ConnectionContext, Context,
    DownstreamBody, DownstreamBodyInner, Duration, Frame, HashMap, HeaderMap, HeaderValue,
    Incoming, Mutex, Pin, Poll, ProxyRuntimeError, ProxyState, Request, Response, StatusCode,
    TcpStream, TokioIo, TransportObservation, Version, connect_tunnel, target_from_absolute_uri,
    timeout,
};
use http::header::{CONNECTION, HOST, PROXY_AUTHORIZATION, UPGRADE};
use hyper::client::conn::http1::SendRequest;
use std::future::Future;
use tracing::instrument::WithSubscriber;

const MAX_ORIGINS_PER_CONNECTION: usize = 32;
type OriginSender = Arc<Mutex<SendRequest<IncomingBody>>>;
pub(super) type ConnectionPool = Arc<Mutex<HashMap<String, OriginSender>>>;

/// A direct Hyper body with backpressure and the normal runtime idle deadline.
pub(super) struct IncomingBody {
    inner: Incoming,
    idle: Pin<Box<tokio::time::Sleep>>,
    idle_timeout: Duration,
    uploaded: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Sender<()>,
    )>,
}

impl IncomingBody {
    fn new(inner: Incoming, idle_timeout: Duration) -> Self {
        Self {
            inner,
            idle: Box::pin(tokio::time::sleep(idle_timeout)),
            idle_timeout,
            uploaded: None,
        }
    }

    fn notifying(
        inner: Incoming,
        idle_timeout: Duration,
    ) -> (
        Self,
        tokio::sync::oneshot::Receiver<()>,
        Option<tokio::sync::oneshot::Receiver<()>>,
    ) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut body = Self::new(inner, idle_timeout);
        if body.is_end_stream() {
            let _ = sender.send(());
            (body, receiver, None)
        } else {
            let (completed, completion) = tokio::sync::oneshot::channel();
            body.uploaded = Some((sender, completed));
            (body, receiver, Some(completion))
        }
    }
}

impl Body for IncomingBody {
    type Data = Bytes;
    type Error = BodyStreamError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BodyStreamError>>> {
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(frame) => {
                if (self.inner.is_end_stream() || frame.is_none())
                    && let Some((sender, completed)) = self.uploaded.take()
                {
                    let _ = sender.send(());
                    let _ = completed.send(());
                }
                let next = tokio::time::Instant::now() + self.idle_timeout;
                self.idle.as_mut().reset(next);
                Poll::Ready(frame.map(|result| {
                    result.map_err(|_| {
                        BodyStreamError::Failed("Uninspected HTTP body transfer failed.".into())
                    })
                }))
            }
            Poll::Pending if self.idle.as_mut().poll(cx).is_ready() => Poll::Ready(Some(Err(
                BodyStreamError::Failed("HTTP body exceeded its configured idle timeout".into()),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

impl ProxyState {
    pub(super) async fn handle_uninspected_request(
        self: Arc<Self>,
        request: Request<Incoming>,
        context: ConnectionContext,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        self.forward_uninspected_http(request, context).await
    }

    async fn connect_uninspected(
        &self,
        authority: &ConnectAuthority,
    ) -> Result<TcpStream, ProxyRuntimeError> {
        // The common connector's temporary timing scratch is never published.
        let mut scratch = TransportObservation::default();
        tokio::select! {
            result = timeout(self.config.limits.tls_handshake_timeout, connect_tunnel::connect_destination(authority, self.config.happy_eyeballs, &mut scratch)) => {
                Ok(result.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Destination connection timed out"))??)
            }
            () = self.final_shutdown.cancelled() => Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Proxy stopped").into()),
        }
    }

    #[allow(clippy::too_many_lines)] // Keep forwarding, upload ownership and upgrade cleanup ordering together.
    async fn forward_uninspected_http(
        self: Arc<Self>,
        mut request: Request<Incoming>,
        context: ConnectionContext,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let target = target_from_absolute_uri(&request)?;
        let upgrade = request.headers().contains_key(UPGRADE)
            && header_has_token(request.headers(), CONNECTION, "upgrade");
        let downstream_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));
        let mut parts = request.uri().clone().into_parts();
        parts.scheme = None;
        parts.authority = None;
        *request.uri_mut() = http::Uri::from_parts(parts).map_err(http::Error::from)?;
        // HTTP proxy credentials and hop-specific options must not reach the origin.
        sanitize_headers(request.headers_mut(), upgrade)?;
        request
            .headers_mut()
            .insert(HOST, HeaderValue::from_str(&target.authority)?);
        *request.version_mut() = Version::HTTP_11;
        let (parts, body) = request.into_parts();
        let (body, uploaded, upload_completion) =
            IncomingBody::notifying(body, self.config.limits.body_idle_timeout);
        let upload =
            upload_completion.map(|completion| (completion, self.activity.continue_transport()));
        let request = Request::from_parts(parts, body);
        let mut pool = context.passthrough_pool.lock().await;
        pool.retain(|_, sender| match sender.try_lock() {
            Ok(sender) => !sender.is_closed(),
            Err(_) => true,
        });
        if !pool.contains_key(&target.authority) {
            if pool.len() >= MAX_ORIGINS_PER_CONNECTION {
                // A bound on private idle origins prevents unbounded state on a
                // long-lived proxy connection. Active response bodies keep their IO.
                if let Some(idle) = pool.keys().next().cloned() {
                    pool.remove(&idle);
                }
            }
            let authority = super::format_authority(&target.host, target.port, 0).parse()?;
            let origin = self.connect_uninspected(&authority).await?;
            origin.set_nodelay(true)?;
            let (sender, connection) = hyper::client::conn::http1::Builder::new()
                .preserve_header_case(true)
                .max_headers(self.config.limits.max_header_count)
                .max_buf_size(self.config.limits.max_header_bytes)
                .handshake(TokioIo::new(origin))
                .await?;
            let state = self.clone();
            let mut tasks = self.tunnels.lock().await;
            while tasks.try_join_next().is_some() {}
            tasks.spawn(
                async move {
                    #[cfg(test)]
                    super::process_inspection_tests::dependency_diagnostic_probe(
                        state.config.limits.max_header_bytes,
                    );
                    tokio::select! {
                        _ = connection.with_upgrades() => {},
                        () = state.final_shutdown.cancelled() => {},
                    }
                }
                .with_subscriber(tracing::subscriber::NoSubscriber::default()),
            );
            pool.insert(target.authority.clone(), Arc::new(Mutex::new(sender)));
        }
        let sender = pool
            .get(&target.authority)
            .expect("origin inserted")
            .clone();
        let mut sending = sender.lock().await;
        let mut response = self
            .send_uninspected_http(&mut sending, request, uploaded)
            .await?;
        if let Some((mut completion, activity)) = upload {
            let consumed = !matches!(
                completion.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            );
            if consumed && sending.is_ready() {
                drop(activity);
            } else {
                self.track_uninspected_upload(sender.clone(), completion, consumed, activity)
                    .await;
            }
        }
        drop(sending);
        if response.status() == StatusCode::SWITCHING_PROTOCOLS && !upgrade {
            pool.remove(&target.authority);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unsolicited destination protocol switch",
            )
            .into());
        }
        if response.status() == StatusCode::SWITCHING_PROTOCOLS && upgrade {
            pool.remove(&target.authority);
            drop(pool);
            let origin_upgrade = hyper::upgrade::on(&mut response);
            let activity = self.activity.continue_transport();
            self.spawn_uninspected_relay(
                downstream_upgrade.expect("upgrade requested"),
                async move {
                    origin_upgrade
                        .await
                        .map(TokioIo::new)
                        .map_err(std::io::Error::other)
                },
                activity,
            )
            .await;
            sanitize_headers(response.headers_mut(), true)?;
            return Ok(response.map(|_| DownstreamBody::empty()));
        }
        if header_has_token(response.headers(), CONNECTION, "close")
            || !self.outbound_protocols.reuse_connections()
        {
            pool.remove(&target.authority);
        }
        drop(pool);
        sanitize_headers(response.headers_mut(), false)?;
        Ok(response.map(|body| DownstreamBody {
            activity: None,
            manual: None,
            inner: DownstreamBodyInner::Uninspected(Box::new(IncomingBody::new(
                body,
                self.config.limits.body_idle_timeout,
            ))),
        }))
    }

    async fn send_uninspected_http(
        &self,
        sender: &mut SendRequest<IncomingBody>,
        request: Request<IncomingBody>,
        uploaded: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<Response<Incoming>, ProxyRuntimeError> {
        // Never replay a failed send: the origin may already have received bytes.
        tokio::select! {
            result = timeout(self.config.limits.body_idle_timeout, sender.ready()) => {
                result.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Destination connection readiness timed out"))??;
            }
            () = self.final_shutdown.cancelled() => return Err(stopped()),
        }
        // A streaming upload has its own idle deadline. Starting the response
        // header deadline earlier would abort a healthy long-running upload.
        let sent = sender.send_request(request);
        tokio::pin!(sent);
        tokio::select! {
            result = &mut sent => Ok(result?),
            _ = uploaded => {
                tokio::select! {
                    result = timeout(self.config.limits.header_read_timeout, &mut sent) => {
                        Ok(result.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Destination response headers timed out"))??)
                    }
                    () = self.final_shutdown.cancelled() => Err(stopped()),
                }
            }
            () = self.final_shutdown.cancelled() => Err(stopped()),
        }
    }

    async fn track_uninspected_upload(
        &self,
        sender: OriginSender,
        completion: tokio::sync::oneshot::Receiver<()>,
        consumed: bool,
        activity: super::ActivityLease,
    ) {
        let shutdown = self.final_shutdown.clone();
        let mut tasks = self.tunnels.lock().await;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(
            async move {
                let _activity = activity;
                let finished = async {
                    if !consumed {
                        let _ = completion.await;
                    }
                    // Hyper drops the body after buffering its final frame. Sender
                    // readiness follows its write-buffer flush, so an early empty
                    // response cannot make drain discard the last upload bytes.
                    let _ = sender.lock().await.ready().await;
                };
                tokio::select! { () = finished => {}, () = shutdown.cancelled() => {} }
            }
            .with_subscriber(tracing::subscriber::NoSubscriber::default()),
        );
    }

    async fn spawn_uninspected_relay<F, S>(
        &self,
        client: hyper::upgrade::OnUpgrade,
        origin: F,
        activity: super::ActivityLease,
    ) where
        F: std::future::Future<Output = Result<S, std::io::Error>> + Send + 'static,
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let shutdown = self.final_shutdown.clone();
        let negotiation_timeout = self.config.limits.tls_handshake_timeout;
        let mut tasks = self.tunnels.lock().await;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(
            async move {
                let _activity = activity;
                let transfer = async {
                    let (client, mut origin) = timeout(negotiation_timeout, async {
                        tokio::try_join!(
                            async { client.await.map_err(std::io::Error::other) },
                            origin
                        )
                    })
                    .await
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "Protocol upgrade timed out",
                        )
                    })??;
                    let mut client = TokioIo::new(client);
                    tokio::io::copy_bidirectional(&mut client, &mut origin).await
                };
                tokio::select! {
                    _ = transfer => {},
                    () = shutdown.cancelled() => {},
                }
            }
            .with_subscriber(tracing::subscriber::NoSubscriber::default()),
        );
    }
}

fn stopped() -> ProxyRuntimeError {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "Proxy stopped").into()
}

fn header_has_token(headers: &HeaderMap, name: http::HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
}

fn sanitize_headers(headers: &mut HeaderMap, upgrade: bool) -> Result<(), ProxyRuntimeError> {
    let mut nominated = Vec::new();
    for value in headers.get_all(CONNECTION) {
        for token in value
            .to_str()
            .map_err(|_| transmog_core::TranslationError::InvalidConnection)?
            .split(',')
        {
            if !token.trim().is_empty() {
                nominated.push(http::HeaderName::from_bytes(token.trim().as_bytes())?);
            }
        }
    }
    for name in nominated {
        if !(upgrade && name == UPGRADE) {
            headers.remove(name);
        }
    }
    for name in [
        "connection",
        "proxy-connection",
        "proxy-authorization",
        "proxy-authenticate",
        "keep-alive",
        "te",
    ] {
        headers.remove(name);
    }
    headers.remove(PROXY_AUTHORIZATION);
    if upgrade {
        headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    } else {
        headers.remove(UPGRADE);
    }
    Ok(())
}

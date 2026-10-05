use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    num::NonZeroUsize,
    sync::Arc,
};

use boring::{hash::MessageDigest, rand::rand_bytes, x509::X509};
use bytes::Bytes;
use quiche::h3::NameValue;
use thiserror::Error;
use tokio::{
    net::UdpSocket,
    sync::{Mutex, mpsc, oneshot},
    time::sleep,
};
use transmog_core::{
    BodyFrame, BodyStream, BodyStreamError, BodyStreamSender, CanonicalRequest, CanonicalResponse,
    HeaderBlock, HeaderField, HttpLegVersion, MessageKind, ResponseHead, StreamingRequest,
    StreamingResponse, TranslationOptions, prepare_headers,
};
use transmog_tls::UpstreamTlsContextFactory;

use crate::{H3ConfigError, H3TransportLimits, build_quiche_config};

const MAX_DATAGRAM_SIZE: usize = 1_350;
const MAX_UDP_PACKET_SIZE: usize = 65_535;

/// Evidence captured from one verified quiche HTTP/3 exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct H3Telemetry {
    /// Negotiated HTTP/3 ALPN.
    pub alpn: String,
    /// UDP peer selected for the exchange.
    pub peer_addr: SocketAddr,
    /// SHA-256 fingerprints for the received peer chain, leaf first.
    pub peer_chain_sha256: Vec<String>,
    /// Immutable trust generation used to create the connection.
    pub trust_generation: u64,
}

/// Canonical response and transport evidence from quiche.
#[derive(Clone, Debug)]
pub struct H3OriginResponse {
    /// Protocol-neutral response.
    pub response: CanonicalResponse,
    /// Verified transport evidence.
    pub telemetry: H3Telemetry,
}

/// Streaming canonical response and transport evidence from quiche.
#[derive(Debug)]
pub struct H3StreamingResponse {
    /// Protocol-neutral response head and backpressured body.
    pub response: StreamingResponse,
    /// Verified transport evidence.
    pub telemetry: H3Telemetry,
}

/// Direct Tokio driver over quiche using the shared `BoringSSL` context.
#[derive(Clone, Debug)]
pub struct H3OriginClient {
    tls: UpstreamTlsContextFactory,
    limits: H3TransportLimits,
    pool: Arc<Mutex<HashMap<PoolKey, mpsc::Sender<DriverCommand>>>>,
}

impl H3OriginClient {
    /// Creates an HTTP/3 client tied to one immutable trust generation.
    pub fn new(tls: UpstreamTlsContextFactory, limits: H3TransportLimits) -> Self {
        Self {
            tls,
            limits,
            pool: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Sends one bounded canonical request to an HTTP/3 origin.
    ///
    /// `peer_port` may come from a validated same-host Alt-Svc entry. The
    /// request's original authority remains in `:authority` and in hostname
    /// verification/SNI.
    ///
    /// # Errors
    ///
    /// Returns [`H3OriginError`] for DNS, UDP, QUIC, HTTP/3, verification, or
    /// canonical translation failures. No protocol fallback occurs here.
    pub async fn execute(
        &self,
        request: CanonicalRequest,
        peer_port: u16,
        max_response_bytes: usize,
    ) -> Result<H3OriginResponse, H3OriginError> {
        let host = request.head.target.host.clone();
        let prepared = PreparedRequest::new(request)?;
        self.execute_prepared_buffered(host, prepared, peer_port, max_response_bytes)
            .await
    }

    /// Streams a bounded request body into one pooled HTTP/3 stream.
    ///
    /// The response remains available through the explicitly bounded
    /// compatibility type.
    ///
    /// # Errors
    ///
    /// Returns [`H3OriginError`] for canonical translation, pool, QUIC, HTTP/3,
    /// or certificate-verification failures.
    pub async fn execute_streaming_request(
        &self,
        request: StreamingRequest,
        peer_port: u16,
        max_response_bytes: usize,
    ) -> Result<H3OriginResponse, H3OriginError> {
        let host = request.head.target.host.clone();
        let prepared = PreparedRequest::streaming(request)?;
        self.execute_prepared_buffered(host, prepared, peer_port, max_response_bytes)
            .await
    }

    /// Streams both sides of an exchange over one pooled HTTP/3 request
    /// stream, returning as soon as the final response head is available.
    ///
    /// # Errors
    ///
    /// Returns [`H3OriginError`] for failures before a final response head.
    /// Later body failures are carried by the returned body stream.
    pub async fn execute_duplex_streaming(
        &self,
        request: StreamingRequest,
        peer_port: u16,
        max_response_bytes: usize,
        body_channel_capacity: NonZeroUsize,
    ) -> Result<H3StreamingResponse, H3OriginError> {
        let host = request.head.target.host.clone();
        let prepared = PreparedRequest::streaming(request)?;
        let key = self.pool_key(host, peer_port);
        let (body_sender, body) = BodyStream::channel(body_channel_capacity);
        let (head_completion, head_result) = oneshot::channel();
        self.submit(
            &key,
            DriverCommand {
                request: prepared,
                response: ResponseTarget::Streaming {
                    head_completion: Some(head_completion),
                    body_sender,
                    bytes: 0,
                    limit: max_response_bytes,
                    pending: VecDeque::new(),
                    finished: false,
                },
            },
        )
        .await?;
        let (head, telemetry) = head_result
            .await
            .map_err(|_| H3OriginError::PoolDriverStopped)??;
        Ok(H3StreamingResponse {
            response: StreamingResponse { head, body },
            telemetry,
        })
    }

    async fn execute_prepared_buffered(
        &self,
        host: String,
        prepared: PreparedRequest,
        peer_port: u16,
        max_response_bytes: usize,
    ) -> Result<H3OriginResponse, H3OriginError> {
        let key = self.pool_key(host, peer_port);
        let (completion, result) = oneshot::channel();
        self.submit(
            &key,
            DriverCommand {
                request: prepared,
                response: ResponseTarget::Buffered {
                    accumulator: ResponseAccumulator::new(max_response_bytes),
                    completion,
                },
            },
        )
        .await?;
        result.await.map_err(|_| H3OriginError::PoolDriverStopped)?
    }

    fn pool_key(&self, host: String, peer_port: u16) -> PoolKey {
        PoolKey {
            host,
            peer_port,
            trust_generation: self.tls.snapshot().generation(),
        }
    }

    async fn submit(&self, key: &PoolKey, mut command: DriverCommand) -> Result<(), H3OriginError> {
        for _ in 0..2 {
            let sender = self.sender_for(key).await?;
            match sender.send(command).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    command = error.0;
                    self.remove_sender(key, &sender).await;
                }
            }
        }
        Err(H3OriginError::PoolDriverStopped)
    }

    async fn sender_for(
        &self,
        key: &PoolKey,
    ) -> Result<mpsc::Sender<DriverCommand>, H3OriginError> {
        let mut pool = self.pool.lock().await;
        pool.retain(|_, sender| !sender.is_closed());
        if let Some(sender) = pool.get(key) {
            return Ok(sender.clone());
        }
        if pool.len() >= self.limits.max_pool_entries {
            return Err(H3OriginError::PoolCapacityExceeded {
                limit: self.limits.max_pool_entries,
            });
        }

        let driver = H3ConnectionDriver::connect(
            key.host.clone(),
            key.peer_port,
            self.tls.clone(),
            self.limits,
        )
        .await?;
        let (sender, receiver) = mpsc::channel(self.limits.pending_requests_per_connection);
        tokio::spawn(driver.run(receiver));
        pool.insert(key.clone(), sender.clone());
        Ok(sender)
    }

    async fn remove_sender(&self, key: &PoolKey, sender: &mpsc::Sender<DriverCommand>) {
        let mut pool = self.pool.lock().await;
        if pool
            .get(key)
            .is_some_and(|current| current.same_channel(sender))
        {
            pool.remove(key);
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PoolKey {
    host: String,
    peer_port: u16,
    trust_generation: u64,
}

struct DriverCommand {
    request: PreparedRequest,
    response: ResponseTarget,
}

struct ActiveRequest {
    body: ActiveRequestBody,
    response: ResponseTarget,
}

enum ResponseTarget {
    Buffered {
        accumulator: ResponseAccumulator,
        completion: oneshot::Sender<Result<H3OriginResponse, H3OriginError>>,
    },
    Streaming {
        head_completion:
            Option<oneshot::Sender<Result<(ResponseHead, H3Telemetry), H3OriginError>>>,
        body_sender: BodyStreamSender,
        bytes: usize,
        limit: usize,
        pending: VecDeque<Result<BodyFrame, BodyStreamError>>,
        finished: bool,
    },
}

impl ResponseTarget {
    fn is_cancelled(&self) -> bool {
        match self {
            Self::Buffered { completion, .. } => completion.is_closed(),
            Self::Streaming {
                head_completion,
                body_sender,
                ..
            } => {
                head_completion
                    .as_ref()
                    .is_some_and(oneshot::Sender::is_closed)
                    || (head_completion.is_none() && body_sender.is_closed())
            }
        }
    }

    fn fail(self, error: H3OriginError) {
        match self {
            Self::Buffered { completion, .. } => {
                let _ = completion.send(Err(error));
            }
            Self::Streaming {
                head_completion: Some(completion),
                ..
            } => {
                let _ = completion.send(Err(error));
            }
            Self::Streaming {
                body_sender,
                head_completion: None,
                ..
            } => {
                tokio::spawn(async move {
                    let _ = body_sender
                        .send(Err(BodyStreamError::Failed(error.to_string())))
                        .await;
                });
            }
        }
    }
}

struct H3ConnectionDriver {
    tls: UpstreamTlsContextFactory,
    limits: H3TransportLimits,
    socket: UdpSocket,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    connection: quiche::Connection,
    h3_config: quiche::h3::Config,
}

impl H3ConnectionDriver {
    async fn connect(
        host: String,
        peer_port: u16,
        tls: UpstreamTlsContextFactory,
        limits: H3TransportLimits,
    ) -> Result<Self, H3OriginError> {
        let peer_addr = tokio::net::lookup_host((host.as_str(), peer_port))
            .await?
            .next()
            .ok_or_else(|| H3OriginError::DnsNoAddresses(host.clone()))?;
        let bind_addr = if peer_addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind_addr).await?;
        socket.connect(peer_addr).await?;
        let local_addr = socket.local_addr()?;

        let mut config = build_quiche_config(&tls, limits)?;
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        let mut source_connection_id = [0_u8; quiche::MAX_CONN_ID_LEN];
        rand_bytes(&mut source_connection_id)?;
        let source_connection_id = quiche::ConnectionId::from_ref(&source_connection_id);
        let connection = quiche::connect(
            Some(host.as_str()),
            &source_connection_id,
            local_addr,
            peer_addr,
            &mut config,
        )?;
        Ok(Self {
            tls,
            limits,
            socket,
            local_addr,
            peer_addr,
            connection,
            h3_config: quiche::h3::Config::new()?,
        })
    }

    async fn run(mut self, receiver: mpsc::Receiver<DriverCommand>) {
        let mut pending = VecDeque::new();
        let mut active = HashMap::new();
        if let Err(error) = self.drive(receiver, &mut pending, &mut active).await {
            let message = error.to_string();
            for command in pending {
                command
                    .response
                    .fail(H3OriginError::ConnectionFailed(message.clone()));
            }
            for (_, request) in active {
                request
                    .response
                    .fail(H3OriginError::ConnectionFailed(message.clone()));
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn drive(
        &mut self,
        mut receiver: mpsc::Receiver<DriverCommand>,
        pending: &mut VecDeque<DriverCommand>,
        active: &mut HashMap<u64, ActiveRequest>,
    ) -> Result<(), H3OriginError> {
        let mut http3 = None;
        let mut recv_buffer = vec![0_u8; MAX_UDP_PACKET_SIZE];
        let mut send_buffer = vec![0_u8; MAX_DATAGRAM_SIZE];
        flush_packets(&self.socket, &mut self.connection, &mut send_buffer).await?;

        loop {
            while let Ok(command) = receiver.try_recv() {
                if !command.response.is_cancelled() {
                    pending.push_back(command);
                }
            }
            if self.connection.is_closed() {
                return Err(H3OriginError::ClosedBeforeResponse);
            }
            if self.connection.is_established() && http3.is_none() {
                http3 = Some(quiche::h3::Connection::with_transport(
                    &mut self.connection,
                    &self.h3_config,
                )?);
            }

            if let Some(http3) = http3.as_mut() {
                dispatch_pending(http3, &mut self.connection, pending, active)?;
                progress_active_requests(http3, &mut self.connection, active);
                flush_streaming_outputs(&mut self.connection, active);
                poll_responses(
                    http3,
                    &mut self.connection,
                    self.peer_addr,
                    &self.tls,
                    &mut recv_buffer,
                    active,
                )?;
                flush_streaming_outputs(&mut self.connection, active);
                // A Data event and its following Finished event can already be
                // queued from the same datagram. Poll once more after handing
                // the frame to its bounded consumer so completion does not
                // depend on another network packet.
                poll_responses(
                    http3,
                    &mut self.connection,
                    self.peer_addr,
                    &self.tls,
                    &mut recv_buffer,
                    active,
                )?;
                flush_streaming_outputs(&mut self.connection, active);
            }
            flush_packets(&self.socket, &mut self.connection, &mut send_buffer).await?;

            let wait = self
                .connection
                .timeout()
                .unwrap_or(self.limits.idle_timeout);
            let wait = if active
                .values()
                .any(|request| request.body.needs_poll() || request.response.is_output_blocked())
            {
                wait.min(std::time::Duration::from_millis(5))
            } else {
                wait
            };
            tokio::select! {
                command = receiver.recv() => {
                    match command {
                        Some(command) if !command.response.is_cancelled() => {
                            pending.push_back(command);
                        }
                        None if active.is_empty() && pending.is_empty() => return Ok(()),
                        Some(_) | None => {}
                    }
                }
                packet = self.socket.recv(&mut recv_buffer) => {
                    let read = packet?;
                    let info = quiche::RecvInfo {
                        from: self.peer_addr,
                        to: self.local_addr,
                    };
                    match self.connection.recv(&mut recv_buffer[..read], info) {
                        Ok(_) | Err(quiche::Error::Done) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                () = sleep(wait) => self.connection.on_timeout(),
            }
        }
    }
}

fn dispatch_pending(
    http3: &mut quiche::h3::Connection,
    connection: &mut quiche::Connection,
    pending: &mut VecDeque<DriverCommand>,
    active: &mut HashMap<u64, ActiveRequest>,
) -> Result<(), H3OriginError> {
    while let Some(command) = pending.pop_front() {
        if command.response.is_cancelled() {
            continue;
        }
        let finished = command.request.body.is_definitely_empty();
        let stream_id = match http3.send_request(connection, &command.request.headers, finished) {
            Ok(stream_id) => stream_id,
            Err(quiche::h3::Error::StreamBlocked) => {
                pending.push_front(command);
                break;
            }
            Err(error) => return Err(error.into()),
        };
        active.insert(
            stream_id,
            ActiveRequest {
                body: command.request.body.into_active(finished),
                response: command.response,
            },
        );
    }
    Ok(())
}

fn progress_active_requests(
    http3: &mut quiche::h3::Connection,
    connection: &mut quiche::Connection,
    active: &mut HashMap<u64, ActiveRequest>,
) {
    let mut failed = Vec::new();
    for (&stream_id, request) in active.iter_mut() {
        if request.response.is_cancelled() {
            failed.push((stream_id, None));
            continue;
        }
        if let Err(error) = progress_request_body(http3, connection, stream_id, &mut request.body) {
            failed.push((stream_id, Some(error)));
        }
    }
    for (stream_id, error) in failed {
        if let Some(request) = active.remove(&stream_id) {
            let _ = connection.stream_shutdown(stream_id, quiche::Shutdown::Read, 0);
            let _ = connection.stream_shutdown(stream_id, quiche::Shutdown::Write, 0);
            if let Some(error) = error {
                request.response.fail(error);
            }
        }
    }
}

fn poll_responses(
    http3: &mut quiche::h3::Connection,
    connection: &mut quiche::Connection,
    peer_addr: SocketAddr,
    tls: &UpstreamTlsContextFactory,
    recv_buffer: &mut [u8],
    active: &mut HashMap<u64, ActiveRequest>,
) -> Result<(), H3OriginError> {
    loop {
        match http3.poll(connection) {
            Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                let result = active
                    .get_mut(&stream_id)
                    .map(|request| request.response.headers(&list, connection, peer_addr, tls));
                if let Some(Err(error)) = result {
                    fail_stream(connection, active, stream_id, error);
                }
            }
            Ok((stream_id, quiche::h3::Event::Data)) => loop {
                match http3.recv_body(connection, stream_id, recv_buffer) {
                    Ok(read) => {
                        let result = active
                            .get_mut(&stream_id)
                            .map(|request| request.response.data(&recv_buffer[..read]));
                        if let Some(Err(error)) = result {
                            fail_stream(connection, active, stream_id, error);
                            break;
                        }
                    }
                    Err(quiche::h3::Error::Done) => break,
                    Err(error) => return Err(error.into()),
                }
            },
            Ok((stream_id, quiche::h3::Event::Finished)) => {
                let is_buffered = active.get(&stream_id).is_some_and(|request| {
                    matches!(&request.response, ResponseTarget::Buffered { .. })
                });
                if is_buffered {
                    if let Some(request) = active.remove(&stream_id) {
                        request.response.finish_buffered(connection, peer_addr, tls);
                    }
                } else if let Some(request) = active.get_mut(&stream_id) {
                    request.response.mark_finished();
                }
            }
            Ok((stream_id, quiche::h3::Event::Reset(code))) => {
                if let Some(request) = active.remove(&stream_id) {
                    request.response.fail(H3OriginError::StreamReset(code));
                }
            }
            Ok((_, quiche::h3::Event::PriorityUpdate | quiche::h3::Event::GoAway)) => {}
            Err(quiche::h3::Error::Done) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
}

fn fail_stream(
    connection: &mut quiche::Connection,
    active: &mut HashMap<u64, ActiveRequest>,
    stream_id: u64,
    error: H3OriginError,
) {
    if let Some(request) = active.remove(&stream_id) {
        let _ = connection.stream_shutdown(stream_id, quiche::Shutdown::Read, 0);
        let _ = connection.stream_shutdown(stream_id, quiche::Shutdown::Write, 0);
        request.response.fail(error);
    }
}

fn flush_streaming_outputs(
    connection: &mut quiche::Connection,
    active: &mut HashMap<u64, ActiveRequest>,
) {
    let mut cancelled = Vec::new();
    let mut finished = Vec::new();
    for (&stream_id, request) in active.iter_mut() {
        match request.response.flush_pending() {
            OutputFlush::Open => {}
            OutputFlush::Cancelled => cancelled.push(stream_id),
            OutputFlush::Finished => finished.push(stream_id),
        }
    }
    for stream_id in cancelled {
        if active.remove(&stream_id).is_some() {
            let _ = connection.stream_shutdown(stream_id, quiche::Shutdown::Read, 0);
            let _ = connection.stream_shutdown(stream_id, quiche::Shutdown::Write, 0);
        }
    }
    for stream_id in finished {
        active.remove(&stream_id);
    }
}

enum OutputFlush {
    Open,
    Cancelled,
    Finished,
}

impl ResponseTarget {
    fn headers(
        &mut self,
        headers: &[quiche::h3::Header],
        connection: &quiche::Connection,
        peer_addr: SocketAddr,
        tls: &UpstreamTlsContextFactory,
    ) -> Result<(), H3OriginError> {
        match self {
            Self::Buffered { accumulator, .. } => accumulator.headers(headers),
            Self::Streaming {
                head_completion,
                pending,
                ..
            } => {
                if head_completion.is_some() {
                    let Some(head) = response_head(headers)? else {
                        return Ok(());
                    };
                    let completion = head_completion.take().expect("checked above");
                    let evidence = telemetry(connection, peer_addr, tls)?;
                    let _ = completion.send(Ok((head, evidence)));
                } else {
                    pending.push_back(Ok(BodyFrame::Trailers(regular_block(headers)?)));
                }
                Ok(())
            }
        }
    }

    fn data(&mut self, data: &[u8]) -> Result<(), H3OriginError> {
        match self {
            Self::Buffered { accumulator, .. } => accumulator.data(data),
            Self::Streaming {
                head_completion,
                bytes,
                limit,
                pending,
                ..
            } => {
                if head_completion.is_some() {
                    return Err(H3OriginError::DataBeforeFinalHeaders);
                }
                let attempted = bytes
                    .checked_add(data.len())
                    .ok_or(H3OriginError::ResponseBodyTooLarge { limit: *limit })?;
                if attempted > *limit {
                    return Err(H3OriginError::ResponseBodyTooLarge { limit: *limit });
                }
                *bytes = attempted;
                pending.push_back(Ok(BodyFrame::Data(Bytes::copy_from_slice(data))));
                Ok(())
            }
        }
    }

    fn is_output_blocked(&self) -> bool {
        matches!(self, Self::Streaming { pending, .. } if !pending.is_empty())
    }

    fn mark_finished(&mut self) {
        if let Self::Streaming { finished, .. } = self {
            *finished = true;
        }
    }

    fn flush_pending(&mut self) -> OutputFlush {
        let Self::Streaming {
            head_completion,
            body_sender,
            pending,
            finished,
            ..
        } = self
        else {
            return OutputFlush::Open;
        };
        if head_completion
            .as_ref()
            .is_some_and(oneshot::Sender::is_closed)
            || (head_completion.is_none() && body_sender.is_closed())
        {
            return OutputFlush::Cancelled;
        }
        while let Some(item) = pending.pop_front() {
            match body_sender.try_send(item) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(item)) => {
                    pending.push_front(item);
                    return OutputFlush::Open;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return OutputFlush::Cancelled,
            }
        }
        if *finished && pending.is_empty() {
            OutputFlush::Finished
        } else {
            OutputFlush::Open
        }
    }

    fn finish_buffered(
        self,
        connection: &quiche::Connection,
        peer_addr: SocketAddr,
        tls: &UpstreamTlsContextFactory,
    ) {
        if let Self::Buffered {
            accumulator,
            completion,
        } = self
        {
            let result = accumulator.finish().and_then(|response| {
                Ok(H3OriginResponse {
                    response,
                    telemetry: telemetry(connection, peer_addr, tls)?,
                })
            });
            let _ = completion.send(result);
        }
    }
}

struct PreparedRequest {
    headers: Vec<quiche::h3::Header>,
    body: PreparedRequestBody,
}

impl PreparedRequest {
    fn new(request: CanonicalRequest) -> Result<Self, H3OriginError> {
        let headers = prepare_request_headers(request.head)?;

        let mut body = Vec::new();
        let mut trailers = None;
        for frame in request.body {
            match frame {
                BodyFrame::Data(data) if trailers.is_none() => body.extend_from_slice(&data),
                BodyFrame::Trailers(block) if trailers.is_none() => {
                    trailers = Some(h3_regular_headers(&block));
                }
                BodyFrame::Data(_) => return Err(H3OriginError::DataAfterTrailers),
                BodyFrame::Trailers(_) => return Err(H3OriginError::DuplicateTrailers),
            }
        }
        Ok(Self {
            headers,
            body: PreparedRequestBody::Buffered { body, trailers },
        })
    }

    fn streaming(request: StreamingRequest) -> Result<Self, H3OriginError> {
        Ok(Self {
            headers: prepare_request_headers(request.head)?,
            body: PreparedRequestBody::Streaming(request.body),
        })
    }
}

fn prepare_request_headers(
    head: transmog_core::RequestHead,
) -> Result<Vec<quiche::h3::Header>, H3OriginError> {
    let headers = prepare_headers(
        &head.headers,
        TranslationOptions {
            destination: HttpLegVersion::Http3,
            kind: MessageKind::Request,
            body_modified: false,
            force_identity_encoding: false,
        },
    )?;
    let mut path = head.target.path;
    if let Some(query) = head.target.query {
        path.push('?');
        path.push_str(&query);
    }
    let mut h3_headers = vec![
        quiche::h3::Header::new(b":method", head.method.as_bytes()),
        quiche::h3::Header::new(b":scheme", head.target.scheme.as_bytes()),
        quiche::h3::Header::new(b":authority", head.target.authority.as_bytes()),
        quiche::h3::Header::new(b":path", path.as_bytes()),
    ];
    h3_headers.extend(h3_regular_headers(&headers));
    Ok(h3_headers)
}

enum PreparedRequestBody {
    Buffered {
        body: Vec<u8>,
        trailers: Option<Vec<quiche::h3::Header>>,
    },
    Streaming(BodyStream),
}

impl PreparedRequestBody {
    fn is_definitely_empty(&self) -> bool {
        matches!(
            self,
            Self::Buffered {
                body,
                trailers: None
            } if body.is_empty()
        )
    }

    fn into_active(self, finished: bool) -> ActiveRequestBody {
        match self {
            Self::Buffered { body, trailers } => ActiveRequestBody::Buffered {
                body,
                offset: 0,
                trailers,
                finished,
            },
            Self::Streaming(stream) => ActiveRequestBody::Streaming {
                stream,
                pending: None,
                trailers: None,
                input_finished: false,
                output_finished: false,
            },
        }
    }
}

enum ActiveRequestBody {
    Buffered {
        body: Vec<u8>,
        offset: usize,
        trailers: Option<Vec<quiche::h3::Header>>,
        finished: bool,
    },
    Streaming {
        stream: BodyStream,
        pending: Option<(Bytes, usize)>,
        trailers: Option<Vec<quiche::h3::Header>>,
        input_finished: bool,
        output_finished: bool,
    },
}

impl ActiveRequestBody {
    fn needs_poll(&self) -> bool {
        matches!(
            self,
            Self::Streaming {
                input_finished: false,
                output_finished: false,
                ..
            }
        )
    }
}

fn progress_request_body(
    http3: &mut quiche::h3::Connection,
    connection: &mut quiche::Connection,
    stream_id: u64,
    body: &mut ActiveRequestBody,
) -> Result<(), H3OriginError> {
    match body {
        ActiveRequestBody::Buffered {
            body,
            offset,
            trailers,
            finished,
        } => {
            if *offset < body.len() {
                let final_chunk = trailers.is_none();
                match http3.send_body(connection, stream_id, &body[*offset..], final_chunk) {
                    Ok(written) => *offset += written,
                    Err(quiche::h3::Error::Done) => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
            }
            if *offset == body.len() && !*finished {
                if let Some(trailers) = trailers {
                    match http3.send_additional_headers(connection, stream_id, trailers, true, true)
                    {
                        Ok(()) => *finished = true,
                        Err(quiche::h3::Error::Done) => {}
                        Err(error) => return Err(error.into()),
                    }
                } else {
                    *finished = true;
                }
            }
        }
        ActiveRequestBody::Streaming {
            stream,
            pending,
            trailers,
            input_finished,
            output_finished,
        } => loop {
            if *output_finished {
                return Ok(());
            }
            if let Some((data, offset)) = pending {
                match http3.send_body(connection, stream_id, &data[*offset..], false) {
                    Ok(written) => {
                        *offset += written;
                        if *offset == data.len() {
                            *pending = None;
                        }
                    }
                    Err(quiche::h3::Error::Done) => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
                continue;
            }
            if let Some(trailers) = trailers.as_ref() {
                match http3.send_additional_headers(connection, stream_id, trailers, true, true) {
                    Ok(()) => *output_finished = true,
                    Err(quiche::h3::Error::Done) => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
                return Ok(());
            }
            if *input_finished {
                match http3.send_body(connection, stream_id, &[], true) {
                    Ok(_) => *output_finished = true,
                    Err(quiche::h3::Error::Done) => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
                return Ok(());
            }
            match stream.try_recv() {
                Ok(Ok(BodyFrame::Data(data))) => *pending = Some((data, 0)),
                Ok(Ok(BodyFrame::Trailers(block))) => {
                    *trailers = Some(h3_regular_headers(&block));
                    *input_finished = true;
                }
                Ok(Err(error)) => return Err(H3OriginError::BodyStream(error)),
                Err(mpsc::error::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::error::TryRecvError::Disconnected) => *input_finished = true,
            }
        },
    }
    Ok(())
}

async fn flush_packets(
    socket: &UdpSocket,
    connection: &mut quiche::Connection,
    buffer: &mut [u8],
) -> Result<(), H3OriginError> {
    loop {
        match connection.send(buffer) {
            Ok((written, send_info)) => {
                tokio::time::sleep_until(tokio::time::Instant::from_std(send_info.at)).await;
                socket.send(&buffer[..written]).await?;
            }
            Err(quiche::Error::Done) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
}

struct ResponseAccumulator {
    head: Option<ResponseHead>,
    body: Vec<BodyFrame>,
    bytes: usize,
    limit: usize,
}

impl ResponseAccumulator {
    fn new(limit: usize) -> Self {
        Self {
            head: None,
            body: Vec::new(),
            bytes: 0,
            limit,
        }
    }

    fn headers(&mut self, headers: &[quiche::h3::Header]) -> Result<(), H3OriginError> {
        if self.head.is_some() {
            self.body.push(BodyFrame::Trailers(regular_block(headers)?));
            return Ok(());
        }
        if let Some(head) = response_head(headers)? {
            self.head = Some(head);
        }
        Ok(())
    }

    fn data(&mut self, data: &[u8]) -> Result<(), H3OriginError> {
        if self.head.is_none() {
            return Err(H3OriginError::DataBeforeFinalHeaders);
        }
        self.bytes = self
            .bytes
            .checked_add(data.len())
            .ok_or(H3OriginError::ResponseBodyTooLarge { limit: self.limit })?;
        if self.bytes > self.limit {
            return Err(H3OriginError::ResponseBodyTooLarge { limit: self.limit });
        }
        self.body
            .push(BodyFrame::Data(Bytes::copy_from_slice(data)));
        Ok(())
    }

    fn finish(self) -> Result<CanonicalResponse, H3OriginError> {
        Ok(CanonicalResponse {
            head: self.head.ok_or(H3OriginError::MissingStatus)?,
            body: self.body,
        })
    }
}

fn response_head(headers: &[quiche::h3::Header]) -> Result<Option<ResponseHead>, H3OriginError> {
    let status = pseudo_value(headers, b":status")?.ok_or(H3OriginError::MissingStatus)?;
    let status = std::str::from_utf8(status)
        .map_err(|_| H3OriginError::InvalidStatus)?
        .parse::<u16>()
        .map_err(|_| H3OriginError::InvalidStatus)?;
    if (100..200).contains(&status) {
        return Ok(None);
    }
    Ok(Some(ResponseHead {
        status,
        headers: regular_block(headers)?,
        source_version: HttpLegVersion::Http3,
    }))
}

fn h3_regular_headers(block: &HeaderBlock) -> Vec<quiche::h3::Header> {
    block
        .iter()
        .map(|field| quiche::h3::Header::new(&field.name().to_ascii_lowercase(), field.value()))
        .collect()
}

fn regular_block(headers: &[quiche::h3::Header]) -> Result<HeaderBlock, H3OriginError> {
    headers
        .iter()
        .filter(|header| !header.name().starts_with(b":"))
        .map(|header| HeaderField::try_new(header.name(), header.value()).map_err(Into::into))
        .collect::<Result<Vec<_>, _>>()
        .map(HeaderBlock::from_fields)
}

fn pseudo_value<'a>(
    headers: &'a [quiche::h3::Header],
    name: &[u8],
) -> Result<Option<&'a [u8]>, H3OriginError> {
    let mut matches = headers.iter().filter(|header| header.name() == name);
    let value = matches.next().map(NameValue::value);
    if matches.next().is_some() {
        return Err(H3OriginError::DuplicatePseudoHeader);
    }
    Ok(value)
}

fn telemetry(
    connection: &quiche::Connection,
    peer_addr: SocketAddr,
    tls: &UpstreamTlsContextFactory,
) -> Result<H3Telemetry, H3OriginError> {
    let alpn = String::from_utf8(connection.application_proto().to_vec())
        .map_err(|_| H3OriginError::InvalidAlpn)?;
    if !alpn.starts_with("h3") {
        return Err(H3OriginError::UnexpectedAlpn(alpn));
    }
    let mut peer_chain_sha256 = Vec::new();
    for der in connection.peer_cert_chain().unwrap_or_default() {
        let certificate = X509::from_der(der)?;
        peer_chain_sha256.push(hex_lower(&certificate.digest(MessageDigest::sha256())?));
    }
    Ok(H3Telemetry {
        alpn,
        peer_addr,
        peer_chain_sha256,
        trust_generation: tls.snapshot().generation(),
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

/// Direct quiche origin-adapter failure.
#[derive(Debug, Error)]
pub enum H3OriginError {
    /// Shared TLS or quiche transport configuration failed.
    #[error(transparent)]
    Config(#[from] H3ConfigError),
    /// UDP or DNS I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// quiche QUIC processing failed.
    #[error(transparent)]
    Quiche(#[from] quiche::Error),
    /// quiche HTTP/3 processing failed.
    #[error(transparent)]
    Http3(#[from] quiche::h3::Error),
    /// `BoringSSL` certificate parsing, hashing, or randomness failed.
    #[error(transparent)]
    Boring(#[from] boring::error::ErrorStack),
    /// Canonical translation rejected unsafe framing.
    #[error(transparent)]
    Translation(#[from] transmog_core::TranslationError),
    /// A canonical regular header was invalid.
    #[error(transparent)]
    Header(#[from] transmog_core::HeaderError),
    /// A streaming request body producer failed.
    #[error(transparent)]
    BodyStream(#[from] BodyStreamError),
    /// DNS resolution returned no address.
    #[error("DNS returned no addresses for {0}")]
    DnsNoAddresses(String),
    /// The bounded authority/trust-generation pool is full.
    #[error("HTTP/3 connection pool reached its configured {limit}-entry limit")]
    PoolCapacityExceeded {
        /// Configured maximum number of live pool entries.
        limit: usize,
    },
    /// A pooled connection task ended before accepting or completing a request.
    #[error("HTTP/3 pooled connection driver stopped")]
    PoolDriverStopped,
    /// A pooled connection failed and terminated its outstanding streams.
    #[error("HTTP/3 pooled connection failed: {0}")]
    ConnectionFailed(String),
    /// Peer closed before a complete response arrived.
    #[error("QUIC connection closed before a complete response")]
    ClosedBeforeResponse,
    /// The request stream was reset by the peer.
    #[error("HTTP/3 request stream reset with code {0}")]
    StreamReset(u64),
    /// Request data appeared after trailers.
    #[error("request body data appeared after trailers")]
    DataAfterTrailers,
    /// Request contained multiple trailer blocks.
    #[error("request contained duplicate trailers")]
    DuplicateTrailers,
    /// Final response omitted `:status`.
    #[error("HTTP/3 response omitted :status")]
    MissingStatus,
    /// Response data arrived before a final non-informational header block.
    #[error("HTTP/3 response data arrived before final headers")]
    DataBeforeFinalHeaders,
    /// Response status was not a three-digit integer.
    #[error("HTTP/3 response contained an invalid :status")]
    InvalidStatus,
    /// A pseudo-header appeared more than once.
    #[error("HTTP/3 response contained a duplicate pseudo-header")]
    DuplicatePseudoHeader,
    /// Negotiated ALPN was not UTF-8.
    #[error("negotiated HTTP/3 ALPN was not UTF-8")]
    InvalidAlpn,
    /// QUIC established without an HTTP/3 ALPN.
    #[error("origin negotiated unexpected ALPN {0:?}")]
    UnexpectedAlpn(String),
    /// Response exceeded the explicit per-exchange byte limit.
    #[error("HTTP/3 response body exceeded configured {limit}-byte limit")]
    ResponseBodyTooLarge {
        /// Configured maximum response bytes.
        limit: usize,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use boring::ssl::{SslContext, SslMethod};
    use tokio::{task::JoinHandle, time::timeout};
    use transmog_core::{RequestHead, Target};
    use transmog_tls::{
        EndpointIdentity, LoadedTrust, ProxyCa, TrustError, TrustSnapshot, TrustSource,
        UpstreamTlsPolicy,
    };

    use super::*;

    struct StaticTrust(Vec<Vec<u8>>);

    impl TrustSource for StaticTrust {
        fn load(&self) -> Result<LoadedTrust, TrustError> {
            Ok(LoadedTrust {
                certificates_der: self.0.clone(),
                source_description: "local h3 root".to_owned(),
                source_version: Some("test".to_owned()),
                diagnostics: Vec::new(),
            })
        }
    }

    #[test]
    fn buffered_h3_response_rejects_data_before_final_headers() {
        let mut response = ResponseAccumulator::new(1024);
        assert!(matches!(
            response.data(b"unexpected"),
            Err(H3OriginError::DataBeforeFinalHeaders)
        ));
    }

    #[tokio::test]
    async fn local_h3_origin_uses_shared_verified_context() {
        let ca = ProxyCa::generate("Transmog h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin, server) = spawn_origin(leaf, bind_ip).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 41)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );
        let request = CanonicalRequest {
            head: RequestHead {
                method: "GET".to_owned(),
                target: Target {
                    scheme: "https".to_owned(),
                    authority: format!("localhost:{}", origin.port()),
                    host: "localhost".to_owned(),
                    port: origin.port(),
                    path: "/proof".to_owned(),
                    query: None,
                },
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("x-h3-proof", "yes").unwrap(),
                ]),
                source_version: HttpLegVersion::Http1,
            },
            body: Vec::new(),
        };
        let result = client.execute(request, origin.port(), 1024).await.unwrap();
        assert_eq!(result.response.head.status, 200);
        assert_eq!(result.response.head.source_version, HttpLegVersion::Http3);
        assert_eq!(result.telemetry.alpn, "h3");
        assert_eq!(result.telemetry.trust_generation, 41);
        assert!(!result.telemetry.peer_chain_sha256.is_empty());
        assert_eq!(
            result.response.body,
            vec![BodyFrame::Data(Bytes::from_static(b"h3-ok"))]
        );
        server.finish().await;
    }

    #[tokio::test]
    async fn local_h3_origin_with_unknown_root_fails_closed() {
        let origin_ca = ProxyCa::generate("Transmog untrusted h3 origin", 2).unwrap();
        let leaf = origin_ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin, server) = spawn_origin(leaf, bind_ip).await;
        let unrelated_ca = ProxyCa::generate("unrelated h3 root", 2).unwrap();
        let trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![unrelated_ca.certificate().to_der().unwrap()]),
                42,
            )
            .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(1),
                ..H3TransportLimits::default()
            },
        );
        let request = CanonicalRequest {
            head: RequestHead {
                method: "GET".to_owned(),
                target: Target {
                    scheme: "https".to_owned(),
                    authority: format!("localhost:{}", origin.port()),
                    host: "localhost".to_owned(),
                    port: origin.port(),
                    path: "/must-fail".to_owned(),
                    query: None,
                },
                headers: HeaderBlock::new(),
                source_version: HttpLegVersion::Http1,
            },
            body: Vec::new(),
        };
        assert!(client.execute(request, origin.port(), 1024).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn local_h3_origin_with_wrong_hostname_fails_closed() {
        let origin_ca = ProxyCa::generate("Transmog wrong-host h3 origin", 2).unwrap();
        let leaf = origin_ca
            .issue(EndpointIdentity::parse("wrong.example").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin, server) = spawn_origin(leaf, bind_ip).await;
        let trust = Arc::new(
            TrustSnapshot::load(
                &StaticTrust(vec![origin_ca.certificate().to_der().unwrap()]),
                43,
            )
            .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(1),
                ..H3TransportLimits::default()
            },
        );
        let request = CanonicalRequest {
            head: RequestHead {
                method: "GET".to_owned(),
                target: Target {
                    scheme: "https".to_owned(),
                    authority: format!("localhost:{}", origin.port()),
                    host: "localhost".to_owned(),
                    port: origin.port(),
                    path: "/must-fail".to_owned(),
                    query: None,
                },
                headers: HeaderBlock::new(),
                source_version: HttpLegVersion::Http1,
            },
            body: Vec::new(),
        };
        assert!(client.execute(request, origin.port(), 1024).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn local_h3_origin_accepts_an_exact_ip_san() {
        let ca = ProxyCa::generate("Transmog IP-SAN h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("127.0.0.1").unwrap(), 1)
            .unwrap();
        let bind_ip = "127.0.0.1".parse().unwrap();
        let (origin, server) = spawn_origin(leaf, bind_ip).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 44)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );
        let request = CanonicalRequest {
            head: RequestHead {
                method: "GET".to_owned(),
                target: Target {
                    scheme: "https".to_owned(),
                    authority: format!("127.0.0.1:{}", origin.port()),
                    host: "127.0.0.1".to_owned(),
                    port: origin.port(),
                    path: "/ip-san".to_owned(),
                    query: None,
                },
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("x-h3-proof", "yes").unwrap(),
                ]),
                source_version: HttpLegVersion::Http1,
            },
            body: Vec::new(),
        };
        let result = client.execute(request, origin.port(), 1024).await.unwrap();
        assert_eq!(result.response.head.status, 200);
        assert_eq!(result.telemetry.trust_generation, 44);
        server.finish().await;
    }

    #[tokio::test]
    async fn duplex_streaming_api_returns_backpressured_response_frames() {
        let ca = ProxyCa::generate("Transmog streaming h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin, server) = spawn_origin(leaf, bind_ip).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 44)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );
        let capacity = NonZeroUsize::new(1).unwrap();
        let (request_sender, request_body) = BodyStream::channel(capacity);
        drop(request_sender);
        let request = h3_request(origin, "/stream");
        let mut result = client
            .execute_duplex_streaming(
                StreamingRequest {
                    head: request.head,
                    body: request_body,
                },
                origin.port(),
                1024,
                capacity,
            )
            .await
            .unwrap();
        assert_eq!(result.response.head.status, 200);
        assert_eq!(result.telemetry.trust_generation, 44);
        assert_eq!(
            result.response.body.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"h3-ok"))
        );
        let terminal = result.response.body.recv().await;
        assert!(
            terminal.is_none(),
            "unexpected terminal frame: {terminal:?}"
        );
        server.finish().await;
    }

    #[tokio::test]
    async fn duplex_response_arrives_before_the_h3_origin_finishes() {
        let ca = ProxyCa::generate("Transmog slow streaming h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let first_sent = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (origin, server) =
            spawn_slow_streaming_origin(leaf, bind_ip, first_sent.clone(), release.clone()).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 45)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );
        let capacity = NonZeroUsize::new(1).unwrap();
        let (request_sender, request_body) = BodyStream::channel(capacity);
        drop(request_sender);
        let request = h3_request(origin, "/slow-stream");
        let mut result = client
            .execute_duplex_streaming(
                StreamingRequest {
                    head: request.head,
                    body: request_body,
                },
                origin.port(),
                1024,
                capacity,
            )
            .await
            .unwrap();
        timeout(std::time::Duration::from_secs(1), first_sent.notified())
            .await
            .expect("H3 fixture never sent its first chunk");
        assert_eq!(
            timeout(
                std::time::Duration::from_millis(500),
                result.response.body.recv()
            )
            .await
            .expect("H3 client buffered until the complete response")
            .unwrap()
            .unwrap(),
            BodyFrame::Data(Bytes::from_static(b"first"))
        );
        assert!(!server.is_finished());
        release.notify_one();
        assert_eq!(
            result.response.body.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"second"))
        );
        assert!(result.response.body.recv().await.is_none());
        server.finish().await;
    }

    #[tokio::test]
    async fn duplex_request_arrives_before_the_producer_finishes() {
        let ca = ProxyCa::generate("Transmog request streaming h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let first_seen = Arc::new(tokio::sync::Notify::new());
        let (origin, server) =
            spawn_request_streaming_origin(leaf, bind_ip, first_seen.clone()).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 46)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );
        let capacity = NonZeroUsize::new(1).unwrap();
        let (request_sender, request_body) = BodyStream::channel(capacity);
        let request = h3_request(origin, "/request-stream");
        let exchange = tokio::spawn(async move {
            client
                .execute_duplex_streaming(
                    StreamingRequest {
                        head: request.head,
                        body: request_body,
                    },
                    origin.port(),
                    1024,
                    capacity,
                )
                .await
        });
        request_sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"first"))))
            .await
            .unwrap();
        timeout(std::time::Duration::from_millis(500), first_seen.notified())
            .await
            .expect("H3 client buffered request data until producer completion");
        request_sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"second"))))
            .await
            .unwrap();
        drop(request_sender);

        let mut result = timeout(std::time::Duration::from_secs(1), exchange)
            .await
            .expect("streaming H3 request did not complete")
            .unwrap()
            .unwrap();
        assert_eq!(
            result.response.body.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"request-ok"))
        );
        assert!(result.response.body.recv().await.is_none());
        server.finish().await;
    }

    #[tokio::test]
    async fn pooled_connection_multiplexes_concurrent_requests() {
        let ca = ProxyCa::generate("Transmog pooled h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin, server) = spawn_multiplex_origin(leaf, bind_ip).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 43)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );

        let slow_client = client.clone();
        let slow = tokio::spawn(async move {
            slow_client
                .execute(h3_request(origin, "/slow"), origin.port(), 1024)
                .await
        });
        let fast = timeout(
            std::time::Duration::from_secs(1),
            client.execute(h3_request(origin, "/fast"), origin.port(), 1024),
        )
        .await
        .expect("fast H3 stream was blocked by the concurrent slow stream")
        .unwrap();
        assert_eq!(
            fast.response.body,
            vec![BodyFrame::Data(Bytes::from_static(b"fast"))]
        );
        assert_eq!(fast.telemetry.trust_generation, 43);

        let slow = timeout(std::time::Duration::from_secs(1), slow)
            .await
            .expect("slow H3 stream did not finish")
            .unwrap()
            .unwrap();
        assert_eq!(
            slow.response.body,
            vec![BodyFrame::Data(Bytes::from_static(b"slow"))]
        );
        assert_eq!(slow.telemetry.peer_addr, fast.telemetry.peer_addr);
        server.finish().await;
    }

    #[tokio::test]
    async fn paused_h3_body_consumer_does_not_block_an_unrelated_stream() {
        let ca = ProxyCa::generate("Transmog backpressured h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin, server) = spawn_multiplex_origin(leaf, bind_ip).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 47)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );
        let capacity = NonZeroUsize::new(1).unwrap();

        let slow_client = client.clone();
        let slow = tokio::spawn(async move {
            let (sender, body) = BodyStream::channel(capacity);
            drop(sender);
            slow_client
                .execute_duplex_streaming(
                    StreamingRequest {
                        head: h3_request(origin, "/slow").head,
                        body,
                    },
                    origin.port(),
                    1024,
                    capacity,
                )
                .await
        });
        let fast_client = client.clone();
        let fast = tokio::spawn(async move {
            let (sender, body) = BodyStream::channel(capacity);
            drop(sender);
            fast_client
                .execute_duplex_streaming(
                    StreamingRequest {
                        head: h3_request(origin, "/fast").head,
                        body,
                    },
                    origin.port(),
                    1024,
                    capacity,
                )
                .await
        });

        let mut slow = timeout(std::time::Duration::from_secs(1), slow)
            .await
            .expect("slow H3 response head was not received")
            .unwrap()
            .unwrap();
        // Deliberately leave the slow response body queued at capacity while
        // the unrelated stream completes on the same QUIC connection.
        let mut fast = timeout(std::time::Duration::from_secs(1), fast)
            .await
            .expect("fast H3 stream was blocked by the paused body consumer")
            .unwrap()
            .unwrap();
        assert_eq!(
            fast.response.body.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"fast"))
        );
        assert!(fast.response.body.recv().await.is_none());
        assert_eq!(
            slow.response.body.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"slow"))
        );
        assert!(slow.response.body.recv().await.is_none());
        assert_eq!(slow.telemetry.peer_addr, fast.telemetry.peer_addr);
        server.finish().await;
    }

    #[tokio::test]
    async fn cancelling_one_h3_response_body_does_not_close_an_unrelated_stream() {
        let ca = ProxyCa::generate("Transmog cancelled h3 origin", 2).unwrap();
        let leaf = ca
            .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
        let bind_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let (origin, server) = spawn_multiplex_origin(leaf, bind_ip).await;
        let trust = Arc::new(
            TrustSnapshot::load(&StaticTrust(vec![ca.certificate().to_der().unwrap()]), 48)
                .unwrap(),
        );
        let client = H3OriginClient::new(
            UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default()),
            H3TransportLimits {
                idle_timeout: std::time::Duration::from_secs(2),
                ..H3TransportLimits::default()
            },
        );
        let capacity = NonZeroUsize::new(1).unwrap();

        let slow_client = client.clone();
        let slow = tokio::spawn(async move {
            let (sender, body) = BodyStream::channel(capacity);
            drop(sender);
            slow_client
                .execute_duplex_streaming(
                    StreamingRequest {
                        head: h3_request(origin, "/slow").head,
                        body,
                    },
                    origin.port(),
                    1024,
                    capacity,
                )
                .await
        });
        let fast_client = client.clone();
        let fast = tokio::spawn(async move {
            let (sender, body) = BodyStream::channel(capacity);
            drop(sender);
            fast_client
                .execute_duplex_streaming(
                    StreamingRequest {
                        head: h3_request(origin, "/fast").head,
                        body,
                    },
                    origin.port(),
                    1024,
                    capacity,
                )
                .await
        });

        let slow = timeout(std::time::Duration::from_secs(1), slow)
            .await
            .expect("cancelled H3 response head was not received")
            .unwrap()
            .unwrap();
        drop(slow.response.body);
        let mut fast = timeout(std::time::Duration::from_secs(1), fast)
            .await
            .expect("unrelated H3 response head was not received")
            .unwrap()
            .unwrap();
        assert_eq!(
            fast.response.body.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"fast"))
        );
        assert!(fast.response.body.recv().await.is_none());
        server.finish().await;
    }

    fn h3_request(origin: SocketAddr, path: &str) -> CanonicalRequest {
        CanonicalRequest {
            head: RequestHead {
                method: "GET".to_owned(),
                target: Target {
                    scheme: "https".to_owned(),
                    authority: format!("localhost:{}", origin.port()),
                    host: "localhost".to_owned(),
                    port: origin.port(),
                    path: path.to_owned(),
                    query: None,
                },
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("x-h3-proof", "yes").unwrap(),
                ]),
                source_version: HttpLegVersion::Http2,
            },
            body: Vec::new(),
        }
    }

    struct TestOrigin {
        // Retain the bound socket after the fixture task returns so Linux does
        // not deliver an ICMP port-unreachable error before the client has
        // consumed the final QUIC packets.
        socket: Arc<UdpSocket>,
        task: JoinHandle<()>,
    }

    impl TestOrigin {
        fn is_finished(&self) -> bool {
            self.task.is_finished()
        }

        async fn finish(self) {
            let Self { socket, task } = self;
            let result = task.await;
            drop(socket);
            result.unwrap();
        }

        fn abort(self) {
            self.task.abort();
        }
    }

    async fn spawn_origin(
        leaf: transmog_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
    ) -> (SocketAddr, TestOrigin) {
        let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
        tls.set_certificate(&leaf.certificate).unwrap();
        tls.set_private_key(&leaf.private_key).unwrap();
        tls.check_private_key().unwrap();
        let mut config =
            quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();
        config.set_max_idle_timeout(2_000);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(1024 * 1024);
        config.set_initial_max_stream_data_bidi_local(256 * 1024);
        config.set_initial_max_stream_data_bidi_remote(256 * 1024);
        config.set_initial_max_stream_data_uni(256 * 1024);
        config.set_initial_max_streams_bidi(16);
        config.set_initial_max_streams_uni(16);
        config.set_disable_active_migration(true);

        let socket = Arc::new(UdpSocket::bind((bind_ip, 0)).await.unwrap());
        let local = socket.local_addr().unwrap();
        let task_socket = Arc::clone(&socket);
        let task = tokio::spawn(async move {
            run_origin(task_socket, local, config).await;
        });
        (local, TestOrigin { socket, task })
    }

    async fn spawn_multiplex_origin(
        leaf: transmog_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
    ) -> (SocketAddr, TestOrigin) {
        let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
        tls.set_certificate(&leaf.certificate).unwrap();
        tls.set_private_key(&leaf.private_key).unwrap();
        tls.check_private_key().unwrap();
        let mut config =
            quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();
        config.set_max_idle_timeout(2_000);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(1024 * 1024);
        config.set_initial_max_stream_data_bidi_local(256 * 1024);
        config.set_initial_max_stream_data_bidi_remote(256 * 1024);
        config.set_initial_max_stream_data_uni(256 * 1024);
        config.set_initial_max_streams_bidi(16);
        config.set_initial_max_streams_uni(16);
        config.set_disable_active_migration(true);

        let socket = Arc::new(UdpSocket::bind((bind_ip, 0)).await.unwrap());
        let local = socket.local_addr().unwrap();
        let task = tokio::spawn(run_multiplex_origin(Arc::clone(&socket), local, config));
        (local, TestOrigin { socket, task })
    }

    async fn spawn_slow_streaming_origin(
        leaf: transmog_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
        first_sent: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> (SocketAddr, TestOrigin) {
        let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
        tls.set_certificate(&leaf.certificate).unwrap();
        tls.set_private_key(&leaf.private_key).unwrap();
        tls.check_private_key().unwrap();
        let mut config =
            quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();
        config.set_max_idle_timeout(2_000);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(1024 * 1024);
        config.set_initial_max_stream_data_bidi_local(256 * 1024);
        config.set_initial_max_stream_data_bidi_remote(256 * 1024);
        config.set_initial_max_stream_data_uni(256 * 1024);
        config.set_initial_max_streams_bidi(16);
        config.set_initial_max_streams_uni(16);
        config.set_disable_active_migration(true);

        let socket = Arc::new(UdpSocket::bind((bind_ip, 0)).await.unwrap());
        let local = socket.local_addr().unwrap();
        let task = tokio::spawn(run_slow_streaming_origin(
            Arc::clone(&socket),
            local,
            config,
            first_sent,
            release,
        ));
        (local, TestOrigin { socket, task })
    }

    async fn spawn_request_streaming_origin(
        leaf: transmog_tls::IssuedLeaf,
        bind_ip: std::net::IpAddr,
        first_seen: Arc<tokio::sync::Notify>,
    ) -> (SocketAddr, TestOrigin) {
        let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
        tls.set_certificate(&leaf.certificate).unwrap();
        tls.set_private_key(&leaf.private_key).unwrap();
        tls.check_private_key().unwrap();
        let mut config =
            quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();
        config.set_max_idle_timeout(2_000);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(1024 * 1024);
        config.set_initial_max_stream_data_bidi_local(256 * 1024);
        config.set_initial_max_stream_data_bidi_remote(256 * 1024);
        config.set_initial_max_stream_data_uni(256 * 1024);
        config.set_initial_max_streams_bidi(16);
        config.set_initial_max_streams_uni(16);
        config.set_disable_active_migration(true);

        let socket = Arc::new(UdpSocket::bind((bind_ip, 0)).await.unwrap());
        let local = socket.local_addr().unwrap();
        let task = tokio::spawn(run_request_streaming_origin(
            Arc::clone(&socket),
            local,
            config,
            first_seen,
        ));
        (local, TestOrigin { socket, task })
    }

    #[allow(clippy::too_many_lines)]
    async fn run_request_streaming_origin(
        socket: Arc<UdpSocket>,
        local: SocketAddr,
        mut config: quiche::Config,
        first_seen: Arc<tokio::sync::Notify>,
    ) {
        let mut connection = None;
        let h3_config = quiche::h3::Config::new().unwrap();
        let mut http3 = None;
        let mut recv_buffer = vec![0_u8; MAX_UDP_PACKET_SIZE];
        let mut send_buffer = vec![0_u8; MAX_DATAGRAM_SIZE];
        let mut request_body = Vec::new();

        loop {
            let (read, from) = timeout(
                std::time::Duration::from_secs(3),
                socket.recv_from(&mut recv_buffer),
            )
            .await
            .expect("request-streaming H3 client stalled")
            .unwrap();
            if connection.is_none() {
                let header =
                    quiche::Header::from_slice(&mut recv_buffer[..read], quiche::MAX_CONN_ID_LEN)
                        .unwrap();
                assert_eq!(header.ty, quiche::Type::Initial);
                let mut source_id = [0_u8; quiche::MAX_CONN_ID_LEN];
                rand_bytes(&mut source_id).unwrap();
                let source_id = quiche::ConnectionId::from_ref(&source_id);
                connection =
                    Some(quiche::accept(&source_id, None, local, from, &mut config).unwrap());
            }
            let connection = connection.as_mut().unwrap();
            match connection.recv(
                &mut recv_buffer[..read],
                quiche::RecvInfo { from, to: local },
            ) {
                Ok(_) | Err(quiche::Error::Done) => {}
                Err(error) => panic!("request-streaming H3 server recv failed: {error}"),
            }
            if connection.is_established() && http3.is_none() {
                http3 =
                    Some(quiche::h3::Connection::with_transport(connection, &h3_config).unwrap());
            }
            let mut finished_stream = None;
            if let Some(http3) = http3.as_mut() {
                loop {
                    match http3.poll(connection) {
                        Ok((_, quiche::h3::Event::Headers { list, .. })) => {
                            assert!(list.iter().any(|header| {
                                header.name() == b"x-h3-proof" && header.value() == b"yes"
                            }));
                        }
                        Ok((stream_id, quiche::h3::Event::Data)) => loop {
                            match http3.recv_body(connection, stream_id, &mut recv_buffer) {
                                Ok(read) => {
                                    request_body.extend_from_slice(&recv_buffer[..read]);
                                    if request_body == b"first" {
                                        first_seen.notify_one();
                                    }
                                }
                                Err(quiche::h3::Error::Done) => break,
                                Err(error) => {
                                    panic!("request-streaming H3 body failed: {error}")
                                }
                            }
                        },
                        Ok((stream_id, quiche::h3::Event::Finished)) => {
                            finished_stream = Some(stream_id);
                        }
                        Ok((_, _)) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(error) => panic!("request-streaming H3 poll failed: {error}"),
                    }
                }
            }
            if let Some(stream_id) = finished_stream {
                assert_eq!(request_body, b"firstsecond");
                let http3 = http3.as_mut().unwrap();
                let headers = [
                    quiche::h3::Header::new(b":status", b"200"),
                    quiche::h3::Header::new(b"content-type", b"text/plain"),
                ];
                http3
                    .send_response(connection, stream_id, &headers, false)
                    .unwrap();
                http3
                    .send_body(connection, stream_id, b"request-ok", true)
                    .unwrap();
            }
            loop {
                match connection.send(&mut send_buffer) {
                    Ok((written, send_info)) => {
                        socket
                            .send_to(&send_buffer[..written], send_info.to)
                            .await
                            .unwrap();
                    }
                    Err(quiche::Error::Done) => break,
                    Err(error) => panic!("request-streaming H3 server send failed: {error}"),
                }
            }
            if finished_stream.is_some() {
                return;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn run_slow_streaming_origin(
        socket: Arc<UdpSocket>,
        local: SocketAddr,
        mut config: quiche::Config,
        first_sent: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) {
        let mut connection = None;
        let h3_config = quiche::h3::Config::new().unwrap();
        let mut http3 = None;
        let mut recv_buffer = vec![0_u8; MAX_UDP_PACKET_SIZE];
        let mut send_buffer = vec![0_u8; MAX_DATAGRAM_SIZE];
        let mut response_stream = None;

        loop {
            let (read, from) = timeout(
                std::time::Duration::from_secs(3),
                socket.recv_from(&mut recv_buffer),
            )
            .await
            .expect("streaming H3 client stalled")
            .unwrap();
            if connection.is_none() {
                let header =
                    quiche::Header::from_slice(&mut recv_buffer[..read], quiche::MAX_CONN_ID_LEN)
                        .unwrap();
                assert_eq!(header.ty, quiche::Type::Initial);
                let mut source_id = [0_u8; quiche::MAX_CONN_ID_LEN];
                rand_bytes(&mut source_id).unwrap();
                let source_id = quiche::ConnectionId::from_ref(&source_id);
                connection =
                    Some(quiche::accept(&source_id, None, local, from, &mut config).unwrap());
            }
            let connection = connection.as_mut().unwrap();
            match connection.recv(
                &mut recv_buffer[..read],
                quiche::RecvInfo { from, to: local },
            ) {
                Ok(_) | Err(quiche::Error::Done) => {}
                Err(error) => panic!("streaming H3 server recv failed: {error}"),
            }
            if connection.is_established() && http3.is_none() {
                http3 =
                    Some(quiche::h3::Connection::with_transport(connection, &h3_config).unwrap());
            }
            if let Some(http3) = http3.as_mut() {
                loop {
                    match http3.poll(connection) {
                        Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                            assert!(list.iter().any(|header| {
                                header.name() == b"x-h3-proof" && header.value() == b"yes"
                            }));
                            let headers = [
                                quiche::h3::Header::new(b":status", b"200"),
                                quiche::h3::Header::new(b"content-type", b"text/plain"),
                            ];
                            http3
                                .send_response(connection, stream_id, &headers, false)
                                .unwrap();
                            http3
                                .send_body(connection, stream_id, b"first", false)
                                .unwrap();
                            response_stream = Some(stream_id);
                        }
                        Ok((stream_id, quiche::h3::Event::Data)) => loop {
                            match http3.recv_body(connection, stream_id, &mut recv_buffer) {
                                Ok(_) => {}
                                Err(quiche::h3::Error::Done) => break,
                                Err(error) => panic!("streaming H3 request body failed: {error}"),
                            }
                        },
                        Ok((_, _)) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(error) => panic!("streaming H3 server poll failed: {error}"),
                    }
                }
            }
            loop {
                match connection.send(&mut send_buffer) {
                    Ok((written, send_info)) => {
                        socket
                            .send_to(&send_buffer[..written], send_info.to)
                            .await
                            .unwrap();
                    }
                    Err(quiche::Error::Done) => break,
                    Err(error) => panic!("streaming H3 server send failed: {error}"),
                }
            }
            if let Some(stream_id) = response_stream {
                first_sent.notify_one();
                release.notified().await;
                let http3 = http3.as_mut().unwrap();
                http3
                    .send_body(connection, stream_id, b"second", true)
                    .unwrap();
                loop {
                    match connection.send(&mut send_buffer) {
                        Ok((written, send_info)) => {
                            socket
                                .send_to(&send_buffer[..written], send_info.to)
                                .await
                                .unwrap();
                        }
                        Err(quiche::Error::Done) => break,
                        Err(error) => panic!("streaming H3 server final send failed: {error}"),
                    }
                }
                return;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn run_multiplex_origin(
        socket: Arc<UdpSocket>,
        local: SocketAddr,
        mut config: quiche::Config,
    ) {
        let mut connection = None;
        let h3_config = quiche::h3::Config::new().unwrap();
        let mut http3 = None;
        let mut recv_buffer = vec![0_u8; MAX_UDP_PACKET_SIZE];
        let mut send_buffer = vec![0_u8; MAX_DATAGRAM_SIZE];
        let mut slow_stream = None;
        let mut fast_stream = None;
        let mut responses_sent = 0_u8;

        loop {
            let (read, from) = timeout(
                std::time::Duration::from_secs(3),
                socket.recv_from(&mut recv_buffer),
            )
            .await
            .expect("pooled H3 client stalled")
            .unwrap();
            if connection.is_none() {
                let header =
                    quiche::Header::from_slice(&mut recv_buffer[..read], quiche::MAX_CONN_ID_LEN)
                        .unwrap();
                assert_eq!(header.ty, quiche::Type::Initial);
                let mut source_id = [0_u8; quiche::MAX_CONN_ID_LEN];
                rand_bytes(&mut source_id).unwrap();
                let source_id = quiche::ConnectionId::from_ref(&source_id);
                connection =
                    Some(quiche::accept(&source_id, None, local, from, &mut config).unwrap());
            }
            let connection = connection.as_mut().unwrap();
            match connection.recv(
                &mut recv_buffer[..read],
                quiche::RecvInfo { from, to: local },
            ) {
                Ok(_) | Err(quiche::Error::Done) => {}
                Err(error) => panic!("pooled H3 server recv failed: {error}"),
            }
            if connection.is_established() && http3.is_none() {
                http3 =
                    Some(quiche::h3::Connection::with_transport(connection, &h3_config).unwrap());
            }
            if let Some(http3) = http3.as_mut() {
                loop {
                    match http3.poll(connection) {
                        Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                            assert!(list.iter().any(|header| {
                                header.name() == b"x-h3-proof" && header.value() == b"yes"
                            }));
                            let path = pseudo_value(&list, b":path").unwrap().unwrap();
                            if path == b"/slow" {
                                slow_stream = Some(stream_id);
                            } else if path == b"/fast" {
                                fast_stream = Some(stream_id);
                            } else {
                                panic!("unexpected pooled H3 request path");
                            }
                            if fast_stream.is_some() && slow_stream.is_some() {
                                let fast = fast_stream.take().unwrap();
                                let slow = slow_stream.take().unwrap();
                                send_fixture_response(http3, connection, fast, b"fast");
                                send_fixture_response(http3, connection, slow, b"slow");
                                responses_sent = 2;
                            }
                        }
                        Ok((stream_id, quiche::h3::Event::Data)) => loop {
                            match http3.recv_body(connection, stream_id, &mut recv_buffer) {
                                Ok(_) => {}
                                Err(quiche::h3::Error::Done) => break,
                                Err(error) => panic!("pooled H3 request body failed: {error}"),
                            }
                        },
                        Ok((_, _)) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(error) => panic!("pooled H3 server poll failed: {error}"),
                    }
                }
            }
            loop {
                match connection.send(&mut send_buffer) {
                    Ok((written, send_info)) => {
                        socket
                            .send_to(&send_buffer[..written], send_info.to)
                            .await
                            .unwrap();
                    }
                    Err(quiche::Error::Done) => break,
                    Err(error) => panic!("pooled H3 server send failed: {error}"),
                }
            }
            if responses_sent == 2 {
                return;
            }
        }
    }

    fn send_fixture_response(
        http3: &mut quiche::h3::Connection,
        connection: &mut quiche::Connection,
        stream_id: u64,
        body: &[u8],
    ) {
        let headers = [
            quiche::h3::Header::new(b":status", b"200"),
            quiche::h3::Header::new(b"content-type", b"text/plain"),
        ];
        http3
            .send_response(connection, stream_id, &headers, false)
            .unwrap();
        http3.send_body(connection, stream_id, body, true).unwrap();
    }

    async fn run_origin(socket: Arc<UdpSocket>, local: SocketAddr, mut config: quiche::Config) {
        let mut connection = None;
        let h3_config = quiche::h3::Config::new().unwrap();
        let mut h3_connection = None;
        let mut recv_buffer = vec![0_u8; MAX_UDP_PACKET_SIZE];
        let mut send_buffer = vec![0_u8; MAX_DATAGRAM_SIZE];
        let mut response_sent = false;

        loop {
            let (read, from) = timeout(
                std::time::Duration::from_secs(3),
                socket.recv_from(&mut recv_buffer),
            )
            .await
            .expect("H3 client stalled")
            .unwrap();
            if connection.is_none() {
                let header =
                    quiche::Header::from_slice(&mut recv_buffer[..read], quiche::MAX_CONN_ID_LEN)
                        .unwrap();
                assert_eq!(header.ty, quiche::Type::Initial);
                let mut source_id = [0_u8; quiche::MAX_CONN_ID_LEN];
                rand_bytes(&mut source_id).unwrap();
                let source_id = quiche::ConnectionId::from_ref(&source_id);
                connection =
                    Some(quiche::accept(&source_id, None, local, from, &mut config).unwrap());
            }
            let connection = connection.as_mut().unwrap();
            let info = quiche::RecvInfo { from, to: local };
            match connection.recv(&mut recv_buffer[..read], info) {
                Ok(_) | Err(quiche::Error::Done) => {}
                Err(error) => panic!("H3 server recv failed: {error}"),
            }
            if connection.is_established() && h3_connection.is_none() {
                h3_connection =
                    Some(quiche::h3::Connection::with_transport(connection, &h3_config).unwrap());
            }
            if let Some(http3) = h3_connection.as_mut() {
                loop {
                    match http3.poll(connection) {
                        Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                            assert!(list.iter().any(|header| {
                                header.name() == b"x-h3-proof" && header.value() == b"yes"
                            }));
                            let headers = [
                                quiche::h3::Header::new(b":status", b"200"),
                                quiche::h3::Header::new(b"content-type", b"text/plain"),
                            ];
                            http3
                                .send_response(connection, stream_id, &headers, false)
                                .unwrap();
                            http3
                                .send_body(connection, stream_id, b"h3-ok", true)
                                .unwrap();
                            response_sent = true;
                        }
                        Ok((stream_id, quiche::h3::Event::Data)) => loop {
                            match http3.recv_body(connection, stream_id, &mut recv_buffer) {
                                Ok(_) => {}
                                Err(quiche::h3::Error::Done) => break,
                                Err(error) => panic!("H3 request body failed: {error}"),
                            }
                        },
                        Ok((_, _)) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(error) => panic!("H3 server poll failed: {error}"),
                    }
                }
            }
            loop {
                match connection.send(&mut send_buffer) {
                    Ok((written, send_info)) => {
                        socket
                            .send_to(&send_buffer[..written], send_info.to)
                            .await
                            .unwrap();
                    }
                    Err(quiche::Error::Done) => break,
                    Err(error) => panic!("H3 server send failed: {error}"),
                }
            }
            if response_sent {
                return;
            }
        }
    }
}

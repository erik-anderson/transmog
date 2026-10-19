//! Observed CONNECT byte forwarding when HTTPS decryption is disabled.
use super::{
    ActivityKind, Arc, CompletedExchange, ConnectAuthority, ConnectionContext, ConnectionMetrics,
    DownstreamBody, ExchangeBoundary, ExchangeFailure, ExchangeFailureKind, ExchangeMetadata,
    ExchangeObserver, ExchangeStage, HappyEyeballsConfig, HeaderBlock, HttpLegVersion, Incoming,
    Instant, MeteredIo, Milestone, ObserverEventKind, PerformanceRecorder, ProtocolObservation,
    ProxyRuntimeError, ProxyState, Request, RequestCancellation, RequestHead, Response,
    ResponseHead, RuntimeIdKind, SessionId, SessionMetadata, StatusCode, StreamId, Target,
    TcpStream, TokioIo, TransportObservation, TransportOutcome, block_from_headers,
    draining_response, timeout, websocket_request_has_body,
};

impl ProxyState {
    #[allow(clippy::too_many_lines)] // CONNECT admission and the eventual relay terminal event share one owned context.
    pub(super) async fn handle_opaque_connect(
        self: Arc<Self>,
        mut request: Request<Incoming>,
        context: ConnectionContext,
        authority: ConnectAuthority,
        cancellation: &mut RequestCancellation,
    ) -> Result<Response<DownstreamBody>, ProxyRuntimeError> {
        let Some(activity) = self.activity.acquire(ActivityKind::Transport) else {
            return Ok(draining_response());
        };
        let head = RequestHead {
            method: "CONNECT".into(),
            target: Target {
                scheme: "https".into(),
                host: authority.host().into(),
                port: authority.port(),
                authority: authority.to_string(),
                path: "/".into(),
                query: None,
            },
            headers: block_from_headers(request.headers())?,
            source_version: HttpLegVersion::Http1,
        };
        let session = SessionMetadata {
            session_id: SessionId(self.ids.next_id(RuntimeIdKind::Exchange)),
            downstream_connection_id: context.connection_id,
            stream_id: StreamId(self.ids.next_id(RuntimeIdKind::Stream)),
            client_addr: context.client_addr,
            client_identity: context.client_identity.clone(),
            proxy_addr: context.proxy_addr,
            ingress_version: HttpLegVersion::Http1,
            egress_version: None,
        };
        let metadata = Arc::new(ExchangeMetadata::from_session_at(
            &session,
            head.target.clone(),
            self.clock.system_time(),
        ));
        let observer = Arc::new(self.observers.start_exchange(Arc::clone(&metadata)));
        let performance = PerformanceRecorder::new(self.clock.system_time(), self.clock.instant());
        performance.mark_at(Milestone::ClientConnected, context.accepted_at);
        performance.mark(Milestone::RequestHeaders);
        performance.mark(Milestone::ClientRequestDone);
        performance.protocol(ProtocolObservation {
            boundary: "client-request".into(),
            version: format!("{:?}", request.version()),
            reason: None,
        });
        let manual = cancellation.manual.clone();
        let request_control = self.requests.register(metadata.exchange_id, manual.clone());
        observer
            .emit(ObserverEventKind::ExchangeStarted {
                metadata: Arc::clone(&metadata),
            })
            .await;
        cancellation.start(
            Arc::clone(&observer),
            Arc::clone(&metadata),
            performance.clone(),
        );
        observer
            .emit(ObserverEventKind::RequestHeadObserved {
                boundary: ExchangeBoundary::ClientRequest,
                head: head.clone(),
            })
            .await;
        observer
            .emit(ObserverEventKind::RequestHeadFinalized(head.clone()))
            .await;
        if websocket_request_has_body(&head.headers)? {
            let error = ProxyRuntimeError::ConnectRequestBodyUnsupported;
            finish_failed_tunnel(
                &observer,
                &performance,
                metadata,
                error.to_string(),
                false,
                false,
                ExchangeFailureKind::Upstream,
            )
            .await;
            return Err(error);
        }
        observer
            .emit(ObserverEventKind::BodyCompleted {
                boundary: ExchangeBoundary::ClientRequest,
            })
            .await;
        observer
            .emit(ObserverEventKind::RouteSelected {
                policy_id: "connect-tunnel".into(),
                reason: "HTTPS decryption disabled; forwarding an opaque TCP tunnel".into(),
            })
            .await;
        let mut facts = TransportObservation {
            leg: "upstream".into(),
            connection_id: format!("tunnel-{}", session.session_id.0),
            tunnel_destination: Some(authority.to_string()),
            ..TransportObservation::default()
        };
        performance.mark(Milestone::UpstreamBegin);
        let connected = timeout(
            self.config.limits.tls_handshake_timeout,
            connect_destination(&authority, self.config.happy_eyeballs, &mut facts),
        )
        .await;
        let origin = match connected {
            Ok(Ok(origin)) => origin,
            result => {
                let error = match result {
                    Ok(Err(error)) => error,
                    Err(_) => std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "CONNECT destination connection timed out",
                    ),
                    _ => unreachable!(),
                };
                performance.transport(facts);
                finish_failed_tunnel(
                    &observer,
                    &performance,
                    metadata,
                    error.to_string(),
                    false,
                    false,
                    ExchangeFailureKind::Upstream,
                )
                .await;
                return Err(error.into());
            }
        };
        facts.peer = origin.peer_addr().ok().map(|address| address.to_string());
        facts.local = origin.local_addr().ok().map(|address| address.to_string());
        facts.outcome = TransportOutcome::Connected;
        let metrics = Arc::new(ConnectionMetrics::default());
        let mut origin = MeteredIo::new_tcp(origin, Arc::clone(&metrics));
        performance.mark(Milestone::UpstreamConnected);
        performance.transport(facts.clone());
        let response_head = ResponseHead {
            status: 200,
            headers: HeaderBlock::default(),
            source_version: HttpLegVersion::Http1,
        };
        observer
            .emit(ObserverEventKind::ResponseHeadObserved {
                boundary: ExchangeBoundary::ClientResponse,
                head: response_head.clone(),
            })
            .await;
        observer
            .emit(ObserverEventKind::ResponseHeadFinalized(
                response_head.clone(),
            ))
            .await;
        observer
            .emit(ObserverEventKind::BodyCompleted {
                boundary: ExchangeBoundary::ClientResponse,
            })
            .await;
        performance.protocol(ProtocolObservation {
            boundary: "client-response".into(),
            version: format!("{:?}", request.version()),
            reason: Some("OK".into()),
        });
        performance.mark(Milestone::ClientResponseQueued);
        observer
            .emit(ObserverEventKind::Performance(performance.snapshot()))
            .await;
        let upgraded = hyper::upgrade::on(&mut request);
        let state = Arc::clone(&self);
        let mut tunnels = self.tunnels.lock().await;
        while tunnels.try_join_next().is_some() {}
        tunnels.spawn(async move {
            let _activity = activity;
            let _request_control = request_control;
            let transfer = async {
                let client = upgraded.await.map_err(std::io::Error::other)?;
                let mut client = TokioIo::new(client);
                tokio::io::copy_bidirectional(&mut client, &mut origin).await
            };
            let (result, failure_kind) = tokio::select! {
                result = transfer => (result, ExchangeFailureKind::Upstream),
                () = manual.cancelled() => (Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "CONNECT tunnel canceled in Transmog.")), ExchangeFailureKind::Cancelled),
                () = state.final_shutdown.cancelled() => (Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "CONNECT tunnel stopped")), ExchangeFailureKind::Shutdown),
            };
            // Release the origin before observer backpressure can delay terminal evidence.
            drop(origin);
            facts.bytes_written = Some(metrics.bytes_written());
            facts.bytes_read = Some(metrics.bytes_read());
            metrics.observe(&mut facts, &performance);
            match result {
                Ok(_) => {
                    performance.transport(facts);
                    performance.mark(Milestone::ExchangeDone);
                    observer
                        .emit(ObserverEventKind::Performance(performance.snapshot()))
                        .await;
                    observer
                        .emit(ObserverEventKind::Completed(CompletedExchange {
                            metadata,
                            request_head: head,
                            response_head,
                        }))
                        .await;
                }
                Err(error) => {
                    performance.transport(facts);
                    finish_failed_tunnel(
                        &observer,
                        &performance,
                        metadata,
                        error.to_string(),
                        metrics.bytes_written() > 0,
                        true,
                        failure_kind,
                    )
                    .await;
                }
            }
        });
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(DownstreamBody::empty())?)
    }
}

async fn connect_destination(
    authority: &ConnectAuthority,
    happy: HappyEyeballsConfig,
    facts: &mut TransportObservation,
) -> Result<TcpStream, std::io::Error> {
    facts.outcome = TransportOutcome::DnsFailed;
    let begin = Instant::now();
    let resolved = tokio::net::lookup_host((authority.host(), authority.port())).await;
    facts.dns_micros = Some(u64::try_from(begin.elapsed().as_micros()).unwrap_or(u64::MAX));
    let addresses = transmog_network::interleave_candidates(resolved?, happy.max_candidates());
    facts.outcome = TransportOutcome::TcpFailed;
    let begin = Instant::now();
    let connected = transmog_network::race_candidates(addresses, happy, TcpStream::connect).await;
    facts.tcp_micros = Some(u64::try_from(begin.elapsed().as_micros()).unwrap_or(u64::MAX));
    connected
        .map(|(socket, _)| socket)
        .map_err(std::io::Error::other)
}

async fn finish_failed_tunnel(
    observer: &ExchangeObserver,
    performance: &PerformanceRecorder,
    metadata: Arc<ExchangeMetadata>,
    message: String,
    request_committed: bool,
    response_committed: bool,
    kind: ExchangeFailureKind,
) {
    performance.mark(Milestone::ExchangeDone);
    observer
        .emit(ObserverEventKind::Performance(performance.snapshot()))
        .await;
    observer
        .emit(ObserverEventKind::Failed(ExchangeFailure {
            metadata,
            stage: if matches!(
                kind,
                ExchangeFailureKind::Cancelled | ExchangeFailureKind::Shutdown
            ) {
                ExchangeStage::Terminal
            } else {
                ExchangeStage::Upstream
            },
            kind,
            request_committed,
            response_committed,
            message,
        }))
        .await;
}

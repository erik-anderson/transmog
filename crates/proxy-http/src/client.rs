use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Request, Version, header::HOST, uri::InvalidUri};
use http_body::{Body, Frame};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::{
    client::legacy::{Client, Error as ClientError},
    rt::TokioExecutor,
};
use thiserror::Error;
use tokio::time::timeout;
use transmog_core::{
    BodyFrame, BodyStream, BodyStreamError, CanonicalRequest, CanonicalResponse, HeaderBlock,
    HeaderField, HttpLegVersion, MessageKind, RequestHead, ResponseHead, StreamingRequest,
    StreamingResponse, TranslationOptions, prepare_headers,
};
use transmog_network::HappyEyeballsConfig;
use transmog_tls::{TrustError, UpstreamTlsContextFactory};

use crate::{
    HyperEgressMode,
    metrics::{ConnectionObservation, MeasuredHttpsConnector},
};
use transmog_core::performance::{Milestone, PerformanceRecorder, ProtocolObservation};

#[derive(Clone)]
struct ResponseObservation {
    connection: ConnectionObservation,
    shared: bool,
    performance: PerformanceRecorder,
}
impl Drop for ResponseObservation {
    fn drop(&mut self) {
        self.performance.transport(
            self.connection
                .snapshot(self.shared, Some(&self.performance)),
        );
    }
}

type OriginClient = Client<MeasuredHttpsConnector, CanonicalHttpBody>;

/// Pooled Hyper origin client with distinct ALPN pools for forced H1 and H2.
#[derive(Clone)]
pub struct HyperOriginClient {
    h1: OriginClient,
    h2: OriginClient,
    auto: OriginClient,
    performance: Option<PerformanceRecorder>,
}

/// Result of an HTTP/1.1 upgrade attempt.
pub enum HyperUpgradeResponse {
    /// Origin accepted the protocol switch and exposed the upgraded stream.
    Switched {
        /// Canonical 101 response head.
        head: ResponseHead,
        /// Future resolving to the origin byte stream.
        upgraded: hyper::upgrade::OnUpgrade,
    },
    /// Origin declined the switch with an ordinary streaming response.
    Rejected(StreamingResponse),
}

impl std::fmt::Debug for HyperUpgradeResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Switched { head, .. } => formatter
                .debug_struct("Switched")
                .field("head", head)
                .finish_non_exhaustive(),
            Self::Rejected(response) => formatter.debug_tuple("Rejected").field(response).finish(),
        }
    }
}

impl HyperOriginClient {
    /// Shares an exchange-local recorder while keeping the same physical pools.
    #[must_use]
    pub fn with_performance(mut self, performance: Option<PerformanceRecorder>) -> Self {
        self.performance = performance;
        self
    }

    async fn request(
        &self,
        mut outgoing: Request<CanonicalHttpBody>,
        mode: HyperEgressMode,
    ) -> Result<http::Response<Incoming>, HyperOriginError> {
        let client = match mode {
            HyperEgressMode::Http1Only => &self.h1,
            HyperEgressMode::Http2Only => &self.h2,
            HyperEgressMode::Auto => &self.auto,
        };
        let mut captured = hyper_util::client::legacy::connect::capture_connection(&mut outgoing);
        let response = client.request(outgoing);
        tokio::pin!(response);
        let mut assigned = None;
        let early = tokio::select! {
            result = &mut response => Some(result),
            connected = captured.wait_for_connection_metadata() => {
                if let Some(connected) = connected.as_ref() {
                    let mut extras = http::Extensions::new();
                    connected.get_extras(&mut extras);
                    if let Some(connection) = extras.get::<ConnectionObservation>() {
                        let shared = connection.claim();
                        assigned = Some((connection.clone(), shared));
                        if let Some(performance) = &self.performance {
                            performance.transport(connection.snapshot(shared,Some(performance)));
                            performance.mark(Milestone::UpstreamConnected);
                            performance.protocol(ProtocolObservation {boundary: "upstream-request".into(), version: if connected.is_negotiated_h2() || mode == HyperEgressMode::Http2Only {"HTTP/2"} else {"HTTP/1.1"}.into(), reason: None});
                        }
                    }
                }
                None
            }
        };
        let result = match early {
            Some(result) => result,
            None => response.await,
        };
        if let Err(error) = &result
            && let Some(performance) = &self.performance
            && let Some(observation) = crate::metrics::failed_observation(error, performance)
        {
            performance.transport(observation);
        }
        let mut response = result?;
        let connection = assigned.or_else(|| {
            response
                .extensions()
                .get::<ConnectionObservation>()
                .map(|connection| (connection.clone(), connection.claim()))
        });
        if let Some(performance) = &self.performance {
            if let Some((connection, shared)) = connection {
                response.extensions_mut().insert(ResponseObservation {
                    connection: connection.clone(),
                    shared,
                    performance: performance.clone(),
                });
                // The actual response protocol also identifies the protocol used by Hyper for this exchange.
                performance.protocol(ProtocolObservation {
                    boundary: "upstream-request".into(),
                    version: if response.version() == Version::HTTP_2 {
                        "HTTP/2"
                    } else {
                        "HTTP/1.1"
                    }
                    .into(),
                    reason: None,
                });
                performance.transport(connection.snapshot(shared, Some(performance)));
            }
            performance.mark(Milestone::ResponseHeaders);
            performance.protocol(ProtocolObservation {
                boundary: "upstream-response".into(),
                version: format!("{:?}", response.version()),
                reason: response
                    .extensions()
                    .get::<hyper::ext::ReasonPhrase>()
                    .map(|reason| String::from_utf8_lossy(reason.as_bytes()).into_owned()),
            });
        }
        Ok(response)
    }

    /// Builds all Hyper pools from one immutable TLS policy generation.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] if a `BoringSSL` connector cannot be
    /// constructed from the shared context factory.
    pub fn new(factory: &UpstreamTlsContextFactory) -> Result<Self, HyperOriginError> {
        Self::with_happy_eyeballs(factory, HappyEyeballsConfig::default())
    }

    /// Builds all Hyper pools with an explicit shared Happy Eyeballs policy.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] if a `BoringSSL` connector cannot be
    /// constructed from the shared context factory.
    pub fn with_happy_eyeballs(
        factory: &UpstreamTlsContextFactory,
        happy_eyeballs: HappyEyeballsConfig,
    ) -> Result<Self, HyperOriginError> {
        let h1 = Client::builder(TokioExecutor::new()).build(crate::metrics::connector(
            factory,
            HyperEgressMode::Http1Only,
            happy_eyeballs,
        )?);
        let mut h2_builder = Client::builder(TokioExecutor::new());
        h2_builder.http2_only(true);
        let h2 = h2_builder.build(crate::metrics::connector(
            factory,
            HyperEgressMode::Http2Only,
            happy_eyeballs,
        )?);
        let auto = Client::builder(TokioExecutor::new()).build(crate::metrics::connector(
            factory,
            HyperEgressMode::Auto,
            happy_eyeballs,
        )?);
        Ok(Self {
            h1,
            h2,
            auto,
            performance: None,
        })
    }

    /// Sends one explicitly bounded canonical exchange through Hyper.
    ///
    /// The request is already buffered by the caller under policy. Response
    /// buffering is bounded by `max_response_bytes`; trailers remain distinct
    /// frames.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] for invalid canonical data, TLS/transport
    /// failure, or a response that exceeds the configured bound.
    pub async fn execute(
        &self,
        request: CanonicalRequest,
        mode: HyperEgressMode,
        max_response_bytes: usize,
        body_idle_timeout: Duration,
    ) -> Result<CanonicalResponse, HyperOriginError> {
        let destination = match mode {
            HyperEgressMode::Http1Only => HttpLegVersion::Http1,
            HyperEgressMode::Http2Only | HyperEgressMode::Auto => HttpLegVersion::Http2,
        };
        let mut headers = prepare_headers(
            &request.head.headers,
            TranslationOptions {
                destination,
                kind: MessageKind::Request,
                body_modified: false,
                force_identity_encoding: false,
            },
        )?;
        if destination == HttpLegVersion::Http1 {
            headers.replace_all(HeaderField::try_new(
                HOST.as_str(),
                request.head.target.authority.as_bytes(),
            )?);
        }

        let uri: http::Uri = format!(
            "{}://{}{}{}",
            request.head.target.scheme,
            request.head.target.authority,
            request.head.target.path,
            request
                .head
                .target
                .query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
        .parse()?;
        let mut outgoing = Request::builder()
            .method(request.head.method.as_str())
            .uri(uri)
            .body(CanonicalHttpBody::new(request.body)?.observed(self.performance.clone()))?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let mut response = self.request(outgoing, mode).await?;
        let source_version = protocol_version(response.version())?;
        let status = response.status().as_u16();
        let headers = block_from_headers(response.headers())?;
        let body = collect_incoming(
            response.body_mut(),
            max_response_bytes,
            body_idle_timeout,
            self.performance.as_ref(),
        )
        .await?;
        if let Some(performance) = &self.performance {
            performance.mark(Milestone::UpstreamResponseDone);
        }
        Ok(CanonicalResponse {
            head: ResponseHead {
                status,
                headers,
                source_version,
            },
            body,
        })
    }

    /// Starts one backpressured canonical exchange through a pooled Hyper
    /// connection and returns as soon as the final response head is available.
    ///
    /// Request and response bodies cross bounded frame channels. The response
    /// producer stops reading Hyper when the consumer is paused, preserving
    /// transport flow control instead of accumulating an exchange in memory.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] for invalid canonical data or a failure
    /// before the final response head is received. Later body failures are
    /// carried in-band by [`StreamingResponse::body`].
    pub async fn execute_streaming(
        &self,
        request: StreamingRequest,
        mode: HyperEgressMode,
        max_response_bytes: usize,
        body_channel_capacity: NonZeroUsize,
        body_idle_timeout: Duration,
    ) -> Result<StreamingResponse, HyperOriginError> {
        let destination = match mode {
            HyperEgressMode::Http1Only => HttpLegVersion::Http1,
            HyperEgressMode::Http2Only | HyperEgressMode::Auto => HttpLegVersion::Http2,
        };
        let mut headers = prepare_headers(
            &request.head.headers,
            TranslationOptions {
                destination,
                kind: MessageKind::Request,
                body_modified: false,
                force_identity_encoding: false,
            },
        )?;
        if destination == HttpLegVersion::Http1 {
            headers.replace_all(HeaderField::try_new(
                HOST.as_str(),
                request.head.target.authority.as_bytes(),
            )?);
        }
        let uri: http::Uri = format!(
            "{}://{}{}{}",
            request.head.target.scheme,
            request.head.target.authority,
            request.head.target.path,
            request
                .head
                .target
                .query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
        .parse()?;
        let mut outgoing = Request::builder()
            .method(request.head.method.as_str())
            .uri(uri)
            .body(CanonicalHttpBody::streaming(request.body).observed(self.performance.clone()))?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let response = self.request(outgoing, mode).await?;
        let source_version = protocol_version(response.version())?;
        let head = ResponseHead {
            status: response.status().as_u16(),
            headers: block_from_headers(response.headers())?,
            source_version,
        };
        let (sender, body) = BodyStream::channel(body_channel_capacity);
        let observation = response.extensions().get::<ResponseObservation>().cloned();
        tokio::spawn(stream_incoming(
            response.into_body(),
            sender,
            max_response_bytes,
            body_idle_timeout,
            self.performance.clone(),
            observation,
        ));
        Ok(StreamingResponse { head, body })
    }

    /// Sends an HTTP/1.1 upgrade request while retaining Hyper's upgraded
    /// origin stream. Hop-by-hop `Connection` and `Upgrade` fields are
    /// intentionally preserved; proxy-only authorization fields are removed.
    ///
    /// A rejected upgrade remains an ordinary bounded streaming response.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] for invalid canonical data or a transport
    /// failure before the response head is available.
    pub async fn execute_upgrade(
        &self,
        request: RequestHead,
        max_response_bytes: usize,
        body_channel_capacity: NonZeroUsize,
        body_idle_timeout: Duration,
    ) -> Result<HyperUpgradeResponse, HyperOriginError> {
        let mut headers = prepare_headers(
            &request.headers,
            TranslationOptions {
                destination: HttpLegVersion::Http1,
                kind: MessageKind::Request,
                body_modified: false,
                force_identity_encoding: false,
            },
        )?;
        headers.replace_all(HeaderField::try_new("connection", "Upgrade")?);
        headers.replace_all(HeaderField::try_new("upgrade", "websocket")?);
        headers.replace_all(HeaderField::try_new(
            HOST.as_str(),
            request.target.authority.as_bytes(),
        )?);
        let uri: http::Uri = format!(
            "{}://{}{}{}",
            request.target.scheme,
            request.target.authority,
            request.target.path,
            request
                .target
                .query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
        .parse()?;
        let mut outgoing = Request::builder()
            .method(request.method.as_str())
            .uri(uri)
            .version(Version::HTTP_11)
            .body(CanonicalHttpBody::new(Vec::new())?.observed(self.performance.clone()))?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let mut response = self.request(outgoing, HyperEgressMode::Http1Only).await?;
        let source_version = protocol_version(response.version())?;
        let head = ResponseHead {
            status: response.status().as_u16(),
            headers: block_from_headers(response.headers())?,
            source_version,
        };
        if response.status() == http::StatusCode::SWITCHING_PROTOCOLS {
            return Ok(HyperUpgradeResponse::Switched {
                head,
                upgraded: hyper::upgrade::on(&mut response),
            });
        }
        let (sender, body) = BodyStream::channel(body_channel_capacity);
        let observation = response.extensions().get::<ResponseObservation>().cloned();
        tokio::spawn(stream_incoming(
            response.into_body(),
            sender,
            max_response_bytes,
            body_idle_timeout,
            self.performance.clone(),
            observation,
        ));
        Ok(HyperUpgradeResponse::Rejected(StreamingResponse {
            head,
            body,
        }))
    }
}

#[derive(Debug)]
struct CanonicalHttpBody {
    performance: Option<PerformanceRecorder>,
    inner: CanonicalHttpBodyInner,
}

#[derive(Debug)]
enum CanonicalHttpBodyInner {
    Buffered(VecDeque<Frame<Bytes>>),
    Streaming(BodyStream),
}

impl CanonicalHttpBody {
    fn observed(mut self, performance: Option<PerformanceRecorder>) -> Self {
        self.performance = performance;
        self
    }
    fn new(frames: Vec<BodyFrame>) -> Result<Self, HyperOriginError> {
        let mut output = VecDeque::with_capacity(frames.len());
        for frame in frames {
            output.push_back(match frame {
                BodyFrame::Data(data) => Frame::data(data),
                BodyFrame::Trailers(trailers) => Frame::trailers(map_from_block(&trailers)?),
            });
        }
        Ok(Self {
            performance: None,
            inner: CanonicalHttpBodyInner::Buffered(output),
        })
    }

    fn streaming(body: BodyStream) -> Self {
        Self {
            performance: None,
            inner: CanonicalHttpBodyInner::Streaming(body),
        }
    }
}

impl Body for CanonicalHttpBody {
    type Data = Bytes;
    type Error = BodyStreamError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let result = match &mut self.inner {
            CanonicalHttpBodyInner::Buffered(frames) => Poll::Ready(frames.pop_front().map(Ok)),
            CanonicalHttpBodyInner::Streaming(body) => match body.poll_recv(context) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(
                    canonical_to_http_frame(frame)
                        .map_err(|error| BodyStreamError::Failed(error.to_string())),
                )),
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        };
        if (matches!(&result, Poll::Ready(None))
            || matches!(&result, Poll::Ready(Some(Ok(_)))) && self.is_end_stream())
            && let Some(performance) = &self.performance
        {
            performance.mark(Milestone::UpstreamRequestConsumed);
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        let empty = matches!(
            &self.inner,
            CanonicalHttpBodyInner::Buffered(frames) if frames.is_empty()
        );
        if empty && let Some(performance) = &self.performance {
            performance.mark(Milestone::UpstreamRequestConsumed);
        }
        empty
    }
}

fn canonical_to_http_frame(frame: BodyFrame) -> Result<Frame<Bytes>, HyperOriginError> {
    Ok(match frame {
        BodyFrame::Data(data) => Frame::data(data),
        BodyFrame::Trailers(trailers) => Frame::trailers(map_from_block(&trailers)?),
    })
}

async fn stream_incoming(
    mut body: Incoming,
    sender: transmog_core::BodyStreamSender,
    limit: usize,
    body_idle_timeout: Duration,
    performance: Option<PerformanceRecorder>,
    _observation: Option<ResponseObservation>,
) {
    let mut received = 0_usize;
    loop {
        let frame = match timeout(body_idle_timeout, body.frame()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                if let Some(performance) = &performance {
                    performance.mark(Milestone::UpstreamResponseDone);
                }
                return;
            }
            Err(_) => {
                let _ = sender.send(Err(BodyStreamError::IdleTimeout)).await;
                return;
            }
        };
        let canonical = match frame {
            Ok(frame) => match frame.into_data() {
                Ok(data) => {
                    let Some(attempted) = received.checked_add(data.len()) else {
                        let _ = sender
                            .send(Err(BodyStreamError::LimitExceeded {
                                limit,
                                attempted: usize::MAX,
                            }))
                            .await;
                        return;
                    };
                    if attempted > limit {
                        let _ = sender
                            .send(Err(BodyStreamError::LimitExceeded { limit, attempted }))
                            .await;
                        return;
                    }
                    received = attempted;
                    BodyFrame::Data(data)
                }
                Err(frame) => match frame.into_trailers() {
                    Ok(trailers) => match block_from_headers(&trailers) {
                        Ok(trailers) => BodyFrame::Trailers(trailers),
                        Err(error) => {
                            let _ = sender
                                .send(Err(BodyStreamError::Failed(error.to_string())))
                                .await;
                            return;
                        }
                    },
                    Err(_) => continue,
                },
            },
            Err(error) => {
                let _ = sender
                    .send(Err(BodyStreamError::Failed(error.to_string())))
                    .await;
                return;
            }
        };
        if matches!(&canonical,BodyFrame::Data(data) if !data.is_empty())
            && let Some(performance) = &performance
        {
            performance.mark(Milestone::UpstreamResponseFirstBody);
        }
        if sender.send(Ok(canonical)).await.is_err() {
            return;
        }
    }
}

async fn collect_incoming(
    body: &mut Incoming,
    limit: usize,
    body_idle_timeout: Duration,
    performance: Option<&PerformanceRecorder>,
) -> Result<Vec<BodyFrame>, HyperOriginError> {
    let mut frames = Vec::new();
    let mut received = 0_usize;
    loop {
        let frame = match timeout(body_idle_timeout, body.frame()).await {
            Ok(Some(frame)) => frame?,
            Ok(None) => return Ok(frames),
            Err(_) => return Err(HyperOriginError::BodyIdleTimeout),
        };
        let frame = match frame.into_data() {
            Ok(data) => {
                if !data.is_empty()
                    && let Some(performance) = performance
                {
                    performance.mark(Milestone::UpstreamResponseFirstBody);
                }
                received = received
                    .checked_add(data.len())
                    .ok_or(HyperOriginError::ResponseBodyTooLarge { limit })?;
                if received > limit {
                    return Err(HyperOriginError::ResponseBodyTooLarge { limit });
                }
                BodyFrame::Data(data)
            }
            Err(frame) => match frame.into_trailers() {
                Ok(trailers) => BodyFrame::Trailers(block_from_headers(&trailers)?),
                Err(_) => continue,
            },
        };
        frames.push(frame);
    }
}

fn append_headers(map: &mut HeaderMap, block: &HeaderBlock) -> Result<(), HyperOriginError> {
    for field in block.iter() {
        map.append(
            HeaderName::from_bytes(field.name())?,
            HeaderValue::from_bytes(field.value())?,
        );
    }
    Ok(())
}

fn map_from_block(block: &HeaderBlock) -> Result<HeaderMap, HyperOriginError> {
    let mut map = HeaderMap::new();
    append_headers(&mut map, block)?;
    Ok(map)
}

fn block_from_headers(headers: &HeaderMap) -> Result<HeaderBlock, HyperOriginError> {
    headers
        .iter()
        .map(|(name, value)| {
            HeaderField::try_new(name.as_str(), value.as_bytes()).map_err(Into::into)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(HeaderBlock::from_fields)
}

fn protocol_version(version: Version) -> Result<HttpLegVersion, HyperOriginError> {
    match version {
        Version::HTTP_09 | Version::HTTP_10 | Version::HTTP_11 => Ok(HttpLegVersion::Http1),
        Version::HTTP_2 => Ok(HttpLegVersion::Http2),
        _ => Err(HyperOriginError::UnsupportedVersion(version)),
    }
}

/// Hyper origin-adapter failure.
#[derive(Debug, Error)]
pub enum HyperOriginError {
    /// Shared TLS connector configuration failed.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// Canonical header validation failed.
    #[error(transparent)]
    Header(#[from] transmog_core::HeaderError),
    /// Canonical translation rejected unsafe framing.
    #[error(transparent)]
    Translation(#[from] transmog_core::TranslationError),
    /// A canonical URI could not be represented for Hyper.
    #[error(transparent)]
    Uri(#[from] InvalidUri),
    /// An HTTP request could not be built.
    #[error(transparent)]
    Http(#[from] http::Error),
    /// Header name was unexpectedly invalid after canonical validation.
    #[error(transparent)]
    HeaderName(#[from] http::header::InvalidHeaderName),
    /// Header value was unexpectedly invalid after canonical validation.
    #[error(transparent)]
    HeaderValue(#[from] http::header::InvalidHeaderValue),
    /// Hyper client or connection operation failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// Reading a Hyper response body failed.
    #[error(transparent)]
    Body(#[from] hyper::Error),
    /// Origin selected an unsupported HTTP version.
    #[error("origin selected unsupported HTTP version {0:?}")]
    UnsupportedVersion(Version),
    /// Response exceeded the explicit per-exchange byte limit.
    #[error("origin response body exceeded configured {limit}-byte limit")]
    ResponseBodyTooLarge {
        /// Configured maximum response bytes.
        limit: usize,
    },
    /// The origin left its response body idle past the configured deadline.
    #[error("origin response body exceeded its idle timeout")]
    BodyIdleTimeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Instant, SystemTime};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use transmog_tls::{SystemTrustSource, TrustSnapshot, UpstreamTlsPolicy};
    #[tokio::test]
    async fn pooled_requests_share_connection_counters_and_preserve_actual_http_minor_version() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for _ in 0..2 {
                let mut head = Vec::new();
                loop {
                    head.push(stream.read_u8().await.unwrap());
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                stream
                    .write_all(b"HTTP/1.1 200 Custom reason\r\nContent-Length: 4\r\n\r\n")
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
                stream.write_all(b"data").await.unwrap();
            }
        });
        let trust = Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap());
        let client = HyperOriginClient::new(&UpstreamTlsContextFactory::new(
            trust,
            UpstreamTlsPolicy::default(),
        ))
        .unwrap();
        let mut samples = Vec::new();
        for _ in 0..2 {
            let performance = PerformanceRecorder::new(SystemTime::now(), Instant::now());
            let response = client
                .clone()
                .with_performance(Some(performance.clone()))
                .execute(
                    CanonicalRequest {
                        head: RequestHead {
                            method: "GET".into(),
                            target: transmog_core::Target {
                                scheme: "http".into(),
                                authority: address.to_string(),
                                host: "127.0.0.1".into(),
                                port: address.port(),
                                path: "/".into(),
                                query: None,
                            },
                            headers: HeaderBlock::new(),
                            source_version: HttpLegVersion::Http1,
                        },
                        body: vec![],
                    },
                    HyperEgressMode::Http1Only,
                    1024,
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert_eq!(
                response.body,
                vec![BodyFrame::Data(Bytes::from_static(b"data"))]
            );
            let sample = performance.snapshot();
            assert!(sample.valid());
            assert!(
                sample
                    .points
                    .iter()
                    .any(|point| point.milestone == Milestone::UpstreamResponseDone)
            );
            let protocol = sample
                .protocols
                .iter()
                .find(|item| item.boundary == "upstream-response")
                .unwrap();
            assert_eq!(protocol.version, "HTTP/1.1");
            assert_eq!(protocol.reason.as_deref(), Some("Custom reason"));
            samples.push(sample);
        }
        assert_eq!(
            samples[0].transports[0].connection_id,
            samples[1].transports[0].connection_id
        );
        assert!(!samples[0].transports[0].shared);
        assert!(samples[1].transports[0].shared);
        assert_eq!(samples[1].transports[0].tcp_micros, Some(0));
        assert!(
            samples[1].transports[0]
                .setup_timings
                .iter()
                .all(|timing| timing.ended_offset_micros < 0 && timing.request_wait_micros == 0)
        );
        assert!(samples[1].transports[0].bytes_read > samples[0].transports[0].bytes_read);
        origin.await.unwrap();
    }
}

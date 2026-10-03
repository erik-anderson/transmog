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
use hyper_boring::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, Error as ClientError, connect::HttpConnector},
    rt::TokioExecutor,
};
use rustymiddle_core::{
    BodyFrame, BodyStream, BodyStreamError, CanonicalRequest, CanonicalResponse, HeaderBlock,
    HeaderField, HttpLegVersion, MessageKind, ResponseHead, StreamingRequest, StreamingResponse,
    TranslationOptions, prepare_headers,
};
use rustymiddle_tls::{TrustError, UpstreamTlsContextFactory};
use thiserror::Error;
use tokio::time::timeout;

use crate::{HyperEgressMode, build_https_connector};

type OriginClient = Client<HttpsConnector<HttpConnector>, CanonicalHttpBody>;

/// Pooled Hyper origin client with distinct ALPN pools for forced H1 and H2.
#[derive(Clone)]
pub struct HyperOriginClient {
    h1: OriginClient,
    h2: OriginClient,
    auto: OriginClient,
}

impl HyperOriginClient {
    /// Builds all Hyper pools from one immutable TLS policy generation.
    ///
    /// # Errors
    ///
    /// Returns [`HyperOriginError`] if a `BoringSSL` connector cannot be
    /// constructed from the shared context factory.
    pub fn new(factory: &UpstreamTlsContextFactory) -> Result<Self, HyperOriginError> {
        let h1 = Client::builder(TokioExecutor::new())
            .build(build_https_connector(factory, HyperEgressMode::Http1Only)?);
        let mut h2_builder = Client::builder(TokioExecutor::new());
        h2_builder.http2_only(true);
        let h2 = h2_builder.build(build_https_connector(factory, HyperEgressMode::Http2Only)?);
        let auto = Client::builder(TokioExecutor::new())
            .build(build_https_connector(factory, HyperEgressMode::Auto)?);
        Ok(Self { h1, h2, auto })
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
            .body(CanonicalHttpBody::new(request.body)?)?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let client = match mode {
            HyperEgressMode::Http1Only => &self.h1,
            HyperEgressMode::Http2Only => &self.h2,
            HyperEgressMode::Auto => &self.auto,
        };
        let mut response = client.request(outgoing).await?;
        let source_version = protocol_version(response.version())?;
        let status = response.status().as_u16();
        let headers = block_from_headers(response.headers())?;
        let body =
            collect_incoming(response.body_mut(), max_response_bytes, body_idle_timeout).await?;
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
            .body(CanonicalHttpBody::streaming(request.body))?;
        append_headers(outgoing.headers_mut(), &headers)?;

        let client = match mode {
            HyperEgressMode::Http1Only => &self.h1,
            HyperEgressMode::Http2Only => &self.h2,
            HyperEgressMode::Auto => &self.auto,
        };
        let response = client.request(outgoing).await?;
        let source_version = protocol_version(response.version())?;
        let head = ResponseHead {
            status: response.status().as_u16(),
            headers: block_from_headers(response.headers())?,
            source_version,
        };
        let (sender, body) = BodyStream::channel(body_channel_capacity);
        tokio::spawn(stream_incoming(
            response.into_body(),
            sender,
            max_response_bytes,
            body_idle_timeout,
        ));
        Ok(StreamingResponse { head, body })
    }
}

#[derive(Debug)]
struct CanonicalHttpBody {
    inner: CanonicalHttpBodyInner,
}

#[derive(Debug)]
enum CanonicalHttpBodyInner {
    Buffered(VecDeque<Frame<Bytes>>),
    Streaming(BodyStream),
}

impl CanonicalHttpBody {
    fn new(frames: Vec<BodyFrame>) -> Result<Self, HyperOriginError> {
        let mut output = VecDeque::with_capacity(frames.len());
        for frame in frames {
            output.push_back(match frame {
                BodyFrame::Data(data) => Frame::data(data),
                BodyFrame::Trailers(trailers) => Frame::trailers(map_from_block(&trailers)?),
            });
        }
        Ok(Self {
            inner: CanonicalHttpBodyInner::Buffered(output),
        })
    }

    fn streaming(body: BodyStream) -> Self {
        Self {
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
        match &mut self.inner {
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
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(
            &self.inner,
            CanonicalHttpBodyInner::Buffered(frames) if frames.is_empty()
        )
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
    sender: rustymiddle_core::BodyStreamSender,
    limit: usize,
    body_idle_timeout: Duration,
) {
    let mut received = 0_usize;
    loop {
        let frame = match timeout(body_idle_timeout, body.frame()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
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
        if sender.send(Ok(canonical)).await.is_err() {
            return;
        }
    }
}

async fn collect_incoming(
    body: &mut Incoming,
    limit: usize,
    body_idle_timeout: Duration,
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
    Header(#[from] rustymiddle_core::HeaderError),
    /// Canonical translation rejected unsafe framing.
    #[error(transparent)]
    Translation(#[from] rustymiddle_core::TranslationError),
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

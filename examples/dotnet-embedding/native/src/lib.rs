#![allow(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Educational C ABI bridge used by the .NET embedding sample.
//!
//! The ABI is intentionally narrow and same-build-only. It demonstrates how a
//! foreign-language host can own configuration and output while Rust retains
//! Transmog's canonical request, route, TLS, protocol, timeout, and body-limit
//! invariants.

use std::{
    collections::HashSet,
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    slice,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use http::Uri;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, timeout_at};
use transmog_core::{
    BodyFrame, BodyStream, HeaderBlock, HeaderField, HttpLegVersion, Replayability, RequestHead,
    RoutePolicy, StreamingRequest, Target,
    intercept::ExchangeCancellation,
    route::{UpstreamDestination, UpstreamPlan, UpstreamPoolKey},
    upstream::{UpstreamExecutor, UpstreamService},
};
use transmog_h3::{H3OriginClient, H3TransportLimits, H3UpstreamService};
use transmog_http::{HyperOriginClient, HyperUpstreamService};
use transmog_tls::{
    SystemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

const ABI_VERSION: u32 = 1;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;
const DEFAULT_TIMEOUT_MILLISECONDS: u64 = 30_000;
const MAX_TIMEOUT_MILLISECONDS: u64 = 300_000;
const BODY_CHANNEL_CAPACITY: NonZeroUsize = NonZeroUsize::new(8).expect("eight is nonzero");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum RequestedProtocol {
    H1,
    H2,
    H3,
}

impl RequestedProtocol {
    const fn label(self) -> &'static str {
        match self {
            Self::H1 => "h1",
            Self::H2 => "h2",
            Self::H3 => "h3",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AllowedProtocols {
    h1: bool,
    h2: bool,
    h3: bool,
}

impl AllowedProtocols {
    fn from_requested(protocols: &[RequestedProtocol]) -> Self {
        Self {
            h1: protocols.contains(&RequestedProtocol::H1),
            h2: protocols.contains(&RequestedProtocol::H2),
            h3: protocols.contains(&RequestedProtocol::H3),
        }
    }

    const fn hyper_route_policy(self) -> Option<RoutePolicy> {
        match (self.h1, self.h2) {
            (true, true) => Some(RoutePolicy::Auto),
            (true, false) => Some(RoutePolicy::Http1Only),
            (false, true) => Some(RoutePolicy::Http2Only),
            (false, false) => None,
        }
    }

    fn labels(self) -> Vec<&'static str> {
        [
            (self.h1, RequestedProtocol::H1),
            (self.h2, RequestedProtocol::H2),
            (self.h3, RequestedProtocol::H3),
        ]
        .into_iter()
        .filter(|(allowed, _)| *allowed)
        .map(|(_, protocol)| protocol.label())
        .collect()
    }

    fn hyper_labels(self) -> Vec<&'static str> {
        [
            (self.h1, RequestedProtocol::H1),
            (self.h2, RequestedProtocol::H2),
        ]
        .into_iter()
        .filter(|(allowed, _)| *allowed)
        .map(|(_, protocol)| protocol.label())
        .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FetchRequest {
    url: String,
    #[serde(default = "default_protocols")]
    protocols: Vec<RequestedProtocol>,
    #[serde(default = "default_max_response_bytes")]
    max_response_bytes: usize,
    #[serde(default = "default_timeout_milliseconds")]
    timeout_milliseconds: u64,
}

const fn default_max_response_bytes() -> usize {
    DEFAULT_MAX_RESPONSE_BYTES
}

const fn default_timeout_milliseconds() -> u64 {
    DEFAULT_TIMEOUT_MILLISECONDS
}

fn default_protocols() -> Vec<RequestedProtocol> {
    vec![RequestedProtocol::H1, RequestedProtocol::H2]
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchMetadata {
    status_code: u16,
    protocol: &'static str,
    allowed_protocols: Vec<&'static str>,
    body_bytes: usize,
    headers: Vec<HeaderMetadata>,
    trailers: Vec<HeaderMetadata>,
    attempts: Vec<AttemptMetadata>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeaderMetadata {
    name: String,
    value: String,
    value_encoding: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttemptMetadata {
    allowed_protocols: Vec<&'static str>,
    negotiated_protocol: Option<&'static str>,
    outcome: String,
}

#[derive(Debug)]
struct FetchSuccess {
    status_code: u16,
    body: Vec<u8>,
    metadata_json: Vec<u8>,
}

/// Opaque result owned by the native bridge until explicitly released.
pub struct FetchHandle {
    success: Option<FetchSuccess>,
    error: Vec<u8>,
}

impl FetchHandle {
    fn succeeded(success: FetchSuccess) -> Self {
        Self {
            success: Some(success),
            error: Vec::new(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            success: None,
            error: message.into().into_bytes(),
        }
    }
}

/// Returns the teaching ABI revision expected by the managed sample.
#[unsafe(no_mangle)]
pub extern "C" fn transmog_dotnet_sample_abi_version() -> u32 {
    ABI_VERSION
}

/// Executes one bounded GET described by UTF-8 JSON and returns an opaque
/// result handle. The call blocks the invoking managed thread until completion.
///
/// A non-null `request_json` must reference `request_json_length` readable bytes
/// for the duration of this call. The returned handle must be released exactly
/// once with [`transmog_dotnet_sample_fetch_free`].
///
/// # Safety
///
/// For a nonzero length, `request_json` must be non-null, correctly aligned,
/// and point to that many initialized bytes which remain readable until this
/// function returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch(
    request_json: *const u8,
    request_json_length: usize,
) -> *mut FetchHandle {
    let handle = if request_json.is_null() {
        FetchHandle::failed("request JSON pointer is null")
    } else {
        // SAFETY: The caller contract requires a readable allocation of the
        // supplied length, and this borrowed slice does not escape the call.
        let input = unsafe { slice::from_raw_parts(request_json, request_json_length) };
        match catch_unwind(AssertUnwindSafe(|| fetch_blocking(input))) {
            Ok(Ok(success)) => FetchHandle::succeeded(success),
            Ok(Err(error)) => FetchHandle::failed(error),
            Err(_) => FetchHandle::failed("the native Transmog bridge panicked"),
        }
    };
    Box::into_raw(Box::new(handle))
}

/// Returns one when the opaque result represents a successful response.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_succeeded(handle: *const FetchHandle) -> i32 {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }
        .is_some_and(|value| value.success.is_some())
        .into()
}

/// Returns the HTTP status code, or zero for a failed/null result.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_status_code(
    handle: *const FetchHandle,
) -> u16 {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }
        .and_then(|value| value.success.as_ref())
        .map_or(0, |success| success.status_code)
}

/// Returns a borrowed response-body pointer valid until the handle is freed.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_body(
    handle: *const FetchHandle,
) -> *const u8 {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }
        .and_then(|value| value.success.as_ref())
        .map_or(std::ptr::null(), |success| success.body.as_ptr())
}

/// Returns the response-body byte length.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_body_length(
    handle: *const FetchHandle,
) -> usize {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }
        .and_then(|value| value.success.as_ref())
        .map_or(0, |success| success.body.len())
}

/// Returns borrowed UTF-8 metadata JSON valid until the handle is freed.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_metadata(
    handle: *const FetchHandle,
) -> *const u8 {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }
        .and_then(|value| value.success.as_ref())
        .map_or(std::ptr::null(), |success| success.metadata_json.as_ptr())
}

/// Returns the UTF-8 metadata JSON byte length.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_metadata_length(
    handle: *const FetchHandle,
) -> usize {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }
        .and_then(|value| value.success.as_ref())
        .map_or(0, |success| success.metadata_json.len())
}

/// Returns a borrowed UTF-8 error message valid until the handle is freed.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_error(
    handle: *const FetchHandle,
) -> *const u8 {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }.map_or(std::ptr::null(), |value| value.error.as_ptr())
}

/// Returns the UTF-8 error-message byte length.
///
/// # Safety
///
/// A non-null `handle` must be a live result returned by
/// [`transmog_dotnet_sample_fetch`] and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_error_length(
    handle: *const FetchHandle,
) -> usize {
    // SAFETY: Validity and lifetime are part of the opaque-handle contract.
    unsafe { handle.as_ref() }.map_or(0, |value| value.error.len())
}

/// Releases one result returned by [`transmog_dotnet_sample_fetch`].
///
/// # Safety
///
/// A non-null `handle` must have been returned by
/// [`transmog_dotnet_sample_fetch`], must not already have been freed, and must
/// not be used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transmog_dotnet_sample_fetch_free(handle: *mut FetchHandle) {
    if !handle.is_null() {
        // SAFETY: The opaque-handle contract requires exactly one release for
        // the pointer returned by `Box::into_raw` in the fetch function.
        drop(unsafe { Box::from_raw(handle) });
    }
}

fn fetch_blocking(input: &[u8]) -> Result<FetchSuccess, String> {
    let request: FetchRequest = serde_json::from_slice(input)
        .map_err(|_| "request configuration is not valid JSON".to_owned())?;
    validate_request(&request)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|_| "the native async runtime could not be created".to_owned())?;
    runtime.block_on(fetch(request))
}

fn validate_request(request: &FetchRequest) -> Result<(), String> {
    if request.url.is_empty() || request.url.len() > 8 * 1024 {
        return Err("url must contain between 1 and 8192 UTF-8 bytes".to_owned());
    }
    if request.protocols.is_empty() {
        return Err("at least one protocol must be allowed".to_owned());
    }
    let mut unique = HashSet::new();
    if !request
        .protocols
        .iter()
        .all(|protocol| unique.insert(*protocol))
    {
        return Err("allowed protocols must not contain duplicates".to_owned());
    }
    if request.max_response_bytes == 0 || request.max_response_bytes > MAX_RESPONSE_BYTES {
        return Err(format!(
            "maxResponseBytes must be between 1 and {MAX_RESPONSE_BYTES}"
        ));
    }
    if request.timeout_milliseconds == 0 || request.timeout_milliseconds > MAX_TIMEOUT_MILLISECONDS
    {
        return Err(format!(
            "timeoutMilliseconds must be between 1 and {MAX_TIMEOUT_MILLISECONDS}"
        ));
    }
    Ok(())
}

async fn fetch(request: FetchRequest) -> Result<FetchSuccess, String> {
    let target = parse_target(&request.url)?;
    let allowed = AllowedProtocols::from_requested(&request.protocols);
    let timeout = Duration::from_millis(request.timeout_milliseconds);
    let trust = Arc::new(
        TrustSnapshot::load(&SystemTrustSource, 1)
            .map_err(|_| "operating-system trust roots could not be loaded".to_owned())?,
    );
    let trust_generation = trust.generation();
    let tls = UpstreamTlsContextFactory::new(trust, UpstreamTlsPolicy::default());
    let hyper = allowed
        .hyper_route_policy()
        .map(|_| HyperOriginClient::new(&tls))
        .transpose()
        .map_err(|_| "the HTTP/1.1 and HTTP/2 client could not be created".to_owned())?;
    let h3 = allowed
        .h3
        .then(|| H3OriginClient::new(tls, H3TransportLimits::default()));

    let mut attempts = Vec::with_capacity(2);
    if allowed.h3 {
        match execute_h3_attempt(
            h3.as_ref()
                .expect("HTTP/3 client exists when HTTP/3 is allowed"),
            &target,
            request.max_response_bytes,
            trust_generation,
            timeout,
        )
        .await
        {
            Ok(response) => {
                let negotiated = response.head.source_version;
                attempts.push(AttemptMetadata {
                    allowed_protocols: vec![RequestedProtocol::H3.label()],
                    negotiated_protocol: Some(protocol_label(negotiated)),
                    outcome: "response received".to_owned(),
                });
                return finish_response(
                    response,
                    allowed,
                    attempts,
                    request.max_response_bytes,
                    timeout,
                )
                .await;
            }
            Err(error) => attempts.push(AttemptMetadata {
                allowed_protocols: vec![RequestedProtocol::H3.label()],
                negotiated_protocol: None,
                outcome: error,
            }),
        }
    }

    if let Some(route_policy) = allowed.hyper_route_policy() {
        match execute_hyper_attempt(
            hyper
                .as_ref()
                .expect("Hyper client exists when HTTP/1.1 or HTTP/2 is allowed"),
            &target,
            route_policy,
            request.max_response_bytes,
            trust_generation,
            timeout,
        )
        .await
        {
            Ok(response) => {
                let negotiated = response.head.source_version;
                attempts.push(AttemptMetadata {
                    allowed_protocols: allowed.hyper_labels(),
                    negotiated_protocol: Some(protocol_label(negotiated)),
                    outcome: "response received".to_owned(),
                });
                return finish_response(
                    response,
                    allowed,
                    attempts,
                    request.max_response_bytes,
                    timeout,
                )
                .await;
            }
            Err(error) => attempts.push(AttemptMetadata {
                allowed_protocols: allowed.hyper_labels(),
                negotiated_protocol: None,
                outcome: error,
            }),
        }
    }

    Err(all_attempts_failed(attempts))
}

fn all_attempts_failed(attempts: Vec<AttemptMetadata>) -> String {
    let summary = attempts
        .into_iter()
        .map(|attempt| {
            format!(
                "{}: {}",
                attempt.allowed_protocols.join("/"),
                attempt.outcome
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("all allowed protocol attempts failed: {summary}")
}

async fn execute_h3_attempt(
    client: &H3OriginClient,
    target: &Target,
    max_response_bytes: usize,
    trust_generation: u64,
    timeout: Duration,
) -> Result<transmog_core::StreamingResponse, String> {
    if target.scheme == "https" {
        let service = Arc::new(H3UpstreamService::new(
            client.clone(),
            max_response_bytes,
            BODY_CHANNEL_CAPACITY,
        ));
        execute_attempt(
            service,
            target,
            RoutePolicy::Http3Only,
            trust_generation,
            timeout,
        )
        .await
    } else {
        Err("HTTP/3 requires an HTTPS URL".to_owned())
    }
}

async fn execute_hyper_attempt(
    client: &HyperOriginClient,
    target: &Target,
    route_policy: RoutePolicy,
    max_response_bytes: usize,
    trust_generation: u64,
    timeout: Duration,
) -> Result<transmog_core::StreamingResponse, String> {
    let service = Arc::new(HyperUpstreamService::new(
        client.clone(),
        max_response_bytes,
        BODY_CHANNEL_CAPACITY,
        timeout,
    ));
    execute_attempt(service, target, route_policy, trust_generation, timeout).await
}

async fn execute_attempt(
    service: Arc<dyn UpstreamService>,
    target: &Target,
    route_policy: RoutePolicy,
    trust_generation: u64,
    timeout: Duration,
) -> Result<transmog_core::StreamingResponse, String> {
    let executor = UpstreamExecutor::new(service, timeout);
    let cancellation = ExchangeCancellation::new();
    let plan = route_plan(target, route_policy, trust_generation);
    executor
        .execute(streaming_get(target)?, plan, &cancellation)
        .await
        .map_err(|error| error.to_string())
}

async fn finish_response(
    response: transmog_core::StreamingResponse,
    allowed: AllowedProtocols,
    attempts: Vec<AttemptMetadata>,
    max_response_bytes: usize,
    timeout: Duration,
) -> Result<FetchSuccess, String> {
    let status_code = response.head.status;
    let negotiated = response.head.source_version;
    let headers = metadata_headers(&response.head.headers);
    let (body, trailers) =
        collect_body(response.body, max_response_bytes, Instant::now() + timeout).await?;
    let metadata = FetchMetadata {
        status_code,
        protocol: protocol_label(negotiated),
        allowed_protocols: allowed.labels(),
        body_bytes: body.len(),
        headers,
        trailers,
        attempts,
    };
    let metadata_json = serde_json::to_vec_pretty(&metadata)
        .map_err(|_| "response metadata could not be serialized".to_owned())?;
    Ok(FetchSuccess {
        status_code,
        body,
        metadata_json,
    })
}

fn parse_target(url: &str) -> Result<Target, String> {
    let uri = Uri::from_str(url).map_err(|_| "url must be a valid absolute URI".to_owned())?;
    let scheme = uri.scheme_str().unwrap_or_default().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err("url must use http or https".to_owned());
    }
    let authority = uri
        .authority()
        .ok_or_else(|| "url must include an authority".to_owned())?;
    if authority.as_str().contains('@') {
        return Err("url must not contain user information".to_owned());
    }
    let authority_host = authority.host();
    let host = authority_host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(authority_host)
        .to_ascii_lowercase();
    let port = authority
        .port_u16()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let path_and_query = uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, None), |(path, query)| (path, Some(query)));
    Ok(Target {
        scheme: scheme.clone(),
        authority: canonical_authority(&scheme, &host, port),
        host,
        port,
        path: path.to_owned(),
        query: query.map(str::to_owned),
    })
}

fn canonical_authority(scheme: &str, host: &str, port: u16) -> String {
    let rendered_host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    if (scheme == "http" && port == 80) || (scheme == "https" && port == 443) {
        rendered_host
    } else {
        format!("{rendered_host}:{port}")
    }
}

fn streaming_get(target: &Target) -> Result<StreamingRequest, String> {
    let mut headers = HeaderBlock::new();
    headers.push(
        HeaderField::try_new("accept", "*/*")
            .map_err(|_| "sample Accept header is invalid".to_owned())?,
    );
    headers.push(
        HeaderField::try_new("user-agent", "TransmogDotNetEmbeddingSample/0.1")
            .map_err(|_| "sample User-Agent header is invalid".to_owned())?,
    );
    let (sender, body) = BodyStream::channel(BODY_CHANNEL_CAPACITY);
    drop(sender);
    Ok(StreamingRequest {
        head: RequestHead {
            method: "GET".to_owned(),
            target: target.clone(),
            headers,
            source_version: HttpLegVersion::Http1,
        },
        body,
    })
}

fn route_plan(target: &Target, route_policy: RoutePolicy, trust_generation: u64) -> UpstreamPlan {
    UpstreamPlan {
        pool_key: UpstreamPoolKey {
            destination: UpstreamDestination::from_target(target),
            version_policy: route_policy,
            trust_generation,
            tls_policy_id: Arc::from("dotnet-sample-system-trust-v1"),
            connector_policy_id: Arc::from("dotnet-sample-direct-v1"),
        },
        route_id: Arc::from("dotnet-sample-allowed-protocols"),
        reason: Arc::from("transport policy derived from the managed host's allowed protocols"),
        replayability: Replayability::SafeMethod,
    }
}

async fn collect_body(
    mut stream: BodyStream,
    limit: usize,
    deadline: Instant,
) -> Result<(Vec<u8>, Vec<HeaderMetadata>), String> {
    let mut body = Vec::new();
    let mut trailers = Vec::new();
    loop {
        let frame = timeout_at(deadline, stream.recv())
            .await
            .map_err(|_| "response body exceeded its overall deadline".to_owned())?;
        let Some(frame) = frame else {
            break;
        };
        match frame.map_err(|error| format!("response body failed: {error}"))? {
            BodyFrame::Data(data) => {
                let attempted = body
                    .len()
                    .checked_add(data.len())
                    .ok_or_else(|| "response body size overflowed".to_owned())?;
                if attempted > limit {
                    return Err(format!(
                        "response body exceeded configured limit {limit} bytes"
                    ));
                }
                body.extend_from_slice(&data);
            }
            BodyFrame::Trailers(fields) => trailers = metadata_headers(&fields),
        }
    }
    Ok((body, trailers))
}

fn metadata_headers(headers: &HeaderBlock) -> Vec<HeaderMetadata> {
    headers
        .iter()
        .map(|field| {
            let (value, value_encoding) = match String::from_utf8(field.value().to_vec()) {
                Ok(value) => (value, "utf8"),
                Err(error) => (hex(error.as_bytes()), "hex"),
            };
            HeaderMetadata {
                name: String::from_utf8_lossy(field.name()).into_owned(),
                value,
                value_encoding,
            }
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

const fn protocol_label(version: HttpLegVersion) -> &'static str {
    match version {
        HttpLegVersion::Http1 => "h1",
        HttpLegVersion::Http2 => "h2",
        HttpLegVersion::Http3 => "h3",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_are_bounded_and_enable_tcp_alpn() {
        let request: FetchRequest = serde_json::from_value(json!({
            "url": "https://example.test/"
        }))
        .unwrap();
        validate_request(&request).unwrap();
        assert_eq!(
            request.protocols,
            vec![RequestedProtocol::H1, RequestedProtocol::H2]
        );
        let allowed = AllowedProtocols::from_requested(&request.protocols);
        assert_eq!(allowed.hyper_route_policy(), Some(RoutePolicy::Auto));
        assert_eq!(request.max_response_bytes, DEFAULT_MAX_RESPONSE_BYTES);
        assert_eq!(request.timeout_milliseconds, DEFAULT_TIMEOUT_MILLISECONDS);
    }

    #[test]
    fn protocol_order_does_not_control_preference() {
        let forward = AllowedProtocols::from_requested(&[
            RequestedProtocol::H1,
            RequestedProtocol::H2,
            RequestedProtocol::H3,
        ]);
        let reverse = AllowedProtocols::from_requested(&[
            RequestedProtocol::H3,
            RequestedProtocol::H2,
            RequestedProtocol::H1,
        ]);
        assert_eq!(forward, reverse);
        assert_eq!(forward.hyper_route_policy(), Some(RoutePolicy::Auto));
        assert!(forward.h3);
        assert_eq!(forward.labels(), vec!["h1", "h2", "h3"]);
    }

    #[test]
    fn singleton_protocol_sets_remain_strict() {
        let h1 = AllowedProtocols::from_requested(&[RequestedProtocol::H1]);
        assert_eq!(h1.hyper_route_policy(), Some(RoutePolicy::Http1Only));

        let h2 = AllowedProtocols::from_requested(&[RequestedProtocol::H2]);
        assert_eq!(h2.hyper_route_policy(), Some(RoutePolicy::Http2Only));

        let h3 = AllowedProtocols::from_requested(&[RequestedProtocol::H3]);
        assert_eq!(h3.hyper_route_policy(), None);
        assert!(h3.h3);
    }

    #[test]
    fn rejects_duplicate_protocols_and_unbounded_configuration() {
        let duplicate: FetchRequest = serde_json::from_value(json!({
            "url": "https://example.test/",
            "protocols": ["h1", "h1"]
        }))
        .unwrap();
        assert_eq!(
            validate_request(&duplicate).unwrap_err(),
            "allowed protocols must not contain duplicates"
        );

        let oversized: FetchRequest = serde_json::from_value(json!({
            "url": "https://example.test/",
            "maxResponseBytes": MAX_RESPONSE_BYTES + 1
        }))
        .unwrap();
        assert!(
            validate_request(&oversized)
                .unwrap_err()
                .contains("maxResponseBytes")
        );
    }

    #[test]
    fn canonicalizes_default_ports_and_ipv6_authorities() {
        let https = parse_target("https://Example.Test:443/a?b=c").unwrap();
        assert_eq!(https.authority, "example.test");
        assert_eq!(https.path, "/a");
        assert_eq!(https.query.as_deref(), Some("b=c"));

        let ipv6 = parse_target("http://[::1]:8080/").unwrap();
        assert_eq!(ipv6.authority, "[::1]:8080");
        assert_eq!(ipv6.host, "::1");
    }

    #[test]
    fn exported_abi_fetches_through_the_canonical_h1_service() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /sample HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 13\r\nConnection: close\r\n\r\nhello from h1",
                )
                .unwrap();
        });
        let input = serde_json::to_vec(&json!({
            "url": format!("http://{address}/sample"),
            "protocols": ["h1"],
            "maxResponseBytes": 1024,
            "timeoutMilliseconds": 5000
        }))
        .unwrap();

        // SAFETY: The input allocation remains valid for the call, and the
        // returned opaque handle is released exactly once below.
        let handle = unsafe { transmog_dotnet_sample_fetch(input.as_ptr(), input.len()) };
        assert!(!handle.is_null());
        // SAFETY: `handle` is live until the final free call.
        unsafe {
            let succeeded = transmog_dotnet_sample_fetch_succeeded(handle);
            if succeeded != 1 {
                let error = slice::from_raw_parts(
                    transmog_dotnet_sample_fetch_error(handle),
                    transmog_dotnet_sample_fetch_error_length(handle),
                );
                panic!("native fetch failed: {}", String::from_utf8_lossy(error));
            }
            assert_eq!(transmog_dotnet_sample_fetch_status_code(handle), 200);
            let body = slice::from_raw_parts(
                transmog_dotnet_sample_fetch_body(handle),
                transmog_dotnet_sample_fetch_body_length(handle),
            );
            assert_eq!(body, b"hello from h1");
            let metadata = slice::from_raw_parts(
                transmog_dotnet_sample_fetch_metadata(handle),
                transmog_dotnet_sample_fetch_metadata_length(handle),
            );
            let metadata: serde_json::Value = serde_json::from_slice(metadata).unwrap();
            assert_eq!(metadata["protocol"], "h1");
            assert_eq!(metadata["statusCode"], 200);
            transmog_dotnet_sample_fetch_free(handle);
        }
        server.join().unwrap();
    }
}

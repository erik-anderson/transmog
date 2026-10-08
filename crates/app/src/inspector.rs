use std::{fmt::Write as _, num::NonZeroUsize};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use transmog_content::{ContentCodingStack, ContentDecoder, ContentEncoder, ContentLimits};
use transmog_core::{
    BodyFrame, HeaderBlock, HeaderField, intercept::HookEffectAction, observe::ExchangeBoundary,
};
use transmog_session::{ApplicationSessionService, BodySnapshot, SessionSnapshot, SessionTerminal};

use crate::body_store::MAX_BODY_READ_BYTES;
use crate::{
    AppError, BodyAvailability, BodyStore, DEFAULT_BODY_READ_BYTES, ErrorCategory,
    StoredBodyMetadata,
    sessions::{ClientIdentityView, client_identity_view},
};

const MAX_DISPLAY_BYTES: usize = 64 * 1024;
const MAX_FIELD_CHARS: usize = 2 * 1024;
const MAX_DIAGNOSTICS: usize = 64;

/// Display-safe ordered header field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderView {
    /// Escaped field name.
    pub name: String,
    /// Escaped text or hexadecimal bytes.
    pub value: String,
    /// Whether the value required hexadecimal representation.
    pub binary: bool,
    /// Whether this credential-bearing field was already redacted.
    pub sensitive: bool,
}

/// Explicit bounded body evidence for one observation boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyView {
    /// Stable observation boundary.
    pub boundary: String,
    /// Total observed byte count.
    pub observed_bytes: u64,
    /// Retained display representation.
    pub display: String,
    /// `text`, `hex`, or `missing`.
    pub representation: &'static str,
    /// Whether bytes are omitted from this view.
    pub truncated: bool,
    /// Bounded trailers.
    pub trailers: Vec<HeaderView>,
}

/// One request or response head at an explicit boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadView {
    /// Observation boundary.
    pub boundary: String,
    /// Request method, when this is a request.
    pub method: Option<String>,
    /// Normalized request target, when this is a request.
    pub target: Option<String>,
    /// Response status, when this is a response.
    pub status: Option<u16>,
    /// Source protocol.
    pub protocol: String,
    /// Ordered duplicate-preserving headers.
    pub headers: Vec<HeaderView>,
}

/// Complete bounded safe inspector model for one retained exchange.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    /// Opaque session identifier.
    pub id: String,
    /// Start timestamp in Unix milliseconds, also used by batch review.
    pub started_at: u64,
    /// Best-effort caller identity captured when the connection opened.
    pub caller: ClientIdentityView,
    /// Request heads at original/effective boundaries.
    pub requests: Vec<HeadView>,
    /// Response heads at original/effective boundaries.
    pub responses: Vec<HeadView>,
    /// Retained body evidence.
    pub bodies: Vec<BodyView>,
    /// Product body-store metadata at original and effective boundaries.
    pub stored_bodies: Vec<StoredBodyMetadata>,
    /// Redaction-safe initialization diagnostics.
    pub diagnostics: Vec<String>,
    /// Centrally attributed hook effects.
    pub hook_effects: Vec<String>,
    /// User-facing attribution when a local autoresponse won.
    pub auto_response: Option<AutoResponseMatchView>,
    /// Selected route and explanation.
    pub route_selection: Option<String>,
    /// Bounded route attempts.
    pub route_attempts: Vec<String>,
    /// Terminal outcome detail.
    pub terminal: String,
    /// Bounded WebSocket terminal evidence.
    pub websocket: Option<String>,
    /// Missing observer sequence count for this exchange.
    pub sequence_loss: u64,
}

/// Stable user-facing attribution for a locally served autoresponse.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResponseMatchView {
    /// Stable internal rule identity used only for navigation.
    pub rule_id: String,
    /// Friendly name captured with this exchange.
    pub rule_name: String,
    /// Immutable rule revision that made the decision.
    pub rule_revision: u64,
    /// One-based evaluation position captured with this exchange.
    pub position: usize,
    /// Exact immutable response asset reference.
    pub asset_reference: String,
    /// Locally generated response status.
    pub status: u16,
    /// Known response bytes, or zero for an unknown streaming length.
    pub body_bytes: usize,
}

/// Requested safe representation of retained body bytes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BodyRepresentation {
    /// Choose formatted JSON, presentable text, or a bounded byte view.
    #[default]
    Auto,
    /// Preserve decoded Unicode text without reformatting.
    OriginalText,
    /// Apply a reviewed structured formatter, currently JSON only.
    Formatted,
    /// Render offset, hexadecimal, and ASCII columns.
    Bytes,
    /// Return metadata without body content.
    Metadata,
    /// Request a safe image preview token.
    Image,
}

/// Bounded body-inspection request accepted from presentation layers.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BodyInspectionRequest {
    /// Opaque session identifier returned by session queries.
    pub session_id: String,
    /// `upstream-response` or `client-response` by default; request boundaries
    /// are accepted only when request retention was explicitly enabled.
    pub boundary: String,
    /// Requested safe representation.
    #[serde(default)]
    pub representation: BodyRepresentation,
    /// Whether content codings should be decoded before presentation.
    #[serde(default)]
    pub decode_content: bool,
    /// Retained-byte offset for byte views.
    #[serde(default)]
    pub offset: u64,
    /// Maximum bytes to read, capped by the store's hard limit.
    pub max_bytes: Option<usize>,
}

/// Safe bounded body representation returned to a presentation layer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyInspection {
    /// Authoritative storage and representation metadata.
    pub metadata: StoredBodyMetadata,
    /// Representation actually returned.
    pub representation: &'static str,
    /// Whether content codings were decoded before rendering.
    pub decoded: bool,
    /// Character encoding used for a returned text representation.
    pub text_encoding: Option<&'static str>,
    /// Safe text or byte-dump content. Presentation layers must assign this to
    /// `textContent`, never HTML.
    pub display: String,
    /// Source bytes consumed to produce this view.
    pub display_bytes: usize,
    /// Whether additional bytes or representation detail were omitted.
    pub truncated: bool,
    /// Next retained-byte offset for a paged raw-byte view.
    pub next_offset: Option<u64>,
    /// Redaction-safe explanation of fallback or unavailability.
    pub warning: Option<String>,
    /// Opaque handle on the isolated preview origin, when available.
    pub preview_handle: Option<String>,
    /// Normalized preview MIME type, when available.
    pub preview_mime_type: Option<&'static str>,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn session_detail(
    service: &ApplicationSessionService,
    body_store: Option<&BodyStore>,
    id: &str,
) -> Result<SessionDetail, AppError> {
    let numeric = parse_session_id(id)?;
    let snapshot = service
        .catalog()
        .get(transmog_core::intercept::ExchangeId(numeric))
        .ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "session is unavailable or has been evicted",
                false,
            )
        })?;
    if snapshot.terminal.is_some()
        && let Some(store) = body_store
    {
        store.flush().map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "response body metadata is temporarily unavailable",
                true,
            )
        })?;
    }

    let requests = snapshot
        .request_heads
        .iter()
        .map(|observed| HeadView {
            boundary: boundary(observed.boundary),
            method: Some(display_text(observed.head.method.as_bytes())),
            target: Some(display_text(
                format!(
                    "{}://{}{}{}",
                    observed.head.target.scheme,
                    observed.head.target.authority,
                    observed.head.target.path,
                    observed
                        .head
                        .target
                        .query
                        .as_ref()
                        .map_or_else(String::new, |query| format!("?{query}"))
                )
                .as_bytes(),
            )),
            status: None,
            protocol: format!("{:?}", observed.head.source_version),
            headers: headers(&observed.head.headers),
        })
        .collect();
    let responses = snapshot
        .response_heads
        .iter()
        .map(|observed| HeadView {
            boundary: boundary(observed.boundary),
            method: None,
            target: None,
            status: Some(observed.head.status),
            protocol: format!("{:?}", observed.head.source_version),
            headers: headers(&observed.head.headers),
        })
        .collect();
    let bodies = snapshot.bodies.iter().map(body).collect();
    let stored_bodies = body_store.map_or_else(Vec::new, |store| {
        store.metadata(transmog_core::intercept::ExchangeId(numeric))
    });
    let diagnostics = snapshot
        .initialization_diagnostics
        .iter()
        .take(MAX_DIAGNOSTICS)
        .map(bounded_debug)
        .collect();
    let hook_effects = snapshot
        .hook_effects
        .iter()
        .take(MAX_DIAGNOSTICS)
        .map(|effect| {
            display_text(
                format!(
                    "{} · {} · {:?} · {:?} · changed={}",
                    effect.sequence,
                    effect.interceptor.id.as_str(),
                    effect.phase,
                    effect.action,
                    effect.changed
                )
                .as_bytes(),
            )
        })
        .collect();
    let route_selection = snapshot
        .route_selection
        .as_ref()
        .map(|(policy, reason)| display_text(format!("{policy}: {reason}").as_bytes()));
    let route_attempts = snapshot
        .route_attempts
        .iter()
        .map(|attempt| {
            display_text(format!("{:?}: {}", attempt.protocol, attempt.outcome).as_bytes())
        })
        .collect();
    let terminal = match &snapshot.terminal {
        Some(SessionTerminal::Completed(_)) => "completed".to_owned(),
        Some(SessionTerminal::Failed(failure)) => display_text(
            format!(
                "failed at {:?}: {:?}; request_committed={}; response_committed={}; {}",
                failure.stage,
                failure.kind,
                failure.request_committed,
                failure.response_committed,
                failure.message
            )
            .as_bytes(),
        ),
        None => "active".to_owned(),
    };
    Ok(SessionDetail {
        id: id.to_ascii_lowercase(),
        started_at: u64::try_from(
            snapshot
                .metadata
                .started_at
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX),
        caller: client_identity_view(&snapshot.metadata.client_identity),
        requests,
        responses,
        bodies,
        stored_bodies,
        diagnostics,
        hook_effects,
        auto_response: auto_response_match(&snapshot),
        route_selection,
        route_attempts,
        terminal,
        websocket: snapshot.websocket.as_ref().map(bounded_debug),
        sequence_loss: snapshot.sequence_loss,
    })
}

pub(crate) fn auto_response_match(snapshot: &SessionSnapshot) -> Option<AutoResponseMatchView> {
    snapshot.hook_effects.iter().find_map(|effect| {
        let HookEffectAction::Respond {
            status, body_bytes, ..
        } = &effect.action
        else {
            return None;
        };
        let identity = effect.interceptor.id.as_str();
        let encoded = identity.strip_prefix("automation/")?;
        let (rule, asset_reference) = encoded.split_once("/request/asset/")?;
        let (rule_id, revision) = rule.rsplit_once('@')?;
        Some(AutoResponseMatchView {
            rule_id: rule_id.to_owned(),
            rule_name: effect.interceptor.name.to_string(),
            rule_revision: revision.parse().ok()?,
            position: effect.interceptor.chain_position.saturating_add(1),
            asset_reference: asset_reference.to_owned(),
            status: *status,
            body_bytes: *body_bytes,
        })
    })
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn inspect_body(
    body_store: Option<&BodyStore>,
    request: BodyInspectionRequest,
) -> Result<BodyInspection, AppError> {
    let store = body_store.ok_or_else(|| {
        AppError::new(
            ErrorCategory::Unavailable,
            "response body retention is not configured",
            false,
        )
    })?;
    let exchange_id = transmog_core::intercept::ExchangeId(parse_session_id(&request.session_id)?);
    let boundary = parse_boundary(&request.boundary)?;
    let metadata = store
        .metadata(exchange_id)
        .into_iter()
        .find(|candidate| candidate.boundary == boundary_name(boundary))
        .ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "body boundary is unavailable",
                false,
            )
        })?;
    if request.representation == BodyRepresentation::Metadata {
        return Ok(BodyInspection {
            metadata,
            representation: "metadata",
            decoded: false,
            text_encoding: None,
            display: String::new(),
            display_bytes: 0,
            truncated: false,
            next_offset: None,
            warning: None,
            preview_handle: None,
            preview_mime_type: None,
        });
    }
    if request.representation == BodyRepresentation::Image {
        return Ok(BodyInspection {
            metadata,
            representation: "unavailable",
            decoded: false,
            text_encoding: None,
            display: String::new(),
            display_bytes: 0,
            truncated: false,
            next_offset: None,
            warning: Some("safe image preview is unavailable for this body".to_owned()),
            preview_handle: None,
            preview_mime_type: None,
        });
    }
    if matches!(
        metadata.availability,
        BodyAvailability::Disabled | BodyAvailability::Evicted | BodyAvailability::QuotaOmitted
    ) || metadata.retained_bytes == 0
    {
        return Ok(BodyInspection {
            metadata,
            representation: "unavailable",
            decoded: false,
            text_encoding: None,
            display: String::new(),
            display_bytes: 0,
            truncated: false,
            next_offset: None,
            warning: Some("retained body bytes are unavailable".to_owned()),
            preview_handle: None,
            preview_mime_type: None,
        });
    }
    let max_bytes = request.max_bytes.unwrap_or(DEFAULT_BODY_READ_BYTES);
    if max_bytes == 0 || max_bytes > crate::body_store::MAX_BODY_READ_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "body preview exceeds the 16 MiB hard limit",
            false,
        ));
    }
    if request.decode_content && request.offset != 0 {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "decoded body views must begin at offset zero",
            false,
        ));
    }
    let read_length = if request.decode_content {
        usize::try_from(metadata.retained_bytes)
            .ok()
            .filter(|length| *length <= crate::body_store::MAX_BODY_READ_BYTES)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Limit,
                    "encoded body is too large for bounded preview decoding",
                    false,
                )
            })?
    } else {
        max_bytes
    };
    let range = store
        .read_range(exchange_id, boundary, request.offset, read_length)
        .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), false))?;
    let mut bytes = range.bytes;
    let mut decoded = false;
    if request.decode_content && !metadata.content_codings.is_empty() {
        if metadata.availability != BodyAvailability::Complete {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                incomplete_body_message(&metadata, "Content decoding"),
                false,
            ));
        }
        bytes = decode_content(&metadata.content_codings, bytes).await?;
        decoded = true;
    }
    let decoded_truncated = decoded && bytes.len() > max_bytes;
    if decoded_truncated {
        bytes.truncate(max_bytes);
    }
    let (representation, display, warning, text_encoding) = render_body(
        request.representation,
        &bytes,
        metadata.charset.as_deref(),
        metadata.media_type.as_deref(),
        request.offset,
    );
    let consumed = bytes.len();
    let next_offset = (!request.decode_content
        && request.offset.saturating_add(consumed as u64) < metadata.retained_bytes)
        .then_some(request.offset.saturating_add(consumed as u64));
    Ok(BodyInspection {
        metadata,
        representation,
        decoded,
        text_encoding,
        display,
        display_bytes: consumed,
        truncated: next_offset.is_some() || decoded_truncated,
        next_offset,
        warning,
        preview_handle: None,
        preview_mime_type: None,
    })
}

pub(crate) async fn image_source(
    body_store: Option<&BodyStore>,
    request: &BodyInspectionRequest,
) -> Result<(StoredBodyMetadata, Vec<u8>, bool), AppError> {
    if request.offset != 0 {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "image previews must begin at offset zero",
            false,
        ));
    }
    let store = body_store.ok_or_else(|| {
        AppError::new(
            ErrorCategory::Unavailable,
            "response body retention is not configured",
            false,
        )
    })?;
    let exchange_id = transmog_core::intercept::ExchangeId(parse_session_id(&request.session_id)?);
    let boundary = parse_boundary(&request.boundary)?;
    let metadata = store
        .metadata(exchange_id)
        .into_iter()
        .find(|candidate| candidate.boundary == boundary_name(boundary))
        .ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "body boundary is unavailable",
                false,
            )
        })?;
    if metadata.availability != BodyAvailability::Complete {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            incomplete_body_message(&metadata, "Image preview"),
            false,
        ));
    }
    let length = usize::try_from(metadata.retained_bytes)
        .ok()
        .filter(|length| *length <= transmog_preview_worker::MAX_SOURCE_BYTES)
        .ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "encoded image exceeds the sixteen MiB preview limit",
                false,
            )
        })?;
    let range = store
        .read_range(exchange_id, boundary, 0, length)
        .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), false))?;
    if range.bytes.len() != length {
        return Err(AppError::new(
            ErrorCategory::Unavailable,
            "complete image bytes are unavailable",
            false,
        ));
    }
    let decoded = !metadata.content_codings.is_empty();
    let bytes = if decoded {
        decode_content(&metadata.content_codings, range.bytes).await?
    } else {
        range.bytes
    };
    if bytes.len() > transmog_preview_worker::MAX_SOURCE_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "decoded image exceeds the sixteen MiB preview limit",
            false,
        ));
    }
    Ok((metadata, bytes, decoded))
}

fn incomplete_body_message(metadata: &StoredBodyMetadata, operation: &str) -> String {
    format!(
        "{operation} needs a complete body; capture status {:?}, {} of {} observed bytes retained. {}",
        metadata.availability,
        metadata.retained_bytes,
        metadata.observed_bytes,
        metadata
            .reason
            .as_deref()
            .unwrap_or("The body is still being captured or its terminal event was not received")
    )
}

pub(crate) fn parse_session_id(id: &str) -> Result<u128, AppError> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "session identifier is invalid",
            false,
        ));
    }
    u128::from_str_radix(id, 16).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "session identifier is invalid",
            false,
        )
    })
}

fn parse_boundary(value: &str) -> Result<ExchangeBoundary, AppError> {
    match value {
        "client-request" => Ok(ExchangeBoundary::ClientRequest),
        "upstream-request" => Ok(ExchangeBoundary::UpstreamRequest),
        "upstream-response" => Ok(ExchangeBoundary::UpstreamResponse),
        "client-response" => Ok(ExchangeBoundary::ClientResponse),
        _ => Err(AppError::new(
            ErrorCategory::InvalidInput,
            "body boundary is invalid",
            false,
        )),
    }
}

const fn boundary_name(value: ExchangeBoundary) -> &'static str {
    match value {
        ExchangeBoundary::ClientRequest => "client-request",
        ExchangeBoundary::UpstreamRequest => "upstream-request",
        ExchangeBoundary::UpstreamResponse => "upstream-response",
        ExchangeBoundary::ClientResponse => "client-response",
    }
}

pub(crate) async fn decode_content(
    codings: &[String],
    encoded: Vec<u8>,
) -> Result<Vec<u8>, AppError> {
    let field = HeaderField::try_new("content-encoding", codings.join(", ")).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "stored content-coding metadata is invalid",
            false,
        )
    })?;
    let headers = HeaderBlock::from_fields(vec![field]);
    let default_limits = ContentLimits::default();
    let preview_limit =
        NonZeroUsize::new(MAX_BODY_READ_BYTES).expect("the inspector hard limit is nonzero");
    let limits = ContentLimits::new(
        preview_limit,
        preview_limit,
        preview_limit,
        default_limits.max_decoder_window_bytes(),
        default_limits.max_expansion_ratio(),
        default_limits.expansion_slack_bytes(),
        default_limits.max_coding_layers(),
    )
    .with_work_limits(default_limits.work_limits());
    let stack = ContentCodingStack::from_headers(&headers, limits.max_coding_layers())
        .map_err(|error| AppError::new(ErrorCategory::InvalidInput, error.to_string(), false))?;
    let mut current = encoded;
    for coding in stack.decode_order() {
        let mut codec = ContentDecoder::new(coding, limits)
            .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), false))?;
        let mut frames = codec
            .on_frame(BodyFrame::Data(Bytes::from(current)))
            .await
            .map_err(content_decode_error)?;
        frames.extend(codec.finish().await.map_err(content_decode_error)?);
        let mut decoded_bytes = Vec::new();
        for frame in frames {
            if let BodyFrame::Data(bytes) = frame {
                decoded_bytes.extend_from_slice(&bytes);
            }
        }
        current = decoded_bytes;
    }
    Ok(current)
}

pub(crate) async fn encode_content(
    codings: &[String],
    decoded: Vec<u8>,
) -> Result<Vec<u8>, AppError> {
    if codings.is_empty() {
        return Ok(decoded);
    }
    let field = HeaderField::try_new("content-encoding", codings.join(", ")).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "stored content-coding metadata is invalid",
            false,
        )
    })?;
    let headers = HeaderBlock::from_fields(vec![field]);
    let default_limits = ContentLimits::default();
    let edit_limit =
        NonZeroUsize::new(MAX_BODY_READ_BYTES).expect("the edit hard limit is nonzero");
    let limits = ContentLimits::new(
        edit_limit,
        edit_limit,
        edit_limit,
        default_limits.max_decoder_window_bytes(),
        default_limits.max_expansion_ratio(),
        default_limits.expansion_slack_bytes(),
        default_limits.max_coding_layers(),
    )
    .with_work_limits(default_limits.work_limits());
    let stack = ContentCodingStack::from_headers(&headers, limits.max_coding_layers())
        .map_err(|error| AppError::new(ErrorCategory::InvalidInput, error.to_string(), false))?;
    let mut current = decoded;
    for coding in stack.encode_order() {
        let mut codec = ContentEncoder::new(coding, limits)
            .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), false))?;
        let mut frames = codec
            .on_frame(BodyFrame::Data(Bytes::from(current)))
            .await
            .map_err(content_encode_error)?;
        frames.extend(codec.finish().await.map_err(content_encode_error)?);
        let mut encoded = Vec::new();
        for frame in frames {
            if let BodyFrame::Data(bytes) = frame {
                encoded.extend_from_slice(&bytes);
            }
        }
        current = encoded;
    }
    Ok(current)
}

#[allow(clippy::needless_pass_by_value)]
fn content_decode_error(error: transmog_content::ContentCodecError) -> AppError {
    AppError::new(
        ErrorCategory::InvalidInput,
        format!("content decoding failed: {error}"),
        false,
    )
}

#[allow(clippy::needless_pass_by_value)]
fn content_encode_error(error: transmog_content::ContentCodecError) -> AppError {
    AppError::new(
        ErrorCategory::InvalidInput,
        format!("content encoding failed: {error}"),
        false,
    )
}

pub(crate) fn render_body(
    requested: BodyRepresentation,
    bytes: &[u8],
    charset: Option<&str>,
    media_type: Option<&str>,
    offset: u64,
) -> (&'static str, String, Option<String>, Option<&'static str>) {
    let text = decode_unicode(bytes, charset);
    let text_encoding = text
        .as_ref()
        .and_then(|_| detect_unicode_encoding(bytes, charset));
    match requested {
        BodyRepresentation::Bytes => ("bytes", hex_dump(bytes, offset), None, None),
        BodyRepresentation::OriginalText => text.map_or_else(
            || {
                (
                    "unavailable",
                    String::new(),
                    Some("body is not valid presentable Unicode text".to_owned()),
                    None,
                )
            },
            |text| ("original-text", text, None, text_encoding),
        ),
        BodyRepresentation::Formatted => format_json(text.as_deref()).map_or_else(
            || {
                (
                    "unavailable",
                    String::new(),
                    Some("no reviewed formatter accepts this body".to_owned()),
                    None,
                )
            },
            |formatted| ("formatted-json", formatted, None, text_encoding),
        ),
        BodyRepresentation::Auto => {
            if media_type.is_some_and(is_json_media_type)
                && let Some(formatted) = format_json(text.as_deref())
            {
                ("formatted-json", formatted, None, text_encoding)
            } else if let Some(text) = text.filter(|_| media_type.is_none_or(is_text_media_type)) {
                ("original-text", text, None, text_encoding)
            } else {
                ("bytes", hex_dump(bytes, offset), None, None)
            }
        }
        BodyRepresentation::Metadata | BodyRepresentation::Image => unreachable!(),
    }
}

fn detect_unicode_encoding(bytes: &[u8], declared: Option<&str>) -> Option<&'static str> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        Some("utf-8-bom")
    } else if bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        Some("utf-32le-bom")
    } else if bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        Some("utf-32be-bom")
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        Some("utf-16le-bom")
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some("utf-16be-bom")
    } else {
        match declared
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("utf-16" | "utf-16le") => Some("utf-16le"),
            Some("utf-16be") => Some("utf-16be"),
            Some("utf-32" | "utf-32le") => Some("utf-32le"),
            Some("utf-32be") => Some("utf-32be"),
            Some("us-ascii") => Some("us-ascii"),
            Some("utf-8") | None => Some("utf-8"),
            Some(_) => None,
        }
    }
}

fn is_json_media_type(media_type: &str) -> bool {
    media_type.eq_ignore_ascii_case("application/json")
        || media_type.to_ascii_lowercase().ends_with("+json")
}

fn is_text_media_type(media_type: &str) -> bool {
    let mime = media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("text/")
        || is_json_media_type(&mime)
        || mime.ends_with("+xml")
        || matches!(
            mime.as_str(),
            "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/ecmascript"
                | "application/x-www-form-urlencoded"
                | "application/yaml"
                | "application/x-yaml"
        )
}

fn format_json(text: Option<&str>) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text?).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

fn decode_unicode(bytes: &[u8], declared: Option<&str>) -> Option<String> {
    let normalized = declared.map(|value| value.trim().to_ascii_lowercase());
    let text = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        String::from_utf8(bytes[3..].to_vec()).ok()?
    } else if bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        decode_utf32(&bytes[4..], true)?
    } else if bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        decode_utf32(&bytes[4..], false)?
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        decode_utf16(&bytes[2..], true)?
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        decode_utf16(&bytes[2..], false)?
    } else {
        match normalized.as_deref() {
            Some("utf-16" | "utf-16le") => decode_utf16(bytes, true)?,
            Some("utf-16be") => decode_utf16(bytes, false)?,
            Some("utf-32" | "utf-32le") => decode_utf32(bytes, true)?,
            Some("utf-32be") => decode_utf32(bytes, false)?,
            Some("us-ascii") if bytes.iter().all(u8::is_ascii) => {
                String::from_utf8(bytes.to_vec()).ok()?
            }
            Some("utf-8") | None => String::from_utf8(bytes.to_vec()).ok()?,
            Some(_) => return None,
        }
    };
    text.chars()
        .all(|character| {
            !character.is_control() || matches!(character, '\r' | '\n' | '\t' | '\u{000c}')
        })
        .then_some(text)
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes.chunks_exact(2).map(|chunk| {
        if little_endian {
            u16::from_le_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], chunk[1]])
        }
    });
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .ok()
}

fn decode_utf32(bytes: &[u8], little_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let value = if little_endian {
                u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
            } else {
                u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
            };
            char::from_u32(value)
        })
        .collect()
}

fn hex_dump(bytes: &[u8], offset: u64) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(4));
    for (line, chunk) in bytes.chunks(16).enumerate() {
        let line_offset = offset.saturating_add((line * 16) as u64);
        let _ = write!(output, "{line_offset:08x}  ");
        for index in 0..16 {
            if let Some(byte) = chunk.get(index) {
                let _ = write!(output, "{byte:02x} ");
            } else {
                output.push_str("   ");
            }
            if index == 7 {
                output.push(' ');
            }
        }
        output.push_str(" |");
        for byte in chunk {
            output.push(if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            });
        }
        output.push_str("|\n");
    }
    output
}

fn headers(block: &HeaderBlock) -> Vec<HeaderView> {
    block
        .iter()
        .take(512)
        .map(|field| {
            let sensitive = field.name_eq("authorization")
                || field.name_eq("proxy-authorization")
                || field.name_eq("cookie")
                || field.name_eq("set-cookie");
            let (value, binary) = display_bytes(field.value());
            HeaderView {
                name: display_text(field.name()),
                value,
                binary,
                sensitive,
            }
        })
        .collect()
}

fn body(body: &BodySnapshot) -> BodyView {
    let retained = body
        .retained_prefix
        .get(..body.retained_prefix.len().min(MAX_DISPLAY_BYTES))
        .unwrap_or_default();
    let (display, binary) = display_bytes(retained);
    BodyView {
        boundary: boundary(body.boundary),
        observed_bytes: body.observed_bytes,
        display,
        representation: if body.observed_bytes > 0 && retained.is_empty() {
            "missing"
        } else if binary {
            "hex"
        } else {
            "text"
        },
        truncated: body.truncated
            || body.retained_prefix.len() > MAX_DISPLAY_BYTES
            || body.observed_bytes > body.retained_prefix.len() as u64,
        trailers: body.trailers.as_ref().map_or_else(Vec::new, headers),
    }
}

fn display_bytes(bytes: &[u8]) -> (String, bool) {
    if std::str::from_utf8(bytes).is_ok_and(|text| {
        text.chars()
            .all(|character| !character.is_control() || matches!(character, '\r' | '\n' | '\t'))
    }) {
        (display_text(bytes), false)
    } else {
        let bytes = &bytes[..bytes.len().min(MAX_DISPLAY_BYTES)];
        let mut output = String::with_capacity(bytes.len().saturating_mul(3));
        for (index, byte) in bytes.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            let _ = write!(output, "{byte:02x}");
        }
        (output, true)
    }
}

fn display_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(MAX_FIELD_CHARS)
        .collect()
}

fn bounded_debug(value: &impl std::fmt::Debug) -> String {
    display_text(format!("{value:?}").as_bytes())
}

fn boundary(value: ExchangeBoundary) -> String {
    match value {
        ExchangeBoundary::ClientRequest => "client-request",
        ExchangeBoundary::UpstreamRequest => "upstream-request",
        ExchangeBoundary::UpstreamResponse => "upstream-response",
        ExchangeBoundary::ClientResponse => "client-response",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_content::{ContentCoding, ContentEncoder};
    use transmog_core::HeaderField;

    #[test]
    fn hostile_text_and_binary_are_explicit_and_bounded() {
        assert_eq!(display_text(b"<script>\n"), "<script>\n");
        let (binary, is_binary) = display_bytes(&[0, 0xff, b'<']);
        assert!(is_binary);
        assert_eq!(binary, "00 ff 3c");
        let long = vec![b'a'; MAX_FIELD_CHARS * 2];
        assert_eq!(display_text(&long).len(), MAX_FIELD_CHARS);
    }

    #[test]
    fn duplicate_sensitive_headers_are_preserved_and_marked() {
        let block = HeaderBlock::from_fields(vec![
            HeaderField::try_new("cookie", "redacted").unwrap(),
            HeaderField::try_new("Cookie", "redacted-again").unwrap(),
        ]);
        let view = headers(&block);
        assert_eq!(view.len(), 2);
        assert!(view.iter().all(|field| field.sensitive));
    }

    #[test]
    fn quoted_header_values_are_escaped_only_by_json_serialization() {
        let block = HeaderBlock::from_fields(vec![
            HeaderField::try_new("sec-ch-ua-platform", r#""Windows""#).unwrap(),
            HeaderField::try_new(
                "sec-ch-ua",
                r#""Chromium";v="154", "Google Chrome";v="154""#,
            )
            .unwrap(),
        ]);
        let view = headers(&block);
        assert_eq!(view[0].value, r#""Windows""#);
        assert_eq!(
            view[1].value,
            r#""Chromium";v="154", "Google Chrome";v="154""#
        );
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains(r#""value":"\"Windows\"""#));
        assert!(!json.contains(r#""value":"\\\"Windows\\\"""#));
    }

    #[test]
    fn head_boundaries_match_stored_body_boundary_identifiers() {
        assert_eq!(boundary(ExchangeBoundary::ClientRequest), "client-request");
        assert_eq!(
            boundary(ExchangeBoundary::UpstreamRequest),
            "upstream-request"
        );
        assert_eq!(
            boundary(ExchangeBoundary::UpstreamResponse),
            "upstream-response"
        );
        assert_eq!(
            boundary(ExchangeBoundary::ClientResponse),
            "client-response"
        );
    }

    #[test]
    fn utf_variants_json_and_byte_dump_are_explicit() {
        assert_eq!(
            decode_unicode(b"plain ASCII", None).as_deref(),
            Some("plain ASCII")
        );
        assert_eq!(
            decode_unicode(&[0xff, 0xfe, b'h', 0, b'i', 0], None).as_deref(),
            Some("hi")
        );
        assert_eq!(
            detect_unicode_encoding(&[0xff, 0xfe, b'h', 0], None),
            Some("utf-16le-bom")
        );
        assert_eq!(
            decode_unicode(&[0, 0, 0xfe, 0xff, 0, 0, 0, b'Z'], None).as_deref(),
            Some("Z")
        );
        let rendered = render_body(
            BodyRepresentation::Auto,
            br#"{"a":1}"#,
            Some("utf-8"),
            Some("application/json"),
            0,
        );
        assert_eq!(rendered.0, "formatted-json");
        assert!(rendered.1.contains('\n'));
        assert_eq!(rendered.3, Some("utf-8"));
        assert!(hex_dump(&[0, b'A', 0xff], 16).starts_with("00000010"));
    }

    #[test]
    fn automatic_viewer_respects_binary_and_text_content_types() {
        for (mime, expected) in [
            (Some("text/html; charset=utf-8"), "original-text"),
            (Some("application/problem+json"), "formatted-json"),
            (Some("application/octet-stream"), "bytes"),
            (None, "original-text"),
        ] {
            assert_eq!(
                render_body(
                    BodyRepresentation::Auto,
                    br#"{"ok":true}"#,
                    Some("utf-8"),
                    mime,
                    0
                )
                .0,
                expected
            );
        }
    }

    #[tokio::test]
    async fn preview_decoder_supports_every_content_coding() {
        for coding in [
            ContentCoding::Gzip,
            ContentCoding::Deflate,
            ContentCoding::Brotli,
            ContentCoding::Zstd,
        ] {
            let mut codec = ContentEncoder::new(coding, ContentLimits::default()).unwrap();
            let mut frames = codec
                .on_frame(BodyFrame::Data(Bytes::from_static(b"encoded preview")))
                .await
                .unwrap();
            frames.extend(codec.finish().await.unwrap());
            let encoded_bytes = frames
                .into_iter()
                .filter_map(|frame| match frame {
                    BodyFrame::Data(bytes) => Some(bytes),
                    BodyFrame::Trailers(_) => None,
                })
                .flatten()
                .collect::<Vec<_>>();
            assert_eq!(
                decode_content(&[coding.as_str().to_owned()], encoded_bytes)
                    .await
                    .unwrap(),
                b"encoded preview"
            );
        }
    }

    #[tokio::test]
    async fn autoresponse_edits_restore_every_original_content_coding() {
        for coding in ["gzip", "deflate", "br", "zstd"] {
            let encoded = encode_content(&[coding.to_owned()], b"edited response".to_vec())
                .await
                .unwrap();
            assert_ne!(encoded, b"edited response");
            assert_eq!(
                decode_content(&[coding.to_owned()], encoded).await.unwrap(),
                b"edited response"
            );
        }
    }
}

use std::fmt::Write as _;

use serde::Serialize;
use transmog_core::{HeaderBlock, observe::ExchangeBoundary};
use transmog_session::{ApplicationSessionService, BodySnapshot, SessionTerminal};

use crate::{AppError, ErrorCategory};

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
    /// Request heads at original/effective boundaries.
    pub requests: Vec<HeadView>,
    /// Response heads at original/effective boundaries.
    pub responses: Vec<HeadView>,
    /// Retained body evidence.
    pub bodies: Vec<BodyView>,
    /// Redaction-safe initialization diagnostics.
    pub diagnostics: Vec<String>,
    /// Centrally attributed hook effects.
    pub hook_effects: Vec<String>,
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

#[allow(clippy::too_many_lines)]
pub(crate) fn session_detail(
    service: &ApplicationSessionService,
    id: &str,
) -> Result<SessionDetail, AppError> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "session identifier is invalid",
            false,
        ));
    }
    let numeric = u128::from_str_radix(id, 16).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "session identifier is invalid",
            false,
        )
    })?;
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
        requests,
        responses,
        bodies,
        diagnostics,
        hook_effects,
        route_selection,
        route_attempts,
        terminal,
        websocket: snapshot.websocket.as_ref().map(bounded_debug),
        sequence_loss: snapshot.sequence_loss,
    })
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
        .flat_map(char::escape_default)
        .take(MAX_FIELD_CHARS)
        .collect()
}

fn bounded_debug(value: &impl std::fmt::Debug) -> String {
    display_text(format!("{value:?}").as_bytes())
}

fn boundary(value: ExchangeBoundary) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_core::HeaderField;

    #[test]
    fn hostile_text_and_binary_are_explicit_and_bounded() {
        assert_eq!(display_text(b"<script>\n"), "<script>\\n");
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
}

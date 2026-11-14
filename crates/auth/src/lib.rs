//! Deterministic offline inspection. Decoding never establishes trust or sends traffic.
#![allow(clippy::missing_errors_doc, clippy::too_many_lines)]
mod body;
mod catalog;
mod context;
mod document;
mod extract;
mod http;
mod jose;
mod json;
mod limits;
mod oauth;
mod saml;
mod scratch;
mod windows;

pub use catalog::{AUTH_CONVERSIONS, AuthConversion, conversion_for_format};
pub(crate) use context::Context;
pub use extract::is_candidate_header;
pub use jose::decode_jwt;
pub use limits::{MAX_BODY, MAX_CANDIDATE, MAX_DECODED_BYTES, MAX_DEPTH, MAX_FIELDS, MAX_TOTAL};
pub use scratch::decode_authentication;

use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic::AtomicBool};

/// Original occurrence in the selected message; transformed offsets never imply compressed mapping.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// Header, query, redirect query/fragment, or decoded body.
    pub kind: String,
    /// Inert human-readable original-source label.
    pub label: String,
    /// Original field index, independent of header display pagination.
    pub occurrence: usize,
    /// Enclosing original header ordinal, independent of a nested parameter occurrence.
    pub header_index: Option<usize>,
    /// Exact byte start within the named source, if available.
    pub offset: Option<usize>,
    /// Exact byte length, if available.
    pub length: Option<usize>,
    /// Ordered transformation provenance.
    pub transforms: Vec<String>,
}
impl Source {
    /// Header-derived parameters still belong to the retained header scan.
    pub(crate) fn coverage_source(&self) -> &str {
        if self.header_index.is_some() || self.kind == "header" {
            "headers"
        } else {
            &self.kind
        }
    }

    /// Decoder transformations may differ while referring to the same original occurrence/range.
    pub(crate) fn same_range(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.label == other.label
            && self.occurrence == other.occurrence
            && self.header_index == other.header_index
            && self.offset == other.offset
            && self.length == other.length
    }
}
/// One masked or readable field. Full values are held separately from serialization.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// Stable field occurrence in one evidence revision.
    pub id: String,
    /// Observed field role.
    pub name: String,
    /// Masked text for sensitive values; bounded inert text otherwise.
    pub value: String,
    /// Whether explicit reveal is required.
    pub sensitive: bool,
    /// Whether the retained value can be retrieved.
    pub available: bool,
    /// Meaning or interpretation limits, without captured secrets.
    pub note: String,
    #[serde(skip)]
    pub(crate) technical: bool,
}
/// One observed format; nested wrappers retain independent statuses.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Stable top-level or nested occurrence.
    pub id: String,
    /// Format name, without captured values.
    pub format: String,
    /// This decoder layer can wrap one confirmed structure from the same retained range.
    pub envelope: bool,
    /// Credential/challenge/protocol role.
    pub role: String,
    /// Recognized or candidate.
    pub recognition: String,
    /// Structural outcome, independent of verification.
    pub status: String,
    /// Not checked or not applicable.
    pub verification: String,
    /// Original source and decoding chain.
    pub source: Source,
    /// Readable bounded fields.
    pub fields: Vec<Field>,
    /// Inner recognized structures.
    pub children: Vec<Finding>,
    /// Finite parser observations that never contain raw input.
    pub diagnostics: Vec<String>,
    /// Available scratch operation; raw input remains in the private value store.
    pub converter_operation: Option<&'static str>,
    // Typed parser material is deliberately excluded from inspection serialization.
    // Only the explicit scratch adapter may expose this credential-bearing content.
    #[serde(skip)]
    pub(crate) content: Vec<(String, json::Node)>,
    #[serde(skip)]
    pub(crate) complete: bool,
    #[serde(skip)]
    token_value: Option<String>,
}
impl Finding {
    pub(crate) fn new(format: &str, role: &str, source: Source) -> Self {
        Self {
            id: String::new(),
            format: format.into(),
            envelope: matches!(
                format,
                "Bearer"
                    | "Negotiate"
                    | "NTLM"
                    | "GSS-API / SPNEGO"
                    | "SPNEGO"
                    | "SAML 2.0"
                    | "Auth parameter"
            ),
            role: role.into(),
            recognition: "recognized".into(),
            status: "decoded".into(),
            verification: "Not applicable".into(),
            source,
            fields: vec![],
            children: vec![],
            diagnostics: vec![],
            content: vec![],
            complete: true,
            converter_operation: None,
            token_value: None,
        }
    }
    pub(crate) fn issue(&mut self, status: &str, note: &str) {
        if ["malformed", "incomplete", "limited", "redacted"].contains(&status) {
            self.complete = false;
        }
        let rank = |status: &str| match status {
            "limited" => 8,
            "redacted" => 7,
            "malformed" => 6,
            "unsupported" => 5,
            "incomplete" => 4,
            "encrypted" => 3,
            "opaque" => 2,
            _ => 1,
        };
        if rank(status) >= rank(&self.status) {
            self.status = status.into();
        }
        if self.diagnostics.len() < limits::MAX_DIAGNOSTICS {
            self.diagnostics.push(note.into());
        }
    }
    pub(crate) fn ambiguity(&mut self, note: &str) {
        let complete = self.complete;
        self.issue("incomplete", note);
        self.complete = complete;
    }
}
/// Coverage distinguishes unscanned and unavailable evidence from a completed negative result.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Selected source category.
    pub source: String,
    /// Complete, pending, unsupported, limited, incomplete, unavailable, redacted, or skipped.
    pub status: String,
    /// Inert explanation.
    pub detail: String,
}
/// Safe summary plus nonserializable active-view material.
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    /// Detected top-level artifacts; nested decoders do not inflate this count.
    pub findings: Vec<Finding>,
    /// What was and was not inspected.
    pub coverage: Vec<Coverage>,
    /// Aggregate limits and cancellation observations.
    pub diagnostics: Vec<String>,
    #[serde(skip)]
    values: Vec<String>,
}
impl Inspection {
    /// Exact retained token representation for an explicit scratch handoff.
    /// The application must authorize the active evidence before and after retrieving it.
    pub fn converter_token(&self, finding: &str) -> Option<(&str, &'static str)> {
        fn find<'a>(items: &'a [Finding], id: &str) -> Option<&'a Finding> {
            items.iter().find_map(|item| {
                if item.id == id {
                    Some(item)
                } else {
                    find(&item.children, id)
                }
            })
        }
        let finding = find(&self.findings, finding)?;
        Some((
            self.value(finding.token_value.as_ref()?)?,
            finding.converter_operation?,
        ))
    }
    /// Explicit selected-value access. The application must reauthorize retention and generation.
    pub fn value(&self, id: &str) -> Option<&str> {
        id.strip_prefix('v')?
            .parse::<usize>()
            .ok()
            .and_then(|index| self.values.get(index))
            .map(String::as_str)
    }
}
/// Original retained header, never a display preview.
pub struct Header {
    /// Source bytes omitted by native candidate/aggregate admission limits.
    pub limited: bool,
    /// Original header name.
    pub name: String,
    /// Exact retained bytes.
    pub value: Vec<u8>,
    /// Redaction is authoritative; bytes must not be decoded.
    pub redacted: bool,
}
/// Decoded retained body with independently enforced HTTP decoding limits.
pub struct Body {
    /// UTF text bytes or a supported BOM encoding.
    pub bytes: Vec<u8>,
    /// Original media type including charset.
    pub media_type: String,
    /// HTTP content decoding steps, preceding body extraction.
    pub transforms: Vec<String>,
}
/// One bounded immutable message; storage and window access are outside the parser.
pub struct Input {
    /// Full ordered retained header block.
    pub headers: Vec<Header>,
    /// Complete request target, if this is a request.
    pub target: Option<String>,
    /// Explicit eligible body scan; no traffic-wide body discovery.
    pub body: Option<Body>,
    /// Body retention/scan state when no body was provided.
    pub body_status: String,
    /// Observed capture time, in Unix seconds, if available.
    pub captured_at: Option<i64>,
}
pub(crate) fn clip(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.into();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}… [display shortened; copy retrieves the retained value]",
        &value[..end]
    )
}
/// Inspect one message with cooperative cancellation and bounded retained material.
// Keep the owning public API for existing worker callers; the internal parser also supports a borrow.
#[allow(clippy::needless_pass_by_value)]
pub fn inspect(input: Input, canceled: Arc<AtomicBool>) -> Inspection {
    let mut context = Context::new(&canceled);
    context.captured_at = input.captured_at;
    extract::message(&mut context, input);
    context.result
}

/// Explicit bounded interpretation of the selected decoded body, retaining normal header/query findings.
/// Unknown or malformed values remain candidates; this does not verify a credential.
pub fn inspect_as(mut input: Input, canceled: Arc<AtomicBool>, format: &str) -> Inspection {
    if format == "auto" {
        return inspect(input, canceled);
    }
    let mut context = Context::new(&canceled);
    context.captured_at = input.captured_at;
    let body = input.body.take();
    extract::message(&mut context, input);
    let Some(body) = body else {
        return context.result;
    };
    context
        .result
        .coverage
        .retain(|source| source.source != "body");
    if body.bytes.len() > MAX_CANDIDATE || !context.reserve_bytes(body.bytes.len()) {
        context.coverage(
            "body",
            "limited",
            "Explicit auth interpretation exceeds the candidate budget.",
        );
        return context.result;
    }
    let mut source = Source {
        kind: "body".into(),
        label: "Selected decoded body · explicit interpretation".into(),
        transforms: body.transforms.clone(),
        ..Source::default()
    };
    source
        .transforms
        .push("Explicit user interpretation; no automatic classification implied".into());
    let text = body::text(&body.bytes, &body.media_type);
    if text.is_some() && ["saml", "jose"].contains(&format) {
        source
            .transforms
            .push(body::character_transform(&body.bytes).into());
    }
    let finding = match format {
        "jose" => text
            .as_ref()
            .and_then(|raw| jose::decode(&mut context, raw.trim(), source.clone())),
        "saml" => text
            .as_ref()
            .and_then(|raw| saml::xml(&mut context, raw.as_bytes(), source.clone())),
        "ntlm" | "negotiate" => {
            use base64::Engine as _;
            let bytes = text
                .as_ref()
                .and_then(|raw| {
                    base64::engine::general_purpose::STANDARD
                        .decode(raw.trim())
                        .ok()
                })
                .or_else(|| windows::is_binary_candidate(&body.bytes).then(|| body.bytes.clone()));
            bytes.map(|bytes| {
                windows::decode(&mut context, &bytes, source.clone(), format == "ntlm")
            })
        }
        _ => None,
    };
    if let Some(finding) = finding {
        context.push(finding);
    } else {
        let mut candidate = Finding::new("Explicit auth interpretation", format, source);
        candidate.recognition = "candidate".into();
        candidate.issue("malformed", "Selected body is not the requested supported format, or exceeds structural/text limits.");
        context.push(candidate);
    }
    context.coverage("body", "complete", "Explicit bounded interpretation of the selected retained body; all values remain unverified.");
    context.result
}

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod windows_tests;

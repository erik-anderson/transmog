//! Deterministic offline inspection. Decoding never establishes trust or sends traffic.
#![allow(clippy::missing_errors_doc, clippy::too_many_lines)]
mod body;
mod extract;
mod http;
mod jose;
mod json;
mod oauth;
mod saml;
mod windows;

pub use extract::is_candidate_header;
pub use jose::decode_jwt;

use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Maximum candidate bytes before any decoding or allocation from encoded lengths.
pub const MAX_CANDIDATE: usize = 1024 * 1024;
/// Maximum automatically read encoded body.
pub const MAX_BODY: usize = 2 * 1024 * 1024;
/// Aggregate input, decoded material and retained value budget.
pub const MAX_TOTAL: usize = 8 * 1024 * 1024;
/// Maximum output fields across all nested findings.
pub const MAX_FIELDS: usize = 2048;
/// Maximum parser nesting.
pub const MAX_DEPTH: usize = 24;

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
}
/// One observed format; nested wrappers retain independent statuses.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Stable top-level or nested occurrence.
    pub id: String,
    /// Format name, without captured values.
    pub format: String,
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
}
impl Finding {
    pub(crate) fn new(format: &str, role: &str, source: Source) -> Self {
        Self {
            id: String::new(),
            format: format.into(),
            role: role.into(),
            recognition: "recognized".into(),
            status: "decoded".into(),
            verification: "Not applicable".into(),
            source,
            fields: vec![],
            children: vec![],
            diagnostics: vec![],
        }
    }
    pub(crate) fn issue(&mut self, status: &str, note: &str) {
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
        if self.diagnostics.len() < 16 {
            self.diagnostics.push(note.into());
        }
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
pub(crate) struct Context {
    result: Inspection,
    canceled: Arc<AtomicBool>,
    bytes_left: usize,
    work_left: usize,
    captured_at: Option<i64>,
}
impl Context {
    fn new(canceled: Arc<AtomicBool>) -> Self {
        Self {
            result: Inspection::default(),
            canceled,
            bytes_left: MAX_TOTAL,
            work_left: 100_000,
            captured_at: None,
        }
    }
    pub(crate) fn step(&mut self) -> bool {
        if self.canceled.load(Ordering::Relaxed) || self.work_left == 0 {
            return false;
        }
        self.work_left -= 1;
        true
    }
    pub(crate) fn charge(&mut self, count: usize) -> bool {
        if count > self.bytes_left || !self.step() {
            self.limited();
            return false;
        }
        self.bytes_left -= count;
        true
    }
    pub(crate) fn limited(&mut self) {
        if self.result.diagnostics.is_empty() {
            self.result.diagnostics.push(
                "Inspection stopped at its finite work, byte or field budget; coverage is partial."
                    .into(),
            );
        }
    }
    pub(crate) fn field(
        &mut self,
        finding: &mut Finding,
        name: &str,
        value: impl Into<String>,
        sensitive: bool,
        note: &str,
    ) {
        if self.result.values.len() >= MAX_FIELDS || !self.step() {
            self.limited();
            finding.status = "limited".into();
            return;
        }
        let value = value.into();
        if value.len() > MAX_CANDIDATE || !self.charge(value.len()) {
            finding.status = "limited".into();
            return;
        }
        let id = format!("v{}", self.result.values.len());
        let shown = if sensitive {
            "••••••••".into()
        } else {
            clip(&value, 8192)
        };
        self.result.values.push(value);
        finding.fields.push(Field {
            id,
            name: clip(name, 256),
            value: shown,
            sensitive,
            available: true,
            note: note.into(),
        });
    }
    pub(crate) fn push(&mut self, mut finding: Finding) {
        fn name_children(finding: &mut Finding) {
            for (index, child) in finding.children.iter_mut().enumerate() {
                child.id = format!("{}.{index}", finding.id);
                name_children(child);
            }
        }
        if self.result.findings.len() >= 64 {
            self.limited();
            return;
        }
        finding.id = format!("f{}", self.result.findings.len());
        name_children(&mut finding);
        self.result.findings.push(finding);
    }
    pub(crate) fn coverage(&mut self, source: &str, status: &str, detail: &str) {
        if let Some(current) = self
            .result
            .coverage
            .iter_mut()
            .find(|coverage| coverage.source == source)
        {
            let rank = |status: &str| match status {
                "complete" => 0,
                "skipped" => 1,
                "pending" => 2,
                "unsupported" => 3,
                "limited" => 5,
                "redacted" => 6,
                _ => 4,
            };
            if rank(status) > rank(&current.status) {
                current.status = status.into();
            }
            if !current.detail.contains(detail) {
                current.detail = clip(&format!("{} {detail}", current.detail), 1024);
            }
            return;
        }
        if self.result.coverage.len() < 16 {
            self.result.coverage.push(Coverage {
                source: source.into(),
                status: status.into(),
                detail: detail.into(),
            });
        }
    }
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
/// Inspect one message with cooperative cancellation and aggregate finite budgets.
pub fn inspect(input: Input, canceled: Arc<AtomicBool>) -> Inspection {
    let mut context = Context::new(canceled);
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
    let mut context = Context::new(canceled);
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
    if body.bytes.len() > MAX_CANDIDATE || !context.charge(body.bytes.len()) {
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
            let bytes =
                text.as_ref()
                    .and_then(|raw| {
                        base64::engine::general_purpose::STANDARD
                            .decode(raw.trim())
                            .ok()
                    })
                    .or_else(|| {
                        (body.bytes.starts_with(b"NTLMSSP\0")
                            || body.bytes.first().is_some_and(|tag| {
                                [0x60, 0xa0, 0xa1, 0x6e, 0x6f, 0x7e].contains(tag)
                            }))
                        .then(|| body.bytes.clone())
                    });
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

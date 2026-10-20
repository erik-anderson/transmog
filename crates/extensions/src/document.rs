use crate::{Error, Limits, decimal, invalid, valid_id};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Maximum recursive structured depth accepted by host renderers.
pub const MAX_TREE_DEPTH: usize = 32;

/// Parse outcome distinct from the completeness of captured evidence.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ParseStatus {
    /// All declared interpretation completed.
    Complete,
    /// Some interpretation or context is unavailable.
    Partial,
    /// Malformed protocol input.
    Malformed,
    /// Declared input unavailable.
    Unavailable,
    /// Host or parser budget exhausted.
    Limited,
}

/// Optional, irreversible update policy for this visualization instance.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePolicy {
    /// No future traffic can improve this view. Stop its live subscription.
    Final,
}

/// Host-owned primary presentation. No templates or executable UI are accepted.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Presentation {
    /// Hierarchical fields.
    Tree,
    /// Homogeneous rows of child fields.
    Table,
    /// Escaped text values.
    Text,
    /// Source bytes with exact offsets.
    Hex,
}

/// Lossless value categories; wide numbers are decimal text.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Value {
    /// Plain captured/interpreted text.
    String(String),
    /// Decimal signed integer, preserving values beyond JS safe integers.
    Integer(String),
    /// Finite floating point value.
    Float(f64),
    /// Boolean value.
    Boolean(bool),
    /// Protocol enumeration label and exact raw integer.
    Enum {
        /// Human-readable label.
        name: String,
        /// Exact raw integer.
        raw: String,
    },
    /// Protocol flag labels and exact raw integer.
    Flags {
        /// Active known labels.
        names: Vec<String>,
        /// Exact raw integer.
        raw: String,
    },
    /// Display timestamp; raw bytes remain linked by spans.
    Timestamp(String),
    /// Standard hyphenated GUID.
    Guid(String),
    /// A byte sequence referenced through source spans.
    Bytes,
    /// Structured fields.
    Object,
    /// Ordered repeated values.
    Array,
}

/// Interpretation metadata for a value or derived structure.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Format {
    /// Protocol or schema name.
    pub protocol: Option<String>,
    /// Character encoding of interpreted bytes.
    pub charset: Option<String>,
    /// Byte order, such as little-endian.
    pub byte_order: Option<String>,
    /// Formatting radix hint, restricted to binary/decimal/hexadecimal.
    pub radix: Option<u8>,
    /// Compression/encoding description, always inert text.
    pub encoding: Option<String>,
    /// Units, when applicable.
    pub units: Option<String>,
}

/// Exact byte range within one granted source or validated derived buffer.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Span {
    /// Opaque source identity, never a path or URL.
    pub source_id: String,
    /// Canonical unsigned byte offset.
    pub offset: String,
    /// Canonical unsigned byte length.
    pub length: String,
}

/// A tree node whose identity and source references the host validates before render.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    /// Stable identity within a document revision.
    pub id: String,
    /// Plain field label.
    pub name: String,
    /// Optional typed value.
    pub value: Option<Value>,
    /// Optional interpretation metadata.
    pub format: Option<Format>,
    /// Associated byte ranges, possibly across multiple sources.
    #[serde(default)]
    pub spans: Vec<Span>,
    /// Already materialized bounded children.
    #[serde(default)]
    pub children: Vec<Node>,
    /// Optional instance-scoped token for deferred child expansion.
    pub children_token: Option<String>,
    /// Nested host renderer hint.
    pub presentation: Option<Presentation>,
}

/// A bounded decoder diagnostic; no rendered markup is interpreted.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diagnostic {
    /// Stable diagnostic category, such as missing-context or unsupported.
    pub code: String,
    /// Plain description intended for the user.
    pub message: String,
    /// Related node, if any.
    pub node_id: Option<String>,
}

/// Generated byte source with explicit provenance back to its enclosing evidence.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivedSource {
    /// Document-local source identity.
    pub id: String,
    /// User-visible byte-source name.
    pub name: String,
    /// Base64 of bounded derived bytes; never a filesystem location.
    pub bytes_base64: String,
    /// Encoded parent regions; reference only original or earlier derived sources.
    pub parents: Vec<Span>,
    /// Transform description, such as MAPI LZ77 decompression.
    pub transform: String,
    /// Whether mapping to each parent span is exact, rather than enclosing-only.
    pub exact_mapping: bool,
}

/// Declarative, inert output consumed only by host-owned renderers.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualizationDocument {
    /// Plain title.
    pub title: String,
    /// Parse status distinct from capture status.
    pub status: ParseStatus,
    /// Omission preserves the current policy; final lasts until a new instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<UpdatePolicy>,
    /// Host renderer selection.
    pub presentation: Presentation,
    /// Plain short summary.
    pub summary: Option<String>,
    /// Bounded root nodes.
    pub roots: Vec<Node>,
    /// Decoder-created sources with parent provenance.
    #[serde(default)]
    pub sources: Vec<DerivedSource>,
    /// Bounded interpretation diagnostics.
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}
impl VisualizationDocument {
    /// Validates all output before exposing any node, byte reference or expansion token.
    /// # Errors
    /// Rejects invalid references, duplicate identities and excessive output cost.
    pub fn validate(
        &self,
        granted_sources: &BTreeMap<String, u64>,
        limits: Limits,
    ) -> Result<(), Error> {
        limits.validate()?;
        let mut budget = Budget {
            strings: 0,
            nodes: 0,
            limits,
        };
        budget.text(&self.title, 512)?;
        if let Some(summary) = &self.summary {
            budget.text(summary, 4096)?;
        }
        if self.sources.len() > 128 || self.diagnostics.len() > 256 {
            return Err(Error::Limit);
        }
        let mut sources = granted_sources.clone();
        let mut derived_bytes = 0_usize;
        for source in &self.sources {
            if !valid_id(&source.id)
                || sources.contains_key(&source.id)
                || source.parents.is_empty()
                || source.parents.len() > 64
            {
                return Err(invalid("Invalid derived source identity or provenance"));
            }
            budget.text(&source.name, 256)?;
            budget.text(&source.transform, 256)?;
            if source.bytes_base64.len() > limits.output_bytes.saturating_mul(2) {
                return Err(Error::Limit);
            }
            let bytes = STANDARD
                .decode(&source.bytes_base64)
                .map_err(|_| invalid("Invalid derived source bytes"))?;
            derived_bytes = derived_bytes.checked_add(bytes.len()).ok_or(Error::Limit)?;
            if derived_bytes > limits.output_bytes {
                return Err(Error::Limit);
            }
            for span in &source.parents {
                check_span(span, &sources)?;
            }
            sources.insert(source.id.clone(), bytes.len() as u64);
        }
        let mut ids = BTreeSet::new();
        for node in &self.roots {
            budget.node(node, 0, &sources, &mut ids)?;
        }
        for diagnostic in &self.diagnostics {
            budget.text(&diagnostic.code, 128)?;
            budget.text(&diagnostic.message, 2048)?;
            if diagnostic
                .node_id
                .as_ref()
                .is_some_and(|id| !ids.contains(id))
            {
                return Err(invalid("Diagnostic refers to an unknown node"));
            }
        }
        let mut encoded = EncodedBudget {
            remaining: limits.output_bytes,
        };
        serde_json::to_writer(&mut encoded, self).map_err(|_| Error::Limit)?;
        Ok(())
    }
}

struct EncodedBudget {
    remaining: usize,
}
impl std::io::Write for EncodedBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("Output byte limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Budget {
    strings: usize,
    nodes: usize,
    limits: Limits,
}
impl Budget {
    fn text(&mut self, text: &str, max: usize) -> Result<(), Error> {
        self.strings = self.strings.checked_add(text.len()).ok_or(Error::Limit)?;
        if text.len() > max || self.strings > self.limits.output_bytes {
            return Err(Error::Limit);
        }
        Ok(())
    }
    fn node(
        &mut self,
        node: &Node,
        depth: usize,
        sources: &BTreeMap<String, u64>,
        ids: &mut BTreeSet<String>,
    ) -> Result<(), Error> {
        self.nodes += 1;
        if self.nodes > self.limits.nodes || depth > MAX_TREE_DEPTH {
            return Err(Error::Limit);
        }
        if !valid_id(&node.id) || !ids.insert(node.id.clone()) || node.spans.len() > 64 {
            return Err(invalid("Duplicate or invalid output node"));
        }
        self.text(&node.name, 512)?;
        if let Some(token) = &node.children_token {
            if !valid_id(token) {
                return Err(invalid("Invalid expansion token"));
            }
            self.text(token, 128)?;
        }
        if let Some(value) = &node.value {
            match value {
                Value::String(s) => self.text(s, 64 * 1024)?,
                Value::Integer(s) => integer(s)?,
                Value::Float(n) if !n.is_finite() => {
                    return Err(invalid("Nonfinite numeric output"));
                }
                Value::Enum { name, raw } => {
                    self.text(name, 256)?;
                    integer(raw)?;
                }
                Value::Flags { names, raw } => {
                    if names.len() > 64 {
                        return Err(Error::Limit);
                    }
                    for name in names {
                        self.text(name, 256)?;
                    }
                    integer(raw)?;
                }
                Value::Timestamp(s) | Value::Guid(s) => self.text(s, 128)?,
                _ => {}
            }
        }
        if let Some(format) = &node.format {
            for s in [
                &format.protocol,
                &format.charset,
                &format.byte_order,
                &format.encoding,
                &format.units,
            ]
            .into_iter()
            .flatten()
            {
                self.text(s, 256)?;
            }
            if format.radix.is_some_and(|r| ![2, 10, 16].contains(&r)) {
                return Err(invalid("Unsupported numeric radix"));
            }
        }
        for span in &node.spans {
            check_span(span, sources)?;
        }
        for child in &node.children {
            self.node(child, depth + 1, sources, ids)?;
        }
        Ok(())
    }
}
fn integer(value: &str) -> Result<(), Error> {
    if value.len() > 40
        || value.parse::<i128>().is_err()
        || value
            .parse::<i128>()
            .ok()
            .is_none_or(|n| n.to_string() != value)
    {
        return Err(invalid("Invalid exact integer output"));
    }
    Ok(())
}
fn check_span(span: &Span, sources: &BTreeMap<String, u64>) -> Result<(), Error> {
    let limit = sources.get(&span.source_id).ok_or(Error::AccessDenied)?;
    if decimal(&span.offset)?
        .checked_add(decimal(&span.length)?)
        .is_none_or(|end| end > *limit)
    {
        return Err(invalid("Output span exceeds its source"));
    }
    Ok(())
}

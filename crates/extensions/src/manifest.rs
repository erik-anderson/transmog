use crate::{Error, invalid, valid_id};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Current tmext container/manifest revision.
pub const PACKAGE_FORMAT_VERSION: u32 = 1;
/// Current visualizer API revision; independent of worker IPC.
pub const VISUALIZER_API_VERSION: u32 = 1;
/// Executable module contract, independent of authors' source language/toolchain.
pub const JAVASCRIPT_RUNTIME: &str = "javascript-es2022";
/// Maximum manifest JSON bytes read during discovery.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
/// Maximum exact source/data bytes in one extension revision.
pub const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum declared payload files.
pub const MAX_PACKAGE_FILES: usize = 2048;

/// Request or response input side, independent of display surface.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// Request headers/body.
    Request,
    /// Response headers/body.
    Response,
}

/// Host-owned surface in which a contribution can be selected.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "camelCase")]
pub enum Surface {
    /// Header interpretation.
    Headers,
    /// Body interpretation.
    Body,
}

/// Explicit access to a message's retained body.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum BodyAccess {
    /// No body handle, read or decode.
    None,
    /// Bounded lazy reads when evidence is available.
    Optional,
    /// Body required for visualization, never for capture.
    Required,
}

/// Mandatory exact input declaration for one message side.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputAccess {
    /// Exact case-insensitive header names; empty means no headers.
    pub headers: Vec<String>,
    /// No default: authors must explicitly declare body access.
    pub body: BodyAccess,
    /// Optional on-demand HTTP content decoding; requires body and content-encoding access.
    #[serde(default, skip_serializing_if = "is_false")]
    pub decode_http_content: bool,
}
impl InputAccess {
    /// Whether this exact header name was declared.
    pub fn permits_header(&self, name: &str) -> bool {
        self.headers.iter().any(|h| h.eq_ignore_ascii_case(name))
    }
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.decode_http_content
            && (self.body == BodyAccess::None || !self.permits_header("content-encoding"))
        {
            return Err(invalid(
                "HTTP decoding requires body and content-encoding access",
            ));
        }
        if self.headers.len() > 64 {
            return Err(Error::Limit);
        }
        let mut names = BTreeSet::new();
        for name in &self.headers {
            if !valid_header_name(name) || !names.insert(name.to_ascii_lowercase()) {
                return Err(invalid(
                    "Declare unique exact HTTP header names without wildcards",
                ));
            }
        }
        Ok(())
    }
}

/// Mandatory independent request and response grants.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inputs {
    /// Exact request-side grant.
    pub request: InputAccess,
    /// Exact response-side grant.
    pub response: InputAccess,
}
impl Inputs {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        self.request.validate()?;
        self.response.validate()
    }
    /// Access declaration for the specified side.
    pub fn side(&self, side: Direction) -> &InputAccess {
        match side {
            Direction::Request => &self.request,
            Direction::Response => &self.response,
        }
    }
}

/// Native header predicate; it never calls extension code.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum HeaderTest {
    /// The named field exists, including explicitly redacted occurrences.
    Present,
    /// At least one nonredacted occurrence equals the exact value bytes.
    Equals {
        /// Exact ASCII comparison value.
        value: String,
    },
    /// The field exists and every occurrence is visible and differs from this value.
    NotEquals {
        /// Exact ASCII comparison value.
        value: String,
    },
}

/// Explicit header dependency for display, live admission or context correlation.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeaderPredicate {
    /// Side on which the named header is read.
    pub direction: Direction,
    /// An exact declared field name.
    pub name: String,
    /// Bounded native comparison.
    pub test: HeaderTest,
}

/// One conjunction of bounded native matching conditions; rules combine with OR.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchRule {
    /// Message side which the contribution displays or consumes.
    pub direction: Direction,
    /// Exact media types, without parameters. Requires content-type access on this side.
    #[serde(default)]
    pub content_types: Vec<String>,
    /// Optional exact request methods.
    #[serde(default)]
    pub methods: Vec<String>,
    /// Optional endpoint path prefixes, scoped further by the active conversation.
    #[serde(default)]
    pub path_prefixes: Vec<String>,
    /// Optional exact observed response statuses.
    #[serde(default)]
    pub statuses: Vec<u16>,
    /// All predicates must pass; every dependency must be declared.
    #[serde(default)]
    pub headers: Vec<HeaderPredicate>,
}

/// Optional immutable metadata declaration for opt-in live feeding.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveUpdates {
    /// Request/response lifecycle interests.
    pub events: Vec<Direction>,
    /// Whether bounded coalesced body-growth events are useful.
    pub body_progress: bool,
}

/// Bounded correlation fields used only within the selected capture and native origin/caller scope.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Context {
    /// Exact declared headers required to identify a conversation.
    pub headers: Vec<HeaderPredicate>,
    /// Maximum related exchanges delivered across this instance's context reads.
    pub max_exchanges: usize,
    /// Permit bounded reads of later retained exchanges, needed by some fragmented protocols.
    #[serde(default, skip_serializing_if = "is_false")]
    pub look_ahead: bool,
}
#[allow(clippy::trivially_copy_pass_by_ref)] // Serde's skip predicate receives a field reference.
fn is_false(value: &bool) -> bool {
    !value
}

/// Host-clamped per-instance processing budgets.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    /// Maximum retained bytes read per operation, across both sides and continuations.
    pub input_bytes: usize,
    /// Maximum serialized output bytes.
    pub output_bytes: usize,
    /// Maximum V8 heap allocation.
    pub heap_bytes: usize,
    /// Maximum active invocation milliseconds.
    pub duration_ms: u64,
    /// Maximum rendered nodes per document.
    pub nodes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: 16 * 1024 * 1024,
            output_bytes: 4 * 1024 * 1024,
            heap_bytes: 128 * 1024 * 1024,
            duration_ms: 2_000,
            nodes: 20_000,
        }
    }
}
impl Limits {
    /// Checks requests against finite host ceilings.
    /// # Errors
    /// Rejects zero or excessive resource requests.
    pub fn validate(&self) -> Result<(), Error> {
        if self.input_bytes == 0
            || self.input_bytes > 64 * 1024 * 1024
            || self.output_bytes == 0
            || self.output_bytes > 8 * 1024 * 1024
            || !(16 * 1024 * 1024..=256 * 1024 * 1024).contains(&self.heap_bytes)
            || !(1..=10_000).contains(&self.duration_ms)
            || !(1..=50_000).contains(&self.nodes)
        {
            return Err(Error::Limit);
        }
        Ok(())
    }
}

/// Independently discoverable contribution within an immutable package revision.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Visualizer {
    /// Stable contribution ID within its package.
    pub id: String,
    /// Human-readable selector label.
    pub name: String,
    /// Package-relative JavaScript ES module (.js or .mjs); dependencies remain local.
    pub entry: String,
    /// Host surfaces where it is offered.
    pub surfaces: Vec<Surface>,
    /// Mandatory exact request and response access declarations.
    pub inputs: Inputs,
    /// Display matching rules.
    pub matches: Vec<MatchRule>,
    /// Allow the visible Auto viewer to attempt this matching contribution.
    /// Only a complete parse may replace the built-in representation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub automatic: bool,
    /// Optional narrower/different declared context/live input matches.
    #[serde(default)]
    pub input_matches: Vec<MatchRule>,
    /// Opt-in live capability; absence means unsupported.
    pub live_updates: Option<LiveUpdates>,
    /// Optional related retained-traffic capability.
    pub context: Option<Context>,
    /// Requested host-clamped budgets.
    #[serde(default)]
    pub limits: Limits,
}
impl Visualizer {
    /// Validates all mandatory declarations and matching dependencies.
    /// # Errors
    /// Rejects invalid declarations, undeclared dependencies and excessive predicates.
    pub fn validate(&self) -> Result<(), Error> {
        if !valid_id(&self.id)
            || !label(&self.name)
            || !std::path::Path::new(&self.entry)
                .extension()
                .is_some_and(|ext| ext == "js" || ext == "mjs")
            || self.surfaces.is_empty()
            || self.surfaces.len() > 2
            || self.surfaces.iter().collect::<BTreeSet<_>>().len() != self.surfaces.len()
        {
            return Err(invalid("Invalid visualizer identity, entry or surfaces"));
        }
        transmog_package_zip::name(self.entry.as_bytes())?;
        self.inputs.validate()?;
        self.limits.validate()?;
        if self.matches.is_empty() || self.matches.len() > 32 || self.input_matches.len() > 32 {
            return Err(invalid("Declare finite selective matching rules"));
        }
        for rule in self.matches.iter().chain(&self.input_matches) {
            validate_rule(&self.inputs, rule)?;
        }
        if let Some(live) = &self.live_updates
            && (live.events.is_empty()
                || live.events.len() > 2
                || live.events.iter().collect::<BTreeSet<_>>().len() != live.events.len()
                || (live.body_progress
                    && live
                        .events
                        .iter()
                        .any(|d| self.inputs.side(*d).body == BodyAccess::None)))
        {
            return Err(invalid("Invalid live update interests"));
        }
        if let Some(context) = &self.context {
            if context.headers.is_empty()
                || context.headers.len() > 8
                || !(1..=256).contains(&context.max_exchanges)
            {
                return Err(invalid("Declare bounded conversation correlation"));
            }
            for field in &context.headers {
                validate_header(&self.inputs, field)?;
            }
        }
        Ok(())
    }
}
fn validate_header(inputs: &Inputs, field: &HeaderPredicate) -> Result<(), Error> {
    if !inputs.side(field.direction).permits_header(&field.name) {
        return Err(Error::AccessDenied);
    }
    if let HeaderTest::Equals { value } | HeaderTest::NotEquals { value } = &field.test
        && (value.len() > 1024 || !value.is_ascii() || value.bytes().any(|b| b.is_ascii_control()))
    {
        return Err(invalid("Invalid header comparison value"));
    }
    Ok(())
}
pub(crate) fn validate_rule(inputs: &Inputs, rule: &MatchRule) -> Result<(), Error> {
    if rule.content_types.len() > 16
        || rule.headers.len() > 16
        || rule.methods.len() > 16
        || rule.statuses.len() > 16
        || rule.path_prefixes.len() > 8
        || (rule.content_types.is_empty()
            && rule.headers.is_empty()
            && rule.path_prefixes.is_empty())
    {
        return Err(invalid("Matching rules must be selective and bounded"));
    }
    if !rule.content_types.is_empty() && !inputs.side(rule.direction).permits_header("content-type")
    {
        return Err(Error::AccessDenied);
    }
    if rule.content_types.iter().any(|t| !media_type(t))
        || rule.methods.iter().any(|m| !valid_header_name(m))
        || rule
            .path_prefixes
            .iter()
            .any(|p| !p.starts_with('/') || p.len() > 1024 || p.chars().any(char::is_control))
        || rule.statuses.iter().any(|s| !(100..=599).contains(s))
    {
        return Err(invalid("Invalid native matching predicate"));
    }
    for field in &rule.headers {
        validate_header(inputs, field)?;
    }
    Ok(())
}

/// Exact payload inventory; only immutable verified bytes enter a worker.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Payload {
    /// Portable package-relative file path.
    pub path: String,
    /// Exact bounded bytes, not an allocation request.
    pub bytes: u64,
    /// Lowercase SHA-256; omitted only in registered development directories.
    pub sha256: Option<String>,
}

/// Inert metadata discovered without compiling or launching extension code.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Container/metadata format version.
    pub format_version: u32,
    /// Visualizer contract revision.
    pub api_version: u32,
    /// Stable JavaScript baseline; TypeScript is an optional authoring tool only.
    pub runtime: String,
    /// Stable extension identity.
    pub id: String,
    /// Extension release version, independently managed from the app.
    pub version: String,
    /// Display name in Extensions settings.
    pub name: String,
    /// Concurrent independent visualizer contributions.
    pub visualizers: Vec<Visualizer>,
    /// Optional header-only traffic columns, with no executable callbacks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traffic_columns: Vec<crate::ClassificationColumn>,
    /// Complete payload inventory, excluding manifest.json itself.
    pub files: Vec<Payload>,
}
impl Manifest {
    /// Decodes bounded metadata and validates it before registration.
    /// # Errors
    /// Rejects oversized, malformed, unsupported or invalid manifests.
    pub fn parse(bytes: &[u8], development: bool) -> Result<Self, Error> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Limit);
        }
        let value: Self = serde_json::from_slice(bytes)?;
        value.validate(development)?;
        Ok(value)
    }
    /// Validates identity, versions, inventories and every independent grant.
    /// # Errors
    /// Rejects malformed, unsupported or excessive package declarations.
    pub fn validate(&self, development: bool) -> Result<(), Error> {
        if self.format_version != PACKAGE_FORMAT_VERSION
            || self.api_version != VISUALIZER_API_VERSION
            || self.runtime != JAVASCRIPT_RUNTIME
        {
            return Err(Error::Unsupported);
        }
        if !valid_id(&self.id)
            || !label(&self.name)
            || self.version.is_empty()
            || self.version.len() > 64
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
            || (self.visualizers.is_empty() && self.traffic_columns.is_empty())
            || self.visualizers.len() > 32
            || self.traffic_columns.len() > 16
            || self.files.len() > MAX_PACKAGE_FILES
        {
            return Err(invalid("Invalid extension identity or inventory"));
        }
        let mut files = BTreeMap::new();
        let mut total = 0_u64;
        for file in &self.files {
            transmog_package_zip::name(file.path.as_bytes())?;
            if file.path.eq_ignore_ascii_case("manifest.json")
                || files.insert(file.path.to_ascii_lowercase(), file).is_some()
                || file.bytes > 16 * 1024 * 1024
                || file.sha256.as_ref().is_some_and(|d| !digest(d))
                || (!development && file.sha256.is_none())
            {
                return Err(invalid("Invalid payload inventory"));
            }
            total = total.checked_add(file.bytes).ok_or(Error::Limit)?;
        }
        if total > MAX_PACKAGE_BYTES {
            return Err(Error::Limit);
        }
        let mut ids = BTreeSet::new();
        for visualizer in &self.visualizers {
            visualizer.validate()?;
            if !ids.insert(&visualizer.id) || !self.files.iter().any(|f| f.path == visualizer.entry)
            {
                return Err(invalid("Duplicate visualizer or missing entry module"));
            }
        }
        let mut columns = BTreeSet::new();
        for column in &self.traffic_columns {
            column.validate()?;
            if !columns.insert(&column.id) {
                return Err(invalid("Duplicate traffic column identity"));
            }
        }
        Ok(())
    }
}
pub(crate) fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 160 && !value.chars().any(char::is_control)
}
pub(crate) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn valid_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'+-.^_`|~".contains(&b))
}
fn media_type(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(a, b)| valid_header_name(a) && valid_header_name(b))
}

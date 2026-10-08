#![deny(missing_docs)]

//! Optional reusable automation compiled into ordinary Hooks v2 interceptors.
//!
//! Embedders may ignore this crate and install custom hooks directly. Every
//! compiled rule phase receives a stable interceptor ID so the core audit trail
//! attributes its effects without giving automation special privileges.

use std::{collections::BTreeSet, sync::Arc};

use bytes::Bytes;
use regex::bytes::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use transmog_core::{
    CanonicalResponse, HeaderError, HeaderField, LocalStreamingResponse, RequestHead, ResponseHead,
    intercept::{
        BodyPlan, BoxHookFuture, BufferedBody, ExchangeInterceptor, ExchangeMetadata,
        HookInitError, InterceptorFactory, InterceptorRegistration, InterceptorRequirement,
        RequestBodyAction, RequestBodyEvent, RequestHeadAction, RequestHeadEvent,
        ResponseBodyAction, ResponseBodyEvent, ResponseHeadAction, ResponseHeadEvent,
    },
};

mod matching;
use matching::CompiledUrl;
pub use matching::{
    ExampleHeader, MatchCapture, MatchCheck, MatchExample, MatchTest, QueryCondition,
    QueryParameter, RegexScope, UrlCondition, UrlPattern, UrlRegex, matcher_key, request_for_test,
    same_matching_behavior, test_matcher,
};

/// Finite compile-time automation bounds.
#[derive(Clone, Copy, Debug)]
pub struct AutomationLimits {
    /// Maximum rules in one compilation unit.
    pub max_rules: usize,
    /// Maximum header operations in either direction for one rule.
    pub max_header_operations_per_rule: usize,
    /// Maximum immediate replacement body.
    pub max_replacement_body_bytes: usize,
    /// Maximum header predicates evaluated by one rule.
    pub max_header_predicates_per_rule: usize,
    /// Maximum UTF-8 bytes in one regular expression.
    pub max_regex_bytes: usize,
}

impl Default for AutomationLimits {
    fn default() -> Self {
        Self {
            max_rules: 1_024,
            max_header_operations_per_rule: 256,
            max_replacement_body_bytes: 8 * 1024 * 1024,
            max_header_predicates_per_rule: 64,
            max_regex_bytes: 4 * 1024,
        }
    }
}

/// One bounded condition applied to every value of a named header.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeaderPredicate {
    /// Case-insensitive HTTP field name.
    pub name: String,
    /// Required value condition.
    pub condition: HeaderCondition,
}

/// Safe header predicate operations supported by native rules.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum HeaderCondition {
    /// At least one field with the requested name exists.
    Exists,
    /// At least one field value is an exact byte match.
    Exact(Vec<u8>),
    /// At least one field value starts with these bytes.
    Prefix(Vec<u8>),
    /// At least one field value ends with these bytes.
    Suffix(Vec<u8>),
    /// At least one field value matches a bounded Rust byte regular expression.
    Regex(String),
}

/// Conservative bounded rule predicate.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleMatcher {
    /// Network-free regression examples; these do not affect matching.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<MatchExample>,
    /// Optional ASCII case-insensitive exact method.
    pub method: Option<String>,
    /// Optional complete normalized absolute-URL condition.
    #[serde(default)]
    pub url: Option<UrlCondition>,
    /// Optional ASCII case-insensitive exact URI scheme.
    pub scheme: Option<String>,
    /// Optional ASCII case-insensitive exact host.
    pub host: Option<String>,
    /// Optional exact destination port.
    pub port: Option<u16>,
    /// Optional case-sensitive path prefix.
    pub path_prefix: Option<String>,
    /// Optional exact query, excluding `?`.
    pub query: Option<String>,
    /// Request-header predicates, all of which must match.
    #[serde(default)]
    pub request_headers: Vec<HeaderPredicate>,
    /// Response-header predicates, all of which must match.
    #[serde(default)]
    pub response_headers: Vec<HeaderPredicate>,
    /// Optional exact response status. Ignored for request phases.
    pub response_status: Option<u16>,
    /// Optional response status class from 1 through 5.
    pub response_status_class: Option<u16>,
}

impl RuleMatcher {
    fn matches_request(
        &self,
        request: &RequestHead,
        regexes: &[Option<Regex>],
        url: Option<&CompiledUrl>,
    ) -> bool {
        self.method
            .as_ref()
            .is_none_or(|method| request.method.eq_ignore_ascii_case(method))
            && url.is_none_or(|url| url.matches(&request.target))
            && self
                .scheme
                .as_ref()
                .is_none_or(|scheme| request.target.scheme.eq_ignore_ascii_case(scheme))
            && self
                .host
                .as_ref()
                .is_none_or(|host| request.target.host.eq_ignore_ascii_case(host))
            && self.port.is_none_or(|port| request.target.port == port)
            && self
                .path_prefix
                .as_ref()
                .is_none_or(|prefix| request.target.path.starts_with(prefix))
            && self
                .query
                .as_ref()
                .is_none_or(|query| request.target.query.as_deref() == Some(query.as_str()))
            && self
                .request_headers
                .iter()
                .zip(regexes)
                .all(|(predicate, regex)| predicate.matches(&request.headers, regex.as_ref()))
    }

    fn matches_response(
        &self,
        request: &RequestHead,
        response: &ResponseHead,
        request_regexes: &[Option<Regex>],
        response_regexes: &[Option<Regex>],
        url: Option<&CompiledUrl>,
    ) -> bool {
        self.matches_request(request, request_regexes, url)
            && self
                .response_status
                .is_none_or(|status| response.status == status)
            && self
                .response_status_class
                .is_none_or(|class| response.status.checked_div(100) == Some(class))
            && self
                .response_headers
                .iter()
                .zip(response_regexes)
                .all(|(predicate, regex)| predicate.matches(&response.headers, regex.as_ref()))
    }

    fn overlaps(&self, other: &Self, response: bool) -> bool {
        optional_ascii_values_overlap(self.method.as_deref(), other.method.as_deref())
            && match (&self.url, &other.url) {
                (Some(UrlCondition::Exact(left)), Some(UrlCondition::Exact(right))) => {
                    left == right
                        || UrlCondition::Exact(left.clone()).matching_key().ok()
                            == UrlCondition::Exact(right.clone()).matching_key().ok()
                }
                _ => true,
            }
            && optional_ascii_values_overlap(self.scheme.as_deref(), other.scheme.as_deref())
            && optional_ascii_values_overlap(self.host.as_deref(), other.host.as_deref())
            && (self.port.is_none() || other.port.is_none() || self.port == other.port)
            && prefixes_overlap(self.path_prefix.as_deref(), other.path_prefix.as_deref())
            && (self.query.is_none() || other.query.is_none() || self.query == other.query)
            && (!response
                || self.response_status.is_none()
                || other.response_status.is_none()
                || self.response_status == other.response_status)
            && (!response
                || self.response_status_class.is_none()
                || other.response_status_class.is_none()
                || self.response_status_class == other.response_status_class)
    }
}

impl HeaderPredicate {
    fn matches(&self, headers: &transmog_core::HeaderBlock, regex: Option<&Regex>) -> bool {
        let mut values = headers.values(&self.name).peekable();
        match &self.condition {
            HeaderCondition::Exists => values.peek().is_some(),
            HeaderCondition::Exact(expected) => values.any(|value| value == expected),
            HeaderCondition::Prefix(expected) => values.any(|value| value.starts_with(expected)),
            HeaderCondition::Suffix(expected) => values.any(|value| value.ends_with(expected)),
            HeaderCondition::Regex(_) => {
                regex.is_some_and(|regex| values.any(|value| regex.is_match(value)))
            }
        }
    }
}

/// Validated deterministic header mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "field", rename_all = "kebab-case")]
pub enum HeaderOperation {
    /// Replace all occurrences while preserving the first position.
    Set(HeaderField),
    /// Append a field after all existing fields.
    Append(HeaderField),
    /// Remove all occurrences case-insensitively.
    Remove(String),
}

impl HeaderOperation {
    /// Creates a validated replacement operation.
    ///
    /// # Errors
    ///
    /// Returns [`HeaderError`] when name or value is invalid HTTP syntax.
    pub fn set(name: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Result<Self, HeaderError> {
        Ok(Self::Set(HeaderField::try_new(name, value)?))
    }

    /// Creates a validated removal operation.
    ///
    /// # Errors
    ///
    /// Returns [`HeaderError`] when the name is not a valid HTTP token.
    pub fn remove(name: impl Into<Vec<u8>>) -> Result<Self, HeaderError> {
        let field = HeaderField::try_new(name, Vec::new())?;
        Ok(Self::Remove(
            String::from_utf8_lossy(field.name()).into_owned(),
        ))
    }

    /// Creates a validated append operation.
    ///
    /// # Errors
    /// Returns [`HeaderError`] when name or value is invalid HTTP syntax.
    pub fn append(
        name: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<Self, HeaderError> {
        Ok(Self::Append(HeaderField::try_new(name, value)?))
    }

    fn target(&self) -> String {
        match self {
            Self::Set(field) | Self::Append(field) => {
                String::from_utf8_lossy(field.name()).to_ascii_lowercase()
            }
            Self::Remove(name) => name.to_ascii_lowercase(),
        }
    }
}

/// Request-side rule actions.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestActions {
    /// Ordered header operations.
    pub headers: Vec<HeaderOperation>,
    /// Optional decoded replacement body.
    pub replace_body: Option<Vec<u8>>,
    /// Discard the matching request body.
    #[serde(default)]
    pub discard_body: bool,
    /// Abort a matching request with this operator-safe reason.
    pub abort_reason: Option<String>,
    /// Stable saved/authored response asset selected for matching requests.
    pub response_asset: Option<String>,
    /// Permit replacement for methods not known to be idempotent.
    pub allow_non_idempotent_body_replacement: bool,
}

impl RequestActions {
    fn is_empty(&self) -> bool {
        self.headers.is_empty()
            && self.replace_body.is_none()
            && !self.discard_body
            && self.abort_reason.is_none()
            && self.response_asset.is_none()
    }
}

/// Response-side rule actions.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseActions {
    /// Ordered header operations.
    pub headers: Vec<HeaderOperation>,
    /// Optional decoded replacement body.
    pub replace_body: Option<Vec<u8>>,
    /// Discard the matching response body.
    #[serde(default)]
    pub discard_body: bool,
    /// Abort a matching response before downstream commitment.
    pub abort_reason: Option<String>,
}

impl ResponseActions {
    fn is_empty(&self) -> bool {
        self.headers.is_empty()
            && self.replace_body.is_none()
            && !self.discard_body
            && self.abort_reason.is_none()
    }
}

/// One declarative rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    /// Stable identifier used in interceptor audit IDs.
    pub id: String,
    /// Operator-facing name retained in per-exchange audit evidence.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Whether this rule participates in newly compiled exchange snapshots.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Monotonic revision included in hook audit identities.
    pub revision: u64,
    /// Higher values take precedence over lower values.
    pub priority: i32,
    /// Immutable request/response predicate.
    pub matcher: RuleMatcher,
    /// Request-side actions.
    pub request: RequestActions,
    /// Response-side actions.
    pub response: ResponseActions,
}

const fn default_enabled() -> bool {
    true
}

/// Successful deterministic compilation.
#[derive(Clone, Debug)]
pub struct CompiledAutomation {
    registrations: Vec<InterceptorRegistration>,
}

/// Resolved local response returned by a product-owned asset store.
#[derive(Debug)]
pub enum AutomationResponse {
    /// Small response represented by bounded in-memory frames.
    Buffered(CanonicalResponse),
    /// Large response represented by a backpressured stream.
    Streaming(LocalStreamingResponse),
}

/// Product-owned response asset resolver used by native automation.
pub trait ResponseAssetResolver: Send + Sync {
    /// Validates that an exact immutable asset revision is complete and servable.
    ///
    /// # Errors
    /// Returns a bounded reason when the reference cannot be activated.
    fn validate(&self, asset_id: &str) -> Result<(), String>;

    /// Opens a fresh response instance for one matching exchange.
    ///
    /// # Errors
    /// Returns a bounded, operator-safe reason when an active asset cannot be served.
    fn resolve(&self, asset_id: &str) -> Result<AutomationResponse, String>;
}

impl CompiledAutomation {
    /// Hook registrations to compose with application-owned registrations.
    pub fn registrations(&self) -> Vec<InterceptorRegistration> {
        self.registrations.clone()
    }
}

/// Compiles rules into identified request and response hook registrations.
///
/// Request registrations are ordered from low to high priority. Response
/// registrations are physically reversed so Hooks v2 reverse unwinding still
/// applies low priority first and high priority last.
///
/// # Errors
///
/// Returns a typed error for invalid limits, IDs, duplicate IDs, oversized
/// actions, or ambiguous equal-priority overlapping writes.
pub fn compile(
    rules: Vec<Rule>,
    limits: AutomationLimits,
) -> Result<CompiledAutomation, CompileError> {
    compile_with_assets(rules, limits, None)
}

/// Compiles rules with a product-owned response-asset resolver.
///
/// # Errors
/// Returns the same deterministic validation failures as [`compile`].
#[allow(clippy::needless_pass_by_value)]
pub fn compile_with_assets(
    mut rules: Vec<Rule>,
    limits: AutomationLimits,
    resolver: Option<Arc<dyn ResponseAssetResolver>>,
) -> Result<CompiledAutomation, CompileError> {
    validate_limits(limits)?;
    if rules.len() > limits.max_rules {
        return Err(CompileError::RuleLimitExceeded);
    }
    rules.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut ids = BTreeSet::new();
    for rule in &rules {
        validate_rule(rule, limits)?;
        if rule.enabled
            && let Some(asset_id) = &rule.request.response_asset
        {
            resolver
                .as_ref()
                .ok_or_else(|| CompileError::ResponseAssetUnavailable(rule.id.clone()))?
                .validate(asset_id)
                .map_err(|_| CompileError::ResponseAssetUnavailable(rule.id.clone()))?;
        }
        if !ids.insert(rule.id.clone()) {
            return Err(CompileError::DuplicateId(rule.id.clone()));
        }
    }
    reject_ambiguous_conflicts(&rules)?;

    let mut registrations = Vec::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.enabled && !rule.request.is_empty())
    {
        registrations.push(registration(rule, RuleDirection::Request, resolver.clone()));
    }
    for rule in rules
        .iter()
        .rev()
        .filter(|rule| rule.enabled && !rule.response.is_empty())
    {
        registrations.push(registration(
            rule,
            RuleDirection::Response,
            resolver.clone(),
        ));
    }
    Ok(CompiledAutomation { registrations })
}

fn validate_limits(limits: AutomationLimits) -> Result<(), CompileError> {
    if limits.max_rules == 0
        || limits.max_header_operations_per_rule == 0
        || limits.max_replacement_body_bytes == 0
        || limits.max_header_predicates_per_rule == 0
        || limits.max_regex_bytes == 0
    {
        return Err(CompileError::InvalidLimits);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_rule(rule: &Rule, limits: AutomationLimits) -> Result<(), CompileError> {
    if rule.id.is_empty()
        || rule.id.len() > 128
        || !rule
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CompileError::InvalidId(rule.id.clone()));
    }
    if rule.display_name.as_ref().is_some_and(|name| {
        name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control)
    }) {
        return Err(CompileError::InvalidId(rule.id.clone()));
    }
    if rule.revision == 0 {
        return Err(CompileError::InvalidRevision(rule.id.clone()));
    }
    for operations in [&rule.request.headers, &rule.response.headers] {
        if operations.len() > limits.max_header_operations_per_rule {
            return Err(CompileError::HeaderOperationLimitExceeded(rule.id.clone()));
        }
        for operation in operations {
            match operation {
                HeaderOperation::Set(field) | HeaderOperation::Append(field) => {
                    HeaderField::try_new(field.name().to_vec(), field.value().to_vec())
                        .map_err(|_| CompileError::InvalidHeaderOperation(rule.id.clone()))?;
                }
                HeaderOperation::Remove(name) => {
                    HeaderField::try_new(name.as_bytes(), Vec::new())
                        .map_err(|_| CompileError::InvalidHeaderOperation(rule.id.clone()))?;
                }
            }
        }
    }
    for body in [
        rule.request.replace_body.as_ref(),
        rule.response.replace_body.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if body.len() > limits.max_replacement_body_bytes {
            return Err(CompileError::BodyLimitExceeded(rule.id.clone()));
        }
    }
    if rule.request.replace_body.is_some() && rule.request.discard_body
        || rule.response.replace_body.is_some() && rule.response.discard_body
    {
        return Err(CompileError::ConflictingBodyActions(rule.id.clone()));
    }
    if let Some(asset) = &rule.request.response_asset {
        if asset.is_empty()
            || asset.len() > 128
            || !asset.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@')
            })
        {
            return Err(CompileError::InvalidResponseAsset(rule.id.clone()));
        }
        if !rule.request.headers.is_empty()
            || rule.request.replace_body.is_some()
            || rule.request.discard_body
            || rule.request.abort_reason.is_some()
        {
            return Err(CompileError::ConflictingResponseAsset(rule.id.clone()));
        }
    }
    for reason in [
        rule.request.abort_reason.as_deref(),
        rule.response.abort_reason.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if reason.is_empty() || reason.len() > 256 || reason.chars().any(char::is_control) {
            return Err(CompileError::InvalidAbortReason(rule.id.clone()));
        }
    }
    if rule
        .matcher
        .path_prefix
        .as_ref()
        .is_some_and(|prefix| !prefix.starts_with('/'))
    {
        return Err(CompileError::InvalidPathPrefix(rule.id.clone()));
    }
    if rule
        .matcher
        .url
        .as_ref()
        .is_some_and(|condition| condition.compile().is_err())
    {
        return Err(CompileError::InvalidMatcher(rule.id.clone()));
    }
    if rule.matcher.url.as_ref().is_some_and(|condition|matches!(condition,UrlCondition::Regex(pattern) if pattern.pattern.len()>limits.max_regex_bytes)) {return Err(CompileError::InvalidRegex(rule.id.clone()));}
    if rule.matcher.examples.len() > 32
        || rule.matcher.examples.iter().any(|example| {
            matching::request_for_test(
                &example.method,
                &example.url,
                &example
                    .headers
                    .iter()
                    .map(|header| (header.name.clone(), header.value.clone()))
                    .collect::<Vec<_>>(),
            )
            .is_err()
        })
    {
        return Err(CompileError::InvalidMatcher(rule.id.clone()));
    }
    if rule.matcher.method.as_ref().is_some_and(|value| {
        value.is_empty()
            || value.len() > 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    }) || rule
        .matcher
        .scheme
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > 32 || !value.is_ascii())
        || rule
            .matcher
            .host
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 255 || !value.is_ascii())
        || rule
            .matcher
            .path_prefix
            .as_ref()
            .is_some_and(|value| value.len() > 2_048)
        || rule
            .matcher
            .query
            .as_ref()
            .is_some_and(|value| value.len() > 4_096)
    {
        return Err(CompileError::InvalidMatcher(rule.id.clone()));
    }
    if rule
        .matcher
        .response_status
        .is_some_and(|status| !(100..=599).contains(&status))
        || rule
            .matcher
            .response_status_class
            .is_some_and(|class| !(1..=5).contains(&class))
    {
        return Err(CompileError::InvalidResponseStatus(rule.id.clone()));
    }
    let predicates = rule
        .matcher
        .request_headers
        .iter()
        .chain(&rule.matcher.response_headers)
        .collect::<Vec<_>>();
    if predicates.len() > limits.max_header_predicates_per_rule {
        return Err(CompileError::HeaderPredicateLimitExceeded(rule.id.clone()));
    }
    for predicate in predicates {
        HeaderField::try_new(predicate.name.as_bytes(), Vec::new())
            .map_err(|_| CompileError::InvalidHeaderPredicate(rule.id.clone()))?;
        if let HeaderCondition::Regex(pattern) = &predicate.condition
            && (pattern.len() > limits.max_regex_bytes || compile_regex(pattern).is_err())
        {
            return Err(CompileError::InvalidRegex(rule.id.clone()));
        }
    }
    Ok(())
}

fn compile_regex(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
}

fn reject_ambiguous_conflicts(rules: &[Rule]) -> Result<(), CompileError> {
    let enabled = rules.iter().filter(|rule| rule.enabled).collect::<Vec<_>>();
    for (index, left) in enabled.iter().enumerate() {
        for right in &enabled[index + 1..] {
            if left.priority != right.priority {
                continue;
            }
            for (response, left_targets, right_targets) in [
                (
                    false,
                    action_targets(
                        &left.request.headers,
                        left.request.replace_body.is_some() || left.request.discard_body,
                        left.request.abort_reason.is_some(),
                        left.request.response_asset.is_some(),
                    ),
                    action_targets(
                        &right.request.headers,
                        right.request.replace_body.is_some() || right.request.discard_body,
                        right.request.abort_reason.is_some(),
                        right.request.response_asset.is_some(),
                    ),
                ),
                (
                    true,
                    action_targets(
                        &left.response.headers,
                        left.response.replace_body.is_some() || left.response.discard_body,
                        left.response.abort_reason.is_some(),
                        false,
                    ),
                    action_targets(
                        &right.response.headers,
                        right.response.replace_body.is_some() || right.response.discard_body,
                        right.response.abort_reason.is_some(),
                        false,
                    ),
                ),
            ] {
                if left.matcher.overlaps(&right.matcher, response)
                    && !left_targets.is_disjoint(&right_targets)
                {
                    return Err(CompileError::AmbiguousConflict {
                        first: left.id.clone(),
                        second: right.id.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn action_targets(
    headers: &[HeaderOperation],
    body: bool,
    abort: bool,
    response_asset: bool,
) -> BTreeSet<String> {
    let mut targets = headers
        .iter()
        .map(HeaderOperation::target)
        .collect::<BTreeSet<_>>();
    if body {
        targets.insert("$body".to_owned());
    }
    if abort {
        targets.insert("$abort".to_owned());
    }
    if response_asset {
        targets.insert("$respond".to_owned());
    }
    targets
}

fn optional_ascii_values_overlap(left: Option<&str>, right: Option<&str>) -> bool {
    left.zip(right)
        .is_none_or(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn prefixes_overlap(left: Option<&str>, right: Option<&str>) -> bool {
    left.zip(right)
        .is_none_or(|(left, right)| left.starts_with(right) || right.starts_with(left))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuleDirection {
    Request,
    Response,
}

fn registration(
    rule: &Rule,
    direction: RuleDirection,
    resolver: Option<Arc<dyn ResponseAssetResolver>>,
) -> InterceptorRegistration {
    let suffix = match direction {
        RuleDirection::Request => "request",
        RuleDirection::Response => "response",
    };
    let asset_suffix = rule
        .request
        .response_asset
        .as_ref()
        .map_or_else(String::new, |asset| format!("/asset/{asset}"));
    InterceptorRegistration::named(
        format!(
            "automation/{}@{}/{suffix}{asset_suffix}",
            rule.id, rule.revision
        ),
        rule.display_name
            .clone()
            .unwrap_or_else(|| format!("Automation rule {}", rule.id)),
        Arc::new(RuleFactory {
            rule: Arc::new(rule.clone()),
            url: rule
                .matcher
                .url
                .as_ref()
                .map(|condition| Arc::new(condition.compile().expect("validated URL condition"))),
            request_regexes: compile_predicates(&rule.matcher.request_headers),
            response_regexes: compile_predicates(&rule.matcher.response_headers),
            resolver,
            direction,
        }),
        InterceptorRequirement::Required,
    )
}

fn compile_predicates(predicates: &[HeaderPredicate]) -> Arc<[Option<Regex>]> {
    predicates
        .iter()
        .map(|predicate| match &predicate.condition {
            HeaderCondition::Regex(pattern) => {
                Some(compile_regex(pattern).expect("validated automation regular expression"))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .into()
}

struct RuleFactory {
    rule: Arc<Rule>,
    url: Option<Arc<CompiledUrl>>,
    request_regexes: Arc<[Option<Regex>]>,
    response_regexes: Arc<[Option<Regex>]>,
    resolver: Option<Arc<dyn ResponseAssetResolver>>,
    direction: RuleDirection,
}

impl InterceptorFactory for RuleFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(RuleInterceptor {
            rule: Arc::clone(&self.rule),
            url: self.url.clone(),
            request_regexes: Arc::clone(&self.request_regexes),
            response_regexes: Arc::clone(&self.response_regexes),
            resolver: self.resolver.clone(),
            direction: self.direction,
        }))
    }
}

struct RuleInterceptor {
    rule: Arc<Rule>,
    url: Option<Arc<CompiledUrl>>,
    request_regexes: Arc<[Option<Regex>]>,
    response_regexes: Arc<[Option<Regex>]>,
    resolver: Option<Arc<dyn ResponseAssetResolver>>,
    direction: RuleDirection,
}

impl ExchangeInterceptor for RuleInterceptor {
    fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        if self.direction != RuleDirection::Request
            || !self.rule.matcher.matches_request(
                &event.head,
                &self.request_regexes,
                self.url.as_deref(),
            )
        {
            return Box::pin(async { RequestHeadAction::Continue });
        }
        if let Some(reason) = &self.rule.request.abort_reason {
            let reason = reason.clone();
            return Box::pin(async move {
                RequestHeadAction::Abort(transmog_core::intercept::HookAbort::Policy(reason))
            });
        }
        if let Some(asset_id) = &self.rule.request.response_asset {
            let response = self
                .resolver
                .as_ref()
                .ok_or_else(|| "response asset resolver is unavailable".to_owned())
                .and_then(|resolver| resolver.resolve(asset_id));
            return Box::pin(async move {
                match response {
                    Ok(AutomationResponse::Buffered(response)) => {
                        RequestHeadAction::Respond(response)
                    }
                    Ok(AutomationResponse::Streaming(response)) => {
                        RequestHeadAction::RespondStreaming(response)
                    }
                    Err(reason) => RequestHeadAction::Abort(
                        transmog_core::intercept::HookAbort::Policy(reason),
                    ),
                }
            });
        }
        let mut head = event.head;
        apply_headers(&mut head.headers, &self.rule.request.headers);
        if self.rule.request.headers.is_empty() {
            Box::pin(async { RequestHeadAction::Continue })
        } else {
            Box::pin(async move { RequestHeadAction::Replace(head) })
        }
    }

    fn on_request_body(&self, event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        let selected = (self.direction == RuleDirection::Request
            && self.rule.matcher.matches_request(
                &event.head,
                &self.request_regexes,
                self.url.as_deref(),
            )
            && (self.rule.request.allow_non_idempotent_body_replacement
                || method_is_idempotent(&event.head.method)))
        .then(|| {
            self.rule.request.replace_body.clone().map_or_else(
                || self.rule.request.discard_body.then_some(None),
                |body| Some(Some(body)),
            )
        })
        .flatten();
        Box::pin(async move {
            selected.map_or_else(RequestBodyAction::pass_through, |replacement| {
                replacement.map_or_else(
                    || RequestBodyAction::decoded(BodyPlan::Discard),
                    |bytes| {
                        RequestBodyAction::decoded(BodyPlan::Replace(
                            BufferedBody::try_new(bytes.len(), Bytes::from(bytes), None)
                                .expect("compiled replacement satisfies its exact bound"),
                        ))
                    },
                )
            })
        })
    }

    fn on_response_head(&self, event: ResponseHeadEvent) -> BoxHookFuture<'_, ResponseHeadAction> {
        if self.direction != RuleDirection::Response
            || !self.rule.matcher.matches_response(
                &event.request_head,
                &event.head,
                &self.request_regexes,
                &self.response_regexes,
                self.url.as_deref(),
            )
        {
            return Box::pin(async { ResponseHeadAction::Continue });
        }
        if let Some(reason) = &self.rule.response.abort_reason {
            let reason = reason.clone();
            return Box::pin(async move {
                ResponseHeadAction::Abort(transmog_core::intercept::HookAbort::Policy(reason))
            });
        }
        let mut head = event.head;
        apply_headers(&mut head.headers, &self.rule.response.headers);
        if self.rule.response.headers.is_empty() {
            Box::pin(async { ResponseHeadAction::Continue })
        } else {
            Box::pin(async move { ResponseHeadAction::Replace(head) })
        }
    }

    fn on_response_body(&self, event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        let selected = (self.direction == RuleDirection::Response
            && self.rule.matcher.matches_response(
                &event.request_head,
                &event.response_head,
                &self.request_regexes,
                &self.response_regexes,
                self.url.as_deref(),
            ))
        .then(|| {
            self.rule.response.replace_body.clone().map_or_else(
                || self.rule.response.discard_body.then_some(None),
                |body| Some(Some(body)),
            )
        })
        .flatten();
        Box::pin(async move {
            selected.map_or_else(ResponseBodyAction::pass_through, |replacement| {
                replacement.map_or_else(
                    || ResponseBodyAction::decoded(BodyPlan::Discard),
                    |bytes| {
                        ResponseBodyAction::decoded(BodyPlan::Replace(
                            BufferedBody::try_new(bytes.len(), Bytes::from(bytes), None)
                                .expect("compiled replacement satisfies its exact bound"),
                        ))
                    },
                )
            })
        })
    }
}

fn apply_headers(headers: &mut transmog_core::HeaderBlock, actions: &[HeaderOperation]) {
    for action in actions {
        match action {
            HeaderOperation::Set(field) => headers.replace_all(field.clone()),
            HeaderOperation::Append(field) => headers.push(field.clone()),
            HeaderOperation::Remove(name) => headers.remove_all(name),
        }
    }
}

fn method_is_idempotent(method: &str) -> bool {
    ["GET", "HEAD", "PUT", "DELETE", "OPTIONS", "TRACE"]
        .iter()
        .any(|candidate| method.eq_ignore_ascii_case(candidate))
}

/// Automation compilation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CompileError {
    /// One or more resource limits were zero.
    #[error("automation limits are invalid")]
    InvalidLimits,
    /// Rule count exceeded its configured bound.
    #[error("automation rule limit exceeded")]
    RuleLimitExceeded,
    /// Rule identifier was empty or contained unsupported characters.
    #[error("automation rule id is invalid: {0}")]
    InvalidId(String),
    /// Rule revision was zero.
    #[error("automation rule revision is invalid: {0}")]
    InvalidRevision(String),
    /// Rule identifier appeared more than once.
    #[error("automation rule id is duplicated: {0}")]
    DuplicateId(String),
    /// Path prefix did not begin with `/`.
    #[error("automation rule has an invalid path prefix: {0}")]
    InvalidPathPrefix(String),
    /// Matcher values were empty, oversized, or syntactically invalid.
    #[error("automation rule has an invalid matcher: {0}")]
    InvalidMatcher(String),
    /// Response status or status class was invalid.
    #[error("automation rule has an invalid response status: {0}")]
    InvalidResponseStatus(String),
    /// Header-operation count exceeded its per-rule bound.
    #[error("automation rule exceeds its header-operation limit: {0}")]
    HeaderOperationLimitExceeded(String),
    /// Persisted header operation bypassed HTTP field validation.
    #[error("automation rule has an invalid header operation: {0}")]
    InvalidHeaderOperation(String),
    /// Header-predicate count exceeded its per-rule bound.
    #[error("automation rule exceeds its header-predicate limit: {0}")]
    HeaderPredicateLimitExceeded(String),
    /// Header predicate used an invalid field name.
    #[error("automation rule has an invalid header predicate: {0}")]
    InvalidHeaderPredicate(String),
    /// Header predicate used an invalid or oversized regular expression.
    #[error("automation rule has an invalid regular expression: {0}")]
    InvalidRegex(String),
    /// Immediate replacement body exceeded its bound.
    #[error("automation rule exceeds its replacement-body limit: {0}")]
    BodyLimitExceeded(String),
    /// Body replacement and discard were both requested.
    #[error("automation rule has conflicting body actions: {0}")]
    ConflictingBodyActions(String),
    /// Abort reason was empty, oversized, or contained control characters.
    #[error("automation rule has an invalid abort reason: {0}")]
    InvalidAbortReason(String),
    /// Response asset identifier was invalid.
    #[error("automation rule has an invalid response asset: {0}")]
    InvalidResponseAsset(String),
    /// A response asset action was combined with incompatible request actions.
    #[error("automation rule has conflicting response asset actions: {0}")]
    ConflictingResponseAsset(String),
    /// Referenced immutable response asset was absent or incomplete.
    #[error("automation rule references an unavailable response asset: {0}")]
    ResponseAssetUnavailable(String),
    /// Equal-priority overlapping rules could mutate the same target.
    #[error("automation rules {first} and {second} have an ambiguous conflict")]
    AmbiguousConflict {
        /// First stable rule ID.
        first: String,
        /// Second stable rule ID.
        second: String,
    },
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use transmog_core::{
        ConnectionId, HeaderBlock, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{
            BodyPlanKind, HookEffectAction, HookLimits, HookPhase, InterceptorChainFactory,
            RequestHeadOutcome, ResponseHeadOutcome,
        },
    };

    use super::*;

    struct StaticAssetResolver;

    impl ResponseAssetResolver for StaticAssetResolver {
        fn validate(&self, asset_id: &str) -> Result<(), String> {
            (asset_id == "saved@3")
                .then_some(())
                .ok_or_else(|| "missing".to_owned())
        }

        fn resolve(&self, asset_id: &str) -> Result<AutomationResponse, String> {
            self.validate(asset_id)?;
            Ok(AutomationResponse::Buffered(CanonicalResponse::local(
                218,
                HeaderBlock::new(),
                Bytes::from_static(b"asset"),
            )))
        }
    }

    fn request(method: &str) -> RequestHead {
        RequestHead {
            method: method.to_owned(),
            target: Target {
                scheme: "https".to_owned(),
                authority: "example.test".to_owned(),
                host: "example.test".to_owned(),
                port: 443,
                path: "/api/items".to_owned(),
                query: None,
            },
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http2,
        }
    }

    fn metadata() -> ExchangeMetadata {
        ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse().unwrap(),
                client_identity: transmog_core::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2000".parse().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: None,
            },
            request("GET").target,
        )
    }

    fn rule(id: &str, priority: i32, value: &str) -> Rule {
        Rule {
            id: id.to_owned(),
            display_name: None,
            enabled: true,
            revision: 1,
            priority,
            matcher: RuleMatcher {
                host: Some("example.test".to_owned()),
                path_prefix: Some("/api".to_owned()),
                ..RuleMatcher::default()
            },
            request: RequestActions {
                headers: vec![HeaderOperation::set("x-priority", value).unwrap()],
                ..RequestActions::default()
            },
            response: ResponseActions {
                headers: vec![HeaderOperation::set("x-priority", value).unwrap()],
                ..ResponseActions::default()
            },
        }
    }

    #[tokio::test]
    async fn priority_is_consistent_and_effects_name_the_rule_phase() {
        let compiled = compile(
            vec![rule("high", 10, "high"), rule("low", 1, "low")],
            AutomationLimits::default(),
        )
        .unwrap();
        assert_eq!(
            compiled
                .registrations()
                .iter()
                .map(|registration| registration.id().as_str().to_owned())
                .collect::<Vec<_>>(),
            [
                "automation/low@1/request",
                "automation/high@1/request",
                "automation/high@1/response",
                "automation/low@1/response"
            ]
        );
        let factory = InterceptorChainFactory::new(
            compiled.registrations(),
            HookLimits {
                callback_timeout: Duration::from_secs(1),
                terminal_timeout: Duration::from_secs(1),
                max_paused_exchanges: std::num::NonZeroUsize::new(4).unwrap(),
            },
        );
        let mut chain = factory.create_exchange(metadata()).unwrap();
        let RequestHeadOutcome::Continue {
            head: request,
            reroute: None,
        } = chain.request_head(request("GET")).await.unwrap()
        else {
            panic!("expected continued request");
        };
        assert_eq!(
            request.headers.values("x-priority").next(),
            Some(&b"high"[..])
        );
        let ResponseHeadOutcome::Continue { head: response, .. } = chain
            .response_head(
                &request,
                ResponseHead {
                    status: 200,
                    headers: HeaderBlock::new(),
                    source_version: HttpLegVersion::Http2,
                },
                None,
                false,
            )
            .await
            .unwrap()
        else {
            panic!("expected continued response");
        };
        assert_eq!(
            response.headers.values("x-priority").next(),
            Some(&b"high"[..])
        );
        let effects = chain.hook_effects();
        assert_eq!(effects.len(), 4);
        assert_eq!(
            effects[0].interceptor.id.as_str(),
            "automation/low@1/request"
        );
        assert_eq!(
            effects[1].interceptor.id.as_str(),
            "automation/high@1/request"
        );
        assert_eq!(
            effects[2].interceptor.id.as_str(),
            "automation/low@1/response"
        );
        assert_eq!(
            effects[3].interceptor.id.as_str(),
            "automation/high@1/response"
        );
        assert!(effects.iter().all(|effect| matches!(
            effect.phase,
            HookPhase::RequestHead | HookPhase::ResponseHead
        )));
    }

    #[tokio::test]
    async fn request_body_replacement_defaults_to_idempotent_methods() {
        let body_rule = Rule {
            id: "body".to_owned(),
            display_name: None,
            enabled: true,
            revision: 1,
            priority: 1,
            matcher: RuleMatcher::default(),
            request: RequestActions {
                replace_body: Some(b"replacement".to_vec()),
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        let compiled = compile(vec![body_rule], AutomationLimits::default()).unwrap();
        let factory = InterceptorChainFactory::new(compiled.registrations(), HookLimits::default());

        let mut post_chain = factory.create_exchange(metadata()).unwrap();
        let RequestHeadOutcome::Continue { head: post, .. } =
            post_chain.request_head(request("POST")).await.unwrap()
        else {
            panic!("expected continued request");
        };
        let post_plans = post_chain.request_body_plans(&post).await.unwrap();
        assert!(matches!(post_plans[0].plan(), BodyPlan::PassThrough));

        let mut put_chain = factory.create_exchange(metadata()).unwrap();
        let RequestHeadOutcome::Continue { head: put, .. } =
            put_chain.request_head(request("PUT")).await.unwrap()
        else {
            panic!("expected continued request");
        };
        let put_plans = put_chain.request_body_plans(&put).await.unwrap();
        assert!(matches!(put_plans[0].plan(), BodyPlan::Replace(_)));
        let effects = put_chain.hook_effects();
        assert!(effects.iter().any(|effect| matches!(
            effect.action,
            HookEffectAction::BodyPlan {
                kind: BodyPlanKind::Replace,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn conditional_user_agent_rule_uses_bounded_header_regex_and_exact_revision_audit() {
        let mut request = request("GET");
        request.target.query = Some("debug=1".to_owned());
        request
            .headers
            .push(HeaderField::try_new("x-environment", "production").unwrap());
        let ua_rule = Rule {
            id: "conditional-ua".to_owned(),
            display_name: None,
            enabled: true,
            revision: 42,
            priority: 10,
            matcher: RuleMatcher {
                scheme: Some("https".to_owned()),
                host: Some("example.test".to_owned()),
                port: Some(443),
                path_prefix: Some("/api".to_owned()),
                query: Some("debug=1".to_owned()),
                request_headers: vec![HeaderPredicate {
                    name: "x-environment".to_owned(),
                    condition: HeaderCondition::Regex("^prod(?:uction)?$".to_owned()),
                }],
                ..RuleMatcher::default()
            },
            request: RequestActions {
                headers: vec![
                    HeaderOperation::set("user-agent", "Transmog-Test/42").unwrap(),
                    HeaderOperation::append("x-automation", "conditional-ua").unwrap(),
                ],
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        let compiled = compile(vec![ua_rule], AutomationLimits::default()).unwrap();
        let factory = InterceptorChainFactory::new(compiled.registrations(), HookLimits::default());
        let mut chain = factory.create_exchange(metadata()).unwrap();
        let RequestHeadOutcome::Continue { head, .. } = chain.request_head(request).await.unwrap()
        else {
            panic!("matching UA rule unexpectedly stopped the request");
        };
        assert_eq!(
            head.headers.values("user-agent").next(),
            Some(&b"Transmog-Test/42"[..])
        );
        assert_eq!(
            head.headers.values("x-automation").next(),
            Some(&b"conditional-ua"[..])
        );
        assert_eq!(
            chain.hook_effects()[0].interceptor.id.as_str(),
            "automation/conditional-ua@42/request"
        );
    }

    #[tokio::test]
    async fn autoresponse_uses_an_exact_validated_asset_revision_without_an_origin() {
        let asset_rule = Rule {
            id: "auto-response".to_owned(),
            display_name: Some("Saved response".to_owned()),
            enabled: true,
            revision: 9,
            priority: 0,
            matcher: RuleMatcher::default(),
            request: RequestActions {
                response_asset: Some("saved@3".to_owned()),
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        let compiled = compile_with_assets(
            vec![asset_rule],
            AutomationLimits::default(),
            Some(Arc::new(StaticAssetResolver)),
        )
        .unwrap();
        let factory = InterceptorChainFactory::new(compiled.registrations(), HookLimits::default());
        let mut chain = factory.create_exchange(metadata()).unwrap();
        let RequestHeadOutcome::Respond { response, .. } =
            chain.request_head(request("GET")).await.unwrap()
        else {
            panic!("expected exact saved response");
        };
        assert_eq!(response.head.status, 218);
        assert_eq!(
            chain.hook_effects()[0].interceptor.id.as_str(),
            "automation/auto-response@9/request/asset/saved@3"
        );
    }

    #[tokio::test]
    async fn ordered_autoresponses_use_exact_url_and_first_enabled_match_wins() {
        let response_rule = |id: &str, priority: i32, enabled: bool, url: Option<&str>| Rule {
            id: id.to_owned(),
            display_name: Some(id.to_owned()),
            enabled,
            revision: 1,
            priority,
            matcher: RuleMatcher {
                method: Some("GET".to_owned()),
                url: url.map(|value| UrlCondition::Exact(value.to_owned())),
                ..RuleMatcher::default()
            },
            request: RequestActions {
                response_asset: Some("saved@3".to_owned()),
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        let mut disabled = response_rule(
            "disabled-first",
            -30,
            false,
            Some("https://example.test/api/items?debug=1"),
        );
        disabled.request.response_asset = Some("not-installed@1".to_owned());
        let compiled = compile_with_assets(
            vec![
                disabled,
                response_rule(
                    "exact-second",
                    -20,
                    true,
                    Some("https://example.test/api/items?debug=1"),
                ),
                response_rule("catch-all-third", -10, true, None),
            ],
            AutomationLimits::default(),
            Some(Arc::new(StaticAssetResolver)),
        )
        .unwrap();
        let factory = InterceptorChainFactory::new(compiled.registrations(), HookLimits::default());
        let mut matching = request("GET");
        matching.target.query = Some("debug=1".to_owned());
        let mut chain = factory.create_exchange(metadata()).unwrap();
        assert!(matches!(
            chain.request_head(matching).await.unwrap(),
            RequestHeadOutcome::Respond { .. }
        ));
        assert_eq!(
            chain.hook_effects()[0].interceptor.id.as_str(),
            "automation/exact-second@1/request/asset/saved@3"
        );

        let mut different_query = request("GET");
        different_query.target.query = Some("debug=2".to_owned());
        let mut chain = factory.create_exchange(metadata()).unwrap();
        assert!(matches!(
            chain.request_head(different_query).await.unwrap(),
            RequestHeadOutcome::Respond { .. }
        ));
        assert_eq!(
            chain.hook_effects()[0].interceptor.id.as_str(),
            "automation/catch-all-third@1/request/asset/saved@3"
        );
    }

    #[tokio::test]
    async fn post_autoresponse_matches_original_request_headers_before_mutations() {
        let autoresponse = Rule {
            id: "post-response".to_owned(),
            display_name: Some("POST response".to_owned()),
            enabled: true,
            revision: 1,
            priority: -20,
            matcher: RuleMatcher {
                method: Some("POST".to_owned()),
                url: Some(UrlCondition::Exact(
                    "https://example.test/api/items".to_owned(),
                )),
                request_headers: vec![HeaderPredicate {
                    name: "x-mode".to_owned(),
                    condition: HeaderCondition::Exact(b"original".to_vec()),
                }],
                ..RuleMatcher::default()
            },
            request: RequestActions {
                response_asset: Some("saved@3".to_owned()),
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        let later_mutation = Rule {
            id: "later-mutation".to_owned(),
            display_name: Some("Later mutation".to_owned()),
            enabled: true,
            revision: 1,
            priority: -10,
            matcher: RuleMatcher::default(),
            request: RequestActions {
                headers: vec![HeaderOperation::set("x-mode", "modified").unwrap()],
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        let compiled = compile_with_assets(
            vec![later_mutation, autoresponse],
            AutomationLimits::default(),
            Some(Arc::new(StaticAssetResolver)),
        )
        .unwrap();
        let factory = InterceptorChainFactory::new(compiled.registrations(), HookLimits::default());
        let mut post = request("POST");
        post.headers
            .push(HeaderField::try_new("x-mode", "original").unwrap());
        let mut chain = factory.create_exchange(metadata()).unwrap();
        assert!(matches!(
            chain.request_head(post).await.unwrap(),
            RequestHeadOutcome::Respond { .. }
        ));
        assert_eq!(chain.hook_effects().len(), 1);
        assert_eq!(
            chain.hook_effects()[0].interceptor.id.as_str(),
            "automation/post-response@1/request/asset/saved@3"
        );

        let mut get = request("GET");
        get.headers
            .push(HeaderField::try_new("x-mode", "original").unwrap());
        let mut chain = factory.create_exchange(metadata()).unwrap();
        assert!(matches!(
            chain.request_head(get).await.unwrap(),
            RequestHeadOutcome::Continue { .. }
        ));
    }

    #[test]
    fn ambiguous_equal_priority_conflicts_are_rejected() {
        let error = compile(
            vec![rule("one", 1, "one"), rule("two", 1, "two")],
            AutomationLimits::default(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            CompileError::AmbiguousConflict {
                first: "one".to_owned(),
                second: "two".to_owned(),
            }
        );
    }

    #[test]
    fn disjoint_rules_and_resource_bounds_are_deterministic() {
        let mut first = rule("one", 1, "one");
        first.matcher.method = Some("GET".to_owned());
        let mut second = rule("two", 1, "two");
        second.matcher.method = Some("POST".to_owned());
        assert!(compile(vec![first, second], AutomationLimits::default()).is_ok());

        let mut relative_url = rule("relative", 2, "value");
        relative_url.matcher.url = Some(UrlCondition::Exact("/not-absolute".to_owned()));
        assert_eq!(
            compile(vec![relative_url], AutomationLimits::default()).unwrap_err(),
            CompileError::InvalidMatcher("relative".to_owned())
        );

        let oversized = Rule {
            id: "oversized".to_owned(),
            display_name: None,
            enabled: true,
            revision: 1,
            priority: 0,
            matcher: RuleMatcher::default(),
            request: RequestActions {
                replace_body: Some(vec![0; 2]),
                ..RequestActions::default()
            },
            response: ResponseActions::default(),
        };
        assert_eq!(
            compile(
                vec![oversized],
                AutomationLimits {
                    max_replacement_body_bytes: 1,
                    ..AutomationLimits::default()
                }
            )
            .unwrap_err(),
            CompileError::BodyLimitExceeded("oversized".to_owned())
        );
    }
}

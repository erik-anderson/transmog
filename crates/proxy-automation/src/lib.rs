#![deny(missing_docs)]

//! Optional reusable automation compiled into ordinary Hooks v2 interceptors.
//!
//! Embedders may ignore this crate and install custom hooks directly. Every
//! compiled rule phase receives a stable interceptor ID so the core audit trail
//! attributes its effects without giving automation special privileges.

use std::{collections::BTreeSet, sync::Arc};

use bytes::Bytes;
use thiserror::Error;
use transmog_core::{
    HeaderError, HeaderField, RequestHead, ResponseHead,
    intercept::{
        BodyPlan, BoxHookFuture, BufferedBody, ExchangeInterceptor, ExchangeMetadata,
        HookInitError, InterceptorFactory, InterceptorRegistration, InterceptorRequirement,
        RequestBodyAction, RequestBodyEvent, RequestHeadAction, RequestHeadEvent,
        ResponseBodyAction, ResponseBodyEvent, ResponseHeadAction, ResponseHeadEvent,
    },
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
}

impl Default for AutomationLimits {
    fn default() -> Self {
        Self {
            max_rules: 1_024,
            max_header_operations_per_rule: 256,
            max_replacement_body_bytes: 8 * 1024 * 1024,
        }
    }
}

/// Conservative exact/prefix rule predicate.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuleMatcher {
    /// Optional ASCII case-insensitive exact method.
    pub method: Option<String>,
    /// Optional ASCII case-insensitive exact host.
    pub host: Option<String>,
    /// Optional case-sensitive path prefix.
    pub path_prefix: Option<String>,
    /// Optional exact response status. Ignored for request phases.
    pub response_status: Option<u16>,
}

impl RuleMatcher {
    fn matches_request(&self, request: &RequestHead) -> bool {
        self.method
            .as_ref()
            .is_none_or(|method| request.method.eq_ignore_ascii_case(method))
            && self
                .host
                .as_ref()
                .is_none_or(|host| request.target.host.eq_ignore_ascii_case(host))
            && self
                .path_prefix
                .as_ref()
                .is_none_or(|prefix| request.target.path.starts_with(prefix))
    }

    fn matches_response(&self, request: &RequestHead, response: &ResponseHead) -> bool {
        self.matches_request(request)
            && self
                .response_status
                .is_none_or(|status| response.status == status)
    }

    fn overlaps(&self, other: &Self, response: bool) -> bool {
        optional_ascii_values_overlap(self.method.as_deref(), other.method.as_deref())
            && optional_ascii_values_overlap(self.host.as_deref(), other.host.as_deref())
            && prefixes_overlap(self.path_prefix.as_deref(), other.path_prefix.as_deref())
            && (!response
                || self.response_status.is_none()
                || other.response_status.is_none()
                || self.response_status == other.response_status)
    }
}

/// Validated deterministic header mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeaderOperation {
    /// Replace all occurrences while preserving the first position.
    Set(HeaderField),
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

    fn target(&self) -> String {
        match self {
            Self::Set(field) => String::from_utf8_lossy(field.name()).to_ascii_lowercase(),
            Self::Remove(name) => name.to_ascii_lowercase(),
        }
    }
}

/// Request-side rule actions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestActions {
    /// Ordered header operations.
    pub headers: Vec<HeaderOperation>,
    /// Optional decoded replacement body.
    pub replace_body: Option<Vec<u8>>,
    /// Permit replacement for methods not known to be idempotent.
    pub allow_non_idempotent_body_replacement: bool,
}

impl RequestActions {
    fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.replace_body.is_none()
    }
}

/// Response-side rule actions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResponseActions {
    /// Ordered header operations.
    pub headers: Vec<HeaderOperation>,
    /// Optional decoded replacement body.
    pub replace_body: Option<Vec<u8>>,
}

impl ResponseActions {
    fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.replace_body.is_none()
    }
}

/// One declarative rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rule {
    /// Stable identifier used in interceptor audit IDs.
    pub id: String,
    /// Higher values take precedence over lower values.
    pub priority: i32,
    /// Immutable request/response predicate.
    pub matcher: RuleMatcher,
    /// Request-side actions.
    pub request: RequestActions,
    /// Response-side actions.
    pub response: ResponseActions,
}

/// Successful deterministic compilation.
#[derive(Clone, Debug)]
pub struct CompiledAutomation {
    registrations: Vec<InterceptorRegistration>,
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
    mut rules: Vec<Rule>,
    limits: AutomationLimits,
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
        if !ids.insert(rule.id.clone()) {
            return Err(CompileError::DuplicateId(rule.id.clone()));
        }
    }
    reject_ambiguous_conflicts(&rules)?;

    let mut registrations = Vec::new();
    for rule in rules.iter().filter(|rule| !rule.request.is_empty()) {
        registrations.push(registration(rule, RuleDirection::Request));
    }
    for rule in rules.iter().rev().filter(|rule| !rule.response.is_empty()) {
        registrations.push(registration(rule, RuleDirection::Response));
    }
    Ok(CompiledAutomation { registrations })
}

fn validate_limits(limits: AutomationLimits) -> Result<(), CompileError> {
    if limits.max_rules == 0
        || limits.max_header_operations_per_rule == 0
        || limits.max_replacement_body_bytes == 0
    {
        return Err(CompileError::InvalidLimits);
    }
    Ok(())
}

fn validate_rule(rule: &Rule, limits: AutomationLimits) -> Result<(), CompileError> {
    if rule.id.is_empty()
        || !rule
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CompileError::InvalidId(rule.id.clone()));
    }
    for operations in [&rule.request.headers, &rule.response.headers] {
        if operations.len() > limits.max_header_operations_per_rule {
            return Err(CompileError::HeaderOperationLimitExceeded(rule.id.clone()));
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
    if rule
        .matcher
        .path_prefix
        .as_ref()
        .is_some_and(|prefix| !prefix.starts_with('/'))
    {
        return Err(CompileError::InvalidPathPrefix(rule.id.clone()));
    }
    Ok(())
}

fn reject_ambiguous_conflicts(rules: &[Rule]) -> Result<(), CompileError> {
    for (index, left) in rules.iter().enumerate() {
        for right in &rules[index + 1..] {
            if left.priority != right.priority {
                continue;
            }
            for (response, left_targets, right_targets) in [
                (
                    false,
                    action_targets(&left.request.headers, left.request.replace_body.is_some()),
                    action_targets(&right.request.headers, right.request.replace_body.is_some()),
                ),
                (
                    true,
                    action_targets(&left.response.headers, left.response.replace_body.is_some()),
                    action_targets(
                        &right.response.headers,
                        right.response.replace_body.is_some(),
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

fn action_targets(headers: &[HeaderOperation], body: bool) -> BTreeSet<String> {
    let mut targets = headers
        .iter()
        .map(HeaderOperation::target)
        .collect::<BTreeSet<_>>();
    if body {
        targets.insert("$body".to_owned());
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

fn registration(rule: &Rule, direction: RuleDirection) -> InterceptorRegistration {
    let suffix = match direction {
        RuleDirection::Request => "request",
        RuleDirection::Response => "response",
    };
    InterceptorRegistration::named(
        format!("automation/{}/{suffix}", rule.id),
        format!("automation rule {} ({suffix})", rule.id),
        Arc::new(RuleFactory {
            rule: Arc::new(rule.clone()),
            direction,
        }),
        InterceptorRequirement::Required,
    )
}

#[derive(Debug)]
struct RuleFactory {
    rule: Arc<Rule>,
    direction: RuleDirection,
}

impl InterceptorFactory for RuleFactory {
    fn create(
        &self,
        _metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        Ok(Arc::new(RuleInterceptor {
            rule: Arc::clone(&self.rule),
            direction: self.direction,
        }))
    }
}

#[derive(Debug)]
struct RuleInterceptor {
    rule: Arc<Rule>,
    direction: RuleDirection,
}

impl ExchangeInterceptor for RuleInterceptor {
    fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        if self.direction != RuleDirection::Request
            || !self.rule.matcher.matches_request(&event.head)
        {
            return Box::pin(async { RequestHeadAction::Continue });
        }
        let mut head = event.head;
        apply_headers(&mut head.headers, &self.rule.request.headers);
        Box::pin(async move { RequestHeadAction::Replace(head) })
    }

    fn on_request_body(&self, event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        let replacement = (self.direction == RuleDirection::Request
            && self.rule.matcher.matches_request(&event.head)
            && (self.rule.request.allow_non_idempotent_body_replacement
                || method_is_idempotent(&event.head.method)))
        .then(|| self.rule.request.replace_body.as_ref())
        .flatten()
        .cloned();
        Box::pin(async move {
            replacement.map_or_else(RequestBodyAction::pass_through, |bytes| {
                RequestBodyAction::decoded(BodyPlan::Replace(
                    BufferedBody::try_new(bytes.len(), Bytes::from(bytes), None)
                        .expect("compiled replacement satisfies its exact bound"),
                ))
            })
        })
    }

    fn on_response_head(&self, event: ResponseHeadEvent) -> BoxHookFuture<'_, ResponseHeadAction> {
        if self.direction != RuleDirection::Response
            || !self
                .rule
                .matcher
                .matches_response(&event.request_head, &event.head)
        {
            return Box::pin(async { ResponseHeadAction::Continue });
        }
        let mut head = event.head;
        apply_headers(&mut head.headers, &self.rule.response.headers);
        Box::pin(async move { ResponseHeadAction::Replace(head) })
    }

    fn on_response_body(&self, event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        let replacement = (self.direction == RuleDirection::Response
            && self
                .rule
                .matcher
                .matches_response(&event.request_head, &event.response_head))
        .then(|| self.rule.response.replace_body.as_ref())
        .flatten()
        .cloned();
        Box::pin(async move {
            replacement.map_or_else(ResponseBodyAction::pass_through, |bytes| {
                ResponseBodyAction::decoded(BodyPlan::Replace(
                    BufferedBody::try_new(bytes.len(), Bytes::from(bytes), None)
                        .expect("compiled replacement satisfies its exact bound"),
                ))
            })
        })
    }
}

fn apply_headers(headers: &mut transmog_core::HeaderBlock, actions: &[HeaderOperation]) {
    for action in actions {
        match action {
            HeaderOperation::Set(field) => headers.replace_all(field.clone()),
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
    /// Rule identifier appeared more than once.
    #[error("automation rule id is duplicated: {0}")]
    DuplicateId(String),
    /// Path prefix did not begin with `/`.
    #[error("automation rule has an invalid path prefix: {0}")]
    InvalidPathPrefix(String),
    /// Header-operation count exceeded its per-rule bound.
    #[error("automation rule exceeds its header-operation limit: {0}")]
    HeaderOperationLimitExceeded(String),
    /// Immediate replacement body exceeded its bound.
    #[error("automation rule exceeds its replacement-body limit: {0}")]
    BodyLimitExceeded(String),
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
                "automation/low/request",
                "automation/high/request",
                "automation/high/response",
                "automation/low/response"
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
        assert_eq!(effects[0].interceptor.id.as_str(), "automation/low/request");
        assert_eq!(
            effects[1].interceptor.id.as_str(),
            "automation/high/request"
        );
        assert_eq!(
            effects[2].interceptor.id.as_str(),
            "automation/low/response"
        );
        assert_eq!(
            effects[3].interceptor.id.as_str(),
            "automation/high/response"
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

        let oversized = Rule {
            id: "oversized".to_owned(),
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

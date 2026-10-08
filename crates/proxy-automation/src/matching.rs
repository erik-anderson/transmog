//! Bounded URL conditions shared by live hooks and the rule editor.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use transmog_core::{HeaderBlock, HeaderField, HttpLegVersion, RequestHead, Target};

use crate::{AutomationLimits, RuleMatcher, compile_predicates};

fn yes() -> bool {
    true
}

/// Match operation for an absolute request URL.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum UrlCondition {
    /// Literal normalized URL, including the query.
    Exact(String),
    /// Literal address with annotated path placeholders.
    Pattern(UrlPattern),
    /// Explicit bounded regular expression.
    Regex(UrlRegex),
}

/// An address pattern. Placeholder names are optional annotations only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlPattern {
    /// Absolute scheme, authority and path; query conditions are separate.
    pub address: String,
    /// Query policy, exact by default (including absence).
    #[serde(default)]
    pub query: QueryCondition,
    /// Whether literal path portions are case sensitive.
    #[serde(default = "yes")]
    pub case_sensitive: bool,
}

/// Explicit URL or path regular expression options.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlRegex {
    /// Rust regular expression, without delimiter slashes or magic prefixes.
    pub pattern: String,
    /// The value evaluated by the expression.
    pub scope: RegexScope,
    /// Match the whole value rather than search within it.
    #[serde(default = "yes")]
    pub whole: bool,
    /// Whether the expression is case sensitive.
    #[serde(default = "yes")]
    pub case_sensitive: bool,
    /// Optional additional query condition, typically for path expressions.
    #[serde(default)]
    pub query: Option<QueryCondition>,
}

/// Regex input scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegexScope {
    /// Normalized absolute URL including query.
    Url,
    /// Origin-form path, excluding query.
    Path,
}

/// Query policy with no implicit ignoring of captured values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum QueryCondition {
    /// Exact raw query, including order; null requires no query.
    Exact(Option<String>),
    /// Every listed decoded name/value pair must occur; additional pairs are allowed.
    Parameters(Vec<QueryParameter>),
    /// Explicitly accept any query.
    Ignore,
}

impl Default for QueryCondition {
    fn default() -> Self {
        Self::Exact(None)
    }
}

/// One required query parameter, retaining repeated names.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryParameter {
    /// Decoded parameter name.
    pub name: String,
    /// Decoded exact value.
    pub value: String,
}

/// A retained matcher expectation, without sending network traffic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchExample {
    /// HTTP method to test.
    pub method: String,
    /// Absolute URL to test.
    pub url: String,
    /// Expected match outcome.
    pub expected: bool,
    /// Explicit request headers retained with this test case.
    #[serde(default)]
    pub headers: Vec<ExampleHeader>,
}

/// A bounded header in a saved network-free test case.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExampleHeader {
    /// Valid HTTP field name.
    pub name: String,
    /// Exact UTF-8 field value.
    pub value: String,
}

/// A named or anonymous matched part of the address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchCapture {
    /// Optional annotation, or a generated positional label.
    pub label: String,
    /// The matched text. Repeated labels do not imply equality.
    pub value: String,
}

/// One independent explanation of a matcher condition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchCheck {
    /// User-facing condition name.
    pub label: String,
    /// Whether it passed.
    pub matched: bool,
    /// Concise explanation of the expected condition.
    pub detail: String,
}

/// Network-free result from the same matcher used by live hooks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchTest {
    /// All applicable request conditions passed.
    pub matched: bool,
    /// Canonical URL seen by the matcher.
    pub normalized_url: String,
    /// Independent condition results.
    pub checks: Vec<MatchCheck>,
    /// Matched annotations or positional captures.
    pub captures: Vec<MatchCapture>,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledUrl {
    regex: Option<Regex>,
    exact: Option<String>,
    scope: RegexScope,
    query: Option<QueryCondition>,
    labels: Vec<String>,
    address_only: bool,
    case_sensitive: bool,
}

/// Parses a synthetic HTTP request using the canonical target representation.
///
/// # Errors
/// Rejects malformed or oversized URLs, methods and headers.
pub fn request_for_test(
    method: &str,
    url: &str,
    headers: &[(String, String)],
) -> Result<RequestHead, String> {
    if method.parse::<http::Method>().is_err() || method.len() > 32 || headers.len() > 64 {
        return Err("Enter a valid HTTP method and at most 64 headers.".into());
    }
    let target = parse_target(url)?;
    let mut block = HeaderBlock::new();
    let mut bytes = 0;
    for (name, value) in headers {
        bytes += name.len() + value.len();
        if bytes > 32 * 1024 {
            return Err("Test headers exceed 32 KiB.".into());
        }
        block.push(
            HeaderField::try_new(name.as_bytes(), value.as_bytes()).map_err(|e| e.to_string())?,
        );
    }
    Ok(RequestHead {
        method: method.to_owned(),
        target,
        headers: block,
        source_version: HttpLegVersion::Http1,
    })
}

fn parse_target(url: &str) -> Result<Target, String> {
    if url.len() > 8192 || url.contains('#') {
        return Err("Use an HTTP or HTTPS URL without a fragment (maximum 8192 bytes).".into());
    }
    let uri: http::Uri = url
        .parse()
        .map_err(|_| "Enter an absolute HTTP or HTTPS URL.")?;
    let scheme = uri.scheme_str().unwrap_or_default().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err("The URL must use HTTP or HTTPS.".into());
    }
    let authority = uri.authority().ok_or("The URL needs a host.")?;
    if authority.as_str().contains('@') {
        return Err("The URL must not contain credentials.".into());
    }
    let host = authority
        .host()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    let port = authority
        .port_u16()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let bracketed = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.clone()
    };
    let authority = if (scheme == "https" && port == 443) || (scheme == "http" && port == 80) {
        bracketed
    } else {
        format!("{bracketed}:{port}")
    };
    Ok(Target {
        scheme,
        authority,
        host,
        port,
        path: uri.path().to_owned(),
        query: uri.query().map(str::to_owned),
    })
}

pub(crate) fn address(target: &Target) -> String {
    let host = target.host.trim_matches(['[', ']']).to_ascii_lowercase();
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let scheme = target.scheme.to_ascii_lowercase();
    let port =
        if (scheme == "https" && target.port == 443) || (scheme == "http" && target.port == 80) {
            String::new()
        } else {
            format!(":{}", target.port)
        };
    format!("{scheme}://{host}{port}{}", target.path)
}

pub(crate) fn absolute_url(target: &Target) -> String {
    let mut value = address(target);
    if let Some(query) = &target.query {
        value.push('?');
        value.push_str(query);
    }
    value
}

impl UrlCondition {
    pub(crate) fn compile(&self) -> Result<CompiledUrl, String> {
        let mut result = CompiledUrl {
            regex: None,
            exact: None,
            scope: RegexScope::Url,
            query: None,
            labels: Vec::new(),
            address_only: false,
            case_sensitive: true,
        };
        match self {
            Self::Exact(value) => result.exact = Some(absolute_url(&parse_target(value)?)),
            Self::Pattern(pattern) => {
                validate_query(&pattern.query)?;
                let (expression, labels) =
                    pattern_expression(&pattern.address, pattern.case_sensitive)?;
                result.regex = Some(bounded_regex(&expression, true)?);
                result.labels = labels;
                result.query = Some(pattern.query.clone());
                result.address_only = true;
            }
            Self::Regex(pattern) => {
                if pattern.pattern.is_empty()
                    || pattern.pattern.len() > AutomationLimits::default().max_regex_bytes
                {
                    return Err("Regex must contain 1–4096 bytes.".into());
                }
                if let Some(query) = &pattern.query {
                    validate_query(query)?;
                }
                let expression = if pattern.whole {
                    format!("\\A(?:{})\\z", pattern.pattern)
                } else {
                    pattern.pattern.clone()
                };
                result.regex = Some(bounded_regex(&expression, pattern.case_sensitive)?);
                result.scope = pattern.scope;
                result.query.clone_from(&pattern.query);
                result.case_sensitive = pattern.case_sensitive;
            }
        }
        Ok(result)
    }

    /// Identity of the matching behavior, excluding annotations.
    ///
    /// # Errors
    /// Returns the same syntax errors as compilation.
    pub fn matching_key(&self) -> Result<String, String> {
        let compiled = self.compile()?;
        let mut query = compiled.query;
        if let Some(QueryCondition::Parameters(items)) = &mut query {
            items.sort_by(|a, b| (&a.name, &a.value).cmp(&(&b.name, &b.value)));
        }
        Ok(format!(
            "{:?}|{:?}|{:?}|{:?}|{}|{}",
            compiled.scope,
            compiled.exact,
            compiled.regex.as_ref().map(Regex::as_str),
            query,
            compiled.address_only,
            compiled.case_sensitive
        ))
    }
}

fn bounded_regex(expression: &str, case_sensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(expression)
        .case_insensitive(!case_sensitive)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
        .map_err(|e| e.to_string())
}

fn validate_query(query: &QueryCondition) -> Result<(), String> {
    let bytes = match query {
        QueryCondition::Exact(value) => value.as_ref().map_or(0, String::len),
        QueryCondition::Parameters(items) => {
            if items.len() > 64 || items.iter().any(|item| item.name.is_empty()) {
                return Err("Use at most 64 named query parameters.".into());
            }
            items
                .iter()
                .map(|item| item.name.len() + item.value.len())
                .sum()
        }
        QueryCondition::Ignore => 0,
    };
    if bytes > 4096 {
        return Err("Query conditions exceed 4096 bytes.".into());
    }
    Ok(())
}

fn pattern_expression(value: &str, case_sensitive: bool) -> Result<(String, Vec<String>), String> {
    if value.len() > 8192 {
        return Err("URL pattern exceeds 8192 bytes.".into());
    }
    let (_, remainder) = value
        .split_once("://")
        .ok_or("Start the pattern with http:// or https://.")?;
    let authority_end =
        value.len() - remainder.len() + remainder.find('/').unwrap_or(remainder.len());
    let (prefix, path) = value.split_at(authority_end);
    if prefix.contains(['{', '}']) {
        return Err("Keep the scheme and host literal; placeholders belong in the path.".into());
    }
    if path.contains(['?', '#']) {
        return Err("Set query conditions in Query; fragments are not sent with requests.".into());
    }
    let prefix = address(&parse_target(&format!("{prefix}/"))?);
    let mut expression = format!("\\A{}", regex::escape(prefix.trim_end_matches('/')));
    if !case_sensitive {
        expression.push_str("(?i)");
    }
    let mut labels = Vec::new();
    let mut remaining = if path.is_empty() { "/" } else { path };
    while let Some(start) = remaining.find('{') {
        let literal = &remaining[..start];
        if literal.contains('}') {
            return Err("Unexpected closing brace.".into());
        }
        expression.push_str(&regex::escape(literal));
        let end = remaining[start + 1..]
            .find('}')
            .ok_or("Close the placeholder with }.")?
            + start
            + 1;
        let token = &remaining[start + 1..end];
        let (label, kind) = if let Some(label) = token.strip_suffix("...") {
            (label, "rest")
        } else {
            token.split_once(':').unwrap_or((token, "segment"))
        };
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(
                "Annotations may contain letters, digits and underscores, or be empty.".into(),
            );
        }
        if !literal.ends_with('/')
            || (!remaining[end + 1..].is_empty() && !remaining[end + 1..].starts_with('/'))
        {
            return Err("A placeholder must occupy a complete path segment.".into());
        }
        let part = match kind {
            "segment" => "([^/?#]+)",
            "digits" => "([0-9]+)",
            "rest" if remaining[end + 1..].is_empty() => "([^/?#]+(?:/[^/?#]+)*)",
            "rest" => return Err("The remaining-path placeholder must come last.".into()),
            _ => return Err("Unknown placeholder type. Use {} or {:digits} or {...}.".into()),
        };
        expression.push_str(part);
        labels.push(if label.is_empty() {
            format!("Part {}", labels.len() + 1)
        } else {
            label.to_owned()
        });
        remaining = &remaining[end + 1..];
    }
    if remaining.contains('}') {
        return Err("Unexpected closing brace.".into());
    }
    expression.push_str(&regex::escape(remaining));
    expression.push_str("\\z");
    Ok((expression, labels))
}

impl CompiledUrl {
    fn matches_address(&self, target: &Target) -> bool {
        let value = if self.address_only {
            address(target)
        } else {
            absolute_url(target)
        };
        let value = if self.scope == RegexScope::Path {
            target.path.as_str()
        } else {
            value.as_str()
        };
        self.exact.as_ref().is_none_or(|exact| exact == value)
            && self
                .regex
                .as_ref()
                .is_none_or(|regex| regex.is_match(value))
    }
    pub(crate) fn matches(&self, target: &Target) -> bool {
        self.matches_address(target)
            && self
                .query
                .as_ref()
                .is_none_or(|query| query_matches(query, target.query.as_deref()))
    }
    fn captures(&self, target: &Target) -> Vec<MatchCapture> {
        let value = address(target);
        self.regex
            .as_ref()
            .and_then(|regex| regex.captures(&value))
            .map_or_else(Vec::new, |captures| {
                self.labels
                    .iter()
                    .enumerate()
                    .filter_map(|(index, label)| {
                        captures.get(index + 1).map(|part| MatchCapture {
                            label: label.clone(),
                            value: part.as_str().to_owned(),
                        })
                    })
                    .collect()
            })
    }
}

/// Compares provably identical predicates, ignoring examples and annotation names.
///
/// # Errors
/// Returns URL syntax errors. Arbitrary regex equivalence is not inferred.
pub fn same_matching_behavior(left: &RuleMatcher, right: &RuleMatcher) -> Result<bool, String> {
    Ok(matcher_key(left)? == matcher_key(right)?)
}

/// Stable identity of provably equivalent conditions, excluding annotations and examples.
///
/// # Errors
/// Returns syntax errors instead of guessing equivalence.
pub fn matcher_key(matcher: &RuleMatcher) -> Result<String, String> {
    let url = matcher
        .url
        .as_ref()
        .map(UrlCondition::matching_key)
        .transpose()?;
    let mut matcher = matcher.clone();
    matcher.url = None;
    matcher.examples.clear();
    matcher.response_headers.clear();
    matcher.response_status = None;
    matcher.response_status_class = None;
    matcher.method = matcher.method.map(|value| value.to_ascii_uppercase());
    matcher.host = matcher.host.map(|value| value.to_ascii_lowercase());
    matcher.scheme = matcher.scheme.map(|value| value.to_ascii_lowercase());
    for header in &mut matcher.request_headers {
        header.name.make_ascii_lowercase();
    }
    matcher
        .request_headers
        .sort_by_key(|header| format!("{}{:?}", header.name, header.condition));
    matcher.request_headers.dedup();
    Ok(format!("{url:?}|{matcher:?}"))
}

fn decode_parameter(value: &str) -> String {
    let mut result = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'+' => result.push(b' '),
            b'%' => {
                let pair = [bytes.next().unwrap_or(b'%'), bytes.next().unwrap_or(b'%')];
                let hex = |byte: u8| char::from(byte).to_digit(16);
                if let (Some(a), Some(b)) = (hex(pair[0]), hex(pair[1])) {
                    result.push(u8::try_from(a * 16 + b).expect("two hexadecimal digits"));
                } else {
                    result.push(b'%');
                    result.extend_from_slice(&pair);
                }
            }
            _ => result.push(byte),
        }
    }
    String::from_utf8_lossy(&result).into_owned()
}

fn query_matches(condition: &QueryCondition, query: Option<&str>) -> bool {
    match condition {
        QueryCondition::Exact(expected) => expected.as_deref() == query,
        QueryCondition::Ignore => true,
        QueryCondition::Parameters(required) => {
            let mut pairs = query
                .unwrap_or_default()
                .split('&')
                .map(|part| part.split_once('=').unwrap_or((part, "")))
                .map(|(name, value)| (decode_parameter(name), decode_parameter(value)))
                .collect::<Vec<_>>();
            required.iter().all(|item| {
                if let Some(index) = pairs
                    .iter()
                    .position(|(name, value)| name == &item.name && value == &item.value)
                {
                    pairs.swap_remove(index);
                    true
                } else {
                    false
                }
            })
        }
    }
}

/// Evaluates a matcher with independent explanations and annotated captures.
///
/// # Errors
/// Returns syntax or resource errors without sending network requests.
pub fn test_matcher(matcher: &RuleMatcher, request: &RequestHead) -> Result<MatchTest, String> {
    let url = matcher
        .url
        .as_ref()
        .map(UrlCondition::compile)
        .transpose()?;
    for predicate in &matcher.request_headers {
        if let crate::HeaderCondition::Regex(pattern) = &predicate.condition {
            crate::compile_regex(pattern).map_err(|error| error.to_string())?;
        }
    }
    crate::compile(
        vec![crate::Rule {
            id: "matcher-test".into(),
            display_name: None,
            enabled: true,
            revision: 1,
            priority: 0,
            matcher: matcher.clone(),
            request: crate::RequestActions::default(),
            response: crate::ResponseActions::default(),
        }],
        AutomationLimits::default(),
    )
    .map_err(|e| e.to_string())?;
    let mut checks = url
        .as_ref()
        .map_or_else(Vec::new, |url| url_checks(url, &request.target));
    let mut add = |label: &str, matched: bool, detail: String| {
        checks.push(MatchCheck {
            label: label.to_owned(),
            matched,
            detail,
        });
    };
    if let Some(method) = &matcher.method {
        add(
            "Method",
            request.method.eq_ignore_ascii_case(method),
            format!("Expected {method}"),
        );
    }
    if let Some(host) = &matcher.host {
        add(
            "Host",
            request.target.host.eq_ignore_ascii_case(host),
            host.clone(),
        );
    }
    if let Some(scheme) = &matcher.scheme {
        add(
            "Scheme",
            request.target.scheme.eq_ignore_ascii_case(scheme),
            scheme.clone(),
        );
    }
    if let Some(port) = matcher.port {
        add("Port", request.target.port == port, port.to_string());
    }
    if let Some(path) = &matcher.path_prefix {
        add(
            "Path prefix",
            request.target.path.starts_with(path),
            path.clone(),
        );
    }
    if let Some(query) = &matcher.query {
        add(
            "Query",
            request.target.query.as_deref() == Some(query),
            query.clone(),
        );
    }
    let regexes = compile_predicates(&matcher.request_headers);
    for (predicate, regex) in matcher.request_headers.iter().zip(regexes.iter()) {
        add(
            "Request header",
            predicate.matches(&request.headers, regex.as_ref()),
            predicate.name.clone(),
        );
    }
    Ok(MatchTest {
        matched: checks.iter().all(|check| check.matched),
        normalized_url: absolute_url(&request.target),
        checks,
        captures: url
            .as_ref()
            .map_or_else(Vec::new, |url| url.captures(&request.target)),
    })
}

fn url_checks(url: &CompiledUrl, target: &Target) -> Vec<MatchCheck> {
    let mut checks = vec![MatchCheck {
        label: "Address".into(),
        matched: url.matches_address(target),
        detail: "Scheme, host and path condition".into(),
    }];
    match &url.query {
        Some(QueryCondition::Exact(expected)) => checks.push(MatchCheck {
            label: "Query".into(),
            matched: target.query == *expected,
            detail: expected.clone().map_or_else(
                || "Requires no query".into(),
                |value| format!("Exact query: {value}"),
            ),
        }),
        Some(QueryCondition::Parameters(items)) => {
            let mut grouped = std::collections::BTreeMap::new();
            for item in items {
                *grouped.entry((&item.name, &item.value)).or_insert(0usize) += 1;
            }
            for ((name, value), count) in grouped {
                let item = QueryParameter {
                    name: name.clone(),
                    value: value.clone(),
                };
                checks.push(MatchCheck {
                    label: "Required parameter".into(),
                    matched: query_matches(
                        &QueryCondition::Parameters(vec![item; count]),
                        target.query.as_deref(),
                    ),
                    detail: format!("{name} = {value} ({count} required)"),
                });
            }
        }
        Some(QueryCondition::Ignore) => checks.push(MatchCheck {
            label: "Query".into(),
            matched: true,
            detail: "Query explicitly ignored".into(),
        }),
        None => {}
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pattern(address: &str, query: QueryCondition) -> UrlCondition {
        UrlCondition::Pattern(UrlPattern {
            address: address.into(),
            query,
            case_sensitive: true,
        })
    }
    fn target(url: &str) -> Target {
        request_for_test("GET", url, &[]).unwrap().target
    }
    #[test]
    fn annotations_are_optional_and_do_not_constrain_values() {
        let named = pattern(
            "https://example.test/{id:digits}/{id}",
            QueryCondition::default(),
        );
        let anonymous = pattern(
            "https://example.test/{:digits}/{}",
            QueryCondition::default(),
        );
        assert_eq!(
            named.matching_key().unwrap(),
            anonymous.matching_key().unwrap()
        );
        let compiled = named.compile().unwrap();
        assert!(compiled.matches(&target("https://EXAMPLE.test:443/42/other")));
        assert!(!compiled.matches(&target("https://example.test/word/other")));
        assert!(!compiled.matches(&target("https://example.test/42/other/extra")));
        let captures = compiled.captures(&target("https://example.test/42/other"));
        assert_eq!(
            captures
                .iter()
                .map(|capture| capture.value.as_str())
                .collect::<Vec<_>>(),
            ["42", "other"]
        );
        assert!(
            pattern("https://example.test/{...}", QueryCondition::Ignore)
                .compile()
                .unwrap()
                .matches(&target("https://example.test/a/b?x=1"))
        );
    }
    #[test]
    fn query_parameters_preserve_duplicates_and_exact_policies() {
        let required = QueryCondition::Parameters(vec![
            QueryParameter {
                name: "tag".into(),
                value: "a b".into(),
            },
            QueryParameter {
                name: "tag".into(),
                value: "c".into(),
            },
        ]);
        let compiled = pattern("https://example.test/items", required)
            .compile()
            .unwrap();
        assert!(compiled.matches(&target("https://example.test/items?tag=c&tag=a+b&extra=1")));
        assert!(!compiled.matches(&target("https://example.test/items?tag=c")));
        assert!(
            !pattern("https://example.test/items", QueryCondition::default())
                .compile()
                .unwrap()
                .matches(&target("https://example.test/items?extra=1"))
        );
    }
    #[test]
    fn syntax_and_regex_modes_fail_predictably() {
        for invalid in [
            "https://example.test/{",
            "https://example.test/{:unknown}",
            "https://example.test/pre{id}",
            "https://example.test/{...}/x",
            "https://example.test/x?lost=yes",
        ] {
            assert!(
                pattern(invalid, QueryCondition::default())
                    .compile()
                    .is_err(),
                "{invalid}"
            );
        }
        let condition = UrlCondition::Regex(UrlRegex {
            pattern: "/items/[0-9]+".into(),
            scope: RegexScope::Path,
            whole: true,
            case_sensitive: true,
            query: None,
        });
        assert!(
            condition
                .compile()
                .unwrap()
                .matches(&target("https://example.test/items/42?ignored=1"))
        );
        assert!(
            !condition
                .compile()
                .unwrap()
                .matches(&target("https://example.test/items/42/extra"))
        );
    }
    #[test]
    fn tester_and_runtime_agree_on_repeated_query_values() {
        let condition = pattern(
            "https://example.test/items",
            QueryCondition::Parameters(vec![
                QueryParameter {
                    name: "tag".into(),
                    value: "a".into()
                };
                2
            ]),
        );
        let matcher = crate::RuleMatcher {
            url: Some(condition.clone()),
            ..Default::default()
        };
        for (url, expected) in [
            ("https://example.test/items?tag=a", false),
            ("https://example.test/items?tag=a&tag=a", true),
        ] {
            let request = request_for_test("GET", url, &[]).unwrap();
            assert_eq!(
                condition.compile().unwrap().matches(&request.target),
                expected
            );
            let test = test_matcher(&matcher, &request).unwrap();
            assert_eq!(test.matched, expected);
            assert_eq!(
                test.checks
                    .iter()
                    .find(|check| check.label == "Required parameter")
                    .unwrap()
                    .matched,
                expected
            );
        }
    }
}

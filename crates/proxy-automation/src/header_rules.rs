//! Header authoring tests use the same matcher and operation implementation as live hooks.
use crate::{
    HeaderOperation, MatchCheck, MatchTest, Rule, apply_headers, compile_predicates, test_matcher,
};
use serde::{Deserialize, Serialize};
use transmog_core::{HeaderBlock, RequestHead, ResponseHead};

/// Direction of a targeted header rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HeaderRulePhase {
    /// Modify the effective request before it reaches the upstream.
    Request,
    /// Modify the current response after origin acquisition and script hooks.
    Response,
}

/// Result of one ordered operation in an explicit network-free draft test.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderOperationResult {
    /// One-based position in the authored operation list.
    pub index: usize,
    /// Set, append or remove.
    pub operation: &'static str,
    /// Case-preserving target header name.
    pub name: String,
    /// Whether this operation changed the ordered header block.
    pub changed: bool,
}

/// An independent draft test, without executing other rules or sending traffic.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderRuleTest {
    /// Same matcher checks and captures as live automation, plus response conditions.
    pub test: MatchTest,
    /// Supplied original sample headers, including duplicate order and raw values.
    pub before: HeaderBlock,
    /// Resulting sample headers; identical to before when the matcher does not match.
    pub after: HeaderBlock,
    /// Ordered operation outcomes; empty for a nonmatching sample.
    pub operations: Vec<HeaderOperationResult>,
}

pub(super) fn protected_header(name: &str) -> bool {
    [
        "host",
        "content-length",
        "transfer-encoding",
        "content-encoding",
        "connection",
        "proxy-connection",
        "keep-alive",
        "te",
        "trailer",
        "upgrade",
    ]
    .iter()
    .any(|protected| name.eq_ignore_ascii_case(protected))
}

/// Recognizes a single-phase header-only rule without reinterpreting advanced actions.
pub fn header_rule_phase(rule: &Rule) -> Option<HeaderRulePhase> {
    if rule.request.user_agent.is_some()
        || rule.request.response_asset.is_some()
        || rule.request.replace_body.is_some()
        || rule.request.discard_body
        || rule.request.abort_reason.is_some()
        || rule.response.replace_body.is_some()
        || rule.response.discard_body
        || rule.response.abort_reason.is_some()
    {
        return None;
    }
    match (
        rule.request.headers.is_empty(),
        rule.response.headers.is_empty(),
    ) {
        (false, true)
            if rule.matcher.response_headers.is_empty()
                && rule.matcher.response_status.is_none()
                && rule.matcher.response_status_class.is_none() =>
        {
            Some(HeaderRulePhase::Request)
        }
        (true, false) => Some(HeaderRulePhase::Response),
        _ => None,
    }
}

/// Evaluates a header-only draft with production matching and ordered operations.
/// Disabled drafts are deliberately tested as if enabled, without changing live state.
///
/// # Errors
/// Rejects invalid rules, syntax or a missing response sample for a response rule.
pub fn test_header_rule(
    rule: &Rule,
    request: &RequestHead,
    response: Option<&ResponseHead>,
) -> Result<HeaderRuleTest, String> {
    let phase =
        header_rule_phase(rule).ok_or("Select a single request or response header-only rule")?;
    crate::compile(vec![rule.clone()], crate::AutomationLimits::default())
        .map_err(|error| error.to_string())?;
    let mut test = test_matcher(&rule.matcher, request)?;
    let (before, actions) = match phase {
        HeaderRulePhase::Request => (request.headers.clone(), &rule.request.headers),
        HeaderRulePhase::Response => {
            let head =
                response.ok_or("Supply a response status and headers for this response rule")?;
            if let Some(status) = rule.matcher.response_status {
                test.checks.push(MatchCheck {
                    label: "Response status".into(),
                    matched: head.status == status,
                    detail: format!("Expected {status}, sample {}", head.status),
                });
            }
            if let Some(class) = rule.matcher.response_status_class {
                test.checks.push(MatchCheck {
                    label: "Response status class".into(),
                    matched: head.status / 100 == class,
                    detail: format!("Expected {class}xx, sample {}", head.status),
                });
            }
            let regexes = compile_predicates(&rule.matcher.response_headers);
            for (predicate, regex) in rule.matcher.response_headers.iter().zip(regexes.iter()) {
                test.checks.push(MatchCheck {
                    label: format!("Response header {}", predicate.name),
                    matched: predicate.matches(&head.headers, regex.as_ref()),
                    detail: "At least one field value must satisfy this saved condition".into(),
                });
            }
            test.matched = test.checks.iter().all(|check| check.matched);
            (head.headers.clone(), &rule.response.headers)
        }
    };
    let mut after = before.clone();
    let mut operations = Vec::new();
    if test.matched {
        for (index, action) in actions.iter().enumerate() {
            let previous = after.clone();
            apply_headers(&mut after, std::slice::from_ref(action));
            let (operation, name) = match action {
                HeaderOperation::Set(field) => {
                    ("Set", String::from_utf8_lossy(field.name()).into_owned())
                }
                HeaderOperation::Append(field) => {
                    ("Append", String::from_utf8_lossy(field.name()).into_owned())
                }
                HeaderOperation::Remove(name) => ("Remove", name.clone()),
            };
            operations.push(HeaderOperationResult {
                index: index + 1,
                operation,
                name,
                changed: previous != after,
            });
        }
    }
    Ok(HeaderRuleTest {
        test,
        before,
        after,
        operations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeaderCondition, HeaderPredicate, RequestActions, ResponseActions, RuleMatcher};
    use transmog_core::{HeaderField, HttpLegVersion};
    fn rule(phase: HeaderRulePhase, operations: Vec<HeaderOperation>) -> Rule {
        Rule {
            id: "headers".into(),
            display_name: Some("Header draft".into()),
            enabled: false,
            revision: 1,
            priority: 0,
            matcher: RuleMatcher::default(),
            request: RequestActions {
                headers: if phase == HeaderRulePhase::Request {
                    operations.clone()
                } else {
                    Vec::new()
                },
                ..RequestActions::default()
            },
            response: ResponseActions {
                headers: if phase == HeaderRulePhase::Response {
                    operations
                } else {
                    Vec::new()
                },
                ..ResponseActions::default()
            },
        }
    }
    fn response(headers: HeaderBlock) -> ResponseHead {
        ResponseHead {
            status: 200,
            headers,
            source_version: HttpLegVersion::Http1,
        }
    }
    #[test]
    fn ordered_operations_preserve_untouched_duplicates_and_non_ascii_bytes() {
        let headers = HeaderBlock::from_fields(vec![
            HeaderField::try_new("X-A", "first").unwrap(),
            HeaderField::try_new("Set-Cookie", "a=1").unwrap(),
            HeaderField::try_new("x-a", "second").unwrap(),
            HeaderField::try_new("set-cookie", "b=2").unwrap(),
            HeaderField::try_new("X-Binary", vec![0xff, 0x80]).unwrap(),
        ]);
        let actions = vec![
            HeaderOperation::set("X-A", "café").unwrap(),
            HeaderOperation::append("x-a", vec![0xfe]).unwrap(),
            HeaderOperation::remove("missing").unwrap(),
            HeaderOperation::remove("x-binary").unwrap(),
        ];
        let rule = rule(HeaderRulePhase::Response, actions);
        let request = crate::request_for_test("GET", "https://example.test/", &[]).unwrap();
        let result = test_header_rule(&rule, &request, Some(&response(headers.clone()))).unwrap();
        assert_eq!(result.before, headers);
        assert_eq!(
            result
                .after
                .iter()
                .map(|f| (f.name().to_vec(), f.value().to_vec()))
                .collect::<Vec<_>>(),
            vec![
                (b"X-A".to_vec(), "café".as_bytes().to_vec()),
                (b"Set-Cookie".to_vec(), b"a=1".to_vec()),
                (b"set-cookie".to_vec(), b"b=2".to_vec()),
                (b"x-a".to_vec(), vec![0xfe])
            ]
        );
        assert_eq!(
            result
                .operations
                .iter()
                .map(|op| op.changed)
                .collect::<Vec<_>>(),
            vec![true, true, false, true]
        );
    }
    #[test]
    fn response_conditions_use_duplicate_values_and_no_match_is_unchanged() {
        let mut rule = rule(
            HeaderRulePhase::Response,
            vec![HeaderOperation::remove("content-security-policy").unwrap()],
        );
        rule.matcher.response_status = Some(200);
        rule.matcher.response_status_class = Some(2);
        rule.matcher.response_headers = vec![
            HeaderPredicate {
                name: "content-type".into(),
                condition: HeaderCondition::Prefix(b"text/html".to_vec()),
            },
            HeaderPredicate {
                name: "x-marker".into(),
                condition: HeaderCondition::Regex("^yes$".into()),
            },
        ];
        let request = crate::request_for_test("GET", "https://example.test/", &[]).unwrap();
        let mut head = response(HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-type", "application/json").unwrap(),
            HeaderField::try_new("Content-Type", "text/html; charset=utf-8").unwrap(),
            HeaderField::try_new("X-Marker", "yes").unwrap(),
            HeaderField::try_new("Content-Security-Policy", "default-src 'none'").unwrap(),
        ]));
        assert!(
            test_header_rule(&rule, &request, Some(&head))
                .unwrap()
                .test
                .matched
        );
        head.status = 404;
        let result = test_header_rule(&rule, &request, Some(&head)).unwrap();
        assert!(!result.test.matched);
        assert_eq!(result.before, result.after);
        assert!(result.operations.is_empty());
    }
    #[test]
    fn request_header_test_preserves_binary_values_and_matches_production_predicates() {
        let mut rule = rule(
            HeaderRulePhase::Request,
            vec![HeaderOperation::append("X-Bytes", vec![0x80, 0xff]).unwrap()],
        );
        rule.matcher.request_headers.push(HeaderPredicate {
            name: "X-Original".into(),
            condition: HeaderCondition::Exact(vec![0xff]),
        });
        let mut request = crate::request_for_test("POST", "https://example.test/", &[]).unwrap();
        request
            .headers
            .push(HeaderField::try_new("x-original", vec![0xff]).unwrap());
        let result = test_header_rule(&rule, &request, None).unwrap();
        assert!(result.test.matched);
        assert_eq!(
            result.after.values("X-Bytes").next(),
            Some(&[0x80, 0xff][..])
        );
        rule.matcher.response_status = Some(200);
        assert!(test_header_rule(&rule, &request, None).is_err());
    }
    #[test]
    fn transport_framing_and_coding_are_rejected_for_every_operation() {
        for name in [
            "Host",
            "Content-Length",
            "TRANSFER-ENCODING",
            "Content-Encoding",
            "Connection",
            "Proxy-Connection",
            "Keep-Alive",
            "TE",
            "Trailer",
            "Upgrade",
        ] {
            for action in [
                HeaderOperation::set(name, "x").unwrap(),
                HeaderOperation::append(name, "x").unwrap(),
                HeaderOperation::remove(name).unwrap(),
            ] {
                assert!(
                    crate::compile(
                        vec![rule(HeaderRulePhase::Request, vec![action.clone()])],
                        crate::AutomationLimits::default()
                    )
                    .is_err()
                );
                assert!(
                    crate::compile(
                        vec![rule(HeaderRulePhase::Response, vec![action])],
                        crate::AutomationLimits::default()
                    )
                    .is_err()
                );
            }
        }
        for name in [
            "Cache-Control",
            "Content-Type",
            "ETag",
            "Access-Control-Allow-Origin",
            "Content-Security-Policy",
        ] {
            assert!(
                crate::compile(
                    vec![rule(
                        HeaderRulePhase::Response,
                        vec![HeaderOperation::remove(name).unwrap()]
                    )],
                    crate::AutomationLimits::default()
                )
                .is_ok()
            );
        }
    }
}

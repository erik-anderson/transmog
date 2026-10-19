use crate::{Error, invalid, model as wire};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use transmog_automation as native;

fn query(value: &native::QueryCondition) -> wire::Query {
    match value {
        native::QueryCondition::Exact(value) => wire::Query::Exact(value.clone()),
        native::QueryCondition::Ignore => wire::Query::Ignore,
        native::QueryCondition::Parameters(values) => wire::Query::Parameters(
            values
                .iter()
                .map(|v| wire::Parameter {
                    name: v.name.clone(),
                    value: v.value.clone(),
                })
                .collect(),
        ),
    }
}
fn address(value: &native::UrlCondition) -> wire::Address {
    match value {
        native::UrlCondition::Exact(value) => wire::Address::Exact(value.clone()),
        native::UrlCondition::Pattern(value) => wire::Address::Pattern(wire::Pattern {
            address: value.address.clone(),
            query: query(&value.query),
            case_sensitive: value.case_sensitive,
        }),
        native::UrlCondition::Regex(value) => wire::Address::Regex(wire::Regex {
            pattern: value.pattern.clone(),
            scope: match value.scope {
                native::RegexScope::Path => "path",
                native::RegexScope::Url => "url",
            }
            .into(),
            whole: value.whole,
            case_sensitive: value.case_sensitive,
            query: value.query.as_ref().map(query),
        }),
    }
}
fn predicate(value: &native::HeaderPredicate) -> wire::Predicate {
    let (kind, content, pattern) = match &value.condition {
        native::HeaderCondition::Exists => ("exists", None, None),
        native::HeaderCondition::Exact(value) => ("exact", Some(STANDARD.encode(value)), None),
        native::HeaderCondition::Prefix(value) => ("prefix", Some(STANDARD.encode(value)), None),
        native::HeaderCondition::Suffix(value) => ("suffix", Some(STANDARD.encode(value)), None),
        native::HeaderCondition::Regex(value) => ("byte-regex", None, Some(value.clone())),
    };
    wire::Predicate {
        name: value.name.clone(),
        kind: kind.into(),
        value_base64: content,
        pattern,
    }
}
fn matcher(value: &native::RuleMatcher) -> wire::Matcher {
    wire::Matcher {
        method: value.method.clone(),
        address: value.url.as_ref().map(address),
        scheme: value.scheme.clone(),
        host: value.host.clone(),
        port: value.port,
        path_prefix: value.path_prefix.clone(),
        query: value.query.clone(),
        request_headers: value.request_headers.iter().map(predicate).collect(),
        response_headers: value.response_headers.iter().map(predicate).collect(),
        response_status: value.response_status,
        response_status_class: value.response_status_class,
        examples: value
            .examples
            .iter()
            .map(|example| wire::Example {
                method: example.method.clone(),
                url: example.url.clone(),
                headers: example
                    .headers
                    .iter()
                    .map(|h| wire::Header {
                        name: h.name.clone(),
                        value_base64: STANDARD.encode(h.value.as_bytes()),
                    })
                    .collect(),
                expected: if example.expected {
                    "match"
                } else {
                    "no-match"
                }
                .into(),
            })
            .collect(),
    }
}
fn operation(value: &native::HeaderOperation) -> wire::Operation {
    let (operation, field) = match value {
        native::HeaderOperation::Remove(name) => {
            return wire::Operation {
                operation: "remove".into(),
                name: name.clone(),
                value_base64: None,
            };
        }
        native::HeaderOperation::Set(field) => ("set", field),
        native::HeaderOperation::OverrideIfPresent(field) => ("override-if-present", field),
        native::HeaderOperation::Append(field) => ("append", field),
    };
    wire::Operation {
        operation: operation.into(),
        name: String::from_utf8_lossy(field.name()).into_owned(),
        value_base64: Some(STANDARD.encode(field.value())),
    }
}
/// Exports exactly a supported rule; advanced mixed actions fail rather than disappear.
/// # Errors
/// Rejects unsupported action combinations or invalid portable identities.
pub fn from_native(value: &native::Rule) -> Result<wire::Rule, Error> {
    let (kind, evaluation, action) = if let Some(reference) = &value.request.response_asset {
        let (id, revision) = reference
            .rsplit_once('@')
            .ok_or_else(|| invalid("Invalid native asset reference"))?;
        (
            "auto-response",
            "original-client-request",
            serde_json::to_value(wire::AutoResponse {
                asset_id: id.into(),
                revision: revision.into(),
            })?,
        )
    } else if let Some(phase) = native::header_rule_phase(value) {
        let (evaluation, operations) = match phase {
            native::HeaderRulePhase::Request => ("effective-request-head", &value.request.headers),
            native::HeaderRulePhase::Response => {
                ("effective-response-head", &value.response.headers)
            }
        };
        (
            "header-edit",
            evaluation,
            serde_json::to_value(wire::HeaderEdit {
                operations: operations.iter().map(operation).collect(),
            })?,
        )
    } else if native::is_text_transform_rule(value) {
        let action = value
            .response
            .text_transform
            .as_ref()
            .ok_or_else(|| invalid("Missing transform"))?;
        (
            "literal-response",
            "final-effective-response-heads",
            serde_json::to_value(wire::Literal {
                find: action.find.clone(),
                replacement: action.replacement.clone(),
                occurrences: match action.occurrences {
                    native::Occurrences::First => "first",
                    native::Occurrences::All => "all",
                }
                .into(),
                expected_matches: action.expected_matches.map(|v| v.to_string()),
                media_type: action.media_type.clone(),
                encoding: action.encoding.clone(),
                max_bytes: action.max_bytes.to_string(),
            })?,
        )
    } else {
        return Err(Error::Unsupported(format!(
            "Native rule {} has no portable action",
            value.id
        )));
    };
    let result = wire::Rule {
        id: value.id.clone(),
        revision: value.revision.to_string(),
        label: value.display_name.clone().unwrap_or_else(|| "Rule".into()),
        enabled_intent: value.enabled,
        priority: Some(value.priority),
        kind: kind.into(),
        evaluation: evaluation.into(),
        matcher: matcher(&value.matcher),
        action,
    };
    let mut normalized = value.clone();
    normalized.display_name = Some(result.label.clone());
    if super::to_native(&result, value.priority)? != normalized {
        return Err(Error::Unsupported(format!(
            "Native rule {} mixes unsupported actions",
            value.id
        )));
    }
    Ok(result)
}

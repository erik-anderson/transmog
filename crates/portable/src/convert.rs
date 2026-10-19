use crate::{Error, invalid, model as wire};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use transmog_automation as native;
use transmog_core::{HeaderBlock, HeaderField};

pub(crate) fn bytes(value: &str) -> Result<Vec<u8>, Error> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| invalid("Invalid padded Base64"))?;
    if STANDARD.encode(&decoded) != value {
        return Err(invalid("Noncanonical padded Base64"));
    }
    Ok(decoded)
}
pub(crate) fn headers(values: &[wire::Header]) -> Result<HeaderBlock, Error> {
    let mut result = HeaderBlock::new();
    if values.len() > 256 {
        return Err(invalid("Too many response headers"));
    }
    let mut count = 0;
    for value in values {
        let decoded = bytes(&value.value_base64)?;
        count += value.name.len() + decoded.len();
        if count > 64 * 1024 {
            return Err(invalid("Response headers exceed 64 KiB"));
        }
        result.push(
            HeaderField::try_new(value.name.as_bytes(), decoded)
                .map_err(|_| invalid("Invalid HTTP field"))?,
        );
    }
    Ok(result)
}
/// Preserves ordered duplicate and binary header values.
pub fn from_headers(values: &HeaderBlock) -> Vec<wire::Header> {
    values
        .iter()
        .map(|value| wire::Header {
            name: String::from_utf8_lossy(value.name()).into_owned(),
            value_base64: STANDARD.encode(value.value()),
        })
        .collect()
}
pub(crate) fn predicate(value: &wire::Predicate) -> Result<native::HeaderPredicate, Error> {
    let condition = match value.kind.as_str() {
        "exists" if value.value_base64.is_none() && value.pattern.is_none() => {
            native::HeaderCondition::Exists
        }
        "byte-regex" if value.value_base64.is_none() => native::HeaderCondition::Regex(
            value
                .pattern
                .clone()
                .ok_or_else(|| invalid("Missing header regex"))?,
        ),
        kind if value.pattern.is_none() => {
            let content = bytes(
                value
                    .value_base64
                    .as_deref()
                    .ok_or_else(|| invalid("Missing header predicate bytes"))?,
            )?;
            match kind {
                "exact" => native::HeaderCondition::Exact(content),
                "prefix" => native::HeaderCondition::Prefix(content),
                "suffix" => native::HeaderCondition::Suffix(content),
                _ => return Err(Error::Unsupported("header predicate".into())),
            }
        }
        _ => return Err(invalid("Inconsistent header predicate fields")),
    };
    Ok(native::HeaderPredicate {
        name: value.name.clone(),
        condition,
    })
}
fn query(value: &wire::Query) -> native::QueryCondition {
    match value {
        wire::Query::Exact(value) => native::QueryCondition::Exact(value.clone()),
        wire::Query::Ignore => native::QueryCondition::Ignore,
        wire::Query::Parameters(values) => native::QueryCondition::Parameters(
            values
                .iter()
                .map(|v| native::QueryParameter {
                    name: v.name.clone(),
                    value: v.value.clone(),
                })
                .collect(),
        ),
    }
}
fn address(value: &wire::Address) -> Result<native::UrlCondition, Error> {
    Ok(match value {
        wire::Address::Exact(value) => native::UrlCondition::Exact(value.clone()),
        wire::Address::Pattern(value) => native::UrlCondition::Pattern(native::UrlPattern {
            address: value.address.clone(),
            query: query(&value.query),
            case_sensitive: value.case_sensitive,
        }),
        wire::Address::Regex(value) => native::UrlCondition::Regex(native::UrlRegex {
            pattern: value.pattern.clone(),
            scope: match value.scope.as_str() {
                "url" => native::RegexScope::Url,
                "path" => native::RegexScope::Path,
                _ => return Err(invalid("Invalid regex scope")),
            },
            whole: value.whole,
            case_sensitive: value.case_sensitive,
            query: value.query.as_ref().map(query),
        }),
    })
}
pub(crate) fn matcher(value: &wire::Matcher) -> Result<native::RuleMatcher, Error> {
    let examples = value
        .examples
        .iter()
        .map(|example| {
            let headers = example
                .headers
                .iter()
                .map(|h| {
                    Ok(native::ExampleHeader {
                        name: h.name.clone(),
                        value: String::from_utf8(bytes(&h.value_base64)?).map_err(|_| {
                            Error::Unsupported(
                                "Binary matcher example; native examples require UTF-8".into(),
                            )
                        })?,
                    })
                })
                .collect::<Result<_, Error>>()?;
            Ok(native::MatchExample {
                method: example.method.clone(),
                url: example.url.clone(),
                expected: match example.expected.as_str() {
                    "match" => true,
                    "no-match" => false,
                    _ => return Err(invalid("Invalid match expectation")),
                },
                headers,
            })
        })
        .collect::<Result<_, Error>>()?;
    Ok(native::RuleMatcher {
        method: value.method.clone(),
        url: value.address.as_ref().map(address).transpose()?,
        scheme: value.scheme.clone(),
        host: value.host.clone(),
        port: value.port,
        path_prefix: value.path_prefix.clone(),
        query: value.query.clone(),
        request_headers: value
            .request_headers
            .iter()
            .map(predicate)
            .collect::<Result<_, _>>()?,
        response_headers: value
            .response_headers
            .iter()
            .map(predicate)
            .collect::<Result<_, _>>()?,
        response_status: value.response_status,
        response_status_class: value.response_status_class,
        examples,
    })
}

/// Translate one explicitly supported action into the production rule model.
/// # Errors
/// Rejects unsupported semantics, invalid byte encodings or integer ranges.
pub fn to_native(value: &wire::Rule, position: i32) -> Result<native::Rule, Error> {
    if !crate::valid_id(&value.id) {
        return Err(invalid("Invalid rule ID"));
    }
    let mut result = native::Rule {
        id: value.id.clone(),
        revision: crate::decimal(&value.revision, false)?,
        display_name: Some(value.label.clone()),
        enabled: value.enabled_intent,
        priority: value.priority.unwrap_or(position),
        matcher: matcher(&value.matcher)?,
        request: native::RequestActions::default(),
        response: native::ResponseActions::default(),
    };
    match (value.kind.as_str(), value.evaluation.as_str()) {
        ("auto-response", "original-client-request") => {
            let action: wire::AutoResponse = serde_json::from_value(value.action.clone())?;
            if !crate::valid_id(&action.asset_id) {
                return Err(invalid("Invalid asset reference ID"));
            }
            crate::decimal(&action.revision, false)?;
            result.request.response_asset =
                Some(format!("{}@{}", action.asset_id, action.revision));
        }
        ("header-edit", phase @ ("effective-request-head" | "effective-response-head")) => {
            if value.priority.is_none() {
                return Err(invalid("Header rules require priority"));
            }
            let action: wire::HeaderEdit = serde_json::from_value(value.action.clone())?;
            let operations = action
                .operations
                .iter()
                .map(operation)
                .collect::<Result<_, _>>()?;
            if phase == "effective-request-head" {
                result.request.headers = operations;
            } else {
                result.response.headers = operations;
            }
        }
        ("literal-response", "final-effective-response-heads") => {
            if value.priority.is_none() {
                return Err(invalid("Literal rules require priority"));
            }
            let action: wire::Literal = serde_json::from_value(value.action.clone())?;
            result.response.text_transform = Some(native::LiteralTransform {
                find: action.find,
                replacement: action.replacement,
                occurrences: match action.occurrences.as_str() {
                    "first" => native::Occurrences::First,
                    "all" => native::Occurrences::All,
                    _ => return Err(invalid("Invalid occurrence choice")),
                },
                expected_matches: action
                    .expected_matches
                    .as_deref()
                    .map(|s| {
                        usize::try_from(crate::decimal(s, true)?)
                            .map_err(|_| invalid("Match count exceeds platform range"))
                    })
                    .transpose()?,
                media_type: action.media_type,
                encoding: action.encoding,
                max_bytes: usize::try_from(crate::decimal(&action.max_bytes, false)?)
                    .map_err(|_| invalid("Body bound exceeds platform range"))?,
            });
        }
        _ => {
            return Err(Error::Unsupported(format!(
                "Rule kind/evaluation {}/{}",
                value.kind, value.evaluation
            )));
        }
    }
    Ok(result)
}
fn operation(value: &wire::Operation) -> Result<native::HeaderOperation, Error> {
    let fail = |_| invalid("Invalid header operation");
    match value.operation.as_str() {
        "remove" if value.value_base64.is_none() => {
            native::HeaderOperation::remove(value.name.as_bytes()).map_err(fail)
        }
        "set" | "override-if-present" | "append" => {
            let content = bytes(
                value
                    .value_base64
                    .as_deref()
                    .ok_or_else(|| invalid("Missing operation value"))?,
            )?;
            if value.operation == "set" {
                native::HeaderOperation::set(value.name.as_bytes(), content).map_err(fail)
            } else if value.operation == "override-if-present" {
                native::HeaderOperation::override_if_present(value.name.as_bytes(), content)
                    .map_err(fail)
            } else {
                native::HeaderOperation::append(value.name.as_bytes(), content).map_err(fail)
            }
        }
        _ => Err(invalid("Unknown header operation or remove value")),
    }
}

#[path = "export.rs"]
mod export;
pub use export::from_native;

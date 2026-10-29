//! Small authoring models translated into native, revision-checked rules.
use super::*;
use transmog_automation::{
    HeaderOperation, LiteralTransform, Occurrences, RequestActions, ResponseActions, RuleMatcher,
};
use transmog_core::HeaderField;

impl AgentApplication {
    pub(super) fn author_rule(draft: &Value, previous: Option<&Rule>) -> Result<Rule, Error> {
        let mut rule = previous.cloned().unwrap_or_else(|| Rule {
            id: String::new(),
            display_name: None,
            enabled: false,
            revision: 1,
            priority: 0,
            matcher: RuleMatcher::default(),
            request: RequestActions::default(),
            response: ResponseActions::default(),
        });
        rule.display_name = Some(string(draft, "name")?.into());
        rule.matcher.host = Some(string(draft, "host")?.into());
        if draft.get("method").is_some() {
            rule.matcher.method = draft["method"].as_str().map(str::to_owned);
        }
        if draft.get("pathPrefix").is_some() {
            rule.matcher.path_prefix = draft["pathPrefix"].as_str().map(str::to_owned);
        }
        rule.request = RequestActions::default();
        rule.response = ResponseActions::default();
        match string(draft, "kind")? {
            "headers" => {
                let phase = draft["phase"].as_str().unwrap_or("request");
                let mut operations = Vec::new();
                for operation in draft["operations"]
                    .as_array()
                    .ok_or_else(|| invalid("rule.operations is required for header rules."))?
                {
                    let name = string(operation, "name")?;
                    let action = string(operation, "operation")?;
                    if action == "remove" {
                        if operation.get("value").is_some() {
                            return Err(invalid("rule.operations: remove must omit value."));
                        }
                        operations.push(HeaderOperation::Remove(name.into()));
                    } else {
                        let field = HeaderField::try_new(
                            name.as_bytes(),
                            operation["value"]
                                .as_str()
                                .ok_or_else(|| {
                                    invalid("rule.operations.value is required for this operation.")
                                })?
                                .as_bytes(),
                        )
                        .map_err(|_| {
                            invalid("rule.operations: invalid HTTP header name or value.")
                        })?;
                        operations.push(match action {
                            "set" => HeaderOperation::Set(field),
                            "append" => HeaderOperation::Append(field),
                            "override-if-present" => HeaderOperation::OverrideIfPresent(field),
                            _ => return Err(invalid("rule.operations.operation is unsupported.")),
                        });
                    }
                }
                if phase == "response" {
                    rule.response.headers = operations;
                } else {
                    rule.request.headers = operations;
                }
            }
            "response-text" => {
                rule.response.text_transform = Some(LiteralTransform {
                    find: draft["find"]
                        .as_str()
                        .ok_or_else(|| invalid("rule.find is required."))?
                        .into(),
                    replacement: draft["replacement"]
                        .as_str()
                        .ok_or_else(|| invalid("rule.replacement is required."))?
                        .into(),
                    occurrences: if draft["occurrences"] == "first" {
                        Occurrences::First
                    } else {
                        Occurrences::All
                    },
                    expected_matches: draft["expectedMatches"]
                        .as_u64()
                        .map(usize::try_from)
                        .transpose()
                        .map_err(|_| {
                            invalid("rule.expectedMatches exceeds this platform limit.")
                        })?,
                    media_type: string(draft, "mediaType")?.into(),
                    encoding: "utf-8".into(),
                    max_bytes: index(draft, "maxBytes", 1_048_576, 1_048_576)?,
                });
            }
            _ => return Err(invalid("rule.kind is unsupported.")),
        }
        Ok(rule)
    }
}

pub(super) fn definition(rule: &Rule) -> Option<Value> {
    let common = json!({"name":rule.display_name,"host":rule.matcher.host,"method":rule.matcher.method,"pathPrefix":rule.matcher.path_prefix});
    let mut result = common;
    if let Some(transform) = &rule.response.text_transform {
        result["kind"] = json!("response-text");
        for (name, value) in [
            ("find", json!(transform.find)),
            ("replacement", json!(transform.replacement)),
            ("occurrences", json!(transform.occurrences)),
            ("expectedMatches", json!(transform.expected_matches)),
            ("mediaType", json!(transform.media_type)),
            ("maxBytes", json!(transform.max_bytes)),
        ] {
            result[name] = value;
        }
    } else if transmog_automation::header_rule_phase(rule).is_some() {
        let response = !rule.response.headers.is_empty();
        result["kind"] = json!("headers");
        result["phase"] = json!(if response { "response" } else { "request" });
        let operations = if response {
            &rule.response.headers
        } else {
            &rule.request.headers
        };
        let mut values = Vec::new();
        for operation in operations {
            let (action, field) = match operation {
                HeaderOperation::Remove(name) => {
                    values.push(json!({"operation":"remove","name":name}));
                    continue;
                }
                HeaderOperation::Set(field) => ("set", field),
                HeaderOperation::Append(field) => ("append", field),
                HeaderOperation::OverrideIfPresent(field) => ("override-if-present", field),
            };
            values.push(json!({"operation":action,"name":std::str::from_utf8(field.name()).ok()?,"value":std::str::from_utf8(field.value()).ok()?}));
        }
        result["operations"] = json!(values);
    } else {
        return None;
    }
    Some(result)
}

pub(super) fn sample_headers(sample: &mut Value) -> Result<(), Error> {
    for name in ["requestHeaders", "responseHeaders"] {
        let mut block = transmog_core::HeaderBlock::new();
        if let Some(fields) = sample[name].as_array() {
            for field in fields {
                block.push(
                    HeaderField::try_new(
                        string(field, "name")?.as_bytes(),
                        field["value"]
                            .as_str()
                            .ok_or_else(|| invalid("sample header value must be text."))?
                            .as_bytes(),
                    )
                    .map_err(|_| invalid("sample contains an invalid HTTP header."))?,
                );
            }
        }
        sample[name] = value(block)?;
    }
    Ok(())
}

pub(super) fn test_headers(headers: &transmog_core::HeaderBlock, grant: &Grant) -> Value {
    json!(headers.iter().map(|field| {
        let name=String::from_utf8_lossy(field.name());
        if sensitive(&name) && !grant.allows(Permission::ReadCredentials) { return json!({"name":name,"value":"[redacted]","encoding":"utf-8","sensitive":true}); }
        match std::str::from_utf8(field.value()) {
            Ok(value)=>json!({"name":name,"value":value,"encoding":"utf-8","sensitive":field.is_redacted()}),
            Err(_)=>json!({"name":name,"value":STANDARD.encode(field.value()),"encoding":"base64","sensitive":field.is_redacted()})
        }
    }).collect::<Vec<_>>())
}

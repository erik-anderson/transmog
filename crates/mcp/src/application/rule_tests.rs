//! Native rule evaluation against full retained evidence or explicit samples.
use super::*;
impl AgentApplication {
    pub(super) async fn test_rule(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        let readable = self.read_rule(grant, string(input, "id")?)?;
        if readable["rule"].is_null() {
            return Err(denied(
                "The full rule definition is unavailable under this key's disclosure permissions.",
            ));
        }
        let rule: Rule = parse(readable["rule"].clone())?;
        let captured = input["sampleExchangeId"].as_str();
        if captured.is_some() == input.get("sample").is_some() {
            return Err(invalid("Supply exactly one of sample or sampleExchangeId."));
        }
        if captured.is_some() && input.get("body").is_some() {
            return Err(invalid(
                "Captured tests read the selected retained response body; do not supply body.",
            ));
        }
        let request_boundary = input["requestBoundary"]
            .as_str()
            .unwrap_or("client-request");
        let response_boundary = input["responseBoundary"]
            .as_str()
            .unwrap_or("client-response");
        let mut required = Vec::new();
        let sample = if let Some(id) = captured {
            let (sample, permissions) =
                self.captured_rule_sample(grant, id, request_boundary, response_boundary, &rule)?;
            required.extend(permissions);
            sample
        } else {
            let mut sample = input["sample"].clone();
            sample["rule"] = value(rule.clone())?;
            authoring::sample_headers(&mut sample)?;
            if sample.get("responseStatus").is_none() {
                sample["responseStatus"] = json!(200);
            }
            parse(sample)?
        };
        if captured.is_none() && !grant.scope.allows_url(&sample.url) {
            return Err(denied(
                "The sample URL is outside this key's destination scope.",
            ));
        }
        let url = if let Some(id) = captured {
            self.application
                .copy_message_url(id, "client-request")
                .map_err(app_error)?
        } else {
            sample.url.clone()
        };
        if grant.allows(Permission::ReadCredentials) {
            required.push(Permission::ReadCredentials);
        }
        if transmog_automation::is_text_transform_rule(&rule) {
            required.push(Permission::ReadBodies);
        }
        let source = json!({"kind":"rule-test","sampleExchangeId":captured,"requestBoundary":request_boundary,"responseBoundary":response_boundary});
        let mut result = self
            .evaluate_rule_sample(
                grant,
                input,
                &rule,
                sample,
                captured.map(|id| (id, response_boundary)),
            )
            .await?;
        let reference = self.context_json(
            grant,
            &result,
            required,
            source.clone(),
            url,
            captured.map(str::to_owned),
        )?;
        let complete = serde_json::to_vec(&result).is_ok_and(|bytes| bytes.len() <= 128 * 1024);
        if !complete {
            result = json!({"kind":result["kind"]});
        }
        result["context"] = reference;
        result["inlineComplete"] = json!(complete);
        result["source"] = source;
        result["ruleId"] = json!(rule.id);
        result["revision"] = json!(rule.revision);
        Ok(result)
    }
    async fn evaluate_rule_sample(
        &self,
        grant: &Grant,
        input: &Value,
        rule: &Rule,
        sample: HeaderRuleTestInput,
        captured: Option<(&str, &str)>,
    ) -> Result<Value, Error> {
        if transmog_automation::is_text_transform_rule(rule) {
            let body = if let Some((id, response_boundary)) = captured {
                grant.require(Permission::ReadBodies)?;
                let body = self
                    .application
                    .complete_body_context(id, response_boundary)
                    .await
                    .map_err(app_error)?;
                if body.text_encoding.is_none() {
                    return Err(invalid(
                        "A response-text rule requires a complete text body.",
                    ));
                }
                String::from_utf8(body.bytes)
                    .map_err(|_| invalid("The decoded body is not UTF-8."))?
            } else {
                input["body"].as_str().unwrap_or_default().to_owned()
            };
            let mut result = value(
                self.application
                    .test_response_transform(&TextTransformTestInput { sample, body })
                    .map_err(app_error)?,
            )?;
            result["kind"] = json!("response-text");
            Ok(result)
        } else {
            let result = self
                .application
                .test_header_rule(&sample)
                .map_err(app_error)?;
            Ok(
                json!({"kind":"headers","test":result.test,"before":authoring::test_headers(&result.before,grant),"after":authoring::test_headers(&result.after,grant),"operations":result.operations}),
            )
        }
    }
    fn captured_rule_sample(
        &self,
        grant: &Grant,
        id: &str,
        request_boundary: &str,
        response_boundary: &str,
        rule: &Rule,
    ) -> Result<(HeaderRuleTestInput, Vec<Permission>), Error> {
        let mut required = Vec::new();
        let detail = self.entry(grant, &json!({"instanceId":self.instance_id,"id":id}), "id")?;
        let (method, url) = self
            .application
            .message_request(id, request_boundary)
            .map_err(app_error)?;
        let mut request_headers = self
            .application
            .message_headers(id, request_boundary)
            .map_err(app_error)?;
        let mut response_headers = self
            .application
            .message_headers(id, response_boundary)
            .map_err(app_error)?;
        if grant.allows(Permission::ReadCredentials) {
            required.push(Permission::ReadCredentials);
        } else {
            let redact = |headers: transmog_core::HeaderBlock| {
                transmog_core::HeaderBlock::from_fields(
                    headers
                        .iter()
                        .cloned()
                        .map(|mut field| {
                            if sensitive(&String::from_utf8_lossy(field.name())) {
                                field.redact_value();
                            }
                            field
                        })
                        .collect(),
                )
            };
            request_headers = redact(request_headers);
            response_headers = redact(response_headers);
        }
        let status = value(detail)?["responses"]
            .as_array()
            .and_then(|heads| {
                heads
                    .iter()
                    .find(|head| head["boundary"] == response_boundary)
            })
            .and_then(|head| head["status"].as_u64())
            .and_then(|status| u16::try_from(status).ok())
            .ok_or_else(|| invalid("The response boundary is unavailable."))?;
        required.push(Permission::Inspect);
        let sample = HeaderRuleTestInput {
            rule: rule.clone(),
            method,
            url,
            request_headers,
            response_headers,
            response_status: status,
        };
        Ok((sample, required))
    }
}

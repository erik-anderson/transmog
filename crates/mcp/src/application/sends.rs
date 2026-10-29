//! Send admission and durable outcomes within one app run.
use super::*;
pub(super) struct SendRecord {
    fingerprint: Vec<u8>,
    url: String,
    outcome: Option<Result<Value, Error>>,
}
struct Admission<'a> {
    app: &'a AgentApplication,
    identity: (String, String),
}
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if let Some(record) = self.app.lock().sends.get_mut(&self.identity)
            && record.outcome.is_none()
        {
            record.outcome = Some(Err(Error::new(
                "outcome_unknown",
                "The admitted send was interrupted. Inspect traffic before deciding whether to send a new operation.",
            )));
        }
    }
}
impl AgentApplication {
    pub(super) fn send_status(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        grant.require(Permission::Send)?;
        let operation = string(input, "operationId")?;
        let runtime = self.lock();
        let record = runtime
            .sends
            .get(&(grant.id.clone(), operation.into()))
            .ok_or_else(|| {
                Error::new(
                    "not_found",
                    "This key has no admitted send with that operationId.",
                )
            })?;
        if !grant.scope.allows_url(&record.url) {
            return Err(denied(
                "The send destination is outside this key's current scope.",
            ));
        }
        Ok(match &record.outcome {
            None => json!({"operationId":operation,"state":"pending","result":null,"error":null}),
            Some(Ok(result)) => {
                json!({"operationId":operation,"state":"completed","result":result,"error":null})
            }
            Some(Err(error)) => {
                json!({"operationId":operation,"state":"outcome-unknown","result":null,"error":error})
            }
        })
    }
    pub(super) async fn send(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        grant.require(Permission::Send)?;
        let request: ComposerRequest = parse(input["request"].clone())?;
        if !grant.scope.allows_url(&request.url) {
            return Err(denied(
                "The request destination is outside this key's scope.",
            ));
        }
        if matches!(request.body_source, Some(ComposerBodySource::File { .. })) {
            return Err(denied(
                "Agent sends cannot read local files. Supply an inline body or authorized captured body.",
            ));
        }
        if let Some(ComposerBodySource::Captured { entry_id }) = &request.body_source {
            grant.require(Permission::ReadBodies)?;
            self.entry(
                grant,
                &json!({"instanceId":self.instance_id,"id":entry_id}),
                "id",
            )?;
        }
        if let Some(id) = &request.source_entry_id {
            self.entry(grant, &json!({"instanceId":self.instance_id,"id":id}), "id")?;
        }

        let operation = string(input, "operationId")?.to_owned();
        let identity = (grant.id.clone(), operation.clone());
        let live = input["mode"] == "live-rules";
        let fingerprint = Sha256::digest(
            serde_json::to_vec(&(live, &request))
                .map_err(|_| invalid("The request could not be encoded."))?,
        )
        .to_vec();
        {
            let runtime = self.lock();
            if let Some(record) = runtime.sends.get(&identity) {
                if record.fingerprint != fingerprint {
                    return Err(Error::new(
                        "conflict",
                        "That operationId was already used for a different request or mode.",
                    ));
                }
                return record.outcome.clone().unwrap_or_else(|| {
                    Err(Error::new(
                        "pending",
                        "This operation is still running. Use transmog_request_status.",
                    ))
                });
            }
        }
        self.application
            .validate_agent_request(&request, live)
            .map_err(app_error)?;
        {
            let mut runtime = self.lock();
            // Admission is atomic even when two callers pass preflight concurrently.
            if let Some(record) = runtime.sends.get(&identity) {
                if record.fingerprint != fingerprint {
                    return Err(Error::new(
                        "conflict",
                        "That operationId was concurrently used for a different request or mode.",
                    ));
                }
                return record.outcome.clone().unwrap_or_else(|| {
                    Err(Error::new(
                        "pending",
                        "This operation was concurrently admitted. Use transmog_request_status.",
                    ))
                });
            }
            if runtime.sends.len() >= 1024 {
                return Err(Error::new(
                    "limit",
                    "The send ledger is full for this app run; existing operation IDs remain protected.",
                ));
            }
            runtime.sends.insert(
                identity.clone(),
                SendRecord {
                    fingerprint,
                    url: request.url.clone(),
                    outcome: None,
                },
            );
        }
        let admission = Admission {
            app: self,
            identity: identity.clone(),
        };
        let outcome=self.application.execute_agent_request(request,live).await.map(|result|json!({"operationId":operation,"instanceId":self.instance_id,"id":result.detail.id,"status":result.status,"mode":if live{"live-rules"}else{"direct"},"attribution":result.attribution,"inspectWith":"transmog_exchange_get"})).map_err(|error|Error::new("outcome_unknown",format!("The send was admitted; it will not be resent. {}",app_error(error).message)));
        self.lock()
            .sends
            .get_mut(&identity)
            .expect("admitted record")
            .outcome = Some(outcome.clone());
        drop(admission);
        self.activity(grant, "transmog_request_send", outcome.is_ok());
        outcome
    }
}

//! Admission and cleanup for independently owned app-specific listeners.

use super::*;
use std::sync::OnceLock;
use transmog_app::{CaCreateRequest, ProxyStartRequest};

const MAX_RECEIPTS_PER_KEY_AND_KIND: usize = 32;
const RECEIPT_AGE: Duration = Duration::from_hours(1);
const START_LISTENER: &str = "transmog_session_listen";
const STOP_LISTENER: &str = "transmog_session_listener_stop";

#[derive(Clone)]
pub(super) struct Receipt {
    session_id: String,
    ended: Arc<OnceLock<EndedSession>>,
}

struct EndedSession {
    removed_at: Instant,
    summary: Value,
}

impl Receipt {
    pub(super) fn new(session_id: String) -> Self {
        Self {
            session_id,
            ended: Arc::default(),
        }
    }

    fn for_record(record: &Record) -> Self {
        record
            .listener_receipt
            .clone()
            .expect("private listeners have receipt state")
    }

    pub(super) fn archive(&self, record: &Record) {
        self.ended.get_or_init(|| {
            let mut summary = record.summary();
            hide_listener_address(&mut summary);
            EndedSession {
                removed_at: Instant::now(),
                summary,
            }
        });
    }
}

fn hide_listener_address(summary: &mut Value) {
    summary["agentAccessEnded"] = json!(true);
    summary["listener"]["endpoint"] = Value::Null;
    summary["listener"]["proxyUrl"] = Value::Null;
    summary["listener"]["certificate"] = Value::Null;
}

struct Completion {
    finished_at: Instant,
    result: Result<Receipt, Error>,
}

pub(super) struct Operation {
    name: String,
    input: Value,
    generation: u64,
    completion: tokio::sync::watch::Sender<Option<Completion>>,
}

impl Operation {
    fn expired_at(&self, now: Instant) -> bool {
        let completion = self.completion.borrow();
        let Some(completion) = completion.as_ref() else {
            return false;
        };
        let since = match &completion.result {
            Ok(receipt) if self.name == START_LISTENER => {
                let Some(ended) = receipt.ended.get() else {
                    return false;
                };
                ended.removed_at
            }
            _ => completion.finished_at,
        };
        now.saturating_duration_since(since) >= RECEIPT_AGE
    }
}

impl State {
    pub(super) fn prune_listener_operations(&mut self, access: &AccessManager) {
        if self.listener_operations.is_empty() {
            return;
        }
        // Revocation is permanent. Dropping these receipts bounds storage across key rotation.
        let owners = access.unrevoked_key_ids();
        let now = Instant::now();
        self.listener_operations
            .retain(|(owner, _), op| owners.contains(owner) && !op.expired_at(now));
    }
}

fn reserve_listener_receipt(
    operations: &mut HashMap<(String, String), Arc<Operation>>,
    owner: &str,
    name: &str,
) -> Result<(), Error> {
    let owned_history = operations
        .iter()
        .filter(|((key_id, _), operation)| key_id == owner && operation.name == name);
    if owned_history.clone().count() < MAX_RECEIPTS_PER_KEY_AND_KIND {
        return Ok(());
    }
    // A completed stop can be repeated safely because a session ID never
    // acquires a replacement listener. Keep every pending operation.
    if name == STOP_LISTENER
        && let Some(oldest) = owned_history
            .filter_map(|(key, operation)| {
                let finished_at = operation.completion.borrow().as_ref()?.finished_at;
                Some((key, finished_at))
            })
            .min_by_key(|(_, finished_at)| *finished_at)
            .map(|(key, _)| key.clone())
    {
        operations.remove(&oldest);
        return Ok(());
    }
    Err(Error::new(
        "limit",
        "This key reached its listener operation receipt limit. Retry after pending work finishes or receipts expire.",
    ))
}

pub(super) fn authorized(grant: &Grant) -> bool {
    // Selected request IDs name shared traffic. Independent runtimes can reuse
    // local exchange numbers, so those grants cannot address a private realm.
    grant.allows(Permission::CollectPrivate) && !matches!(grant.scope, Scope::Requests { .. })
}

impl AgentApplication {
    pub(super) async fn listener_operation(
        self: &Arc<Self>,
        grant: &Grant,
        name: &str,
        input: &Value,
    ) -> Result<Value, Error> {
        grant.require(Permission::CollectPrivate)?;
        if !authorized(grant) {
            return Err(denied(
                "Private listeners require an all-traffic or host-scoped key.",
            ));
        }
        let key = (grant.id.clone(), string(input, "operationId")?.to_owned());
        let mut completion = {
            let mut registry = self.sessions.lock();
            if registry.closing {
                return Err(Error::new(
                    "unavailable",
                    "The MCP server is shutting down.",
                ));
            }
            if registry.jobs.contains_key(&key) {
                return Err(Error::new(
                    "conflict",
                    "That operationId already names a capture operation.",
                ));
            }
            registry.prune_listener_operations(&self.access);
            if let Some(operation) = registry.listener_operations.get(&key) {
                if operation.name != name || &operation.input != input {
                    return Err(Error::new(
                        "conflict",
                        "Use a new operationId for different listener arguments.",
                    ));
                }
                operation.completion.subscribe()
            } else {
                self.check_listener_grant(grant)?;
                reserve_listener_receipt(&mut registry.listener_operations, &grant.id, name)?;
                let (completion, receiver) = tokio::sync::watch::channel(None);
                let operation = Arc::new(Operation {
                    name: name.into(),
                    input: input.clone(),
                    generation: registry.generation,
                    completion,
                });
                registry.listener_operations.insert(key, operation.clone());
                let root = self.clone();
                let grant = grant.clone();
                tokio::spawn(async move {
                    let result = root.run_listener_operation(&grant, &operation).await;
                    root.activity(&grant, &operation.name, result.is_ok());
                    operation.completion.send_replace(Some(Completion {
                        finished_at: Instant::now(),
                        result,
                    }));
                    root.changes
                        .send_modify(|generation| *generation = generation.wrapping_add(1));
                });
                receiver
            }
        };
        let receipt = {
            let completion = completion.wait_for(Option::is_some).await.map_err(|_| {
                Error::new("unavailable", "The listener operation ended unexpectedly.")
            })?;
            completion
                .as_ref()
                .expect("completion contains an outcome")
                .result
                .clone()?
        };
        self.listener_receipt(grant, &receipt, input)
    }

    fn listener_receipt(
        &self,
        grant: &Grant,
        receipt: &Receipt,
        input: &Value,
    ) -> Result<Value, Error> {
        self.check_listener_grant(grant)?;
        let record = self
            .sessions
            .lock()
            .records
            .get(&receipt.session_id)
            .filter(|record| record.owner == grant.id)
            .cloned();
        let mut summary = if let Some(record) = record {
            let mut summary = record.summary();
            if !record.available_to(grant) {
                hide_listener_address(&mut summary);
            }
            summary
        } else if let Some(ended) = receipt.ended.get() {
            ended.summary.clone()
        } else {
            return Err(Error::new(
                "unavailable",
                "The listener receipt is unavailable.",
            ));
        };
        summary["operationId"] = input["operationId"].clone();
        Ok(summary)
    }

    pub(super) fn listener_job_status(
        &self,
        grant: &Grant,
        name: &str,
        input: &Value,
    ) -> Option<Result<Value, Error>> {
        let key = (grant.id.clone(), input["operationId"].as_str()?.to_owned());
        let operation = {
            let mut registry = self.sessions.lock();
            registry.prune_listener_operations(&self.access);
            registry.listener_operations.get(&key)?.clone()
        };
        Some((|| {
            self.check_listener_grant(grant)?;
            let mut summary = json!({"operationId":input["operationId"],"sessionId":operation.input["sessionId"],"state":"pending","cancelRequested":false,"completed":0,"total":1,"result":null,"error":null,"actionRequired":null});
            let result = operation
                .completion
                .borrow()
                .as_ref()
                .map(|completion| completion.result.clone());
            match result {
                Some(Ok(receipt)) => {
                    let result = self.listener_receipt(grant, &receipt, input)?;
                    summary["sessionId"] = result["sessionId"].clone();
                    summary["state"] = json!("completed");
                    summary["completed"] = json!(1);
                    summary["result"] = result;
                }
                Some(Err(error)) => {
                    summary["state"] = json!("failed");
                    summary["error"] = json!(error);
                }
                None if name == "transmog_operation_cancel" => {
                    return Err(Error::new(
                        "cannot_cancel",
                        "The listener operation is already admitted. Poll its status to get the outcome.",
                    ));
                }
                None => {}
            }
            Ok(summary)
        })())
    }

    fn check_listener_grant(&self, grant: &Grant) -> Result<(), Error> {
        if self
            .access
            .session_authority(&grant.id)
            .is_some_and(|current| current.revision == grant.revision && authorized(&current))
        {
            return Ok(());
        }
        Err(denied(
            "Private listener access changed while this operation was running.",
        ))
    }

    async fn run_listener_operation(
        &self,
        grant: &Grant,
        operation: &Operation,
    ) -> Result<Receipt, Error> {
        self.check_listener_grant(grant)?;
        let input = &operation.input;
        if operation.name == STOP_LISTENER {
            let record = self.session_record(grant, string(input, "sessionId")?)?;
            self.stop_private_record(&record, Some(grant)).await?;
            return Ok(Receipt::for_record(&record));
        }
        let root = self
            .own
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| invalid("This MCP instance ended."))?;
        let port = u16::try_from(integer(input, "port", 0, u64::from(u16::MAX))?)
            .map_err(|_| invalid("Use a port between 0 and 65535."))?;
        let name = input["name"].as_str().unwrap_or("Private listener");
        let record = root.create_session(grant, name, Some(operation.generation))?;
        let result = {
            let _operation = record.adapter.operations.lock().await;
            if record.closed.load(Ordering::Acquire) {
                Err(Error::new(
                    "unavailable",
                    "The session closed before binding.",
                ))
            } else {
                let decrypt = input["decryptHttps"].as_bool().unwrap_or_else(|| {
                    record
                        .adapter
                        .application
                        .product_state()
                        .preferences
                        .decrypt_https
                });
                let started = record.adapter.bind_private_listener(port, decrypt).await;
                started.and_then(|certificate| {
                    self.check_listener_grant(grant)?;
                    if record.closed.load(Ordering::Acquire) {
                        return Err(Error::new(
                            "unavailable",
                            "The session closed while binding.",
                        ));
                    }
                    record.lock().certificate = certificate;
                    record.ready.store(true, Ordering::Release);
                    Ok(Receipt::for_record(&record))
                })
            }
        };
        if result.is_err()
            && let Err(error) = self.remove_session(&record, true).await
        {
            self.application.record_diagnostic(
                transmog_app::DiagnosticLevel::Error,
                "mcp",
                "listener-cleanup-failed",
                &error.message,
            );
        }
        result
    }

    async fn bind_private_listener(
        &self,
        port: u16,
        decrypt_https: bool,
    ) -> Result<Option<transmog_app::CaIdentity>, Error> {
        let request = ProxyStartRequest {
            listen: ([127, 0, 0, 1], port).into(),
            ca_certificate_path: self.root.join("interception-ca.pem"),
            ca_private_key_path: self.root.join("interception-ca.key"),
            ..ProxyStartRequest::default()
        };
        if decrypt_https {
            let mut state = self.application.product_state();
            state.preferences.decrypt_https = true;
            self.application
                .save_product_state(state)
                .map_err(app_error)?;
            let certificate = self
                .application
                .create_ca(CaCreateRequest {
                    certificate_path: request.ca_certificate_path.clone(),
                    private_key_path: request.ca_private_key_path.clone(),
                    common_name: "Transmog private listener CA".into(),
                    validity_days: 7,
                })
                .await
                .map_err(app_error)?;
            self.application
                .start_proxy(request, None)
                .await
                .map_err(app_error)?;
            Ok(Some(certificate))
        } else {
            self.application
                .start_https_tunnels(request, None)
                .await
                .map_err(app_error)?;
            Ok(None)
        }
    }

    pub(super) async fn stop_private_record(
        &self,
        record: &Record,
        grant: Option<&Grant>,
    ) -> Result<(), Error> {
        if !record.adapter.private_listener {
            return Err(Error::new(
                "unsupported_session_operation",
                "This inspection session has no private listener.",
            ));
        }
        let _operation = record.adapter.operations.lock().await;
        if let Some(grant) = grant {
            self.check_listener_grant(grant)?;
            self.session_record_current(grant, &record.adapter.session_id)?;
            record.touch();
        }
        record.adapter.stop_listener().await
    }

    pub(super) fn listener_needs_shutdown(&self) -> bool {
        let status = self.application.status();
        status.routing_mode.is_some()
            || matches!(
                status.lifecycle,
                AppLifecycle::Running | AppLifecycle::Draining | AppLifecycle::Stopping
            )
            || matches!(
                self.application.capture_status(),
                CaptureReadModel::Active { .. }
            )
    }

    pub(super) async fn stop_listener(&self) -> Result<(), Error> {
        let result = if self.listener_needs_shutdown() {
            self.application.shutdown().await.map_err(app_error)
        } else {
            Ok(())
        };
        if result.is_ok() || !self.listener_needs_shutdown() {
            self.lock().recording = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pruning_releases_revoked_receipts_even_with_mcp_disabled() {
        let access = AccessManager::open(None).unwrap();
        let create_key = |name: &str| {
            access
                .create(crate::GrantInput {
                    name: name.into(),
                    permissions: [Permission::CollectPrivate].into(),
                    scope: Scope::All,
                    omit_sensitive_values: true,
                    expires_at: None,
                })
                .unwrap()
                .0
        };
        let retained = create_key("Retained key");
        let revoked = create_key("Revoked key");
        let (completion, _) = tokio::sync::watch::channel(None);
        let operation = Arc::new(Operation {
            name: START_LISTENER.into(),
            input: Value::Null,
            generation: 0,
            completion,
        });
        let mut state = State::default();
        let retained_key = (retained.id, "start".into());
        state
            .listener_operations
            .insert(retained_key.clone(), operation.clone());
        state
            .listener_operations
            .insert((revoked.id.clone(), "start".into()), operation);

        assert!(!access.status().enabled);
        access
            .revoke(Some((&revoked.id, revoked.revision)))
            .unwrap();
        state.prune_listener_operations(&access);
        assert_eq!(state.listener_operations.len(), 1);
        assert!(state.listener_operations.contains_key(&retained_key));
    }

    #[test]
    fn start_receipt_expiry_begins_after_session_removal() {
        let finished_at = Instant::now();
        let receipt = Receipt::new("listener".into());
        let (completion, _) = tokio::sync::watch::channel(Some(Completion {
            finished_at,
            result: Ok(receipt.clone()),
        }));
        let operation = Operation {
            name: START_LISTENER.into(),
            input: Value::Null,
            generation: 0,
            completion,
        };
        let removed_at = finished_at + 2 * RECEIPT_AGE;
        assert!(!operation.expired_at(removed_at));
        assert!(
            receipt
                .ended
                .set(EndedSession {
                    removed_at,
                    summary: Value::Null
                })
                .is_ok()
        );
        assert!(!operation.expired_at(removed_at));
        assert!(operation.expired_at(removed_at + RECEIPT_AGE));
    }
}

//! Key-owned inspection and listener lifetimes.
use super::*;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Instant,
};
use transmog_app::{AppLifecycle, CaptureReadModel, PreparedTraceImport, TraceImportRequest};
mod jobs;
mod listener;

const IDLE_SECONDS: u64 = 30 * 60;
const MAX_SESSIONS: usize = 8;
const MAX_MEMBERSHIPS: usize = 200_000;
const MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Registry(Mutex<State>);
#[derive(Default)]
struct State {
    records: BTreeMap<String, Arc<Record>>,
    jobs: HashMap<(String, String), Arc<jobs::Job>>,
    listener_operations: HashMap<(String, String), Arc<listener::Operation>>,
    closing: bool,
    generation: u64,
}
impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
struct Record {
    adapter: Arc<AgentApplication>,
    listener_receipt: Option<listener::Receipt>,
    owner: String,
    owner_name: String,
    name: String,
    viewers: AtomicUsize,
    closed: AtomicBool,
    ready: AtomicBool,
    state: Mutex<SessionState>,
}
struct SessionState {
    deadline: Instant,
    expires_at: u64,
    live: Option<Live>,
    captures: BTreeMap<String, Value>,
    dirty: bool,
    live_error: Option<String>,
    certificate: Option<transmog_app::CaIdentity>,
}
#[derive(Clone)]
struct Live {
    after: u64,
    host: Option<String>,
    method: Option<String>,
}
impl Record {
    fn available_to(&self, grant: &Grant) -> bool {
        self.owner == grant.id
            && !self.closed.load(Ordering::Acquire)
            && self.ready.load(Ordering::Acquire)
            && (!self.adapter.private_listener || listener::authorized(grant))
            && ((!self.adapter.private_listener && self.viewers.load(Ordering::Acquire) > 0)
                || self.lock().deadline > Instant::now())
    }

    fn captures(&self) -> Result<Vec<Value>, Error> {
        let known = self.lock().captures.clone();
        let mut ids = self.adapter.application.loaded_capture_ids();
        ids.sort();
        ids.into_iter().map(|id|known.get(&id).cloned().map_or_else(||self.adapter.application.trace_metadata(&id).map(|metadata|json!({"captureId":id,"name":metadata.name,"format":metadata.format,"hasWarnings":!metadata.notes.is_empty()})).map_err(app_error),Ok)).collect()
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn touch(&self) {
        let mut state = self.lock();
        state.deadline = Instant::now() + Duration::from_secs(IDLE_SECONDS);
        state.expires_at = now() + IDLE_SECONDS;
    }
    fn dirty(&self) -> bool {
        self.lock().dirty || self.adapter.application.has_unsaved_session_changes()
    }
    fn summary(&self) -> Value {
        let state = self.lock();
        let mut summary = json!({"sessionId":self.adapter.session_id,"name":self.name,"ownerId":self.owner,"ownerName":self.owner_name,"expiresAt":state.expires_at,"liveSubscribed":state.live.is_some(),"captureCount":self.adapter.application.loaded_capture_ids().len(),"reviewing":self.viewers.load(Ordering::Acquire)>0,"agentAccessEnded":self.closed.load(Ordering::Acquire),"needsHumanAction":state.dirty||self.adapter.application.has_unsaved_session_changes(),"liveError":state.live_error,"kind":"inspection","listener":null});
        if self.adapter.private_listener {
            let status = self.adapter.application.status();
            summary["kind"] = json!("listener");
            let endpoint = match status.lifecycle {
                AppLifecycle::Stopped | AppLifecycle::Failed => None,
                _ => status.listener,
            };
            let proxy_url = endpoint
                .as_ref()
                .map(|endpoint| format!("http://{endpoint}"));
            let (recording_state, recording_path) = match self.adapter.application.capture_status()
            {
                CaptureReadModel::Idle | CaptureReadModel::Shutdown => ("idle", None),
                CaptureReadModel::Active { path, .. } => ("active", Some(path)),
                CaptureReadModel::Sealed { path, .. } => ("sealed", Some(path)),
                CaptureReadModel::Failed { path, .. } => ("failed", path),
            };
            let artifact = recording_path
                .as_deref()
                .and_then(std::path::Path::file_name)
                .and_then(|name| name.to_str());
            summary["listener"] = json!({
                "lifecycle": status.lifecycle,
                "endpoint": endpoint,
                "proxyUrl": proxy_url,
                "routingMode": "manual",
                "decryptHttps": state.certificate.is_some(),
                "certificate": state.certificate,
                "recording": recording_state == "active",
                "recordingState": recording_state,
                "recordingArtifact": artifact,
            });
        }
        summary
    }
}

/// Holds the existing session backend while a trusted review window is open.
pub struct SessionReview {
    record: Arc<Record>,
}
impl SessionReview {
    /// Backend shared by the agent and this review window.
    pub fn application(&self) -> Application {
        self.record.adapter.application.clone()
    }
    /// Stable session identity, independent of window lifetime.
    pub fn id(&self) -> &str {
        &self.record.adapter.session_id
    }
    /// User-facing session label.
    pub fn name(&self) -> &str {
        &self.record.name
    }
    /// Tracks unsaved human rule-editor work; agents cannot set this flag.
    pub fn set_dirty(&self, dirty: bool) {
        self.record.lock().dirty = dirty;
    }
    /// Whether closing the review would discard unsaved human work.
    pub fn is_dirty(&self) -> bool {
        self.record.dirty()
    }
    /// Whether the rule editor contains unsaved human changes.
    pub fn has_rule_draft(&self) -> bool {
        self.record.lock().dirty
    }
    /// Whether the agent's access to this review has ended.
    pub fn access_ended(&self) -> bool {
        self.record.closed.load(Ordering::Acquire)
    }
    /// Copies a saved private draft to shared rules, disabled, after a human action.
    /// # Errors
    /// Rejects stale revisions and an unavailable shared backend.
    pub async fn publish(&self, id: &str, revision: u64) -> Result<Value, Error> {
        if self.record.adapter.private_listener {
            return Err(Error::new(
                "unsupported_session_operation",
                "Private listener review cannot publish a shared rule.",
            ));
        }
        let root = self
            .record
            .adapter
            .parent
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| invalid("The live workspace is unavailable."))?;
        let _private = self.record.adapter.operations.lock().await;
        let _root = root.operations.lock().await;
        let mut rule = self
            .record
            .adapter
            .application
            .automation_status()
            .rules
            .into_iter()
            .find(|rule| rule.id == id && rule.revision == revision)
            .ok_or_else(|| {
                Error::new(
                    "conflict",
                    "This rule changed. Refresh it before publishing.",
                )
            })?;
        rule.id = format!("rule-{}", random(12)?);
        rule.revision = 1;
        rule.enabled = false;
        root.save_rule(rule, None)
    }
}
impl Drop for SessionReview {
    fn drop(&mut self) {
        self.record.viewers.fetch_sub(1, Ordering::AcqRel);
        if !self.record.adapter.private_listener {
            self.record.touch();
        }
    }
}

pub(crate) fn inspection_tool(name: &str) -> bool {
    crate::catalog::sessions::inspection_tool(name)
}
fn management_tool_inner(name: &str) -> bool {
    matches!(
        name,
        "transmog_session_create"
            | "transmog_session_listen"
            | "transmog_session_listener_stop"
            | "transmog_session_list"
            | "transmog_session_renew"
            | "transmog_session_close"
            | "transmog_capture_load"
            | "transmog_capture_list"
            | "transmog_capture_unload"
            | "transmog_session_live_subscribe"
            | "transmog_session_live_unsubscribe"
            | "transmog_operation_status"
            | "transmog_operation_cancel"
            | "transmog_rule_publish"
    )
}
pub(super) fn management_tool(name: &str) -> bool {
    management_tool_inner(name)
}

fn list_session_captures(
    grant: &Grant,
    record: &Record,
    include_context: bool,
) -> Result<Value, Error> {
    grant.require(Permission::Inspect)?;
    let mut captures = record.captures()?;
    if include_context {
        grant.require(Permission::ReadBodies)?;
        grant.require(Permission::ReadCredentials)?;
        if !matches!(grant.scope, Scope::All) {
            return Err(denied(
                "Complete capture-level context requires all-traffic scope.",
            ));
        }
        for capture in &mut captures {
            let id = string(capture, "captureId")?;
            let metadata = record
                .adapter
                .application
                .trace_metadata(id)
                .map_err(app_error)?;
            capture["context"] = record.adapter.retain_context(
                grant,
                context::ContextInput {
                    bytes: serde_json::to_vec(&metadata)
                        .map_err(|_| invalid("Capture context could not be encoded."))?,
                    required: vec![
                        Permission::Inspect,
                        Permission::ReadBodies,
                        Permission::ReadCredentials,
                    ],
                    source: json!({"kind":"capture","captureId":id}),
                    metadata: json!({}),
                    url: "http://capture.invalid/".into(),
                    entry_id: None,
                    representation: "json",
                    text: true,
                },
            )?;
        }
    }
    Ok(json!({"captures":captures}))
}

impl AgentApplication {
    #[cfg(test)]
    pub(super) fn expire_session_for_test(&self, id: &str) {
        if let Some(record) = self.sessions.lock().records.get(id) {
            record.lock().deadline = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        }
    }
    pub(super) fn session_record_current(&self, grant: &Grant, id: &str) -> Result<(), Error> {
        self.session_record(grant, id).map(|_| ())
    }
    pub(super) fn session_target(
        self: &Arc<Self>,
        grant: &Grant,
        id: &str,
    ) -> Result<Arc<Self>, Error> {
        if id == "main" {
            return Ok(self.clone());
        }
        let record = self.session_record(grant, id)?;
        record.touch();
        Ok(record.adapter.clone())
    }
    fn session_record(&self, grant: &Grant, id: &str) -> Result<Arc<Record>, Error> {
        self.sessions
            .lock()
            .records
            .get(id)
            .filter(|record| record.available_to(grant))
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "session_unavailable",
                    "This key has no active inspection session with that ID.",
                )
            })
    }
    fn create_session(
        self: &Arc<Self>,
        grant: &Grant,
        name: &str,
        listener_generation: Option<u64>,
    ) -> Result<Arc<Record>, Error> {
        let mut registry = self.sessions.lock();
        if registry.closing || listener_generation.is_some_and(|epoch| epoch != registry.generation)
        {
            return Err(Error::new(
                "unavailable",
                "The MCP server is shutting down.",
            ));
        }
        let private_listener = listener_generation.is_some();
        if registry.records.len() >= MAX_SESSIONS
            || registry
                .records
                .values()
                .filter(|record| record.owner == grant.id)
                .count()
                >= 4
        {
            return Err(Error::new(
                "limit",
                "End an inspection session before creating another (4 per key, 8 total).",
            ));
        }
        let directory = tempfile::Builder::new()
            .prefix("transmog-agent-session-")
            .tempdir_in(&self.root)
            .map_err(|_| invalid("Session storage could not be created."))?;
        let id = random(18)?;
        let application = if private_listener {
            self.application
                .create_private_listener_session(directory.path())
        } else {
            self.application
                .create_inspection_session(directory.path().to_path_buf())
        }
        .map_err(app_error)?;
        let directory = Arc::new(directory);
        let adapter = Arc::new(Self {
            _session_directory: Some(directory.clone()),
            session_id: id.clone(),
            private_listener,
            parent: Some(Arc::downgrade(self)),
            own: std::sync::OnceLock::new(),
            sessions: Registry::default(),
            context_cache: self.context_cache.clone(),
            snapshot_cache: self.snapshot_cache.clone(),
            send_ledger: self.send_ledger.clone(),
            application,
            access: self.access.clone(),
            root: directory.path().to_path_buf(),
            instance_id: self.instance_id.clone(),
            host: self.host.clone(),
            operations: Arc::default(),
            runtime: Mutex::default(),
            changes: self.changes.clone(),
        });
        let _ = adapter.own.set(Arc::downgrade(&adapter));
        let record = Arc::new(Record {
            adapter,
            listener_receipt: private_listener.then(|| listener::Receipt::new(id.clone())),
            owner: grant.id.clone(),
            owner_name: grant.name.clone(),
            name: name.into(),
            viewers: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            ready: AtomicBool::new(!private_listener),
            state: Mutex::new(SessionState {
                deadline: Instant::now() + Duration::from_secs(IDLE_SECONDS),
                expires_at: now() + IDLE_SECONDS,
                live: None,
                captures: BTreeMap::new(),
                dirty: false,
                live_error: None,
                certificate: None,
            }),
        });
        registry.records.insert(id, record.clone());
        Ok(record)
    }
    pub(super) fn session_summaries(&self, owner: Option<&Grant>) -> Vec<Value> {
        self.sessions
            .lock()
            .records
            .values()
            .filter(|record| {
                owner.map_or_else(
                    || record.ready.load(Ordering::Acquire),
                    |grant| record.available_to(grant),
                )
            })
            .map(|record| record.summary())
            .collect()
    }
    async fn remove_session(&self, record: &Arc<Record>, force: bool) -> Result<(), Error> {
        if !force && !record.adapter.private_listener && record.dirty() {
            return Err(Error::new(
                "needs_human_action",
                "Resolve unsaved edits in the review window before ending this session.",
            ));
        }
        let was_closed = record.closed.swap(true, Ordering::AcqRel);
        let stop_result = if record.adapter.private_listener {
            let _operation = record.adapter.operations.lock().await;
            record.adapter.stop_listener().await
        } else {
            Ok(())
        };
        record.lock().live = None;
        self.cancel_session_jobs(&record.adapter.session_id);
        self.context_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|context| context.realm != record.adapter.session_id);
        self.snapshot_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|snapshot| snapshot.realm != record.adapter.session_id);
        // A failed recording can leave a recoverable file after the port is
        // released. Keep the session only for unfinished cleanup or human work.
        let removed = (stop_result.is_ok() || !record.adapter.listener_needs_shutdown())
            && !record.dirty()
            && {
                let mut registry = self.sessions.lock();
                // Publish the final recording state before the live lookup stops finding it.
                if let Some(receipt) = &record.listener_receipt {
                    receipt.archive(record);
                }
                registry
                    .records
                    .remove(&record.adapter.session_id)
                    .is_some()
            };
        if !was_closed || removed {
            self.changes
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        }
        stop_result
    }
    /// Reopens session admission when the desktop restarts the MCP endpoint.
    pub fn open_session_registry(&self) {
        self.sessions.lock().closing = false;
    }
    pub(super) async fn close_all_sessions(&self) -> Result<(), Error> {
        let records = {
            let mut registry = self.sessions.lock();
            registry.closing = true;
            registry.generation = registry.generation.wrapping_add(1);
            registry.records.values().cloned().collect::<Vec<_>>()
        };
        let mut result = Ok(());
        for record in records {
            if let Err(error) = self.remove_session(&record, true).await {
                result = Err(error);
            }
        }
        result
    }
    /// Opens a trusted review handle over the existing private backend.
    /// # Errors
    /// Rejects unknown or already ended sessions without human work to preserve.
    pub fn review_session(&self, id: &str) -> Result<SessionReview, Error> {
        let record = self
            .sessions
            .lock()
            .records
            .get(id)
            .filter(|record| record.ready.load(Ordering::Acquire))
            .cloned()
            .ok_or_else(|| invalid("This inspection session ended."))?;
        record.viewers.fetch_add(1, Ordering::AcqRel);
        if !record.adapter.private_listener {
            record.touch();
        }
        Ok(SessionReview { record })
    }
    /// Ends an inspection session from trusted desktop UI, preserving unsaved work.
    /// # Errors
    /// Rejects an unknown session or unsaved edits in an offline inspection session.
    pub async fn end_inspection_session(&self, id: &str) -> Result<(), Error> {
        let record = self
            .sessions
            .lock()
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| invalid("This inspection session ended."))?;
        self.remove_session(&record, false).await
    }

    /// Stops a private listener from the desktop and retains its traffic for review.
    ///
    /// # Errors
    /// Returns an unknown-session or cleanup failure.
    pub async fn stop_listener_from_desktop(&self, id: &str) -> Result<(), Error> {
        let record = self
            .sessions
            .lock()
            .records
            .get(id)
            .filter(|record| record.ready.load(Ordering::Acquire))
            .cloned()
            .ok_or_else(|| invalid("This inspection session ended."))?;
        self.stop_private_record(&record, None).await?;
        self.changes
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    pub(super) async fn session_operation(
        self: &Arc<Self>,
        grant: &Grant,
        name: &str,
        input: &Value,
    ) -> Result<Value, Error> {
        self.check_instance(input)?;
        match name {
            "transmog_session_create" => Ok(self
                .create_session(
                    grant,
                    input["name"].as_str().unwrap_or("Agent inspection"),
                    None,
                )?
                .summary()),
            "transmog_session_listen" | "transmog_session_listener_stop" => {
                self.listener_operation(grant, name, input).await
            }
            "transmog_session_list" => {
                Ok(json!({"sessions":self.session_summaries(Some(grant)),"mainSessionId":"main"}))
            }
            "transmog_operation_status" | "transmog_operation_cancel" => self
                .listener_job_status(grant, name, input)
                .unwrap_or_else(|| self.job_operation(grant, name, input)),
            _ => {
                let record = self.session_record(grant, string(input, "sessionId")?)?;
                if record.adapter.private_listener
                    && matches!(
                        name,
                        "transmog_session_live_subscribe"
                            | "transmog_session_live_unsubscribe"
                            | "transmog_rule_publish"
                    )
                {
                    return Err(Error::new(
                        "unsupported_session_operation",
                        "Private listeners capture their own traffic and cannot attach to shared live operations.",
                    ));
                }
                record.touch();
                match name {
                    "transmog_session_renew" => Ok(record.summary()),
                    "transmog_session_close" => {
                        if record.adapter.private_listener {
                            // The connection may close while shutdown is joining the runtime.
                            let root = self.clone();
                            tokio::spawn(async move { root.remove_session(&record, false).await })
                                .await
                                .map_err(|_| {
                                    invalid("The session cleanup task ended unexpectedly.")
                                })??;
                        } else {
                            self.remove_session(&record, false).await?;
                        }
                        Ok(json!({"closed":true}))
                    }
                    "transmog_capture_list" => {
                        list_session_captures(grant, &record, input["includeContext"] == true)
                    }
                    "transmog_capture_load" | "transmog_capture_unload" => {
                        self.start_capture_job(grant, &record, name, input)
                    }
                    "transmog_session_live_unsubscribe" => {
                        self.fanout_live(None);
                        record.lock().live = None;
                        Ok(record.summary())
                    }
                    "transmog_session_live_subscribe" => {
                        grant.require(Permission::Inspect)?;
                        let live = Live {
                            after: if input["includeExisting"] == true {
                                0
                            } else {
                                self.application
                                    .live_exchange_memberships()
                                    .iter()
                                    .map(|(_, number, _, _)| *number)
                                    .max()
                                    .unwrap_or(0)
                            },
                            host: optional_string(input, "host")?,
                            method: optional_string(input, "method")?,
                        };
                        record.lock().live = Some(live);
                        record.lock().live_error = None;
                        self.fanout_live(None);
                        Ok(record.summary())
                    }
                    "transmog_rule_publish" => self.publish_rule(grant, &record, input).await,
                    _ => Err(invalid("Unknown session operation.")),
                }
            }
        }
    }
    async fn publish_rule(
        &self,
        grant: &Grant,
        record: &Record,
        input: &Value,
    ) -> Result<Value, Error> {
        grant.require(Permission::EditSharedRules)?;
        let _private = record.adapter.operations.lock().await;
        let _root = self.operations.lock().await;
        let readable = record.adapter.read_rule(grant, string(input, "id")?)?;
        if readable["rule"].is_null() {
            return Err(denied(
                "Review and approve unredacted access before publishing a rule with protected values.",
            ));
        }
        let mut rule: Rule = parse(readable["rule"].clone())?;
        if rule.revision != input["expectedRevision"].as_u64().unwrap_or(0) {
            return Err(Error::new(
                "conflict",
                "The private rule changed. Review its current revision.",
            ));
        }
        Self::check_rule_scope(grant, &rule)?;
        rule.id = format!("rule-{}", random(12)?);
        rule.revision = 1;
        rule.enabled = false;
        self.save_rule(rule, None)
    }
    pub(super) async fn maintain_sessions(&self) -> Result<(), Error> {
        let records = self
            .sessions
            .lock()
            .records
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut result = Ok(());
        for record in records {
            let invalid = self
                .access
                .session_authority(&record.owner)
                .is_none_or(|grant| {
                    record.adapter.private_listener && !listener::authorized(&grant)
                });
            let expired = (record.adapter.private_listener
                || record.viewers.load(Ordering::Acquire) == 0)
                && record.lock().deadline <= Instant::now();
            if invalid || expired || record.closed.load(Ordering::Acquire) {
                if let Err(error) = self.remove_session(&record, true).await {
                    result = Err(error);
                }
            } else if record.adapter.private_listener
                && record.ready.load(Ordering::Acquire)
                && let Ok(_operation) = record.adapter.operations.try_lock()
                && let Err(error) = record.adapter.cleanup().await
            {
                result = Err(error);
            }
        }
        self.maintain_jobs();
        result
    }
    pub(super) fn fanout_live(&self, changed: Option<&str>) {
        let records = self
            .sessions
            .lock()
            .records
            .values()
            .filter(|record| {
                !record.adapter.private_listener && !record.closed.load(Ordering::Acquire)
            })
            .cloned()
            .collect::<Vec<_>>();
        if records.is_empty() {
            return;
        }
        let admissions = changed.map_or_else(
            || self.application.live_exchange_memberships(),
            |id| {
                self.application
                    .live_exchange_membership(id)
                    .into_iter()
                    .collect()
            },
        );
        let mut total = records
            .iter()
            .map(|record| record.adapter.application.live_membership_count())
            .sum::<usize>();
        for record in records {
            let Some(grant) = self.access.session_authority(&record.owner) else {
                continue;
            };
            if grant.require(Permission::Inspect).is_err() {
                continue;
            }
            let live = record.lock().live.clone();
            if let Some(id) = changed {
                let _ = record.adapter.application.refresh_live_exchange(id);
            } else {
                for (id, _, _, _) in record.adapter.application.live_exchange_memberships() {
                    let _ = record.adapter.application.refresh_live_exchange(&id);
                }
            }
            for (id, number, url, method) in &admissions {
                let _ = record.adapter.application.refresh_live_exchange(id);
                let Some(live) = &live else {
                    continue;
                };
                if *number <= live.after
                    || !grant.scope.allows_entry(&self.instance_id, id, url)
                    || live
                        .method
                        .as_ref()
                        .is_some_and(|filter| !filter.eq_ignore_ascii_case(method))
                    || live.host.as_ref().is_some_and(|host| {
                        url::Url::parse(url)
                            .ok()
                            .and_then(|url| url.host_str().map(str::to_owned))
                            .is_none_or(|actual| !actual.eq_ignore_ascii_case(host))
                    })
                {
                    continue;
                }
                if total >= MAX_MEMBERSHIPS {
                    record.lock().live_error = Some(
                        "Shared live membership limit reached. End an inspection session to release evidence.".into(),
                    );
                    break;
                }
                match record
                    .adapter
                    .application
                    .reference_live_exchanges(std::slice::from_ref(id))
                {
                    Ok(added) => {
                        total += added;
                        if added > 0 {
                            record.lock().live_error = None;
                        }
                    }
                    Err(error) => {
                        record.lock().live_error = Some(error.message);
                        break;
                    }
                }
            }
        }
    }
}

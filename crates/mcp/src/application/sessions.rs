//! Key-owned inspection lifetimes. Live controls and the proxy remain on the root adapter.
use super::*;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Instant,
};
use transmog_app::{PreparedTraceImport, TraceImportRequest};
mod jobs;

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
    owner: String,
    owner_name: String,
    name: String,
    viewers: AtomicUsize,
    closed: AtomicBool,
    state: Mutex<SessionState>,
}
struct SessionState {
    deadline: Instant,
    expires_at: u64,
    live: Option<Live>,
    captures: BTreeMap<String, Value>,
    dirty: bool,
    live_error: Option<String>,
}
#[derive(Clone)]
struct Live {
    after: u64,
    host: Option<String>,
    method: Option<String>,
}
impl Record {
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
        json!({"sessionId":self.adapter.session_id,"name":self.name,"ownerId":self.owner,"ownerName":self.owner_name,"expiresAt":state.expires_at,"liveSubscribed":state.live.is_some(),"captureCount":self.adapter.application.loaded_capture_ids().len(),"reviewing":self.viewers.load(Ordering::Acquire)>0,"agentAccessEnded":self.closed.load(Ordering::Acquire),"needsHumanAction":state.dirty||self.adapter.application.has_unsaved_session_changes(),"liveError":state.live_error})
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
        self.record.touch();
    }
}

pub(crate) fn inspection_tool(name: &str) -> bool {
    crate::catalog::sessions::inspection_tool(name)
}
fn management_tool_inner(name: &str) -> bool {
    matches!(
        name,
        "transmog_session_create"
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
            .filter(|record| {
                record.owner == grant.id
                    && !record.closed.load(Ordering::Acquire)
                    && (record.viewers.load(Ordering::Acquire) > 0
                        || record.lock().deadline > Instant::now())
            })
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "session_unavailable",
                    "This key has no active inspection session with that ID.",
                )
            })
    }
    fn create_session(self: &Arc<Self>, grant: &Grant, name: &str) -> Result<Value, Error> {
        let mut registry = self.sessions.lock();
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
        let application = self
            .application
            .create_inspection_session(directory.path().to_path_buf())
            .map_err(app_error)?;
        let directory = Arc::new(directory);
        let adapter = Arc::new(Self {
            _session_directory: Some(directory.clone()),
            session_id: id.clone(),
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
            operations: tokio::sync::Mutex::new(()),
            runtime: Mutex::default(),
            changes: self.changes.clone(),
        });
        let _ = adapter.own.set(Arc::downgrade(&adapter));
        let record = Arc::new(Record {
            adapter,
            owner: grant.id.clone(),
            owner_name: grant.name.clone(),
            name: name.into(),
            viewers: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            state: Mutex::new(SessionState {
                deadline: Instant::now() + Duration::from_secs(IDLE_SECONDS),
                expires_at: now() + IDLE_SECONDS,
                live: None,
                captures: BTreeMap::new(),
                dirty: false,
                live_error: None,
            }),
        });
        let summary = record.summary();
        registry.records.insert(id, record);
        Ok(summary)
    }
    pub(super) fn session_summaries(&self, owner: Option<&Grant>) -> Vec<Value> {
        self.sessions
            .lock()
            .records
            .values()
            .filter(|record| {
                owner.is_none_or(|grant| {
                    record.owner == grant.id && !record.closed.load(Ordering::Acquire)
                })
            })
            .map(|record| record.summary())
            .collect()
    }
    fn remove_session(&self, record: &Arc<Record>, force: bool) -> Result<(), Error> {
        if !force && record.dirty() {
            return Err(Error::new(
                "needs_human_action",
                "Resolve unsaved edits in the review window before ending this session.",
            ));
        }
        let was_closed = record.closed.swap(true, Ordering::AcqRel);
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
        // Dirty human work retains its backend, but all agent access has ended.
        if !record.dirty() {
            self.sessions
                .lock()
                .records
                .remove(&record.adapter.session_id);
        }
        if !was_closed {
            self.changes
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        }
        Ok(())
    }
    pub(super) fn close_all_sessions(&self) {
        let records = self
            .sessions
            .lock()
            .records
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for record in records {
            let _ = self.remove_session(&record, true);
        }
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
            .cloned()
            .ok_or_else(|| invalid("This inspection session ended."))?;
        record.viewers.fetch_add(1, Ordering::AcqRel);
        record.touch();
        Ok(SessionReview { record })
    }
    /// Ends an inspection session from trusted desktop UI, preserving unsaved work.
    /// # Errors
    /// Rejects unsaved edits or an unknown session.
    pub fn end_inspection_session(&self, id: &str) -> Result<(), Error> {
        let record = self
            .sessions
            .lock()
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| invalid("This inspection session ended."))?;
        self.remove_session(&record, false)
    }

    pub(super) async fn session_operation(
        self: &Arc<Self>,
        grant: &Grant,
        name: &str,
        input: &Value,
    ) -> Result<Value, Error> {
        self.check_instance(input)?;
        match name {
            "transmog_session_create" => {
                self.create_session(grant, input["name"].as_str().unwrap_or("Agent inspection"))
            }
            "transmog_session_list" => {
                Ok(json!({"sessions":self.session_summaries(Some(grant)),"mainSessionId":"main"}))
            }
            "transmog_operation_status" | "transmog_operation_cancel" => {
                self.job_operation(grant, name, input)
            }
            _ => {
                let record = self.session_record(grant, string(input, "sessionId")?)?;
                record.touch();
                match name {
                    "transmog_session_renew" => Ok(record.summary()),
                    "transmog_session_close" => {
                        self.remove_session(&record, false)?;
                        Ok(json!({"closed":true}))
                    }
                    "transmog_capture_list" => {
                        grant.require(Permission::Inspect)?;
                        let mut captures = record.captures()?;
                        if input["includeContext"] == true {
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
                                        bytes: serde_json::to_vec(&metadata).map_err(|_| {
                                            invalid("Capture context could not be encoded.")
                                        })?,
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
    pub(super) fn maintain_sessions(&self) {
        let records = self
            .sessions
            .lock()
            .records
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for record in records {
            let invalid = self.access.session_authority(&record.owner).is_none();
            let expired = record.viewers.load(Ordering::Acquire) == 0
                && record.lock().deadline <= Instant::now();
            if invalid || expired || record.closed.load(Ordering::Acquire) {
                let _ = self.remove_session(&record, true);
            }
        }
        self.maintain_jobs();
    }
    pub(super) fn fanout_live(&self, changed: Option<&str>) {
        let records = self
            .sessions
            .lock()
            .records
            .values()
            .filter(|record| !record.closed.load(Ordering::Acquire))
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

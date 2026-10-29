//! Bounded, idempotent capture operations. Passwords enter only through trusted desktop UI.
use super::*;

pub(super) struct Job {
    owner: String,
    revision: u64,
    id: String,
    fingerprint: Vec<u8>,
    record: std::sync::Weak<Record>,
    session_id: String,
    name: String,
    native_id: String,
    path: Option<PathBuf>,
    cancel: tokio::sync::watch::Sender<bool>,
    state: Mutex<JobState>,
}
struct JobState {
    status: &'static str,
    running: bool,
    source: Option<PreparedTraceImport>,
    capture_id: Option<String>,
    result: Option<Value>,
    error: Option<Error>,
    completed: u64,
    total: u64,
}
impl Job {
    async fn prepare_source(
        &self,
        application: &Application,
    ) -> Result<Option<PreparedTraceImport>, Error> {
        let source = self.lock().source.clone();
        if let Some(source) = source {
            Ok(Some(source))
        } else if let Some(path) = self.path.clone() {
            let application = application.clone();
            let native_id = self.native_id.clone();
            tokio::task::spawn_blocking(move || {
                application.prepare_trace_import(TraceImportRequest {
                    path,
                    operation_id: native_id,
                    password: None,
                    max_file_bytes: 1024 * 1024 * 1024,
                })
            })
            .await
            .map_err(|_| invalid("Capture source worker failed."))
            .and_then(|result| result.map(Some).map_err(app_error))
        } else {
            Ok(None)
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, JobState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn summary(&self) -> Value {
        let state = self.lock();
        json!({"name":self.name,"operationId":self.id,"sessionId":self.session_id,"state":state.status,"cancelRequested":*self.cancel.borrow(),"completed":state.completed,"total":state.total,"result":state.result,"error":state.error,"actionRequired":if state.status=="waiting-password" {Some("Enter the capture password in Transmog's agent activity panel.")}else{None}})
    }
    fn cancel(&self) {
        let mut state = self.lock();
        if !matches!(state.status, "pending" | "waiting-password") {
            return;
        }
        self.cancel.send_replace(true);
        if !state.running && state.status == "waiting-password" {
            state.status = "cancelled";
            state.source = None;
        }
    }
}
impl AgentApplication {
    fn remaining_capture_budget(&self) -> u64 {
        MAX_INDEX_BYTES.saturating_sub(
            self.sessions
                .lock()
                .records
                .values()
                .map(|record| record.adapter.application.imported_index_bytes())
                .sum::<u64>(),
        )
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Idempotent job admission validates and reserves one operation under one registry lock."
    )]
    pub(super) fn start_capture_job(
        self: &Arc<Self>,
        grant: &Grant,
        record: &Arc<Record>,
        name: &str,
        input: &Value,
    ) -> Result<Value, Error> {
        grant.require(Permission::LoadCaptures)?;
        if matches!(grant.scope, Scope::Requests { .. }) {
            return Err(denied(
                "Selected-request access cannot load new capture identities. Choose an all-traffic or host scope in the app.",
            ));
        }
        let id = string(input, "operationId")?.to_owned();
        let fingerprint = Sha256::digest(
            serde_json::to_vec(&(name, input))
                .map_err(|_| invalid("Operation inputs could not be encoded."))?,
        )
        .to_vec();
        let mut registry = self.sessions.lock();
        if let Some(job) = registry.jobs.get(&(grant.id.clone(), id.clone())) {
            if job.fingerprint != fingerprint {
                return Err(Error::new(
                    "conflict",
                    "That operationId already names a different capture operation.",
                ));
            }
            return Ok(job.summary());
        }
        if registry.jobs.len() >= 128 {
            return Err(Error::new(
                "limit",
                "The capture operation ledger is full for this app run.",
            ));
        }
        if registry
            .jobs
            .values()
            .filter(|job| matches!(job.lock().status, "pending" | "waiting-password"))
            .count()
            >= 4
        {
            return Err(Error::new(
                "limit",
                "Finish or cancel a pending capture operation before starting another (maximum 4).",
            ));
        }
        if registry.jobs.values().any(|job| job.lock().running) {
            return Err(Error::new(
                "busy",
                "Another capture operation is running. Wait for it to finish before starting this operation.",
            ));
        }
        if name == "transmog_capture_unload"
            && (record.lock().dirty || record.adapter.application.has_unsaved_session_changes())
        {
            return Err(Error::new(
                "needs_human_action",
                "Resolve unsaved edits in the review window before changing its captures.",
            ));
        }
        let native_id = format!("mcp-{}", random(18)?);
        let (path, capture_id) = if name == "transmog_capture_load" {
            if record.adapter.application.loaded_capture_ids().len() >= 16
                || registry
                    .records
                    .values()
                    .map(|record| record.adapter.application.loaded_capture_ids().len())
                    .sum::<usize>()
                    >= 64
            {
                return Err(Error::new(
                    "limit",
                    "Unload a capture before loading another (16 per session, 64 total).",
                ));
            }
            (Some(PathBuf::from(string(input, "path")?)), None)
        } else {
            let capture_id = string(input, "captureId")?.to_owned();
            if !record
                .adapter
                .application
                .loaded_capture_ids()
                .contains(&capture_id)
            {
                return Err(Error::new(
                    "not_found",
                    "This capture is not loaded in this session.",
                ));
            }
            (None, Some(capture_id))
        };
        let (cancel, _) = tokio::sync::watch::channel(false);
        let job = Arc::new(Job {
            name: format!(
                "{} · {}",
                grant.name,
                input["path"]
                    .as_str()
                    .and_then(|path| std::path::Path::new(path).file_name())
                    .map_or_else(
                        || "Capture unload".into(),
                        |name| name.to_string_lossy().into_owned()
                    )
            ),
            owner: grant.id.clone(),
            revision: grant.revision,
            id: id.clone(),
            fingerprint,
            record: Arc::downgrade(record),
            session_id: record.adapter.session_id.clone(),
            native_id,
            path,
            cancel,
            state: Mutex::new(JobState {
                status: "pending",
                running: true,
                source: None,
                capture_id,
                result: None,
                error: None,
                completed: 0,
                total: 0,
            }),
        });
        registry.jobs.insert((grant.id.clone(), id), job.clone());
        drop(registry);
        self.spawn_capture_job(job.clone(), None);
        Ok(job.summary())
    }
    fn spawn_capture_job(self: &Arc<Self>, job: Arc<Job>, password: Option<String>) {
        let root = Arc::downgrade(self);
        tokio::spawn(async move {
            let Some(root) = root.upgrade() else {
                return;
            };
            let Some(record) = job.record.upgrade() else {
                return;
            };
            let source = job.prepare_source(&record.adapter.application).await;
            let result = match source {
                Err(error) => Err(error),
                Ok(Some(source)) => {
                    let source = source.with_index_limit(root.remaining_capture_budget());
                    job.lock().source = Some(source.clone());
                    let password = password.map(|password| parse(json!(password))).transpose();
                    match password {
                        Err(error) => Err(error),
                        Ok(password) => {
                            let mut cancellation = job.cancel.subscribe();
                            let progress_job = job.clone();
                            let progress =
                                Arc::new(move |progress: transmog_app::TraceImportProgress| {
                                    let mut state = progress_job.lock();
                                    state.completed = progress.completed;
                                    state.total = progress.total;
                                });
                            let import = record
                                .adapter
                                .application
                                .import_prepared_trace(source, password, progress);
                            tokio::pin!(import);
                            let imported = if *cancellation.borrow() {
                                Err(Error::new("cancelled", "Capture operation cancelled."))
                            } else {
                                tokio::select! {
                                    result=&mut import=>result.map_err(app_error),
                                    _=cancellation.changed()=>{record.adapter.application.cancel_trace_import(&job.native_id);import.await.map_err(app_error)}
                                }
                            };
                            match imported {
                                Ok(result) => root.finish_import(&job, &record, &result),
                                Err(error) => Err(error),
                            }
                        }
                    }
                }
                Ok(None) => {
                    let capture_id = job.lock().capture_id.clone().unwrap_or_default();
                    if *job.cancel.borrow() {
                        Err(Error::new("cancelled", "Capture operation cancelled."))
                    } else {
                        record
                            .adapter
                            .application
                            .unload_capture(&capture_id)
                            .map_err(app_error)
                            .map(|()| {
                                record.lock().captures.remove(&capture_id);
                                root.invalidate_session_evidence(&job.session_id);
                                json!({"captureId":capture_id,"unloaded":true})
                            })
                    }
                }
            };
            let mut state = job.lock();
            state.running = false;
            match result {
                Ok(result) => {
                    state.status = "completed";
                    state.result = Some(result);
                    state.source = None;
                }
                Err(error)
                    if matches!(error.code, "password_required" | "invalid_password")
                        && !record.closed.load(Ordering::Acquire)
                        && !*job.cancel.borrow() =>
                {
                    state.status = "waiting-password";
                    state.error = Some(Error::new(
                        error.code,
                        "Enter the capture password in Transmog.",
                    ));
                }
                Err(error) => {
                    state.status = if *job.cancel.borrow() {
                        "cancelled"
                    } else {
                        "failed"
                    };
                    state.error = Some(error);
                    state.source = None;
                }
            }
            root.changes
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        });
    }
    fn finish_import(
        &self,
        job: &Job,
        record: &Record,
        result: &transmog_app::TraceImportResult,
    ) -> Result<Value, Error> {
        let authority = self.access.session_authority(&job.owner);
        let index_bytes = self
            .sessions
            .lock()
            .records
            .values()
            .map(|record| record.adapter.application.imported_index_bytes())
            .sum::<u64>();
        if record.closed.load(Ordering::Acquire)
            || authority.as_ref().is_none_or(|grant| {
                grant.revision != job.revision || grant.require(Permission::LoadCaptures).is_err()
            })
            || index_bytes > MAX_INDEX_BYTES
        {
            let _ = record.adapter.application.unload_capture(&result.trace.id);
            return Err(Error::new(
                "cancelled",
                "Capture load ended because access changed, the session ended, or the shared import budget was reached.",
            ));
        }
        let capture = json!({"captureId":result.trace.id,"name":result.trace.name,"format":result.trace.format,"hasWarnings":!result.issues.is_empty()||!result.trace.notes.is_empty()});
        record
            .lock()
            .captures
            .insert(result.trace.id.clone(), capture.clone());
        // Source contexts remain in the session backend and obey current disclosure rules on read.
        Ok(capture)
    }
    fn invalidate_session_evidence(&self, id: &str) {
        self.context_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|context| context.realm != id);
        self.snapshot_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|snapshot| snapshot.realm != id);
    }
    pub(super) fn job_operation(
        &self,
        grant: &Grant,
        name: &str,
        input: &Value,
    ) -> Result<Value, Error> {
        let job = self
            .sessions
            .lock()
            .jobs
            .get(&(grant.id.clone(), string(input, "operationId")?.into()))
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "not_found",
                    "This key has no capture operation with that ID.",
                )
            })?;
        if name == "transmog_operation_cancel" {
            job.cancel();
        }
        Ok(job.summary())
    }
    pub(super) fn cancel_session_jobs(&self, id: &str) {
        for job in self
            .sessions
            .lock()
            .jobs
            .values()
            .filter(|job| job.session_id == id)
        {
            job.cancel();
        }
    }
    pub(super) fn maintain_jobs(&self) {
        for job in self.sessions.lock().jobs.values() {
            if self
                .access
                .session_authority(&job.owner)
                .is_none_or(|grant| {
                    grant.revision != job.revision
                        || grant.require(Permission::LoadCaptures).is_err()
                })
            {
                job.cancel();
            }
        }
    }
    /// Pending encrypted imports for the trusted desktop; contains no secrets or file contents.
    pub fn pending_capture_passwords(&self) -> Vec<Value> {
        self.sessions
            .lock()
            .jobs
            .values()
            .filter(|job| job.lock().status == "waiting-password")
            .map(|job| job.summary())
            .collect()
    }
    /// Resumes a pinned import after a password was entered in the app.
    /// # Errors
    /// Rejects changed permissions, concurrent operations and stale prompts.
    pub fn supply_capture_password(
        self: &Arc<Self>,
        session_id: &str,
        operation_id: &str,
        password: String,
    ) -> Result<Value, Error> {
        let registry = self.sessions.lock();
        let job = registry
            .jobs
            .values()
            .find(|job| job.session_id == session_id && job.id == operation_id)
            .cloned()
            .ok_or_else(|| invalid("This password request ended."))?;
        if registry.jobs.values().any(|job| job.lock().running) {
            return Err(Error::new(
                "busy",
                "Wait for the current capture operation to finish.",
            ));
        }
        if self
            .access
            .session_authority(&job.owner)
            .is_none_or(|grant| {
                grant.revision != job.revision || grant.require(Permission::LoadCaptures).is_err()
            })
            || job
                .record
                .upgrade()
                .is_none_or(|record| record.closed.load(Ordering::Acquire))
        {
            return Err(denied("This capture load no longer has access."));
        }
        let mut state = job.lock();
        if state.status != "waiting-password" || *job.cancel.borrow() {
            return Err(invalid("This password request ended."));
        }
        state.running = true;
        state.status = "pending";
        state.error = None;
        drop(state);
        drop(registry);
        self.spawn_capture_job(job.clone(), Some(password));
        Ok(job.summary())
    }
}

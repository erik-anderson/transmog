use super::{
    AppError, Application, Arc, DebugConfiguration, DebugSessionInfo, Deserialize, invalid, outcome,
};
/// Explicit reservation of one exact applied revision and its actual worker.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugLiveRequest {
    /// Registry generation reviewed by the author.
    pub expected_generation: u64,
    /// Applied identity, never an edited draft.
    pub script_id: String,
    /// Immutable applied revision.
    pub revision: u64,
    /// Exact executable digest.
    pub javascript_hash: String,
    /// Native method/host/path filter selecting the next exchange.
    pub filter: transmog_script::ScriptPrefilter,
    /// Read-only debugger controls.
    pub configuration: DebugConfiguration,
}
impl Application {
    /// Reserves an idle applied worker for one matching live exchange for up to 60 seconds.
    /// # Errors
    /// Rejects stale revisions, outstanding exchanges, competing owners and invalid filters.
    pub fn debug_next_script_exchange(
        &self,
        owner: &str,
        request: &DebugLiveRequest,
    ) -> Result<DebugSessionInfo, AppError> {
        self.scripts
            .with_debug_revision(request, |revision, runner| {
                let run = self.debuggers.create(
                    owner,
                    &request.configuration,
                    live_info(revision),
                    Some(revision.source_map.clone()),
                )?;
                let weak = Arc::downgrade(&run);
                run.session.on_closed(move || {
                    if let Some(run) = weak.upgrade() {
                        record_completion(&run);
                    }
                });
                if let Err(failure) = runner.arm_debugger(&run.session, request.filter.clone()) {
                    run.finish(outcome(
                        "failed",
                        &failure.message,
                        None,
                        failure.exception,
                        failure.logs,
                    ));
                    return Err(invalid(&failure.message));
                }
                Ok(run.info.clone())
            })
    }
}

fn live_info(revision: &crate::ScriptRevision) -> DebugSessionInfo {
    DebugSessionInfo {
        session: String::new(),
        mode: "Live exchange".into(),
        label: format!(
            "{} · applied revision {}",
            revision.manifest.id, revision.manifest.revision
        ),
        artifact: format!(
            "{}:{}:{}:{}",
            revision.manifest.id,
            revision.manifest.revision,
            revision.javascript_hash,
            revision.source_map_hash
        ),
        source: if matches!(revision.language, crate::ScriptLanguage::TypeScript) {
            format!("file:///transmog-scripts/{}/main.ts", revision.manifest.id)
        } else {
            "file:///transmog-script/main.js".into()
        },
    }
}

fn record_completion(run: &super::Run) {
    let page = run.session.page(u64::MAX);
    let (status, summary) = if run.session.detached() {
        if page.exchange_id.is_some() {
            (
                "completed",
                "Selected exchange completed; debugger detached and worker retained",
            )
        } else {
            (
                "completed",
                "Debugger detached without selecting an exchange",
            )
        }
    } else {
        (
            "stopped",
            "Live debugging ended. Retry the applied script if its worker is unavailable.",
        )
    };
    run.record(outcome(status, summary, None, None, page.logs));
}

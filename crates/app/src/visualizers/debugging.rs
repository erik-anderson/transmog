use super::{
    AppError, Application, Arc, Deserialize, VisualizerActivation, VisualizerHandle,
    VisualizerView, invalid, unavailable, worker_failure,
};
use crate::{DebugConfiguration, DebugSessionInfo, debugging::outcome};

/// A fresh explicitly selected visualizer fixture, using ordinary retained-input admission.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugVisualizerRequest {
    /// Exact current selected message, contribution and native page/generation.
    pub activation: VisualizerActivation,
    /// Native read-only debugger settings.
    pub configuration: DebugConfiguration,
}
fn source(entry: &str) -> Result<String, AppError> {
    url::Url::parse("file:///transmog-extension/")
        .expect("constant module root")
        .join(entry)
        .map(|url| url.to_string())
        .map_err(|_| invalid("Visualizer entry is unavailable"))
}
fn bind_lifetime(
    run: &Arc<crate::debugging::Run>,
    ticket: &Arc<super::Ticket>,
    runtime: tokio::runtime::Handle,
) {
    let ticket = Arc::downgrade(ticket);
    let owner = Arc::downgrade(run);
    run.session.on_closed(move || {
        if owner.upgrade().is_some_and(|run| run.session.detached()) {
            return;
        }
        if let Some(ticket) = ticket.upgrade() {
            runtime.spawn_blocking(move || ticket.cancel());
        }
    });
}
impl Application {
    /// Starts a fresh fixture controller before module initialization or selected-message parsing.
    /// # Errors
    /// Rejects unmatched declarations, stale views, unavailable packages or excessive controllers.
    pub fn debug_visualizer(
        &self,
        owner: &str,
        request: DebugVisualizerRequest,
    ) -> Result<DebugSessionInfo, AppError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| unavailable())?;
        if !self
            .visualizer_choices(&request.activation.selection)?
            .iter()
            .any(|choice| {
                choice.extension_id == request.activation.extension_id
                    && choice.visualizer_id == request.activation.visualizer_id
            })
        {
            return Err(invalid(
                "The selected visualizer does not match this message",
            ));
        }
        let snapshot = self
            .visualizers
            .registry
            .snapshot(&request.activation.extension_id)
            .map_err(|_| unavailable())?;
        let declaration = snapshot
            .manifest
            .visualizers
            .iter()
            .find(|entry| entry.id == request.activation.visualizer_id)
            .ok_or_else(unavailable)?;
        let info = DebugSessionInfo {
            session: String::new(),
            mode: "Visualizer fixture".into(),
            label: format!(
                "{} · {} · {}",
                snapshot.manifest.id, declaration.id, snapshot.digest
            ),
            artifact: snapshot.digest.clone(),
            source: source(&declaration.entry)?,
        };
        let run = self
            .debuggers
            .create_package(owner, &request.configuration, info, snapshot)?;
        let ticket = self
            .visualizers
            .begin(
                &request.activation.slot,
                &request.activation.generation,
                &request.activation.page_id,
            )
            .inspect_err(|_| {
                run.session.stop("Visualizer activation is stale");
            })?;
        *ticket.debug.lock().map_err(|_| unavailable())? = Some(Arc::downgrade(&run));
        bind_lifetime(&run, &ticket, runtime.clone());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *run.visualizer_result.lock().map_err(|_| unavailable())? = Some(receiver);
        let info = run.info.clone();
        let app = self.clone();
        runtime.spawn(async move {
            let result = async {
                let mut view = app
                    .activate_visualizer_ticket(&request.activation, &ticket)
                    .await?;
                view.metadata_changed = app.commit_visualizer_document(&ticket).await?;
                Ok::<_, AppError>(view)
            }
            .await;
            match &result {
                Ok(view) => run.record(outcome(
                    "completed",
                    "Fixture rendered; debugger remains attached for expansion and live callbacks",
                    None,
                    None,
                    view.logs.clone(),
                )),
                Err(error) => {
                    run.finish(outcome(
                        "failed",
                        "Visualizer debug run failed",
                        None,
                        error.exception.clone(),
                        error.logs.clone(),
                    ));
                    app.visualizers.cancel(
                        &request.activation.slot,
                        &request.activation.generation,
                        &request.activation.page_id,
                    );
                }
            }
            let _ = sender.send(result);
        });
        Ok(info)
    }
    /// Receives the ordinary validated visualizer view while its separate controller stays usable.
    /// # Errors
    /// Rejects another window, duplicate receiver, canceled inputs and ordinary parser failures.
    pub async fn await_debug_visualizer(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<VisualizerView, AppError> {
        let run = self.debuggers.owned(owner, id)?;
        let receiver = run
            .visualizer_result
            .lock()
            .map_err(|_| unavailable())?
            .take()
            .ok_or_else(|| invalid("Visualizer debug result is already being received"))?;
        receiver.await.map_err(|_| unavailable())?
    }
    /// Attaches to the exact existing parser at an idle operation boundary, preserving its heap.
    /// # Errors
    /// Rejects busy/replaced views, unavailable workers and another debugger owner.
    pub fn attach_visualizer_debugger(
        &self,
        owner: &str,
        handle: &VisualizerHandle,
        configuration: &DebugConfiguration,
    ) -> Result<DebugSessionInfo, AppError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| unavailable())?;
        let ticket = self.visualizers.ticket_binding(handle)?;
        let _operation = ticket.operation.try_lock().map_err(|_| {
            invalid("Visualizer is busy. Wait for its current operation to finish.")
        })?;
        let instance = ticket.current()?;
        let runner = self
            .visualizers
            .registry
            .runner(&self.visualizers.owner, &instance.id)
            .map_err(|_| unavailable())?;
        let snapshot = runner.snapshot();
        let info = DebugSessionInfo {
            session: String::new(),
            mode: "Existing visualizer".into(),
            label: format!(
                "{} · {} · {}",
                instance.extension_id, instance.visualizer_id, instance.digest
            ),
            artifact: instance.digest.clone(),
            source: source(&instance.declaration.entry)?,
        };
        let run = self
            .debuggers
            .create_package(owner, configuration, info, snapshot)?;
        if let Err(failure) = runner.attach_debugger(Arc::clone(&run.session)) {
            run.session.stop("Visualizer debugger could not attach");
            return Err(worker_failure(failure));
        }
        if let Ok(resources) = self
            .visualizers
            .registry
            .resources::<super::Resources>(&self.visualizers.owner, &instance.id)
            && let Some(feed) = resources.live.lock().map_err(|_| unavailable())?.as_ref()
        {
            feed.set_debug_clock(run.session.clock());
        }
        *ticket.debug.lock().map_err(|_| unavailable())? = Some(Arc::downgrade(&run));
        bind_lifetime(&run, &ticket, runtime);
        Ok(run.info.clone())
    }
}

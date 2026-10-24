//! Explicit visualizer activation above retention, with no capture-path callback.
use crate::{AppError, Application, BodyStore, ErrorCategory, inspector};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use transmog_core::intercept::ExchangeId;
use transmog_extension_registry::{Instance, Registry};
use transmog_extensions::{
    Invocation, ReadRequest, Surface, VisualizationDocument, VisualizationStep, decimal, valid_id,
};
mod admission;
mod context;
pub(crate) mod decoding;
mod evidence;
mod live;
mod reload;
use admission::Admission;
use evidence::{Resources, boundaries, project_input, source_infos};
pub use reload::VisualizerReload;
use transmog_session::SessionSnapshot;
#[cfg(test)]
mod tests;

/// Exact selected message and host surface; discovering choices reads no bodies.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualizerSelection {
    /// Retained session in this owning application.
    pub session_id: String,
    /// Exact request/response observation boundary.
    pub boundary: String,
    /// Host-owned display surface.
    pub surface: Surface,
}
/// A matching enabled contribution, discovered from inert declarations.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerChoice {
    /// Installation identity.
    pub extension_id: String,
    /// Independent contribution identity.
    pub visualizer_id: String,
    /// Plain selector label.
    pub name: String,
    /// This matching contribution opts into Auto parsing for the visible message.
    pub automatic: bool,
}
/// Explicit user selection; a slot allows cancellation before worker startup completes.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualizerActivation {
    /// UI-owned opaque pane identity, retained across its visible activations.
    pub slot: String,
    /// Canonical monotonically increasing pane generation, including close events.
    pub generation: String,
    /// Host-issued document epoch; a previous `WebView` page cannot start new work.
    pub page_id: String,
    /// Exact retained evidence and surface.
    pub selection: VisualizerSelection,
    /// Enabled package selected by the user.
    pub extension_id: String,
    /// Selected contribution, never every package entry.
    pub visualizer_id: String,
    /// Explicit opt-in for this fresh activation; omitted means snapshot-only.
    #[serde(default)]
    pub live: bool,
    /// Auto selection requires a declaration opt-in and a complete parse result.
    #[serde(default)]
    pub automatic: bool,
}
impl VisualizerActivation {
    fn handle(&self, instance: &Instance) -> VisualizerHandle {
        VisualizerHandle {
            slot: self.slot.clone(),
            generation: self.generation.clone(),
            instance_id: instance.id.clone(),
            page_id: self.page_id.clone(),
        }
    }
}
/// Validated host-owned document from one still-active instance.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerView {
    /// Native notification hint after a validated development revision was published.
    #[serde(skip)]
    pub metadata_changed: bool,
    /// Exact host binding for subsequent source and expansion requests.
    pub handle: VisualizerHandle,
    /// Exact executing revision and input declaration.
    pub instance: Instance,
    /// Inert tree/table/text/hex presentation data.
    pub document: VisualizationDocument,
    /// Host-owned names and extents for original sources, without reading their bytes.
    pub sources: Vec<VisualizerSourceInfo>,
    /// Native feed status; the extension cannot enable delivery itself.
    pub live: VisualizerLiveStatus,
}
/// Native opt-in feed state, without traffic content or access history.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerLiveStatus {
    /// Whether the declaration and selected live capture support updates.
    pub available: bool,
    /// Whether native admission is currently registered.
    pub enabled: bool,
    /// Exact count of lost update hints; their matching set may be uncertain.
    pub missed_hints: String,
}
/// One bounded poll result. Empty queues do not invoke extension code or read bodies.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerLiveUpdate {
    /// A validated replacement document, only when admitted traffic was parsed.
    pub view: Option<VisualizerView>,
    /// Current native admission and gap state.
    pub status: VisualizerLiveStatus,
    /// Another parser operation is active; no queued updates were consumed.
    pub busy: bool,
}
/// A source/action binding that cannot silently move to a newer worker or page.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualizerHandle {
    /// Owning pane identity.
    pub slot: String,
    /// Exact pane activation generation.
    pub generation: String,
    /// Exact executing worker, including after development reload.
    pub instance_id: String,
    /// Exact host document epoch.
    pub page_id: String,
}
/// Inert host-owned source labels for the renderer's byte/source selector.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerSourceInfo {
    /// Original granted source identity.
    pub id: String,
    /// Human-readable boundary/header label.
    pub name: String,
    /// Exact frozen retained extent.
    pub length: String,
    /// Original availability, independent of parse outcome.
    pub availability: String,
    /// Owning retained exchange in this application.
    pub session_id: String,
    /// Exact message boundary represented by these bytes.
    pub boundary: String,
    /// Host-owned HTTP transform and enclosing encoded-body provenance, when decoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_decoded: Option<transmog_extensions::DecodedBody>,
}
/// Bounded source bytes for host-owned span highlighting.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerSourcePage {
    /// Exact source capability in this active instance.
    pub source_id: String,
    /// Exact source length without JavaScript rounding.
    pub total_bytes: String,
    /// Returned source offset.
    pub offset: String,
    /// Inert binary source bytes.
    pub bytes_base64: String,
}

pub(super) struct Service {
    pub(super) registry: Registry,
    owner: String,
    closed: AtomicBool,
    page_id: AtomicU64,
    visible: AtomicBool,
    slots: Mutex<BTreeMap<String, Slot>>,
    live: Mutex<Option<Arc<live::Hub>>>,
}
struct Slot {
    generation: u64,
    ticket: Option<Arc<Ticket>>,
    candidate: Option<Arc<Ticket>>,
}
struct Ticket {
    registry: Registry,
    owner: String,
    canceled: AtomicBool,
    instance: Mutex<Option<Instance>>,
    generation: u64,
    activation: Mutex<Option<VisualizerActivation>>,
    reload_contract: Option<transmog_extensions::Visualizer>,
    operation: tokio::sync::Mutex<()>,
}
impl Slot {
    fn tickets(self) -> impl Iterator<Item = Arc<Ticket>> {
        self.ticket.into_iter().chain(self.candidate)
    }
    fn take_tickets(&mut self) -> impl Iterator<Item = Arc<Ticket>> + use<> {
        self.ticket.take().into_iter().chain(self.candidate.take())
    }
}
impl Ticket {
    fn new(
        registry: Registry,
        owner: String,
        generation: u64,
        reload_contract: Option<transmog_extensions::Visualizer>,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            owner,
            generation,
            reload_contract,
            canceled: AtomicBool::new(false),
            instance: Mutex::new(None),
            activation: Mutex::new(None),
            operation: tokio::sync::Mutex::new(()),
        })
    }
    fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
        let instance = self
            .instance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(instance) = instance {
            self.registry.deactivate(&self.owner, &instance.id);
        }
    }
    fn bind(&self, instance: Instance) -> Result<(), AppError> {
        let mut current = self.instance.lock().map_err(|_| unavailable())?;
        if self.canceled.load(Ordering::Acquire) {
            self.registry.deactivate(&self.owner, &instance.id);
            return Err(unavailable());
        }
        *current = Some(instance);
        Ok(())
    }
    fn current(&self) -> Result<Instance, AppError> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        self.instance
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Service {
    pub(super) fn new(registry: Registry, owner: String) -> Self {
        Self {
            registry,
            owner,
            closed: AtomicBool::new(false),
            page_id: AtomicU64::new(1),
            visible: AtomicBool::new(true),
            slots: Mutex::new(BTreeMap::new()),
            live: Mutex::new(None),
        }
    }
    fn begin(&self, slot: &str, generation: &str, page_id: &str) -> Result<Arc<Ticket>, AppError> {
        if !valid_id(slot) {
            return Err(invalid("Invalid visualizer activation identity"));
        }
        let generation =
            decimal(generation).map_err(|_| invalid("Invalid visualizer generation"))?;
        let ticket = Ticket::new(self.registry.clone(), self.owner.clone(), generation, None);
        let mut slots = self.slots.lock().map_err(|_| unavailable())?;
        if self.closed.load(Ordering::Acquire)
            || !self.visible.load(Ordering::Acquire)
            || !self.current_page(page_id)
            || slots.get(slot).is_some_and(|s| s.generation >= generation)
        {
            return Err(unavailable());
        }
        // Small generation tombstones contain no traffic. They prevent a close
        // arriving before startup from being forgotten, without retaining a worker.
        if (!slots.contains_key(slot) && slots.len() >= 128)
            || slots
                .iter()
                .filter(|(id, s)| id.as_str() != slot && s.ticket.is_some())
                .count()
                >= 16
        {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Close another visualizer before opening one",
                false,
            ));
        }
        let previous = slots.insert(
            slot.into(),
            Slot {
                generation,
                ticket: Some(Arc::clone(&ticket)),
                candidate: None,
            },
        );
        drop(slots);
        if let Some(previous) = previous {
            for ticket in previous.tickets() {
                ticket.cancel();
            }
        }
        Ok(ticket)
    }
    fn ticket_binding(&self, handle: &VisualizerHandle) -> Result<Arc<Ticket>, AppError> {
        let slot = &handle.slot;
        let generation = &handle.generation;
        let generation =
            decimal(generation).map_err(|_| invalid("Invalid visualizer generation"))?;
        let slots = self.slots.lock().map_err(|_| unavailable())?;
        if !self.current_page(&handle.page_id) || !self.visible.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let ticket = slots
            .get(slot)
            .and_then(|s| s.ticket.clone())
            .filter(|ticket| ticket.generation == generation)
            .ok_or_else(unavailable)?;
        if ticket.current()?.id != handle.instance_id {
            return Err(unavailable());
        }
        Ok(ticket)
    }
    fn cancel(&self, slot: &str, generation: &str, page_id: &str) {
        if !valid_id(slot) {
            return;
        }
        let Ok(generation) = decimal(generation) else {
            return;
        };
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.current_page(page_id) {
            return;
        }
        let tickets = if let Some(state) = slots.get_mut(slot) {
            if state.generation > generation {
                return;
            }
            state.generation = generation;
            state.take_tickets().collect::<Vec<_>>()
        } else {
            if slots.len() < 128 {
                slots.insert(
                    slot.into(),
                    Slot {
                        generation,
                        ticket: None,
                        candidate: None,
                    },
                );
            }
            Vec::new()
        };
        drop(slots);
        for ticket in tickets {
            ticket.cancel();
        }
    }
    fn cancel_all(&self) {
        self.closed.store(true, Ordering::Release);
        let slots = std::mem::take(
            &mut *self
                .slots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for ticket in slots.into_values().flat_map(Slot::tickets) {
            ticket.cancel();
        }
    }
    /// Operation failure belongs to a ticket, including after failed reloads advanced the watermark.
    fn cancel_ticket(&self, handle: &VisualizerHandle, ticket: &Arc<Ticket>) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.current_page(&handle.page_id) {
            return;
        }
        let tickets = slots
            .get_mut(&handle.slot)
            .filter(|slot| {
                slot.ticket
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, ticket))
            })
            .map(|slot| slot.take_tickets().collect::<Vec<_>>())
            .unwrap_or_default();
        drop(slots);
        for ticket in tickets {
            ticket.cancel();
        }
    }
    fn current_page(&self, page_id: &str) -> bool {
        decimal(page_id).is_ok_and(|id| id == self.page_id.load(Ordering::Acquire))
    }
    fn reset_document(&self) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.page_id.fetch_add(1, Ordering::AcqRel);
        let previous = std::mem::take(&mut *slots);
        drop(slots);
        for ticket in previous.into_values().flat_map(Slot::tickets) {
            ticket.cancel();
        }
    }
    fn visible(&self, visible: bool) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.visible.store(visible, Ordering::Release);
        let stopped = if visible {
            Vec::new()
        } else {
            slots
                .values_mut()
                .flat_map(Slot::take_tickets)
                .collect::<Vec<_>>()
        };
        drop(slots);
        for ticket in stopped {
            ticket.cancel();
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

impl Application {
    /// Shared metadata-only installation registry, also used by other windows.
    pub fn extensions(&self) -> Registry {
        self.visualizers.registry.clone()
    }

    /// Offers only enabled contributions whose native selectors match this exact boundary.
    /// # Errors
    /// Rejects unavailable evidence and invalid boundaries without loading extension code.
    /// An empty enabled registry returns no choices without looking up retained evidence.
    pub fn visualizer_choices(
        &self,
        selection: &VisualizerSelection,
    ) -> Result<Vec<VisualizerChoice>, AppError> {
        let declarations = self.visualizers.registry.enabled_visualizers();
        if declarations.is_empty() {
            return Ok(Vec::new());
        }
        let (snapshot, _) = self.visualizer_evidence(&selection.session_id)?;
        let selected = boundaries(&selection.boundary)?;
        let admission = Admission::new(&snapshot, selected);
        let mut choices = Vec::new();
        for (extension_id, declaration) in declarations {
            if admission.applicable(&declaration, selection.surface) {
                choices.push(VisualizerChoice {
                    extension_id,
                    visualizer_id: declaration.id,
                    name: declaration.name,
                    automatic: declaration.automatic,
                });
            }
        }
        Ok(choices)
    }

    /// Activates one explicitly selected visualizer and resolves only authorized byte continuations.
    /// # Errors
    /// Returns stale selection, unavailable retained evidence or bounded parser failures.
    pub async fn activate_visualizer(
        &self,
        request: VisualizerActivation,
    ) -> Result<VisualizerView, AppError> {
        let ticket =
            self.visualizers
                .begin(&request.slot, &request.generation, &request.page_id)?;
        let result = async {
            let mut view = self.activate_visualizer_ticket(&request, &ticket).await?;
            view.metadata_changed = self.commit_visualizer_document(&ticket).await?;
            Ok(view)
        }
        .await;
        if result.is_err() {
            self.visualizers
                .cancel(&request.slot, &request.generation, &request.page_id);
        }
        result
    }
    async fn initialize_visualizer_ticket(
        &self,
        request: &VisualizerActivation,
        ticket: &Arc<Ticket>,
    ) -> Result<Instance, AppError> {
        *ticket.activation.lock().map_err(|_| unavailable())? = Some(request.clone());
        // Native admission happens before reading any source module or body metadata.
        if !self
            .visualizer_choices(&request.selection)?
            .iter()
            .any(|choice| {
                choice.extension_id == request.extension_id
                    && choice.visualizer_id == request.visualizer_id
                    && (!request.automatic || choice.automatic)
            })
        {
            return Err(invalid(
                "The selected visualizer does not match this message",
            ));
        }
        let registry = self.visualizers.registry.clone();
        let owner = self.visualizers.owner.clone();
        let extension = request.extension_id.clone();
        let contribution = request.visualizer_id.clone();
        let startup = Arc::clone(ticket);
        let instance = tokio::task::spawn_blocking(move || {
            registry.activate_cancellable(&owner, &extension, &contribution, || {
                startup.canceled.load(Ordering::Acquire)
            })
        })
        .await
        .map_err(|_| unavailable())?
        .map_err(worker_failure)?;
        ticket.bind(instance.clone())?;
        Ok(instance)
    }
    async fn activate_visualizer_ticket(
        &self,
        request: &VisualizerActivation,
        ticket: &Arc<Ticket>,
    ) -> Result<VisualizerView, AppError> {
        let instance = self.initialize_visualizer_ticket(request, ticket).await?;
        if request.automatic && !instance.declaration.automatic {
            return Err(invalid("This visualizer no longer supports Auto selection"));
        }
        let (mut snapshot, store) = self.visualizer_evidence(&request.selection.session_id)?;
        let selected = boundaries(&request.selection.boundary)?;
        if !Admission::new(&snapshot, selected)
            .applicable(&instance.declaration, request.selection.surface)
        {
            return Err(invalid(
                "The visualizer declaration changed and no longer matches this message",
            ));
        }
        let live_available = live::available(self, &snapshot, selected, &instance.declaration);
        let mut effective = request.clone();
        effective.live &= ticket
            .reload_contract
            .as_ref()
            .is_none_or(|old| old == &instance.declaration);
        let feed = self.start_visualizer_feed(
            &effective,
            &instance,
            &mut snapshot,
            selected,
            store.as_ref(),
        )?;
        let mut sources = BTreeMap::new();
        let input = project_input(
            &snapshot,
            selected,
            &instance.declaration,
            store.as_ref(),
            &instance.id,
            &mut sources,
        )?;
        let resources = Arc::new(Resources {
            live_available,
            finalized: AtomicBool::new(false),
            live: Mutex::new(feed),
            stopped: AtomicBool::new(false),
            sources: Mutex::new(
                sources
                    .into_iter()
                    .map(|(id, source)| (id, Arc::new(source)))
                    .collect(),
            ),
            info: Mutex::new(source_infos(&snapshot, selected, &input)),
            context: Mutex::new(instance.declaration.context.as_ref().map(|_| {
                context::Broker::new(
                    self,
                    &snapshot,
                    selected,
                    &instance.declaration,
                    &instance.id,
                    store,
                )
            })),
        });
        self.visualizers
            .registry
            .attach_resources(
                &self.visualizers.owner,
                &instance.id,
                Arc::clone(&resources),
            )
            .map_err(|_| unavailable())?;
        let invocation = Invocation::Visualize {
            input: Box::new(input),
        };
        let mut document = self.run_visualizer(ticket, invocation).await?;
        let live = resources.finish_document(&mut document);
        if request.automatic && document.status != transmog_extensions::ParseStatus::Complete {
            return Err(invalid("Auto visualization requires a complete parse"));
        }
        Ok(VisualizerView {
            metadata_changed: false,
            handle: request.handle(&instance),
            instance,
            document,
            sources: resources.info()?,
            live,
        })
    }

    /// Expands one deferred node without granting new evidence or broader input access.
    /// # Errors
    /// Rejects inactive instances, invalid tokens and bounded parser failures.
    pub async fn expand_visualizer(
        &self,
        handle: &VisualizerHandle,
        token: String,
    ) -> Result<VisualizerView, AppError> {
        let ticket = self.visualizers.ticket_binding(handle)?;
        let result = self
            .run_visualizer(&ticket, Invocation::Expand { token })
            .await;
        match result {
            Ok(mut document) => {
                let instance = ticket.current()?;
                let resources = self
                    .visualizers
                    .registry
                    .resources::<Resources>(&self.visualizers.owner, &instance.id)
                    .map_err(|_| unavailable())?;
                let live = resources.finish_document(&mut document);
                Ok(VisualizerView {
                    metadata_changed: false,
                    handle: handle.clone(),
                    instance,
                    document,
                    sources: resources.info()?,
                    live,
                })
            }
            Err(error) => {
                self.visualizers.cancel_ticket(handle, &ticket);
                Err(error)
            }
        }
    }
    async fn commit_visualizer_document(&self, ticket: &Ticket) -> Result<bool, AppError> {
        let registry = self.visualizers.registry.clone();
        let owner = self.visualizers.owner.clone();
        let worker = ticket.current()?.id;
        let changed =
            tokio::task::spawn_blocking(move || registry.commit_document(&owner, &worker))
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
        ticket.current()?;
        Ok(changed)
    }
    async fn run_visualizer(
        &self,
        ticket: &Ticket,
        invocation: Invocation,
    ) -> Result<VisualizationDocument, AppError> {
        let _operation = ticket
            .operation
            .try_lock()
            .map_err(|_| invalid("Another visualizer operation is still running"))?;
        self.invoke_visualizer(ticket, invocation).await
    }
    async fn invoke_visualizer(
        &self,
        ticket: &Ticket,
        mut invocation: Invocation,
    ) -> Result<VisualizationDocument, AppError> {
        let instance = ticket.current()?;
        let runner = self
            .visualizers
            .registry
            .runner(&self.visualizers.owner, &instance.id)
            .map_err(|_| unavailable())?;
        let resources = self
            .visualizers
            .registry
            .resources::<Resources>(&self.visualizers.owner, &instance.id)
            .map_err(|_| unavailable())?;
        let operation = async {
            for _ in 0..512 {
                if resources.stopped.load(Ordering::Acquire)
                    || ticket.canceled.load(Ordering::Acquire)
                {
                    return Err(unavailable());
                }
                let step = runner.invoke(invocation).await.map_err(worker_failure)?;
                if resources.stopped.load(Ordering::Acquire)
                    || ticket.canceled.load(Ordering::Acquire)
                {
                    return Err(unavailable());
                }
                match step {
                    VisualizationStep::Document { mut document } => {
                        resources.finish_document(&mut document);
                        return Ok(document);
                    }
                    VisualizationStep::NeedInput { reads, token } => {
                        let resources = Arc::clone(&resources);
                        let pages = tokio::task::spawn_blocking(move || {
                            reads
                                .iter()
                                .map(|read| resources.read(read).map(|(_, page)| page))
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .await
                        .map_err(|_| unavailable())??;
                        invocation = Invocation::Resume { token, pages };
                    }
                    VisualizationStep::NeedContext { request, token } => {
                        let resources = Arc::clone(&resources);
                        let page = tokio::task::spawn_blocking(move || resources.context(&request))
                            .await
                            .map_err(|_| unavailable())??;
                        invocation = Invocation::ResumeContext { token, page };
                    }
                    VisualizationStep::NeedDecoded { source_id, token } => {
                        let result = resources
                            .decode_http(&source_id, instance.declaration.limits.input_bytes)
                            .await?;
                        invocation = Invocation::ResumeDecoded { token, result };
                    }
                }
            }
            Err(AppError::new(
                ErrorCategory::Limit,
                "The visualizer requested too many byte pages",
                false,
            ))
        };
        tokio::time::timeout(
            std::time::Duration::from_millis(instance.declaration.limits.duration_ms),
            operation,
        )
        .await
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Limit,
                "The visualizer operation timed out",
                false,
            )
        })?
    }

    /// Reads one granted source page for the host renderer, never an arbitrary retained body.
    /// # Errors
    /// Rejects inactive, unknown or out-of-range sources.
    pub async fn visualizer_source(
        &self,
        handle: &VisualizerHandle,
        request: ReadRequest,
    ) -> Result<VisualizerSourcePage, AppError> {
        let ticket = self.visualizers.ticket_binding(handle)?;
        let instance = ticket.current()?;
        let resources = self
            .visualizers
            .registry
            .resources::<Resources>(&self.visualizers.owner, &instance.id)
            .map_err(|_| unavailable())?;
        let (length, page) = tokio::task::spawn_blocking(move || resources.read(&request))
            .await
            .map_err(|_| unavailable())??;
        ticket.current()?;
        Ok(VisualizerSourcePage {
            source_id: page.source_id,
            offset: page.offset,
            total_bytes: length.to_string(),
            bytes_base64: STANDARD.encode(page.bytes),
        })
    }
    /// Immediately cancels startup, pending work and retained resources for one hidden pane.
    pub fn deactivate_visualizer(&self, slot: &str, generation: &str, page_id: &str) {
        self.visualizers.cancel(slot, generation, page_id);
    }
    /// Returns the current host-issued document epoch, without loading any code.
    pub fn visualizer_page(&self) -> String {
        self.visualizers.page_id.load(Ordering::Acquire).to_string()
    }
    pub(crate) fn visualizer_snapshot_current(&self, page_id: &str) -> bool {
        !self.visualizers.closed.load(Ordering::Acquire)
            && self.visualizers.visible.load(Ordering::Acquire)
            && self.visualizers.current_page(page_id)
    }
    /// Invalidates the prior `WebView` document's workers, queued starts and source capabilities.
    pub fn reset_visualizer_document(&self) {
        self.auth.reset(None);
        self.visualizers.reset_document();
        self.comparisons.cancel_fields();
    }
    /// Native visibility gate. Hiding/minimizing stops all workers; focus changes alone do not.
    pub fn set_visualizers_visible(&self, visible: bool) {
        self.auth.reset(Some(visible));
        self.visualizers.visible(visible);
        if !visible {
            self.comparisons.cancel_fields();
        }
    }
    /// Whether an exact visible binding still owns a ready worker.
    pub fn visualizer_active(&self, handle: &VisualizerHandle) -> bool {
        self.visualizers
            .ticket_binding(handle)
            .ok()
            .and_then(|ticket| ticket.current().ok())
            .and_then(|instance| {
                self.visualizers
                    .registry
                    .runner(&self.visualizers.owner, &instance.id)
                    .ok()
            })
            .is_some_and(|runner| !runner.is_stopped())
    }
    /// Unloads every visible or starting instance when this application/window closes.
    pub fn deactivate_visualizers(&self) {
        self.auth.reset(Some(false));
        self.visualizers.cancel_all();
        self.comparisons.cancel_fields();
    }

    pub(crate) fn visualizer_evidence(
        &self,
        id: &str,
    ) -> Result<(SessionSnapshot, Option<BodyStore>), AppError> {
        if let Some(response) = self.composer.response(id) {
            return Ok((response.snapshot.clone(), Some(response.store.clone())));
        }
        let exchange = ExchangeId(inspector::parse_session_id(id)?);
        let snapshot = self
            .service
            .catalog()
            .get(exchange)
            .ok_or_else(unavailable)?;
        Ok((snapshot, self.body_store.clone()))
    }
}

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn unavailable() -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        "The selected visualizer or retained source is no longer available",
        false,
    )
}
fn worker_failure(error: transmog_extensions::Failure) -> AppError {
    let category = match error.kind {
        transmog_extensions::FailureKind::Limit | transmog_extensions::FailureKind::Timeout => {
            ErrorCategory::Limit
        }
        transmog_extensions::FailureKind::Validation
        | transmog_extensions::FailureKind::Protocol => ErrorCategory::InvalidInput,
        _ => ErrorCategory::Unavailable,
    };
    AppError::new(category, error.message, false)
}

//! Explicit visualizer activation above retention, with no capture-path callback.
use crate::{AppError, Application, BodyStore, ErrorCategory, inspector};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_core::intercept::ExchangeId;
use transmog_extension_registry::{Instance, Registry};
use transmog_extensions::{
    Invocation, ReadRequest, Surface, VisualizationDocument, VisualizationStep, decimal, valid_id,
};
mod evidence;
use evidence::{Resources, applicable, boundaries, project_input};
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
}
/// Explicit user selection; a slot allows cancellation before worker startup completes.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualizerActivation {
    /// UI-owned opaque pane identity, retained across its visible activations.
    pub slot: String,
    /// Canonical monotonically increasing pane generation, including close events.
    pub generation: String,
    /// Exact retained evidence and surface.
    pub selection: VisualizerSelection,
    /// Enabled package selected by the user.
    pub extension_id: String,
    /// Selected contribution, never every package entry.
    pub visualizer_id: String,
}
/// Validated host-owned document from one still-active instance.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerView {
    /// Exact executing revision and input declaration.
    pub instance: Instance,
    /// Inert tree/table/text/hex presentation data.
    pub document: VisualizationDocument,
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
    slots: Mutex<BTreeMap<String, Slot>>,
}
struct Slot {
    generation: u64,
    ticket: Option<Arc<Ticket>>,
}
struct Ticket {
    registry: Registry,
    owner: String,
    canceled: AtomicBool,
    instance: Mutex<Option<Instance>>,
    operation: tokio::sync::Mutex<()>,
}
impl Ticket {
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
            slots: Mutex::new(BTreeMap::new()),
        }
    }
    fn begin(&self, slot: &str, generation: &str) -> Result<Arc<Ticket>, AppError> {
        if !valid_id(slot) {
            return Err(invalid("Invalid visualizer activation identity"));
        }
        let generation =
            decimal(generation).map_err(|_| invalid("Invalid visualizer generation"))?;
        let ticket = Arc::new(Ticket {
            registry: self.registry.clone(),
            owner: self.owner.clone(),
            canceled: AtomicBool::new(false),
            instance: Mutex::new(None),
            operation: tokio::sync::Mutex::new(()),
        });
        let mut slots = self.slots.lock().map_err(|_| unavailable())?;
        if self.closed.load(Ordering::Acquire)
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
            },
        );
        drop(slots);
        if let Some(previous) = previous.and_then(|s| s.ticket) {
            previous.cancel();
        }
        Ok(ticket)
    }
    fn ticket_generation(&self, slot: &str, generation: &str) -> Result<Arc<Ticket>, AppError> {
        let generation =
            decimal(generation).map_err(|_| invalid("Invalid visualizer generation"))?;
        self.slots
            .lock()
            .map_err(|_| unavailable())?
            .get(slot)
            .filter(|s| s.generation == generation)
            .and_then(|s| s.ticket.clone())
            .ok_or_else(unavailable)
    }
    fn cancel(&self, slot: &str, generation: &str) {
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
        let ticket = if let Some(state) = slots.get_mut(slot) {
            if state.generation > generation {
                return;
            }
            state.generation = generation;
            state.ticket.take()
        } else {
            if slots.len() < 128 {
                slots.insert(
                    slot.into(),
                    Slot {
                        generation,
                        ticket: None,
                    },
                );
            }
            None
        };
        drop(slots);
        if let Some(ticket) = ticket {
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
        for ticket in slots.into_values().filter_map(|s| s.ticket) {
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
    pub fn visualizer_choices(
        &self,
        selection: &VisualizerSelection,
    ) -> Result<Vec<VisualizerChoice>, AppError> {
        let (snapshot, _) = self.visualizer_evidence(&selection.session_id)?;
        let selected = boundaries(&selection.boundary)?;
        let mut choices = Vec::new();
        for entry in self
            .visualizers
            .registry
            .status()
            .entries
            .into_iter()
            .filter(|e| e.catalog.enabled)
        {
            for declaration in entry.catalog.manifest.visualizers {
                let Ok(input) = project_input(
                    &snapshot,
                    selected,
                    &declaration,
                    None,
                    "discovery",
                    &mut BTreeMap::new(),
                ) else {
                    continue;
                };
                if applicable(&declaration, selection.surface, &input) {
                    choices.push(VisualizerChoice {
                        extension_id: entry.catalog.manifest.id.clone(),
                        visualizer_id: declaration.id,
                        name: declaration.name,
                    });
                }
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
        let ticket = self.visualizers.begin(&request.slot, &request.generation)?;
        let result = self.activate_visualizer_ticket(&request, &ticket).await;
        if result.is_err() {
            self.visualizers.cancel(&request.slot, &request.generation);
        }
        result
    }
    async fn activate_visualizer_ticket(
        &self,
        request: &VisualizerActivation,
        ticket: &Arc<Ticket>,
    ) -> Result<VisualizerView, AppError> {
        // Native admission happens before reading any source module or body metadata.
        if !self
            .visualizer_choices(&request.selection)?
            .iter()
            .any(|choice| {
                choice.extension_id == request.extension_id
                    && choice.visualizer_id == request.visualizer_id
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
        let (snapshot, store) = self.visualizer_evidence(&request.selection.session_id)?;
        let selected = boundaries(&request.selection.boundary)?;
        let metadata = project_input(
            &snapshot,
            selected,
            &instance.declaration,
            None,
            "discovery",
            &mut BTreeMap::new(),
        )?;
        if !applicable(&instance.declaration, request.selection.surface, &metadata) {
            return Err(invalid(
                "The visualizer declaration changed and no longer matches this message",
            ));
        }
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
            stopped: AtomicBool::new(false),
            sources: Mutex::new(
                sources
                    .into_iter()
                    .map(|(id, source)| (id, Arc::new(source)))
                    .collect(),
            ),
        });
        self.visualizers
            .registry
            .attach_resources(&self.visualizers.owner, &instance.id, resources)
            .map_err(|_| unavailable())?;
        let document = self
            .run_visualizer(
                ticket,
                Invocation::Visualize {
                    input: Box::new(input),
                },
            )
            .await?;
        Ok(VisualizerView { instance, document })
    }

    /// Expands one deferred node without granting new evidence or broader input access.
    /// # Errors
    /// Rejects inactive instances, invalid tokens and bounded parser failures.
    pub async fn expand_visualizer(
        &self,
        slot: &str,
        generation: &str,
        token: String,
    ) -> Result<VisualizerView, AppError> {
        let ticket = self.visualizers.ticket_generation(slot, generation)?;
        let result = self
            .run_visualizer(&ticket, Invocation::Expand { token })
            .await;
        match result {
            Ok(document) => Ok(VisualizerView {
                instance: ticket.current()?,
                document,
            }),
            Err(error) => {
                self.visualizers.cancel(slot, generation);
                Err(error)
            }
        }
    }
    async fn run_visualizer(
        &self,
        ticket: &Ticket,
        mut invocation: Invocation,
    ) -> Result<VisualizationDocument, AppError> {
        let _operation = ticket
            .operation
            .try_lock()
            .map_err(|_| invalid("Another visualizer operation is still running"))?;
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
                    VisualizationStep::Document { document } => return Ok(document),
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
        slot: &str,
        generation: &str,
        request: ReadRequest,
    ) -> Result<VisualizerSourcePage, AppError> {
        let ticket = self.visualizers.ticket_generation(slot, generation)?;
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
    pub fn deactivate_visualizer(&self, slot: &str, generation: &str) {
        self.visualizers.cancel(slot, generation);
    }
    /// Unloads every visible or starting instance when this application/window closes.
    pub fn deactivate_visualizers(&self) {
        self.visualizers.cancel_all();
    }

    fn visualizer_evidence(
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

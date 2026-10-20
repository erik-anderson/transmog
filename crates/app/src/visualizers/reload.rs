//! Candidate revisions retain the current worker until validated replacement succeeds.
use super::{
    AppError, Application, Service, Ticket, VisualizerHandle, VisualizerView, invalid, unavailable,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, MutexGuard, atomic::Ordering},
};

/// Explicit replacement of one still-visible development visualization.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualizerReload {
    /// Exact result being replaced; it remains usable while the candidate runs.
    pub handle: VisualizerHandle,
    /// A fresh pane generation, also used when canceling an in-flight reload.
    pub generation: String,
}

impl Service {
    fn begin_reload(
        &self,
        handle: &VisualizerHandle,
        generation: &str,
    ) -> Result<Arc<Ticket>, AppError> {
        let current = self.ticket_binding(handle)?;
        let generation = transmog_extensions::decimal(generation)
            .map_err(|_| invalid("Invalid visualizer generation"))?;
        let contract = current.current()?.declaration;
        let mut slots = self.slots.lock().map_err(|_| unavailable())?;
        let slot = slots.get_mut(&handle.slot).ok_or_else(unavailable)?;
        if generation <= slot.generation
            || !slot
                .ticket
                .as_ref()
                .is_some_and(|ticket| Arc::ptr_eq(ticket, &current))
            || !self.visible.load(Ordering::Acquire)
            || !self.current_page(&handle.page_id)
        {
            return Err(unavailable());
        }
        let candidate = Ticket::new(
            self.registry.clone(),
            self.owner.clone(),
            generation,
            Some(contract),
        );
        let previous = slot.candidate.replace(Arc::clone(&candidate));
        slot.generation = generation;
        drop(slots);
        if let Some(previous) = previous {
            previous.cancel();
        }
        Ok(candidate)
    }
    fn discard_reload(&self, slot: &str, candidate: &Arc<Ticket>) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = slots.get_mut(slot)
            && slot
                .candidate
                .as_ref()
                .is_some_and(|ticket| Arc::ptr_eq(ticket, candidate))
        {
            slot.candidate = None;
        }
        drop(slots);
        candidate.cancel();
    }
    fn reload_guard(
        &self,
        handle: &VisualizerHandle,
        candidate: &Arc<Ticket>,
    ) -> Result<MutexGuard<'_, BTreeMap<String, super::Slot>>, AppError> {
        let mut slots = self.slots.lock().map_err(|_| unavailable())?;
        let slot = slots.get_mut(&handle.slot).ok_or_else(unavailable)?;
        if !self.visible.load(Ordering::Acquire)
            || !self.current_page(&handle.page_id)
            || !slot.ticket.as_ref().is_some_and(|ticket| {
                ticket.generation == transmog_extensions::decimal(&handle.generation).unwrap_or(0)
                    && ticket
                        .current()
                        .is_ok_and(|instance| instance.id == handle.instance_id)
            })
            || !slot
                .candidate
                .as_ref()
                .is_some_and(|ticket| Arc::ptr_eq(ticket, candidate))
            || candidate.canceled.load(Ordering::Acquire)
        {
            return Err(unavailable());
        }
        candidate.current()?;
        Ok(slots)
    }
    fn replace_reload(
        handle: &VisualizerHandle,
        candidate: &Arc<Ticket>,
        mut slots: MutexGuard<'_, BTreeMap<String, super::Slot>>,
    ) -> Result<(), AppError> {
        let slot = slots.get_mut(&handle.slot).ok_or_else(unavailable)?;
        let previous = slot.ticket.replace(Arc::clone(candidate));
        slot.candidate = None;
        drop(slots);
        if let Some(previous) = previous {
            previous.cancel();
        }
        Ok(())
    }
    /// Retains the pane guard across the final atomic catalog rename and pointer swap only.
    fn commit_reload(
        &self,
        handle: &VisualizerHandle,
        candidate: &Arc<Ticket>,
    ) -> Result<bool, AppError> {
        let (changed, slots) = self
            .registry
            .commit_document_guarded(&self.owner, &candidate.current()?.id, || {
                self.reload_guard(handle, candidate)
                    .map_err(|_| transmog_extensions::Error::AccessDenied)
            })
            .map_err(|_| unavailable())?;
        Self::replace_reload(handle, candidate, slots)?;
        Ok(changed)
    }
    #[cfg(test)]
    fn commit_reload_for_test(
        &self,
        handle: &VisualizerHandle,
        candidate: &Arc<Ticket>,
        publish: impl FnOnce() -> Result<bool, AppError>,
    ) -> Result<bool, AppError> {
        let slots = self.reload_guard(handle, candidate)?;
        let changed = publish()?;
        Self::replace_reload(handle, candidate, slots)?;
        Ok(changed)
    }
}

impl Application {
    /// Polls native source metadata notifications for one exact visible development result.
    /// # Errors
    /// Rejects inactive bindings and unavailable installed entries, without reading captured data.
    pub async fn visualizer_reload_status(
        &self,
        handle: &VisualizerHandle,
    ) -> Result<transmog_extension_registry::ReloadStatus, AppError> {
        let ticket = self.visualizers.ticket_binding(handle)?;
        let registry = self.visualizers.registry.clone();
        let owner = self.visualizers.owner.clone();
        let instance = ticket.current()?.id;
        let status = tokio::task::spawn_blocking(move || registry.reload_status(&owner, &instance))
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
        ticket.current()?;
        Ok(status)
    }
    /// Builds a fresh isolated revision and atomically replaces a visible development instance.
    /// # Errors
    /// Preserves the previous result on source, sandbox, parsing, publication or capacity failures.
    pub async fn reload_visualizer(
        &self,
        request: VisualizerReload,
    ) -> Result<VisualizerView, AppError> {
        let previous = self.visualizers.ticket_binding(&request.handle)?;
        if !self
            .visualizers
            .registry
            .development_instance(&self.visualizers.owner, &request.handle.instance_id)
        {
            return Err(invalid(
                "Reload requires an active unpacked development extension",
            ));
        }
        let mut activation = previous
            .activation
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        let resources = self
            .visualizers
            .registry
            .resources::<super::Resources>(&self.visualizers.owner, &request.handle.instance_id)
            .map_err(|_| unavailable())?;
        activation.live = resources.live_status().enabled;
        activation.generation.clone_from(&request.generation);
        let candidate = self
            .visualizers
            .begin_reload(&request.handle, &request.generation)?;
        self.visualizers
            .registry
            .acknowledge_reload(&self.visualizers.owner, &request.handle.instance_id);
        let result = async {
            let mut view = self
                .activate_visualizer_ticket(&activation, &candidate)
                .await?;
            let service = Arc::clone(&self.visualizers);
            let commit = Arc::clone(&candidate);
            let handle = request.handle.clone();
            view.metadata_changed =
                tokio::task::spawn_blocking(move || service.commit_reload(&handle, &commit))
                    .await
                    .map_err(|_| unavailable())??;
            candidate.current()?;
            Ok(view)
        }
        .await;
        if result.is_err() {
            self.visualizers
                .discard_reload(&request.handle.slot, &candidate);
        }
        result
    }
}

#[cfg(test)]
mod tests;

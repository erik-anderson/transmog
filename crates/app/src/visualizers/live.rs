//! One native metadata subscription per owning application, only while live visualizers exist.
use super::{
    AppError,
    context::{RelatedGuard, live_index_scope},
    evidence::Boundaries,
    invalid, unavailable,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use transmog_core::{intercept::ExchangeId, observe::ExchangeBoundary};
use transmog_extensions::{ActiveSubscriptions, BodyAccess, Direction, TrafficPhase, Visualizer};
use transmog_session::{CatalogChange, CatalogDelta, SessionCatalog, SubscriptionEvent};

const QUEUE_EVENTS: usize = 32;
pub(super) fn available(
    app: &crate::Application,
    snapshot: &transmog_session::SessionSnapshot,
    selected: Boundaries,
    declaration: &Visualizer,
) -> bool {
    declaration.live_updates.is_some()
        && !snapshot.imported
        && RelatedGuard::new(app, snapshot, selected, declaration).is_some()
}
#[cfg(test)]
mod tests;
#[derive(Clone, Copy)]
pub(super) struct Queued {
    pub(super) sequence: u64,
    pub(super) exchange: ExchangeId,
    pub(super) order: u64,
    pub(super) revision: u64,
    pub(super) direction: Direction,
    pub(super) phase: TrafficPhase,
    ready_since: std::time::Instant,
}
#[derive(Default)]
struct Queue {
    serial: u64,
    pending: BTreeMap<u64, Queued>,
}
pub(super) struct Feed {
    pub(super) guard: Arc<RelatedGuard>,
    pub(super) selected: Boundaries,
    pub(super) declaration: Visualizer,
    pub(super) store: Option<crate::BodyStore>,
    anchor: ExchangeId,
    baseline: AtomicU64,
    enabled: AtomicBool,
    missed: AtomicU64,
    queue: Mutex<Queue>,
    subscription: Mutex<Option<Subscription>>,
}
impl Feed {
    pub(super) fn new(
        guard: Arc<RelatedGuard>,
        selected: Boundaries,
        declaration: Visualizer,
        anchor: ExchangeId,
        store: Option<crate::BodyStore>,
    ) -> Arc<Self> {
        let store = store.filter(|_| {
            declaration.inputs.request.body != BodyAccess::None
                || declaration.inputs.response.body != BodyAccess::None
        });
        Arc::new(Self {
            guard,
            selected,
            declaration,
            store,
            anchor,
            baseline: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
            missed: AtomicU64::new(0),
            queue: Mutex::new(Queue::default()),
            subscription: Mutex::new(None),
        })
    }
    pub(super) fn baseline(&self, revision: u64) {
        self.baseline.store(revision, Ordering::Release);
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .retain(|_, event| event.exchange != self.anchor || event.revision > revision);
    }
    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub(super) fn missed(&self) -> u64 {
        self.missed.load(Ordering::Acquire)
    }
    fn lag(&self, count: u64) {
        let _ = self
            .missed
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |prior| {
                Some(prior.saturating_add(count))
            });
    }
    pub(super) fn stop(&self) {
        self.enabled.store(false, Ordering::Release);
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .clear();
        self.subscription
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
    fn accepts(&self, change: CatalogChange, direction: Direction) -> bool {
        if !self.enabled() {
            return false;
        }
        let Some(live) = &self.declaration.live_updates else {
            return false;
        };
        if !live.events.contains(&direction) {
            return false;
        }
        match change {
            CatalogChange::Body(_) => {
                live.body_progress
                    && self.declaration.inputs.side(direction).body != BodyAccess::None
            }
            CatalogChange::BodyCompleted(_) => {
                self.declaration.inputs.side(direction).body != BodyAccess::None
            }
            CatalogChange::Head(_) | CatalogChange::Terminal => true,
            CatalogChange::Metadata => false,
        }
    }
    fn offer(&self, mut event: Queued) {
        if !self.enabled()
            || (event.exchange == self.anchor
                && event.revision <= self.baseline.load(Ordering::Acquire))
        {
            return;
        }
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.enabled()
            || (event.exchange == self.anchor
                && event.revision <= self.baseline.load(Ordering::Acquire))
        {
            return;
        }
        queue.pending.retain(|_, prior| {
            prior.exchange != event.exchange || prior.direction != event.direction
        });
        if queue.pending.len() == QUEUE_EVENTS {
            queue.pending.pop_first();
            self.lag(1);
        }
        let Some(sequence) = queue.serial.checked_add(1) else {
            self.lag(1);
            return;
        };
        queue.serial = sequence;
        event.sequence = sequence;
        queue.pending.insert(sequence, event);
    }
    pub(super) fn take(&self) -> Vec<Queued> {
        if !self.enabled() {
            return Vec::new();
        }
        std::mem::take(
            &mut self
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pending,
        )
        .into_values()
        .collect()
    }
    fn has_pending(&self) -> bool {
        !self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .is_empty()
    }
    pub(super) fn restore(&self, events: impl IntoIterator<Item = Queued>) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.enabled() {
            return;
        }
        for event in events {
            // A newer coalesced event supersedes this deferred older revision.
            if queue.pending.values().any(|current| {
                current.exchange == event.exchange && current.direction == event.direction
            }) {
                continue;
            }
            queue.pending.insert(event.sequence, event);
            while queue.pending.len() > QUEUE_EVENTS {
                queue.pending.pop_first();
                self.lag(1);
            }
        }
    }
}
impl Drop for Feed {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Subscription {
    hub: Weak<Hub>,
    instance: String,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.remove(&self.instance);
        }
    }
}
#[derive(Default)]
struct State {
    index: ActiveSubscriptions,
    feeds: BTreeMap<String, Weak<Feed>>,
    epoch: u64,
}
pub(super) struct Hub {
    catalog: SessionCatalog,
    owner: String,
    state: Mutex<State>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}
impl Hub {
    pub(super) fn new(catalog: SessionCatalog, owner: String) -> Arc<Self> {
        Arc::new(Self {
            catalog,
            owner,
            state: Mutex::new(State::default()),
            task: Mutex::new(None),
        })
    }
    pub(super) fn subscribe(
        self: &Arc<Self>,
        instance: &str,
        feed: &Arc<Feed>,
    ) -> Result<(), AppError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let receiver = state
            .feeds
            .is_empty()
            .then(|| self.catalog.subscribe())
            .transpose()
            .map_err(|_| unavailable())?;
        if receiver.is_some() {
            state.epoch = state.epoch.checked_add(1).ok_or_else(unavailable)?;
        }
        let epoch = state.epoch;
        state
            .index
            .subscribe(
                instance.into(),
                feed.guard.index_scope(),
                feed.declaration.clone(),
            )
            .map_err(|_| {
                invalid("Live updates were not declared or no feed slots are available")
            })?;
        state.feeds.insert(instance.into(), Arc::downgrade(feed));
        *feed.subscription.lock().map_err(|_| unavailable())? = Some(Subscription {
            hub: Arc::downgrade(self),
            instance: instance.into(),
        });
        if let Some(mut receiver) = receiver {
            let hub = Arc::downgrade(self);
            let task = tokio::spawn(async move {
                while let Ok(event) = receiver.recv().await {
                    let Some(hub) = hub.upgrade() else {
                        break;
                    };
                    if hub
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .epoch
                        != epoch
                    {
                        break;
                    }
                    match event {
                        SubscriptionEvent::Delta(delta) => hub.dispatch(epoch, delta),
                        SubscriptionEvent::Lagged(count) => {
                            for feed in hub.feeds(epoch) {
                                feed.lag(count);
                            }
                        }
                    }
                }
            });
            *self.task.lock().map_err(|_| unavailable())? = Some(task.abort_handle());
        }
        Ok(())
    }
    fn remove(&self, instance: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.index.unsubscribe(instance);
        state.feeds.remove(instance);
        if state.feeds.is_empty()
            && let Some(task) = self
                .task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        {
            task.abort();
        }
    }
    fn feeds(&self, epoch: u64) -> Vec<Arc<Feed>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.epoch != epoch {
            return Vec::new();
        }
        state.feeds.values().filter_map(Weak::upgrade).collect()
    }
    fn dispatch(&self, epoch: u64, delta: CatalogDelta) {
        use ExchangeBoundary::{ClientRequest, ClientResponse, UpstreamRequest, UpstreamResponse};
        if delta.change == CatalogChange::Metadata {
            if delta.sequence == 0 {
                for feed in self.feeds(epoch) {
                    if !feed.guard.matches(feed.anchor) {
                        feed.stop();
                    }
                }
            }
            return;
        }
        for (request, response) in [
            (ClientRequest, ClientResponse),
            (UpstreamRequest, UpstreamResponse),
        ] {
            let selected = Boundaries {
                request,
                response,
                direction: Direction::Response,
            };
            let directions = match delta.change {
                CatalogChange::Head(boundary)
                | CatalogChange::Body(boundary)
                | CatalogChange::BodyCompleted(boundary)
                    if boundary == request =>
                {
                    vec![Direction::Request]
                }
                CatalogChange::Head(boundary)
                | CatalogChange::Body(boundary)
                | CatalogChange::BodyCompleted(boundary)
                    if boundary == response =>
                {
                    vec![Direction::Response]
                }
                CatalogChange::Terminal => vec![Direction::Request, Direction::Response],
                _ => continue,
            };
            for direction in directions {
                self.admit(epoch, delta, selected, direction);
            }
        }
    }
    fn admit(&self, epoch: u64, delta: CatalogDelta, selected: Boundaries, direction: Direction) {
        // Keep upgraded feeds alive outside the state lock: dropping the last feed unregisters it.
        let feeds = self.feeds(epoch);
        if !feeds.iter().any(|feed| {
            feed.selected.request == selected.request && feed.accepts(delta.change, direction)
        }) {
            return;
        }
        let candidates = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.epoch != epoch {
                return;
            }
            self.catalog
                .with_heads(delta.exchange_id, |heads| {
                    let Some(scope) = live_index_scope(&heads, selected, &self.owner) else {
                        return Vec::new();
                    };
                    let admission = super::admission::Admission::from_heads(
                        heads.request_heads,
                        heads.response_heads,
                        selected,
                    );
                    state
                        .index
                        .admitted(&scope, admission.metadata(), direction)
                        .into_iter()
                        .filter_map(|id| state.feeds.get(id)?.upgrade())
                        .map(|feed| (feed, heads.request_number))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        for (feed, order) in candidates {
            if !feed.accepts(delta.change, direction)
                || !feed.guard.allowed(delta.exchange_id, order)
            {
                continue;
            }
            let phase = match delta.change {
                CatalogChange::Head(_) => TrafficPhase::Head,
                CatalogChange::Body(_) => TrafficPhase::Body,
                CatalogChange::BodyCompleted(_) => TrafficPhase::BodyCompleted,
                _ => TrafficPhase::Ended,
            };
            feed.offer(Queued {
                sequence: 0,
                exchange: delta.exchange_id,
                order,
                revision: delta.sequence,
                direction,
                phase,
                ready_since: std::time::Instant::now(),
            });
        }
    }
}
impl Drop for Hub {
    fn drop(&mut self) {
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

impl super::Application {
    pub(super) fn start_visualizer_feed(
        &self,
        request: &super::VisualizerActivation,
        instance: &transmog_extension_registry::Instance,
        snapshot: &mut transmog_session::SessionSnapshot,
        selected: Boundaries,
        store: Option<&crate::BodyStore>,
    ) -> Result<Option<Arc<Feed>>, AppError> {
        if !request.live {
            return Ok(None);
        }
        if instance.declaration.live_updates.is_none() || snapshot.imported {
            return Err(invalid(
                "This visualizer or capture does not support live updates",
            ));
        }
        let guard = RelatedGuard::new(self, snapshot, selected, &instance.declaration)
            .ok_or_else(unavailable)?;
        let feed = Feed::new(
            guard,
            selected,
            instance.declaration.clone(),
            snapshot.exchange_id,
            store.cloned(),
        );
        let hub = {
            let mut hub = self.visualizers.live.lock().map_err(|_| unavailable())?;
            Arc::clone(hub.get_or_insert_with(|| {
                Hub::new(
                    self.service.catalog().clone(),
                    self.visualizers.owner.clone(),
                )
            }))
        };
        hub.subscribe(&instance.id, &feed)?;
        // Subscribe before freezing initial input: subsequent hints remain queued during startup.
        *snapshot = self.visualizer_evidence(&request.selection.session_id)?.0;
        if !feed.guard.snapshot_matches(snapshot)
            || !super::admission::Admission::new(snapshot, selected)
                .applicable(&instance.declaration, request.selection.surface)
        {
            return Err(unavailable());
        }
        feed.baseline(snapshot.last_sequence);
        Ok(Some(feed))
    }
    /// Stops native admission while leaving the visible snapshot and expansion worker available.
    /// # Errors
    /// Rejects stale or foreign visualizer bindings.
    pub fn stop_visualizer_live(
        &self,
        handle: &super::VisualizerHandle,
    ) -> Result<super::VisualizerLiveStatus, AppError> {
        let ticket = self.visualizers.ticket_binding(handle)?;
        let instance = ticket.current()?;
        let resources = self
            .visualizers
            .registry
            .resources::<super::Resources>(&self.visualizers.owner, &instance.id)
            .map_err(|_| unavailable())?;
        if let Some(feed) = resources.live.lock().map_err(|_| unavailable())?.take() {
            feed.stop();
        }
        Ok(resources.live_status())
    }
    /// Parses one coalesced admitted batch. Empty queues acquire no body capabilities and invoke no script.
    /// # Errors
    /// Rejects unavailable evidence, exhausted budgets and parser failures; failures unload the instance.
    pub async fn poll_visualizer_live(
        &self,
        handle: &super::VisualizerHandle,
    ) -> Result<super::VisualizerLiveUpdate, AppError> {
        let ticket = self.visualizers.ticket_binding(handle)?;
        let instance = ticket.current()?;
        let resources = self
            .visualizers
            .registry
            .resources::<super::Resources>(&self.visualizers.owner, &instance.id)
            .map_err(|_| unavailable())?;
        let Ok(_operation) = ticket.operation.try_lock() else {
            return Ok(super::VisualizerLiveUpdate {
                view: None,
                status: resources.live_status(),
                busy: true,
            });
        };
        let operation = self.poll_live_batch(handle, &ticket, &instance, &resources);
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(instance.declaration.limits.duration_ms),
            operation,
        )
        .await
        .map_err(|_| unavailable())
        .and_then(std::convert::identity);
        if result.is_err() {
            self.visualizers
                .cancel(&handle.slot, &handle.generation, &handle.page_id);
        }
        result
    }
    async fn poll_live_batch(
        &self,
        handle: &super::VisualizerHandle,
        ticket: &super::Ticket,
        instance: &transmog_extension_registry::Instance,
        resources: &Arc<super::Resources>,
    ) -> Result<super::VisualizerLiveUpdate, AppError> {
        let feed = resources.live.lock().map_err(|_| unavailable())?.clone();
        let Some(feed) = feed.filter(|feed| feed.enabled()) else {
            return Ok(super::VisualizerLiveUpdate {
                view: None,
                status: resources.live_status(),
                busy: false,
            });
        };
        if !feed.has_pending() {
            return Ok(super::VisualizerLiveUpdate {
                view: None,
                status: resources.live_status(),
                busy: false,
            });
        }
        let catalog = self.service.catalog().clone();
        let instance_id = instance.id.clone();
        let projected = Arc::clone(resources);
        let current = Arc::clone(&feed);
        let events = tokio::task::spawn_blocking(move || {
            project_batch(&catalog, &current, &projected, &instance_id)
        })
        .await
        .map_err(|_| unavailable())??;
        ticket.current()?;
        if events.is_empty() || !feed.enabled() {
            return Ok(super::VisualizerLiveUpdate {
                view: None,
                status: resources.live_status(),
                busy: false,
            });
        }
        let mut document = self
            .invoke_visualizer(
                ticket,
                transmog_extensions::Invocation::Traffic {
                    events,
                    missed_hints: feed.missed().to_string(),
                },
            )
            .await?;
        ticket.current()?;
        if feed.missed() != 0 && document.status == transmog_extensions::ParseStatus::Complete {
            document.status = transmog_extensions::ParseStatus::Partial;
        }
        let status = resources.live_status();
        Ok(super::VisualizerLiveUpdate {
            view: Some(super::VisualizerView {
                handle: handle.clone(),
                instance: instance.clone(),
                document,
                sources: resources.info()?,
                live: status.clone(),
            }),
            status,
            busy: false,
        })
    }
}
fn project_batch(
    catalog: &SessionCatalog,
    feed: &Feed,
    resources: &super::Resources,
    instance: &str,
) -> Result<Vec<transmog_extensions::TrafficEvent>, AppError> {
    let pending = feed.take();
    let mut result = Vec::new();
    // Keep decimal byte arrays and metadata within the private IPC frame ceiling.
    let mut remaining = feed.declaration.limits.input_bytes.min(512 * 1024);
    for (index, queued) in pending.iter().enumerate() {
        if resources.stopped.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        if !feed.enabled() {
            return Ok(Vec::new());
        }
        if !feed.guard.allowed(queued.exchange, queued.order) {
            continue;
        }
        let Some(bytes) = feed.guard.projected_bytes(queued.exchange) else {
            continue;
        };
        if bytes > remaining {
            if result.is_empty() {
                return Err(AppError::new(
                    crate::ErrorCategory::Limit,
                    "Live input headers exceed the visualizer budget",
                    false,
                ));
            }
            feed.restore(pending[index..].iter().copied());
            break;
        }
        let Some(snapshot) = catalog.get(queued.exchange) else {
            continue;
        };
        if snapshot.request_number != queued.order || !feed.guard.snapshot_matches(&snapshot) {
            continue;
        }
        let rules = if feed.declaration.input_matches.is_empty() {
            &feed.declaration.matches
        } else {
            &feed.declaration.input_matches
        };
        let admission = super::admission::Admission::new(&snapshot, feed.selected);
        if !rules
            .iter()
            .any(|rule| transmog_extensions::matches(rule, admission.metadata(), queued.direction))
        {
            continue;
        }
        if !body_ready(&snapshot, feed, queued) {
            if queued.ready_since.elapsed()
                > std::time::Duration::from_millis(feed.declaration.limits.duration_ms)
            {
                return Err(unavailable());
            }
            feed.restore(pending[index..].iter().copied());
            break;
        }
        let mut input = feed
            .guard
            .project(
                &snapshot,
                resources,
                instance,
                feed.store.as_ref(),
                &mut remaining,
            )?
            .input;
        input.direction = queued.direction;
        result.push(transmog_extensions::TrafficEvent {
            sequence: queued.sequence.to_string(),
            exchange_id: input.exchange_id.clone(),
            direction: queued.direction,
            phase: queued.phase,
            revision: input.generation.clone(),
            input,
        });
    }
    Ok(result)
}

fn body_ready(snapshot: &transmog_session::SessionSnapshot, feed: &Feed, queued: &Queued) -> bool {
    if !matches!(
        queued.phase,
        TrafficPhase::BodyCompleted | TrafficPhase::Ended
    ) {
        return true;
    }
    let Some(store) = &feed.store else {
        return true;
    };
    let metadata = store.metadata(snapshot.exchange_id);
    for (side, boundary) in [
        (Direction::Request, feed.selected.request),
        (Direction::Response, feed.selected.response),
    ] {
        if feed.declaration.inputs.side(side).body == BodyAccess::None
            || (queued.phase == TrafficPhase::BodyCompleted && side != queued.direction)
        {
            continue;
        }
        let expected = snapshot
            .bodies
            .iter()
            .find(|body| body.boundary == boundary)
            .map_or(0, |body| body.observed_bytes);
        let current = metadata
            .iter()
            .find(|body| body.boundary == super::evidence::boundary_name(boundary));
        match current {
            None if expected != 0 => return false,
            Some(body)
                if body.availability == crate::BodyAvailability::Capturing
                    && (body.observed_bytes < expected || queued.phase == TrafficPhase::Ended) =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

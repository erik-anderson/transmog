use super::*;
use crate::{
    AppConfig, Application,
    visualizers::{
        boundaries,
        context::tests::{fixture, publish},
        evidence::Resources,
    },
};
use std::time::Duration;
use transmog_core::{ClientIdentity, ConnectionId, HeaderBlock, HeaderField};
use transmog_extension_registry::InstanceResources as _;
use transmog_extensions::LiveUpdates;
use transmog_session::SessionSnapshot;

fn live_fixture(app: &Application) -> (SessionSnapshot, Visualizer) {
    let (snapshot, mut declaration) = fixture(app);
    declaration.live_updates = Some(LiveUpdates {
        events: vec![Direction::Request, Direction::Response],
        body_progress: false,
    });
    declaration.input_matches = declaration.matches.clone();
    for rule in &mut declaration.input_matches {
        rule.methods = vec!["POST".into()];
    }
    (snapshot, declaration)
}
fn feed(app: &Application, anchor: &SessionSnapshot, declaration: &Visualizer) -> Arc<Feed> {
    let selected = boundaries("client-response").unwrap();
    let guard = RelatedGuard::new(app, anchor, selected, declaration).unwrap();
    let feed = Feed::new(
        guard,
        selected,
        declaration.clone(),
        anchor.exchange_id,
        None,
    );
    feed.baseline(anchor.last_sequence);
    feed
}
fn resources(feed: &Arc<Feed>) -> Resources {
    Resources {
        live_available: true,
        finalized: AtomicBool::new(false),
        live: Mutex::new(Some(Arc::clone(feed))),
        stopped: AtomicBool::new(false),
        sources: Mutex::new(BTreeMap::new()),
        info: Mutex::new(Vec::new()),
        context: Mutex::new(None),
    }
}
async fn settled(feed: &Feed, id: ExchangeId) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if feed
                .queue
                .lock()
                .unwrap()
                .pending
                .values()
                .any(|event| event.exchange == id && event.direction == Direction::Response)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn live_option_requires_a_declared_retained_conversation() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (snapshot, declaration) = live_fixture(&app);
    let selected = boundaries("client-response").unwrap();
    let mut composer = snapshot.clone();
    composer.exchange_id = ExchangeId(999);
    assert!(!available(&app, &composer, selected, &declaration));
    let anchor = publish(&app, 99, &snapshot);
    assert!(available(&app, &anchor, selected, &declaration));
    let mut imported = anchor.clone();
    imported.imported = true;
    assert!(!available(&app, &imported, selected, &declaration));
    let mut unavailable = anchor.clone();
    unavailable.request_heads[0].head.headers = HeaderBlock::default();
    assert!(!available(&app, &unavailable, selected, &declaration));
    let mut snapshot_only = declaration;
    snapshot_only.live_updates = None;
    assert!(!available(&app, &anchor, selected, &snapshot_only));
}

#[tokio::test]
async fn native_admission_filters_before_projection_and_unload_clears_the_feed() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (snapshot, declaration) = live_fixture(&app);
    let anchor = publish(&app, 100, &snapshot);
    let feed = feed(&app, &anchor, &declaration);
    let hub = Hub::new(app.service.catalog().clone(), app.visualizers.owner.clone());
    assert!(hub.task.lock().unwrap().is_none());
    hub.subscribe("worker", &feed).unwrap();
    publish(&app, 101, &snapshot);
    for (id, mutate) in [
        (
            102,
            (|value: &mut SessionSnapshot| value.request_heads[0].head.target.port = 8443)
                as fn(&mut SessionSnapshot),
        ),
        (103, |value| {
            value.request_heads[0].head.target.host = "other.invalid".into();
        }),
        (104, |value| {
            value.request_heads[0].head.target.scheme = "http".into();
        }),
        (105, |value| {
            value.request_heads[0].head.method = "PUT".into();
        }),
        (106, |value| {
            value.request_heads[0].head.headers = HeaderBlock::from_fields(vec![
                HeaderField::try_new("X-Kind", "another conversation").unwrap(),
            ]);
        }),
        (107, |value| {
            value.request_heads.remove(0);
        }),
    ] {
        let mut changed = snapshot.clone();
        mutate(&mut changed);
        publish(&app, id, &changed);
    }
    publish(&app, 199, &snapshot);
    settled(&feed, ExchangeId(199)).await;
    let resources = resources(&feed);
    assert!(
        resources.sources.lock().unwrap().is_empty(),
        "Admission must not project values or create body capabilities"
    );
    let events = project_batch(app.service.catalog(), &feed, &resources, "worker").unwrap();
    assert_eq!(events.len(), 4);
    for event in &events {
        assert!(
            event.exchange_id == format!("{:032x}", 101)
                || event.exchange_id == format!("{:032x}", 199)
        );
        assert_eq!(event.exchange_id, event.input.exchange_id);
        assert!(event.input.request.as_ref().unwrap().body.is_none());
        assert!(event.input.response.as_ref().unwrap().body.is_none());
        assert!(
            event
                .input
                .request
                .as_ref()
                .unwrap()
                .headers
                .iter()
                .all(|header| header.name != "Hidden")
        );
    }
    assert!(
        events
            .windows(2)
            .all(|events| events[0].sequence.parse::<u64>().unwrap()
                < events[1].sequence.parse::<u64>().unwrap())
    );
    resources.stop();
    assert!(!feed.enabled() && feed.take().is_empty());
    assert!(resources.sources.lock().unwrap().is_empty());
    assert!(hub.state.lock().unwrap().index.is_empty());
    assert!(hub.task.lock().unwrap().is_none());
}

#[tokio::test]
async fn finalized_view_releases_admission_and_preserves_retained_source_access() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (snapshot, declaration) = live_fixture(&app);
    let anchor = publish(&app, 700, &snapshot);
    let feed = feed(&app, &anchor, &declaration);
    let hub = Hub::new(app.service.catalog().clone(), app.visualizers.owner.clone());
    hub.subscribe("finalized", &feed).unwrap();
    publish(&app, 701, &snapshot);
    settled(&feed, ExchangeId(701)).await;
    let resources = resources(&feed);
    project_batch(app.service.catalog(), &feed, &resources, "finalized").unwrap();
    let sources = resources.sources.lock().unwrap().len();
    assert!(sources > 0);
    publish(&app, 702, &snapshot);
    settled(&feed, ExchangeId(702)).await;

    resources.finalize_updates();
    resources.finalize_updates();
    let status = resources.live_status();
    assert!(!status.available && !status.enabled);
    assert!(feed.take().is_empty());
    assert!(!feed.enabled());
    assert!(hub.state.lock().unwrap().index.is_empty());
    assert!(hub.task.lock().unwrap().is_none());
    assert!(!resources.stopped.load(Ordering::Acquire));
    assert_eq!(resources.sources.lock().unwrap().len(), sources);
    assert!(resources.info().is_ok());
    publish(&app, 703, &snapshot);
    assert!(feed.take().is_empty());
    assert!(!resources.live_status().available);
}

#[tokio::test]
async fn finalization_and_expansion_retain_missed_hints() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (snapshot, declaration) = live_fixture(&app);
    let anchor = publish(&app, 710, &snapshot);
    let feed = feed(&app, &anchor, &declaration);
    let resources = resources(&feed);
    let mut document: transmog_extensions::VisualizationDocument = serde_json::from_value(
        serde_json::json!({"title":"Final view","status":"complete","presentation":"tree","roots":[]}),
    ).unwrap();
    assert_eq!(resources.finish_document(&mut document).missed_hints, "0");
    assert_eq!(document.status, transmog_extensions::ParseStatus::Complete);
    // A hint can arrive after parsing, before the host publishes its result.
    feed.lag(7);
    document.updates = Some(transmog_extensions::UpdatePolicy::Final);
    assert_eq!(resources.finish_document(&mut document).missed_hints, "7");
    assert_eq!(resources.live_status().missed_hints, "7");
    assert!(!resources.live_status().available);
    assert_eq!(document.status, transmog_extensions::ParseStatus::Partial);
    document.updates = None;
    document.status = transmog_extensions::ParseStatus::Complete;
    resources.finish_document(&mut document);
    assert_eq!(document.status, transmog_extensions::ParseStatus::Partial);
    assert_eq!(resources.live_status().missed_hints, "7");
    assert!(!feed.enabled());
}

#[tokio::test]
async fn one_catalog_subscription_serves_multiple_independent_contributions() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (mut snapshot, contextual) = live_fixture(&app);
    Arc::make_mut(&mut snapshot.metadata).client_identity = ClientIdentity::LocalProcess {
        pid: 42,
        name: Some("client.exe".into()),
    };
    let anchor = publish(&app, 200, &snapshot);
    let mut local = contextual.clone();
    local.context = None;
    let contextual = feed(&app, &anchor, &contextual);
    let local = feed(&app, &anchor, &local);
    let hub = Hub::new(app.service.catalog().clone(), app.visualizers.owner.clone());
    hub.subscribe("contextual", &contextual).unwrap();
    hub.subscribe("local", &local).unwrap();
    let mut subscriptions = Vec::new();
    while let Ok(subscription) = app.service.catalog().subscribe() {
        subscriptions.push(subscription);
    }
    assert_eq!(
        subscriptions.len(),
        15,
        "Contributions must share one of the 16 native subscription slots"
    );
    let mut another_connection = snapshot.clone();
    Arc::make_mut(&mut another_connection.metadata).downstream_connection_id = ConnectionId(99);
    publish(&app, 201, &another_connection);
    publish(&app, 202, &snapshot);
    settled(&contextual, ExchangeId(202)).await;
    settled(&local, ExchangeId(202)).await;
    assert!(
        contextual
            .take()
            .iter()
            .any(|event| event.exchange == ExchangeId(201))
    );
    assert!(
        local
            .take()
            .iter()
            .all(|event| event.exchange != ExchangeId(201))
    );
    contextual.stop();
    assert_eq!(hub.state.lock().unwrap().index.len(), 1);
    assert!(local.enabled());
    local.stop();
    tokio::task::yield_now().await;
    assert!(app.service.catalog().subscribe().is_ok());
    let old_epoch = hub.state.lock().unwrap().epoch;
    let replacement = feed(&app, &anchor, &local.declaration);
    hub.subscribe("replacement", &replacement).unwrap();
    let hint = CatalogDelta {
        exchange_id: anchor.exchange_id,
        sequence: anchor.last_sequence + 1,
        terminal: false,
        change: CatalogChange::Head(ExchangeBoundary::ClientResponse),
    };
    hub.dispatch(old_epoch, hint);
    assert!(
        replacement.take().is_empty(),
        "A stopped receiver must not feed a replacement generation"
    );
    let epoch = hub.state.lock().unwrap().epoch;
    hub.dispatch(epoch, hint);
    assert_eq!(replacement.take().len(), 1);
    replacement.stop();
}

#[test]
fn changed_input_rules_are_rechecked_before_projection() {
    use transmog_core::observe::{ObserverEvent, ObserverEventKind};
    let app = Application::new(AppConfig::default()).unwrap();
    let (snapshot, declaration) = live_fixture(&app);
    let anchor = publish(&app, 800, &snapshot);
    let feed = feed(&app, &anchor, &declaration);
    let selected = publish(&app, 801, &snapshot);
    feed.offer(Queued {
        sequence: 0,
        exchange: selected.exchange_id,
        order: selected.request_number,
        revision: selected.last_sequence,
        direction: Direction::Response,
        phase: TrafficPhase::Head,
        ready_since: std::time::Instant::now(),
    });
    let mut changed = selected.request_heads[0].head.clone();
    changed.method = "PUT".into();
    app.service.catalog().apply(ObserverEvent {
        exchange_id: selected.exchange_id,
        sequence: selected.last_sequence + 1,
        kind: ObserverEventKind::RequestHeadObserved {
            boundary: ExchangeBoundary::ClientRequest,
            head: changed,
        },
    });
    let resources = resources(&feed);
    assert_eq!(
        project_batch(app.service.catalog(), &feed, &resources, "worker").unwrap(),
        [] as [transmog_extensions::TrafficEvent; 0]
    );
    assert!(resources.sources.lock().unwrap().is_empty());
    resources.stop();
}

#[test]
fn queues_coalesce_and_bound_metadata_and_report_lost_hints() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (snapshot, declaration) = live_fixture(&app);
    let anchor = publish(&app, 300, &snapshot);
    let feed = feed(&app, &anchor, &declaration);
    let update = |id, revision| Queued {
        sequence: 0,
        exchange: ExchangeId(id),
        order: u64::try_from(id).unwrap(),
        revision,
        direction: Direction::Response,
        phase: TrafficPhase::Head,
        ready_since: std::time::Instant::now(),
    };
    for revision in 1..=100 {
        feed.offer(update(301, revision));
    }
    let coalesced = feed.take();
    assert_eq!(coalesced.len(), 1);
    assert_eq!(coalesced[0].revision, 100);
    assert_eq!(feed.missed(), 0);
    for id in 400..500 {
        feed.offer(update(id, 1));
    }
    let bounded = feed.take();
    assert_eq!(bounded.len(), QUEUE_EVENTS);
    assert_eq!(feed.missed(), 68);
    assert!(
        bounded
            .windows(2)
            .all(|values| values[0].sequence < values[1].sequence)
    );
    feed.offer(update(499, 2));
    feed.restore(bounded);
    assert_eq!(feed.queue.lock().unwrap().pending.len(), QUEUE_EVENTS);
    assert_eq!(
        feed.queue
            .lock()
            .unwrap()
            .pending
            .values()
            .find(|value| value.exchange == ExchangeId(499))
            .unwrap()
            .revision,
        2
    );
    feed.lag(3);
    assert_eq!(feed.missed(), 71);
    feed.stop();
    feed.offer(update(500, 1));
    assert!(feed.take().is_empty());
}

#[test]
fn body_growth_requires_opt_in_and_body_completion_requires_body_access() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (snapshot, mut declaration) = live_fixture(&app);
    let anchor = publish(&app, 600, &snapshot);
    let headers = feed(&app, &anchor, &declaration);
    let growth = CatalogChange::Body(ExchangeBoundary::ClientResponse);
    let completed = CatalogChange::BodyCompleted(ExchangeBoundary::ClientResponse);
    assert!(!headers.accepts(growth, Direction::Response));
    assert!(!headers.accepts(completed, Direction::Response));
    declaration.inputs.response.body = BodyAccess::Optional;
    let body = feed(&app, &anchor, &declaration);
    assert!(!body.accepts(growth, Direction::Response));
    assert!(body.accepts(completed, Direction::Response));
    declaration.live_updates.as_mut().unwrap().body_progress = true;
    declaration.live_updates.as_mut().unwrap().events = vec![Direction::Response];
    let progress = feed(&app, &anchor, &declaration);
    assert!(progress.accepts(growth, Direction::Response));
    assert!(!progress.accepts(growth, Direction::Request));
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One deterministic cross-dispatcher body-completion fixture.
async fn final_updates_wait_for_admitted_retention_metadata_without_flushing_unrelated_work() {
    use transmog_core::{
        intercept::CompletedExchange,
        observe::{ObservedBodyChunk, Observer as _, ObserverEvent, ObserverEventKind},
    };
    let root = tempfile::tempdir().unwrap();
    let app = Application::new(AppConfig {
        body_store: Some(crate::BodyStoreConfig::product_default(
            root.path().join("bodies"),
        )),
        ..AppConfig::default()
    })
    .unwrap();
    let (snapshot, mut declaration) = live_fixture(&app);
    declaration.inputs.response.body = BodyAccess::Optional;
    let anchor = publish(&app, 700, &snapshot);
    let selected = boundaries("client-response").unwrap();
    let store = app.body_store.clone().unwrap();
    let feed = Feed::new(
        RelatedGuard::new(&app, &anchor, selected, &declaration).unwrap(),
        selected,
        declaration,
        anchor.exchange_id,
        Some(store.clone()),
    );
    let resources = resources(&feed);
    let chunk = ObserverEvent {
        exchange_id: anchor.exchange_id,
        sequence: anchor.last_sequence + 1,
        kind: ObserverEventKind::BodyChunk(ObservedBodyChunk {
            boundary: selected.response,
            byte_count: 4,
            sample: Some(bytes::Bytes::from_static(&[1, 2, 3, 4])),
            truncated: false,
        }),
    };
    app.service.catalog().apply(chunk.clone());
    let completed = ObserverEvent {
        exchange_id: anchor.exchange_id,
        sequence: anchor.last_sequence + 2,
        kind: ObserverEventKind::BodyCompleted {
            boundary: selected.response,
        },
    };
    app.service.catalog().apply(completed.clone());
    feed.offer(Queued {
        sequence: 0,
        exchange: anchor.exchange_id,
        order: anchor.request_number,
        revision: completed.sequence,
        direction: Direction::Response,
        phase: TrafficPhase::BodyCompleted,
        ready_since: std::time::Instant::now(),
    });
    assert_eq!(
        project_batch(app.service.catalog(), &feed, &resources, "worker").unwrap(),
        [] as [transmog_extensions::TrafficEvent; 0]
    );
    assert!(resources.sources.lock().unwrap().is_empty());
    let mut kinds = vec![ObserverEventKind::ExchangeStarted {
        metadata: Arc::clone(&anchor.metadata),
    }];
    kinds.extend(
        anchor
            .request_heads
            .iter()
            .map(|head| ObserverEventKind::RequestHeadObserved {
                boundary: head.boundary,
                head: head.head.clone(),
            }),
    );
    kinds.extend(anchor.response_heads.iter().map(|head| {
        ObserverEventKind::ResponseHeadObserved {
            boundary: head.boundary,
            head: head.head.clone(),
        }
    }));
    for (index, kind) in kinds.into_iter().enumerate() {
        store
            .on_event(ObserverEvent {
                exchange_id: anchor.exchange_id,
                sequence: index as u64 + 1,
                kind,
            })
            .await
            .unwrap();
    }
    store.on_event(chunk).await.unwrap();
    store.on_event(completed).await.unwrap();
    store.flush().unwrap();
    let events = project_batch(app.service.catalog(), &feed, &resources, "worker").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0]
            .input
            .response
            .as_ref()
            .unwrap()
            .body
            .as_ref()
            .unwrap()
            .length,
        "4"
    );
    let ended = ObserverEvent {
        exchange_id: anchor.exchange_id,
        sequence: anchor.last_sequence + 3,
        kind: ObserverEventKind::Completed(CompletedExchange {
            metadata: Arc::clone(&anchor.metadata),
            request_head: anchor.request_heads[0].head.clone(),
            response_head: anchor.response_heads[0].head.clone(),
        }),
    };
    app.service.catalog().apply(ended.clone());
    feed.offer(Queued {
        sequence: 0,
        exchange: anchor.exchange_id,
        order: anchor.request_number,
        revision: ended.sequence,
        direction: Direction::Response,
        phase: TrafficPhase::Ended,
        ready_since: std::time::Instant::now(),
    });
    assert_eq!(
        project_batch(app.service.catalog(), &feed, &resources, "worker").unwrap(),
        [] as [transmog_extensions::TrafficEvent; 0]
    );
    store.on_event(ended).await.unwrap();
    store.flush().unwrap();
    let events = project_batch(app.service.catalog(), &feed, &resources, "worker").unwrap();
    assert_eq!(
        events[0]
            .input
            .response
            .as_ref()
            .unwrap()
            .body
            .as_ref()
            .unwrap()
            .availability,
        "complete"
    );
    resources.stop();
}

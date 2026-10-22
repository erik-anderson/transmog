use super::evidence::{Source, project_input};
use super::*;
use crate::{AppConfig, BodyStoreConfig, body_store::SavedBodySource};
use std::{
    io::{self, Read},
    sync::atomic::AtomicUsize,
};
use transmog_core::{
    ClientIdentity, ConnectionId, HeaderBlock, HeaderField, HttpLegVersion, RequestHead,
    ResponseHead, SessionId, SessionMetadata, StreamId, Target,
    intercept::ExchangeMetadata as CapturedMetadata,
    observe::{ExchangeBoundary, ObserverEvent, ObserverEventKind},
};
use transmog_extension_registry::InstanceResources;
use transmog_extensions::{BodyAccess, Direction, Manifest};

pub(super) fn declaration() -> transmog_extensions::Visualizer {
    serde_json::from_value::<Manifest>(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.evidence","version":"1.0.0","name":"Evidence",
        "visualizers":[{"id":"view","name":"View","entry":"main.js","surfaces":["headers","body"],
            "inputs":{"request":{"headers":["x-kind","cookie"],"body":"none"},"response":{"headers":["content-type"],"body":"none"}},
            "matches":[{"direction":"request","headers":[{"direction":"request","name":"x-kind","test":{"op":"present"}}]},{"direction":"response","contentTypes":["application/example"]}]}],
        "files":[{"path":"main.js","bytes":0}]
    })).unwrap().visualizers.remove(0)
}
#[allow(clippy::too_many_lines)] // One deterministic client/upstream observation fixture.
pub(super) fn snapshot(app: &Application) -> SessionSnapshot {
    let target = Target {
        scheme: "https".into(),
        authority: "example.invalid".into(),
        host: "example.invalid".into(),
        port: 443,
        path: "/protocol".into(),
        query: Some("secret=query".into()),
    };
    let metadata = Arc::new(CapturedMetadata::from_session(
        &SessionMetadata {
            session_id: SessionId(1),
            downstream_connection_id: ConnectionId(1),
            stream_id: StreamId(1),
            client_addr: "127.0.0.1:1000".parse().unwrap(),
            client_identity: ClientIdentity::default(),
            proxy_addr: "127.0.0.1:8888".parse().unwrap(),
            ingress_version: HttpLegVersion::Http1,
            egress_version: None,
        },
        target.clone(),
    ));
    app.service.catalog().apply(ObserverEvent {
        exchange_id: ExchangeId(1),
        sequence: 1,
        kind: ObserverEventKind::ExchangeStarted { metadata },
    });
    let mut redacted = HeaderField::try_new("Cookie", "secret").unwrap();
    redacted.redact_value();
    let headers = HeaderBlock::from_fields(vec![
        HeaderField::try_new("Unlisted", "never delivered").unwrap(),
        HeaderField::try_new("X-Kind", "a".repeat(5000)).unwrap(),
        HeaderField::try_new("x-kind", "duplicate").unwrap(),
        redacted,
    ]);
    for (sequence, kind) in [
        (
            2,
            ObserverEventKind::RequestHeadObserved {
                boundary: ExchangeBoundary::ClientRequest,
                head: RequestHead {
                    method: "POST".into(),
                    target: target.clone(),
                    headers,
                    source_version: HttpLegVersion::Http1,
                },
            },
        ),
        (
            3,
            ObserverEventKind::RequestHeadObserved {
                boundary: ExchangeBoundary::UpstreamRequest,
                head: RequestHead {
                    method: "PUT".into(),
                    target,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("X-Kind", "upstream").unwrap(),
                    ]),
                    source_version: HttpLegVersion::Http1,
                },
            },
        ),
        (
            4,
            ObserverEventKind::ResponseHeadObserved {
                boundary: ExchangeBoundary::ClientResponse,
                head: ResponseHead {
                    status: 200,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("Content-Type", "application/example").unwrap(),
                    ]),
                    source_version: HttpLegVersion::Http1,
                },
            },
        ),
    ] {
        app.service.catalog().apply(ObserverEvent {
            exchange_id: ExchangeId(1),
            sequence,
            kind,
        });
    }
    app.service.catalog().get(ExchangeId(1)).unwrap()
}
#[derive(Debug)]
struct CountingBody {
    opened: Arc<AtomicUsize>,
    read: Arc<AtomicUsize>,
}
struct CountingReader {
    position: u64,
    read: Arc<AtomicUsize>,
}
impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer
            .len()
            .min(usize::try_from(4 - self.position).unwrap());
        for byte in &mut buffer[..count] {
            *byte = 5 + u8::try_from(self.position).unwrap();
            self.position += 1;
        }
        self.read.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }
}
impl SavedBodySource for CountingBody {
    fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        self.open_at(0)
    }
    fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        self.opened.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(CountingReader {
            position: offset,
            read: Arc::clone(&self.read),
        }))
    }
    fn length(&self) -> Option<u64> {
        Some(4)
    }
}

#[test]
fn close_before_start_and_late_close_cannot_resurrect_or_cancel_newer_generations() {
    let service = Service::new(Registry::default(), "owner".into());
    service.cancel("pane", "1", "1");
    assert!(service.begin("pane", "1", "1").is_err());
    let second = service.begin("pane", "2", "1").unwrap();
    service.cancel("pane", "1", "1");
    assert!(!second.canceled.load(Ordering::Acquire));
    let third = service.begin("pane", "3", "1").unwrap();
    assert!(second.canceled.load(Ordering::Acquire));
    service.cancel("pane", "2", "1");
    assert!(!third.canceled.load(Ordering::Acquire));
    service.cancel_all();
    assert!(third.canceled.load(Ordering::Acquire));
    assert!(service.begin("new-pane", "1", "1").is_err());
}

#[test]
fn navigation_and_native_visibility_invalidate_queued_work_without_affecting_focus() {
    let service = Service::new(Registry::default(), "owner".into());
    let first = service.begin("pane", "1", "1").unwrap();
    service.reset_document();
    assert!(first.canceled.load(Ordering::Acquire));
    assert!(service.begin("pane", "99", "1").is_err());
    let second = service.begin("pane", "1", "2").unwrap();
    service.cancel("pane", "99", "1");
    assert!(!second.canceled.load(Ordering::Acquire));
    service.visible(true);
    assert!(!second.canceled.load(Ordering::Acquire));
    service.visible(false);
    assert!(second.canceled.load(Ordering::Acquire));
    assert!(service.begin("new-pane", "2", "2").is_err());
    service.visible(true);
    assert!(service.begin("pane", "2", "2").is_ok());
}

#[test]
fn instance_bindings_cannot_follow_a_replacement_or_another_page() {
    let service = Service::new(Registry::default(), "owner".into());
    let ticket = service.begin("pane", "1", "1").unwrap();
    ticket
        .bind(Instance {
            id: "first-worker".into(),
            extension_id: "example.evidence".into(),
            visualizer_id: "view".into(),
            digest: "a".repeat(64),
            development: true,
            declaration: declaration(),
        })
        .unwrap();
    let mut handle = VisualizerHandle {
        slot: "pane".into(),
        generation: "1".into(),
        page_id: "1".into(),
        instance_id: "first-worker".into(),
    };
    assert!(service.ticket_binding(&handle).is_ok());
    handle.instance_id = "another-worker".into();
    assert!(service.ticket_binding(&handle).is_err());
    handle.instance_id = "first-worker".into();
    handle.page_id = "2".into();
    assert!(service.ticket_binding(&handle).is_err());
}

#[test]
fn native_admission_borrows_values_and_does_not_confuse_redaction_or_boundaries() {
    use transmog_extensions::{
        ActiveSubscriptions, HeaderMetadata, HeaderTest, LiveUpdates, Scope,
    };
    let app = Application::new(AppConfig::default()).unwrap();
    let mut snapshot = snapshot(&app);
    let selected = boundaries("client-request").unwrap();
    let mut visualizer = declaration();
    let admission = Admission::new(&snapshot, selected);
    let metadata = admission.metadata();
    let headers = metadata.request.unwrap().headers;
    assert!(std::ptr::eq(
        headers[1].value().unwrap().as_ptr(),
        snapshot.request_heads[0]
            .head
            .headers
            .iter()
            .nth(1)
            .unwrap()
            .value()
            .as_ptr()
    ));
    assert!(admission.applicable(&visualizer, Surface::Headers));
    assert!(headers[3].redacted() && headers[3].value().is_none());
    visualizer.matches[0].headers[0].test = HeaderTest::Equals {
        value: "duplicate".into(),
    };
    assert!(admission.applicable(&visualizer, Surface::Headers));
    visualizer.matches[0].headers[0].name = "cookie".into();
    visualizer.matches[0].headers[0].test = HeaderTest::NotEquals {
        value: "other".into(),
    };
    assert!(!admission.applicable(&visualizer, Surface::Headers));
    visualizer.matches[0].headers[0].name = "x-kind".into();
    visualizer.matches[0].headers[0].test = HeaderTest::Equals {
        value: "duplicate".into(),
    };
    visualizer.live_updates = Some(LiveUpdates {
        events: vec![Direction::Request],
        body_progress: false,
    });
    let scope = Scope {
        capture: "capture".into(),
        conversation: "conversation".into(),
    };
    let mut active = ActiveSubscriptions::default();
    active
        .subscribe("worker".into(), scope.clone(), visualizer)
        .unwrap();
    assert_eq!(
        active.admitted(&scope, metadata, Direction::Request),
        ["worker"]
    );
    assert_eq!(
        active.admitted(&scope, metadata, Direction::Response),
        [] as [&str; 0]
    );
    snapshot.response_heads[0].head.headers = HeaderBlock::from_fields(vec![
        HeaderField::try_new("Content-Type", "Application/Example; charset=utf-8").unwrap(),
        HeaderField::try_new("content-type", "application/example").unwrap(),
    ]);
    assert!(
        Admission::new(&snapshot, boundaries("client-response").unwrap())
            .applicable(&declaration(), Surface::Body)
    );
    snapshot.response_heads[0]
        .head
        .headers
        .push(HeaderField::try_new("Content-Type", "other/type").unwrap());
    assert!(
        !Admission::new(&snapshot, boundaries("client-response").unwrap())
            .applicable(&declaration(), Surface::Body)
    );
}

#[test]
fn no_enabled_extensions_skip_retained_evidence_discovery() {
    let app = Application::new(AppConfig::default()).unwrap();
    assert!(
        app.visualizer_choices(&VisualizerSelection {
            session_id: "00000000000000000000000000000001".into(),
            boundary: "client-request".into(),
            surface: Surface::Body
        })
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn automatic_activation_without_opt_in_is_denied_before_any_source_is_loaded() {
    let root = tempfile::tempdir().unwrap();
    let registry = Registry::load(root.path().join("registry"), None);
    let directory = root.path().join("development");
    std::fs::create_dir(&directory).unwrap();
    let manifest = serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.evidence","version":"1.0.0","name":"Evidence",
        "visualizers":[declaration()],"files":[{"path":"main.js","bytes":0}]
    });
    std::fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    // The module is deliberately absent: rejection must occur before loading it.
    registry.register_directory(&directory, false).unwrap();
    let app = Application::new(AppConfig {
        extensions: Some(registry),
        ..AppConfig::default()
    })
    .unwrap();
    let snapshot = snapshot(&app);
    let selection = VisualizerSelection {
        session_id: format!("{:032x}", snapshot.exchange_id.0),
        boundary: "client-response".into(),
        surface: Surface::Body,
    };
    let choices = app.visualizer_choices(&selection).unwrap();
    assert_eq!(choices.len(), 1);
    assert!(!choices[0].automatic);
    let request = VisualizerActivation {
        slot: "auto-pane".into(),
        generation: "1".into(),
        page_id: "1".into(),
        selection,
        extension_id: "example.evidence".into(),
        visualizer_id: "view".into(),
        live: false,
        automatic: true,
    };
    let error = app.activate_visualizer(request).await.unwrap_err();
    assert!(error.to_string().contains("does not match"), "{error}");
}

#[test]
fn headers_are_complete_projected_and_bound_to_the_exact_message_stage() {
    let app = Application::new(AppConfig::default()).unwrap();
    let snapshot = snapshot(&app);
    let declaration = declaration();
    let mut sources = BTreeMap::new();
    let input = project_input(
        &snapshot,
        boundaries("client-request").unwrap(),
        &declaration,
        None,
        "active",
        &mut sources,
    )
    .unwrap();
    let info = evidence::source_infos(&snapshot, boundaries("client-request").unwrap(), &input);
    assert_eq!(info.len(), sources.len());
    assert!(info.iter().any(|info| info.name.contains("X-Kind")
        && info.length == "5000"
        && info.boundary == "client-request"));
    assert!(
        Admission::new(&snapshot, boundaries("client-request").unwrap())
            .applicable(&declaration, Surface::Headers)
    );
    let request = input.request.unwrap();
    assert_eq!(request.path.as_deref(), Some("/protocol"));
    assert_eq!(request.headers.len(), 3);
    assert_eq!(request.headers[0].index, 1);
    assert_eq!(request.headers[0].value.as_ref().unwrap().len(), 5000);
    assert_eq!(
        request.headers[1].value.as_deref(),
        Some(b"duplicate".as_slice())
    );
    assert!(
        request.headers[2].redacted
            && request.headers[2].value.is_none()
            && request.headers[2].source_id.is_none()
    );
    let header = request.headers[0].source_id.as_ref().unwrap();
    assert!(matches!(sources.get(header), Some(Source::Header(_))));
    let upstream = project_input(
        &snapshot,
        boundaries("upstream-response").unwrap(),
        &declaration,
        None,
        "active",
        &mut BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(upstream.request.unwrap().method.as_deref(), Some("PUT"));
    assert!(upstream.response.is_none());
    assert_eq!(input.direction, Direction::Request);
}

#[test]
fn body_sources_are_lazy_and_header_only_inputs_never_open_or_read_bodies() {
    let root = tempfile::tempdir().unwrap();
    let mut config = BodyStoreConfig::product_default(root.path().join("body"));
    config.max_read_bytes = 2;
    let app = Application::new(AppConfig {
        body_store: Some(config),
        ..AppConfig::default()
    })
    .unwrap();
    let snapshot = snapshot(&app);
    let opened = Arc::new(AtomicUsize::new(0));
    let read = Arc::new(AtomicUsize::new(0));
    let store = app.body_store.as_ref().unwrap();
    store.register_saved(
        ExchangeId(1),
        ExchangeBoundary::ClientResponse,
        &HeaderBlock::default(),
        None,
        Arc::new(CountingBody {
            opened: Arc::clone(&opened),
            read: Arc::clone(&read),
        }),
        4,
        true,
    );
    let mut declaration = declaration();
    let mut sources = BTreeMap::new();
    let input = project_input(
        &snapshot,
        boundaries("client-response").unwrap(),
        &declaration,
        Some(store),
        "active",
        &mut sources,
    )
    .unwrap();
    assert!(input.response.unwrap().body.is_none());
    assert!(sources.values().all(|s| matches!(s, Source::Header(_))));
    assert_eq!(opened.load(Ordering::Relaxed), 0);
    declaration.inputs.response.body = BodyAccess::Required;
    let mut sources = BTreeMap::new();
    let input = project_input(
        &snapshot,
        boundaries("client-response").unwrap(),
        &declaration,
        Some(store),
        "active",
        &mut sources,
    )
    .unwrap();
    assert_eq!(opened.load(Ordering::Relaxed), 0);
    let body = input.response.unwrap().body.unwrap();
    let resources = Resources {
        live_available: false,
        finalized: AtomicBool::new(false),
        live: Mutex::new(None),
        info: Mutex::new(Vec::new()),
        context: Mutex::new(None),
        stopped: AtomicBool::new(false),
        sources: Mutex::new(
            sources
                .into_iter()
                .map(|(id, s)| (id, Arc::new(s)))
                .collect(),
        ),
    };
    let request = ReadRequest {
        source_id: body.source_id,
        offset: "1".into(),
        max_bytes: 3,
    };
    assert_eq!(resources.read(&request).unwrap().1.bytes, [6, 7, 8]);
    let read_before_stop = read.load(Ordering::Relaxed);
    assert!(read_before_stop <= 4);
    resources.stop();
    assert!(resources.read(&request).is_err());
    assert!(resources.sources.lock().unwrap().is_empty());
    assert_eq!(read.load(Ordering::Relaxed), read_before_stop);
}

#[test]
fn required_missing_counterpart_and_ungranted_source_ranges_fail_closed() {
    let app = Application::new(AppConfig::default()).unwrap();
    let snapshot = snapshot(&app);
    let mut declaration = declaration();
    declaration.inputs.response.body = BodyAccess::Required;
    assert!(
        project_input(
            &snapshot,
            boundaries("upstream-request").unwrap(),
            &declaration,
            None,
            "active",
            &mut BTreeMap::new()
        )
        .is_err()
    );
    let resources = Resources {
        live_available: false,
        finalized: AtomicBool::new(false),
        live: Mutex::new(None),
        info: Mutex::new(Vec::new()),
        context: Mutex::new(None),
        stopped: AtomicBool::new(false),
        sources: Mutex::new(BTreeMap::from([(
            "header-one".into(),
            Arc::new(Source::Header(Arc::from(b"ab".as_slice()))),
        )])),
    };
    for (id, offset, count) in [
        ("outside", "0", 1),
        ("header-one", "01", 1),
        ("header-one", "1", 2),
        ("header-one", "0", 65537),
    ] {
        assert!(
            resources
                .read(&ReadRequest {
                    source_id: id.into(),
                    offset: offset.into(),
                    max_bytes: count
                })
                .is_err()
        );
    }
}

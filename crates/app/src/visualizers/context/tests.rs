use super::*;
use crate::{
    AppConfig,
    visualizers::{
        boundaries,
        tests::{declaration, snapshot},
    },
};
use std::sync::{Mutex, atomic::AtomicBool};
use transmog_core::{
    HeaderField,
    intercept::OriginalTarget,
    observe::{ObserverEvent, ObserverEventKind},
};
use transmog_extension_registry::InstanceResources as _;
use transmog_extensions::{Context, HeaderPredicate};

pub(in crate::visualizers) fn fixture(app: &Application) -> (SessionSnapshot, Visualizer) {
    let mut snapshot = snapshot(app);
    snapshot.request_heads[0].head.headers = HeaderBlock::from_fields(vec![
        HeaderField::try_new("X-Kind", "conversation").unwrap(),
        HeaderField::try_new("Cookie", "granted").unwrap(),
        HeaderField::try_new("Hidden", "never delivered").unwrap(),
    ]);
    let mut declaration = declaration();
    declaration.context = Some(Context {
        headers: vec![HeaderPredicate {
            direction: Direction::Request,
            name: "x-kind".into(),
            test: HeaderTest::Present,
        }],
        max_exchanges: 8,
        look_ahead: true,
    });
    (snapshot, declaration)
}
pub(in crate::visualizers) fn publish(
    app: &Application,
    id: u128,
    snapshot: &SessionSnapshot,
) -> SessionSnapshot {
    let mut metadata = (*snapshot.metadata).clone();
    metadata.exchange_id = ExchangeId(id);
    let mut kinds = vec![ObserverEventKind::ExchangeStarted {
        metadata: Arc::new(metadata),
    }];
    kinds.extend(snapshot.request_heads.iter().map(|head| {
        ObserverEventKind::RequestHeadObserved {
            boundary: head.boundary,
            head: head.head.clone(),
        }
    }));
    kinds.extend(snapshot.response_heads.iter().map(|head| {
        ObserverEventKind::ResponseHeadObserved {
            boundary: head.boundary,
            head: head.head.clone(),
        }
    }));
    for (index, kind) in kinds.into_iter().enumerate() {
        app.service.catalog().apply(ObserverEvent {
            exchange_id: ExchangeId(id),
            sequence: index as u64 + 1,
            kind,
        });
    }
    app.service.catalog().get(ExchangeId(id)).unwrap()
}
fn resources(broker: Broker) -> Resources {
    Resources {
        live_available: false,
        live: Mutex::new(None),
        stopped: AtomicBool::new(false),
        sources: Mutex::new(BTreeMap::new()),
        info: Mutex::new(Vec::new()),
        context: Mutex::new(Some(broker)),
    }
}
fn request(position: ContextPosition, cursor: Option<String>, limit: usize) -> ContextRequest {
    ContextRequest {
        position,
        cursor,
        limit,
    }
}
fn identity(snapshot: &SessionSnapshot, declaration: &Visualizer, capture: &str) -> Option<Scope> {
    scope(
        &snapshot.into(),
        boundaries("client-response").unwrap(),
        declaration,
        capture.into(),
        None,
    )
}

#[test]
fn origin_capture_caller_endpoint_and_correlation_are_independent_native_constraints() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (base, declaration) = fixture(&app);
    let expected = identity(&base, &declaration, "capture-a").unwrap();
    let changed = |mutate: fn(&mut SessionSnapshot)| {
        let mut value = base.clone();
        mutate(&mut value);
        assert_ne!(
            identity(&value, &declaration, "capture-a").as_ref(),
            Some(&expected)
        );
    };
    changed(|s| s.request_heads[0].head.target.scheme = "http".into());
    changed(|s| s.request_heads[0].head.target.host = "other.invalid".into());
    changed(|s| s.request_heads[0].head.target.port = 8443);
    changed(|s| {
        let target = Target {
            port: 8443,
            ..s.metadata.original_target.as_target().clone()
        };
        Arc::make_mut(&mut s.metadata).original_target = OriginalTarget::new(target);
    });
    changed(|s| s.request_heads[0].head.target.path = "/other".into());
    changed(|s| s.request_heads[0].head.target.query = Some("another=conversation".into()));
    changed(|s| {
        Arc::make_mut(&mut s.metadata).client_identity = ClientIdentity::LocalProcess {
            pid: 99,
            name: Some("other.exe".into()),
        }
    });
    changed(|s| {
        Arc::make_mut(&mut s.metadata).downstream_connection_id = transmog_core::ConnectionId(99);
    });
    changed(|s| {
        s.request_heads[0].head.headers =
            HeaderBlock::from_fields(vec![HeaderField::try_new("X-Kind", "other").unwrap()]);
    });
    changed(|s| {
        s.request_heads[0].head.headers = HeaderBlock::from_fields(vec![
            HeaderField::try_new("X-Kind", "conversation").unwrap(),
            HeaderField::try_new("x-kind", "conversation").unwrap(),
        ]);
    });
    changed(|s| {
        let mut field = HeaderField::try_new("X-Kind", "conversation").unwrap();
        field.redact_value();
        s.request_heads[0].head.headers = HeaderBlock::from_fields(vec![field]);
    });
    changed(|s| s.request_heads[0].head.headers = HeaderBlock::default());
    assert_ne!(
        identity(&base, &declaration, "capture-b").as_ref(),
        Some(&expected)
    );
    let mut case = base.clone();
    case.request_heads[0].head.target.scheme = "HTTPS".into();
    case.request_heads[0].head.target.host = "EXAMPLE.INVALID".into();
    case.request_heads[0].head.target.authority = "example.invalid:443".into();
    assert_eq!(
        identity(&case, &declaration, "capture-a").as_ref(),
        Some(&expected)
    );
    let upstream = scope(
        &(&base).into(),
        boundaries("upstream-response").unwrap(),
        &declaration,
        "capture-a".into(),
        None,
    );
    assert_ne!(upstream.as_ref(), Some(&expected));
    assert_eq!(
        Origin::new(&Target {
            host: "2001:db8:0:0::1".into(),
            ..case.request_heads[0].head.target.clone()
        })
        .unwrap()
        .host,
        "2001:db8::1"
    );
}

#[test]
fn pagination_and_source_reads_never_cross_origin_or_outlive_anchor() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (base, declaration) = fixture(&app);
    let first = publish(&app, 10, &base);
    let mut other = base.clone();
    other.request_heads[0].head.target.port = 8443;
    publish(&app, 11, &other);
    let second = publish(&app, 12, &base);
    let anchor = publish(&app, 13, &base);
    let selected = boundaries("client-response").unwrap();
    let resources = resources(Broker::new(
        &app,
        &anchor,
        selected,
        &declaration,
        "worker-one",
        None,
    ));
    let page = resources
        .context(&request(ContextPosition::Before, None, 1))
        .unwrap();
    assert_eq!(page.exchanges.len(), 1);
    assert_eq!(page.exchanges[0].order, second.request_number.to_string());
    assert!(
        page.exchanges[0]
            .input
            .request
            .as_ref()
            .unwrap()
            .headers
            .iter()
            .all(|header| header.name != "Hidden")
    );
    assert_eq!(
        page.exchanges[0]
            .input
            .request
            .as_ref()
            .unwrap()
            .path
            .as_deref(),
        Some("/protocol")
    );
    assert_eq!(page.status, ContextStatus::More);
    let cursor = page.next_cursor.unwrap();
    let second_page = resources
        .context(&request(ContextPosition::Before, Some(cursor.clone()), 1))
        .unwrap();
    assert_eq!(
        second_page.exchanges[0].order,
        first.request_number.to_string()
    );
    assert!(
        resources
            .context(&request(ContextPosition::Before, Some(cursor), 1))
            .is_err()
    );
    assert!(
        resources
            .context(&request(ContextPosition::Before, Some("forged".into()), 1))
            .is_err()
    );
    let id = page.exchanges[0].input.request.as_ref().unwrap().headers[0]
        .source_id
        .clone()
        .unwrap();
    let read = transmog_extensions::ReadRequest {
        source_id: id,
        offset: "0".into(),
        max_bytes: 1,
    };
    assert_eq!(resources.read(&read).unwrap().1.bytes, b"c");
    let mut moved_head = second.request_heads[0].head.clone();
    moved_head.target.host = "other.invalid".into();
    app.service.catalog().apply(ObserverEvent {
        exchange_id: second.exchange_id,
        sequence: second.last_sequence + 1,
        kind: ObserverEventKind::RequestHeadObserved {
            boundary: selected.request,
            head: moved_head,
        },
    });
    assert!(resources.read(&read).is_err());
    app.service.catalog().dismiss(&[anchor.exchange_id], &[]);
    assert!(resources.read(&read).is_err());
    assert_eq!(
        resources
            .context(&request(ContextPosition::Before, None, 1))
            .unwrap()
            .status,
        ContextStatus::Unavailable
    );
    resources.stop();
    assert!(resources.context.lock().unwrap().is_none());
    assert!(resources.sources.lock().unwrap().is_empty());
}

#[test]
fn later_reads_require_declaration_and_instance_budget_cannot_be_reset_by_new_cursors() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (base, mut declaration) = fixture(&app);
    let anchor = publish(&app, 20, &base);
    publish(&app, 21, &base);
    publish(&app, 22, &base);
    declaration.context.as_mut().unwrap().look_ahead = false;
    let selected = boundaries("client-response").unwrap();
    let resources = resources(Broker::new(
        &app,
        &anchor,
        selected,
        &declaration,
        "worker-two",
        None,
    ));
    assert!(
        resources
            .context(&request(ContextPosition::After, None, 1))
            .is_err()
    );
    declaration.context.as_mut().unwrap().look_ahead = true;
    declaration.context.as_mut().unwrap().max_exchanges = 1;
    let resources = self::resources(Broker::new(
        &app,
        &anchor,
        selected,
        &declaration,
        "worker-three",
        None,
    ));
    let page = resources
        .context(&request(ContextPosition::After, None, 1))
        .unwrap();
    assert_eq!(page.exchanges.len(), 1);
    assert_eq!(page.status, ContextStatus::Limited);
    assert_eq!(
        resources
            .context(&request(ContextPosition::After, None, 1))
            .unwrap()
            .status,
        ContextStatus::Limited
    );
    let other = self::resources(Broker::new(
        &app,
        &anchor,
        selected,
        &declaration,
        "worker-four",
        None,
    ));
    assert!(
        other
            .context(&request(
                ContextPosition::After,
                Some("context-worker-three-1".into()),
                1
            ))
            .is_err()
    );
}

#[test]
fn cursor_cannot_switch_direction_or_capture_namespace() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (base, declaration) = fixture(&app);
    publish(&app, 30, &base);
    let anchor = publish(&app, 31, &base);
    publish(&app, 32, &base);
    publish(&app, 33, &base);
    let selected = boundaries("client-response").unwrap();
    let resources = resources(Broker::new(
        &app,
        &anchor,
        selected,
        &declaration,
        "worker-five",
        None,
    ));
    let page = resources
        .context(&request(ContextPosition::After, None, 1))
        .unwrap();
    assert_eq!(page.status, ContextStatus::More);
    let cursor = page.next_cursor.unwrap();
    assert!(
        resources
            .context(&request(ContextPosition::Before, Some(cursor.clone()), 1))
            .is_err()
    );
    assert!(
        resources
            .context(&request(ContextPosition::After, Some(cursor), 1))
            .is_err()
    );
    let state = resources.context.lock().unwrap();
    let guard = state.as_ref().unwrap().guard.as_ref().unwrap();
    assert_eq!(guard.capture(true, None), None);
    assert_eq!(
        guard.capture(true, Some("source-a".into())).as_deref(),
        Some("saved:source-a")
    );
    assert_ne!(
        guard.capture(true, Some("source-a".into())),
        guard.capture(true, Some("source-b".into()))
    );
    assert_ne!(
        guard.capture(true, Some("source-a".into())),
        guard.capture(false, None)
    );
    assert_eq!(guard.capture(false, Some("source-a".into())), None);
}

#[test]
fn large_declared_headers_page_before_ipc_and_inactive_instances_retain_no_context() {
    let app = Application::new(AppConfig::default()).unwrap();
    let (mut base, declaration) = fixture(&app);
    base.request_heads[0].head.headers = HeaderBlock::from_fields(vec![
        HeaderField::try_new("X-Kind", "conversation").unwrap(),
        HeaderField::try_new("Cookie", "x".repeat(200_000)).unwrap(),
    ]);
    for id in 40..44 {
        publish(&app, id, &base);
    }
    let anchor = publish(&app, 44, &base);
    let selected = boundaries("client-response").unwrap();
    let resources = resources(Broker::new(
        &app,
        &anchor,
        selected,
        &declaration,
        "worker-six",
        None,
    ));
    assert!(resources.sources.lock().unwrap().is_empty());
    let page = resources
        .context(&request(ContextPosition::Before, None, 8))
        .unwrap();
    assert_eq!(page.exchanges.len(), 2);
    assert_eq!(page.status, ContextStatus::More);
    let page = resources
        .context(&request(ContextPosition::Before, page.next_cursor, 8))
        .unwrap();
    assert_eq!(page.exchanges.len(), 2);
    resources.stop();
    assert!(resources.info.lock().unwrap().is_empty());
    assert!(
        resources
            .context(&request(ContextPosition::Before, None, 1))
            .is_err()
    );
}

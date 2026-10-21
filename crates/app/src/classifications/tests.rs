use super::*;
use crate::{AppConfig, BodyStoreConfig, body_store::SavedBodySource};
use std::{
    io::{self, Read},
    sync::Arc,
};
use transmog_core::{
    ClientIdentity, ConnectionId, HeaderBlock, HttpLegVersion, RequestHead, ResponseHead,
    SessionId, SessionMetadata, StreamId, Target,
    intercept::{ExchangeId, ExchangeMetadata as CapturedMetadata},
    observe::{ObserverEvent, ObserverEventKind},
};
use transmog_extensions::Manifest;

fn manifest() -> Manifest {
    serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"office","name":"Office","version":"1.0.0","visualizers":[],"files":[],
        "trafficColumns":[{"id":"protocol","name":"Protocol","inputs":{"request":{"headers":["soapaction"],"body":"none"},"response":{"headers":["content-type"],"body":"none"}},"values":[
            {"value":"MS-FSSHTTP","matches":[{"direction":"response","contentTypes":["application/soap+xml"],"headers":[{"direction":"request","name":"soapaction","test":{"op":"equals","value":"CellStorageService"}}]}]},
            {"value":"Office","matches":[{"direction":"request","headers":[{"direction":"request","name":"soapaction","test":{"op":"present"}}]}]}
        ]}]
    })).unwrap()
}
#[derive(Debug)]
struct NeverRead;
impl SavedBodySource for NeverRead {
    fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        panic!("A classification must never open a body")
    }
    fn length(&self) -> Option<u64> {
        Some(1_000_000_000)
    }
}
#[test]
#[allow(clippy::too_many_lines)] // Keep the ordered real catalog and unread saved-body fixture together.
fn retained_classifications_use_both_client_heads_without_loading_code_or_bodies() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("extension");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&manifest()).unwrap(),
    )
    .unwrap();
    let registry = transmog_extension_registry::Registry::load(root.path().join("installed"), None);
    registry.register_directory(&directory, false).unwrap();
    let app = Application::new(AppConfig {
        extensions: Some(registry.clone()),
        body_store: Some(BodyStoreConfig::product_default(root.path().join("bodies"))),
        ..AppConfig::default()
    })
    .unwrap();
    let target = Target {
        scheme: "https".into(),
        authority: "office.invalid".into(),
        host: "office.invalid".into(),
        port: 443,
        path: "/".into(),
        query: None,
    };
    let metadata = CapturedMetadata::from_session(
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
    );
    let catalog = app.service.catalog();
    catalog.apply(ObserverEvent {
        exchange_id: ExchangeId(1),
        sequence: 1,
        kind: ObserverEventKind::ExchangeStarted {
            metadata: Arc::new(metadata.clone()),
        },
    });
    catalog.apply(ObserverEvent {
        exchange_id: ExchangeId(1),
        sequence: 2,
        kind: ObserverEventKind::RequestHeadObserved {
            boundary: ExchangeBoundary::ClientRequest,
            head: RequestHead {
                method: "POST".into(),
                target: target.clone(),
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("SOAPAction", "CellStorageService").unwrap(),
                ]),
                source_version: HttpLegVersion::Http1,
            },
        },
    });
    app.body_store.as_ref().unwrap().register_saved(
        ExchangeId(1),
        ExchangeBoundary::ClientResponse,
        &HeaderBlock::default(),
        None,
        Arc::new(NeverRead),
        1_000_000_000,
        true,
    );
    let ids = vec![format!("{:032x}", 1)];
    let keys = vec!["office:protocol".into()];
    assert_eq!(app.extension_traffic_columns().len(), 1);
    assert_eq!(
        app.classify_traffic_rows(&ids, &keys).unwrap()[0].values[&keys[0]],
        vec!["Office"]
    );
    for (sequence, boundary, expected) in [
        (3, ExchangeBoundary::UpstreamResponse, vec!["Office"]),
        (
            4,
            ExchangeBoundary::ClientResponse,
            vec!["MS-FSSHTTP", "Office"],
        ),
    ] {
        catalog.apply(ObserverEvent {
            exchange_id: ExchangeId(1),
            sequence,
            kind: ObserverEventKind::ResponseHeadObserved {
                boundary,
                head: ResponseHead {
                    status: 200,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("Content-Type", "application/soap+xml; charset=utf-8")
                            .unwrap(),
                    ]),
                    source_version: HttpLegVersion::Http1,
                },
            },
        });
        assert_eq!(
            app.classify_traffic_rows(&ids, &keys).unwrap()[0].values[&keys[0]],
            expected
        );
    }
    assert!(app.classify_traffic_rows(&ids, &[]).unwrap().is_empty());
    assert!(
        app.classify_traffic_rows(&vec![ids[0].clone(); 513], &keys)
            .is_err()
    );
    // A second retained exchange has no matching headers. Its saved body must
    // remain untouched while list and facet queries scan retained headers.
    let mut second_metadata = metadata;
    second_metadata.exchange_id = ExchangeId(2);
    catalog.apply(ObserverEvent {
        exchange_id: ExchangeId(2),
        sequence: 1,
        kind: ObserverEventKind::ExchangeStarted {
            metadata: Arc::new(second_metadata),
        },
    });
    catalog.apply(ObserverEvent {
        exchange_id: ExchangeId(2),
        sequence: 2,
        kind: ObserverEventKind::RequestHeadObserved {
            boundary: ExchangeBoundary::ClientRequest,
            head: RequestHead {
                method: "GET".into(),
                target,
                headers: HeaderBlock::default(),
                source_version: HttpLegVersion::Http1,
            },
        },
    });
    let filter = ClassificationFilter {
        column: keys[0].clone(),
        values: vec![crate::TrafficFilterValue::Text("ms-fsshttp".into())],
    };
    let query = crate::SessionQueryInput {
        extension_filters: vec![filter.clone()],
        limit: Some(1),
        ..Default::default()
    };
    let page = app.query_sessions(query.clone()).unwrap();
    assert_eq!(page.total_matched, 1);
    assert_eq!(page.retained_count, 2);
    assert_eq!(page.sessions[0].id, ids[0]);
    assert_eq!(app.matching_traffic_ids(&query).unwrap(), ids);
    let view = app.query_traffic_view(&query).unwrap();
    assert_eq!(view.ids, ids);
    app.release_traffic_view(&view.id);
    let counts = app
        .extension_traffic_facets(&ClassificationFacetInput {
            query: query.clone(),
            column: keys[0].clone(),
        })
        .unwrap();
    assert_eq!(counts.matching_entries, 2);
    assert_eq!(counts.omitted_entries, 0);
    assert_eq!(
        counts
            .options
            .iter()
            .map(|option| (option.label.as_str(), option.count))
            .collect::<Vec<_>>(),
        vec![("MS-FSSHTTP", 1), ("Office", 1), ("No match", 1)]
    );
    let unmatched = crate::SessionQueryInput {
        extension_filters: vec![ClassificationFilter {
            values: vec![crate::TrafficFilterValue::Unavailable],
            ..filter.clone()
        }],
        ..Default::default()
    };
    assert_eq!(
        app.query_sessions(unmatched).unwrap().sessions[0].id,
        format!("{:032x}", 2)
    );
    let host_request = crate::TrafficFacetInput {
        query: crate::SessionQueryInput {
            host: Some("other.invalid".into()),
            ..query.clone()
        },
        dimension: crate::TrafficDimension::Column(crate::workspace::TrafficColumn::Host),
        search: None,
        include_values: vec![],
        offset: 0,
        limit: None,
    };
    let hosts = app.traffic_facets(&host_request).unwrap();
    assert_eq!(
        hosts.matching_entries, 1,
        "Header scan incorrectly included the edited host restriction"
    );
    assert_eq!(hosts.options[0].label, "office.invalid");
    for column in ["not-a-qualified-key", "missing:column"] {
        let query = crate::SessionQueryInput {
            extension_filters: vec![ClassificationFilter {
                column: column.into(),
                ..filter.clone()
            }],
            ..Default::default()
        };
        assert!(app.query_sessions(query).is_err());
    }
    registry.set_enabled("office", false).unwrap();
    assert!(
        app.query_sessions(query).is_err(),
        "Unavailable filters must not silently broaden Traffic"
    );
    assert!(app.extension_traffic_columns().is_empty());
    assert!(app.classify_traffic_rows(&ids, &keys).unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(&directory).unwrap().count(),
        1,
        "No source module was required"
    );
}

#[test]
fn partial_header_work_never_yields_a_misleading_classification() {
    let declaration = manifest().traffic_columns.remove(0);
    let columns = vec![("office:protocol".into(), declaration)];
    let names = [
        BTreeSet::from(["soapaction".into()]),
        BTreeSet::from(["content-type".into()]),
    ];
    let mut request = vec![HeaderField::try_new("SOAPAction", "CellStorageService").unwrap()];
    let response = vec![HeaderField::try_new("Content-Type", "application/soap+xml").unwrap()];
    let run = |request: &[HeaderField], budget: &mut Budget| {
        classify(
            "row",
            Some(request),
            Some(&response),
            &columns,
            &names,
            budget,
        )
    };
    assert_eq!(
        run(&request, &mut Budget::default()).values["office:protocol"],
        vec!["MS-FSSHTTP", "Office"]
    );
    request[0].redact_value();
    assert_eq!(
        run(&request, &mut Budget::default()).values["office:protocol"],
        vec!["Office"],
        "Present may match a redacted name; a value predicate cannot"
    );
    for mut budget in [
        Budget {
            work: Cell::new(3),
            ..Budget::default()
        },
        Budget {
            bytes: 1,
            ..Budget::default()
        },
        Budget {
            output: 1,
            ..Budget::default()
        },
    ] {
        let result = run(&request, &mut budget);
        assert!(result.limited);
        assert!(result.values.is_empty());
    }
}

#[test]
fn matching_work_charges_value_bytes_before_parsing_long_content_types() {
    let column = manifest().traffic_columns.remove(0);
    let columns = vec![("office:protocol".into(), column)];
    let names = [
        BTreeSet::from(["soapaction".into()]),
        BTreeSet::from(["content-type".into()]),
    ];
    let request = [HeaderField::try_new("SOAPAction", "CellStorageService").unwrap()];
    let response = [HeaderField::try_new(
        "Content-Type",
        format!("application/soap+xml; {}", " ".repeat(512 * 1024)),
    )
    .unwrap()];
    let result = classify(
        "row",
        Some(&request),
        Some(&response),
        &columns,
        &names,
        &mut Budget::default(),
    );
    assert!(result.limited);
    assert!(result.values.is_empty());
}

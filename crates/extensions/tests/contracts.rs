//! Behavioral coverage for permissions, native admission and inert package/presentation contracts.
use std::{
    collections::BTreeMap,
    io::Cursor,
    sync::{Arc, atomic::AtomicBool},
};
use transmog_extensions::*;

fn manifest() -> Manifest {
    serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.protocol","version":"1.0.0","name":"Protocol",
        "visualizers":[{"id":"response","name":"Protocol response","entry":"src/main.js","surfaces":["body"],
            "inputs":{"request":{"headers":["x-requesttype"],"body":"none"},"response":{"headers":["content-type"],"body":"required"}},
            "matches":[{"direction":"response","contentTypes":["application/example"]}],
            "liveUpdates":{"events":["response"],"bodyProgress":false}}],
        "files":[{"path":"src/main.js","bytes":3}]
    })).unwrap()
}
fn header(name: &str, value: &[u8], index: u32) -> Header {
    Header {
        source_id: None,
        index,
        name: name.into(),
        value: Some(value.into()),
        redacted: false,
        original_bytes: Some(value.len().to_string()),
    }
}
fn document() -> VisualizationDocument {
    serde_json::from_value(
        serde_json::json!({"title":"Example","status":"complete","presentation":"tree","roots":[{
            "id":"root","name":"Root","value":{"kind":"integer","value":"18446744073709551615"},
            "spans":[{"sourceId":"response-body","offset":"0","length":"4"}]
        }]}),
    )
    .unwrap()
}

#[test]
fn final_updates_are_optional_and_independent_of_parse_status() {
    let original = document();
    assert!(original.updates.is_none());
    assert!(
        serde_json::to_value(&original)
            .unwrap()
            .get("updates")
            .is_none()
    );
    let mut final_view = original;
    final_view.status = ParseStatus::Partial;
    final_view.updates = Some(UpdatePolicy::Final);
    let mut encoded = serde_json::to_value(&final_view).unwrap();
    assert_eq!(encoded["updates"], "final");
    assert_eq!(
        serde_json::from_value::<VisualizationDocument>(encoded.clone()).unwrap(),
        final_view
    );
    encoded["updates"] = serde_json::json!("restart");
    assert!(serde_json::from_value::<VisualizationDocument>(encoded).is_err());
}

#[test]
fn automatic_visualizers_opt_in_without_changing_existing_package_revisions() {
    let original = manifest();
    assert!(!original.visualizers[0].automatic);
    assert!(
        serde_json::to_value(&original).unwrap()["visualizers"][0]
            .get("automatic")
            .is_none()
    );
    let mut automatic = original.clone();
    automatic.visualizers[0].automatic = true;
    let encoded = serde_json::to_vec(&automatic).unwrap();
    let round_trip = Manifest::parse(&encoded, true).unwrap();
    assert!(round_trip.visualizers[0].automatic);
    // Opt-in does not weaken native matching or input declarations.
    automatic.visualizers[0].matches.clear();
    assert!(automatic.validate(true).is_err());
}

#[test]
fn missing_access_is_invalid_and_matching_cannot_read_undeclared_headers() {
    let mut value = serde_json::to_value(manifest()).unwrap();
    value["visualizers"][0]["inputs"]["request"]
        .as_object_mut()
        .unwrap()
        .remove("body");
    assert!(Manifest::parse(&serde_json::to_vec(&value).unwrap(), true).is_err());
    let mut m = manifest();
    m.visualizers[0].inputs.response.headers.clear();
    assert!(matches!(m.validate(true), Err(Error::AccessDenied)));
    for invalid in ["*", "x-*", "content-type\r\n", "", "Cookie: "] {
        let mut m = manifest();
        m.visualizers[0].inputs.request.headers = vec![invalid.into()];
        assert!(m.validate(true).is_err());
    }
    let mut m = manifest();
    m.visualizers[0].inputs.request.headers = vec!["Name".into(), "name".into()];
    assert!(m.validate(true).is_err());
}

#[test]
fn http_decoding_is_explicit_and_requires_both_header_and_body_dependencies() {
    let original = manifest();
    let encoded = serde_json::to_value(&original).unwrap();
    assert!(
        encoded["visualizers"][0]["inputs"]["response"]
            .get("decodeHttpContent")
            .is_none(),
        "Existing canonical revisions must not change"
    );
    let mut candidate = original.clone();
    candidate.visualizers[0].inputs.response.decode_http_content = true;
    assert!(candidate.validate(true).is_err());
    candidate.visualizers[0]
        .inputs
        .response
        .headers
        .push("Content-Encoding".into());
    candidate.validate(true).unwrap();
    candidate.visualizers[0].inputs.response.body = BodyAccess::None;
    assert!(candidate.validate(true).is_err());
    let step = VisualizationStep::NeedDecoded {
        source_id: "body-123".into(),
        token: "decode".into(),
    };
    let value = serde_json::to_value(&step).unwrap();
    assert_eq!(value["sourceId"], "body-123");
    assert_eq!(value["kind"], "needDecoded");
    assert!(!value.as_object().unwrap().contains_key("source_id"));
}

#[test]
fn context_requires_declared_correlation_and_finite_opt_in_lookahead() {
    let mut m = manifest();
    m.visualizers[0].context = Some(Context {
        headers: vec![HeaderPredicate {
            direction: Direction::Request,
            name: "x-requesttype".into(),
            test: HeaderTest::Present,
        }],
        max_exchanges: 64,
        look_ahead: false,
    });
    m.validate(true).unwrap();
    assert!(
        serde_json::to_value(&m.visualizers[0]).unwrap()["context"]
            .get("lookAhead")
            .is_none(),
        "Default lookahead must preserve existing canonical package digests"
    );
    let mut request = ContextRequest {
        position: ContextPosition::Before,
        cursor: None,
        limit: 32,
    };
    let context = m.visualizers[0].context.as_ref().unwrap();
    request.validate(context).unwrap();
    for limit in [0, 33, 257] {
        request.limit = limit;
        assert!(request.validate(context).is_err());
    }
    request.limit = 1;
    request.position = ContextPosition::After;
    assert!(matches!(
        request.validate(context),
        Err(Error::AccessDenied)
    ));
    let context = m.visualizers[0].context.as_mut().unwrap();
    context.look_ahead = true;
    request.validate(context).unwrap();
    request.cursor = Some("../../another-capture".into());
    assert!(request.validate(context).is_err());
    context.headers[0].name = "undeclared".into();
    assert!(matches!(m.validate(true), Err(Error::AccessDenied)));
}

#[test]
fn executable_contract_is_explicit_javascript_with_optional_inert_authoring_sources() {
    let m = manifest();
    let mut value = serde_json::to_value(&m).unwrap();
    value.as_object_mut().unwrap().remove("runtime");
    assert!(Manifest::parse(&serde_json::to_vec(&value).unwrap(), true).is_err());
    for runtime in ["typescript", "javascript-latest", "javascript-es2099"] {
        let mut other = m.clone();
        other.runtime = runtime.into();
        assert!(matches!(other.validate(true), Err(Error::Unsupported)));
    }
    for extension in ["js", "mjs"] {
        let mut other = m.clone();
        other.visualizers[0].entry = format!("src/main.{extension}");
        other.files[0].path.clone_from(&other.visualizers[0].entry);
        // An author can include source/maps for development; they never become entry modules.
        other.files.push(Payload {
            path: "source/main.ts".into(),
            bytes: 0,
            sha256: None,
        });
        other.files.push(Payload {
            path: "src/main.js.map".into(),
            bytes: 0,
            sha256: None,
        });
        other.validate(true).unwrap();
    }
    for extension in ["ts", "tsx", "jsx", "cjs", "json"] {
        let mut other = m.clone();
        other.visualizers[0].entry = format!("src/main.{extension}");
        other.files[0].path.clone_from(&other.visualizers[0].entry);
        assert!(other.validate(true).is_err());
    }
}

#[test]
fn exact_projection_preserves_duplicates_and_redaction_without_bodies() {
    let m = manifest();
    let mut secret = header("x-requesttype", b"not-visible", 4);
    secret.redacted = true;
    let fields = vec![
        header("Cookie", b"secret", 0),
        header("X-RequestType", b"Execute", 1),
        header("x-requesttype", b"Connect", 2),
        secret,
    ];
    let projected = project_headers(&m.visualizers[0].inputs, Direction::Request, &fields).unwrap();
    assert_eq!(projected.len(), 3);
    assert_eq!(projected[0].index, 1);
    assert_eq!(projected[1].index, 2);
    assert!(projected[2].value.is_none());
    assert!(matches!(
        check_body_access(&m.visualizers[0].inputs, Direction::Request),
        Err(Error::AccessDenied)
    ));
}

#[test]
fn native_index_delivers_only_matching_active_instances_in_the_exact_scope() {
    let scope = Scope {
        capture: "capture-a".into(),
        conversation: "conversation-a".into(),
    };
    let other = Scope {
        capture: "capture-b".into(),
        conversation: "conversation-a".into(),
    };
    let headers = vec![header(
        "Content-Type",
        b"Application/Example; charset=utf-8",
        0,
    )];
    let response = MessageMetadata {
        direction: Direction::Response,
        headers: &headers,
        method: None,
        path: None,
        status: Some(200),
    };
    let exchange = ExchangeMetadata {
        request: None,
        response: Some(response),
    };
    let mut index = ActiveSubscriptions::default();
    assert!(
        index
            .admitted(&scope, exchange, Direction::Response)
            .is_empty()
    );
    index
        .subscribe(
            "instance-a".into(),
            scope.clone(),
            manifest().visualizers.remove(0),
        )
        .unwrap();
    index
        .subscribe(
            "instance-b".into(),
            other.clone(),
            manifest().visualizers.remove(0),
        )
        .unwrap();
    assert_eq!(
        index.admitted(&scope, exchange, Direction::Response),
        vec!["instance-a"]
    );
    index.unsubscribe("instance-a");
    assert!(
        index
            .admitted(&scope, exchange, Direction::Response)
            .is_empty()
    );
    assert_eq!(
        index.admitted(&other, exchange, Direction::Response),
        vec!["instance-b"]
    );
    let conflicting = vec![
        header("content-type", b"application/example", 0),
        header("Content-Type", b"text/html", 1),
    ];
    let exchange = ExchangeMetadata {
        response: Some(MessageMetadata {
            headers: &conflicting,
            ..response
        }),
        ..exchange
    };
    assert!(
        index
            .admitted(&other, exchange, Direction::Response)
            .is_empty()
    );
}

#[test]
fn document_validation_rejects_forged_spans_duplicate_nodes_and_false_provenance() {
    let sources = BTreeMap::from([("response-body".into(), 4)]);
    let mut doc = document();
    doc.validate(&sources, Limits::default()).unwrap();
    doc.roots[0].spans[0].length = "5".into();
    assert!(doc.validate(&sources, Limits::default()).is_err());
    let mut doc = document();
    doc.roots.push(doc.roots[0].clone());
    assert!(doc.validate(&sources, Limits::default()).is_err());
    let mut doc = document();
    doc.sources.push(DerivedSource {
        id: "decoded".into(),
        name: "Decoded".into(),
        bytes_base64: "AA==".into(),
        parents: vec![Span {
            source_id: "decoded".into(),
            offset: "0".into(),
            length: "1".into(),
        }],
        transform: "Decode".into(),
        exact_mapping: false,
    });
    assert!(doc.validate(&sources, Limits::default()).is_err());
    assert!(serde_json::from_value::<VisualizationDocument>(serde_json::json!({"title":"bad","status":"complete","presentation":"tree","roots":[],"html":"<script/>"})).is_err());
}

#[test]
fn tmext_round_trip_is_zip64_and_payloads_are_integrity_checked_on_activation() {
    let files = BTreeMap::from([("src/main.js".into(), Arc::<[u8]>::from(&b"abc"[..]))]);
    let snapshot = Snapshot::new(manifest(), files).unwrap();
    let mut bytes = vec![];
    write_package(&mut bytes, &snapshot, &AtomicBool::new(false)).unwrap();
    assert!(bytes.windows(4).any(|w| w == [0x50, 0x4b, 0x06, 0x06]));
    let mut package = Package::open(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(package.manifest().id, "example.protocol");
    assert_eq!(package.payload("src/main.js").unwrap(), b"abc");
    assert!(matches!(
        package.payload("../secrets"),
        Err(Error::AccessDenied)
    ));
    let pos = bytes.windows(3).position(|w| w == b"abc").unwrap();
    bytes[pos] = b'z';
    let mut changed = Package::open(Cursor::new(bytes)).unwrap();
    assert!(changed.payload("src/main.js").is_err());
}

#[test]
fn output_limit_counts_serialized_structure_and_identifiers_are_safe_on_windows() {
    for id in [".", "..", "con", "aux.txt", "com1", "item.", "_item"] {
        assert!(!valid_id(id), "{id}");
    }
    let sources = BTreeMap::from([("response-body".into(), 4)]);
    let mut doc = document();
    doc.roots[0].spans = vec![doc.roots[0].spans[0].clone(); 64];
    let limits = Limits {
        output_bytes: 512,
        ..Limits::default()
    };
    assert!(matches!(doc.validate(&sources, limits), Err(Error::Limit)));
}

#[test]
fn static_columns_are_header_only_and_packages_can_contain_just_metadata() {
    let column = serde_json::json!({"id":"protocol","name":"Protocol",
        "inputs":{"request":{"headers":["soapaction"],"body":"none"},"response":{"headers":["content-type"],"body":"none"}},
        "values":[{"value":"MS-FSSHTTP","matches":[{"direction":"response","contentTypes":["application/soap+xml"],"headers":[{"direction":"request","name":"soapaction","test":{"op":"equals","value":"CellStorageService"}}]}]}]});
    let mut metadata = serde_json::to_value(manifest()).unwrap();
    metadata["visualizers"] = serde_json::json!([]);
    metadata["files"] = serde_json::json!([]);
    metadata["trafficColumns"] = serde_json::json!([column.clone()]);
    let parsed = Manifest::parse(&serde_json::to_vec(&metadata).unwrap(), true).unwrap();
    let snapshot = Snapshot::new(parsed, BTreeMap::new()).unwrap();
    let mut package = Vec::new();
    write_package(&mut package, &snapshot, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        Package::open(Cursor::new(package))
            .unwrap()
            .manifest()
            .traffic_columns[0]
            .values[0]
            .value,
        "MS-FSSHTTP"
    );
    for (path, value) in [
        ("/inputs/request/body", serde_json::json!("optional")),
        (
            "/inputs/response/decodeHttpContent",
            serde_json::json!(true),
        ),
        ("/inputs/request/headers", serde_json::json!([])),
        ("/values/0/matches/0/methods", serde_json::json!(["POST"])),
        ("/values/0/matches/0/pathPrefixes", serde_json::json!(["/"])),
        ("/values/0/matches/0/statuses", serde_json::json!([200])),
    ] {
        let mut invalid = column.clone();
        let (parent, key) = path.rsplit_once('/').unwrap();
        invalid
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), value);
        assert!(
            serde_json::from_value::<ClassificationColumn>(invalid)
                .unwrap()
                .validate()
                .is_err(),
            "{path}"
        );
    }
    assert!(
        !serde_json::to_value(manifest())
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("trafficColumns"),
        "Old package revisions must retain their canonical manifest bytes"
    );
    for key in ["", "x", "a:b:c", "con:protocol", "a:../b"] {
        assert!(!valid_classification_key(key));
    }
    assert!(valid_classification_key(&classification_key(
        "office", "protocol"
    )));
}

#[test]
fn viewer_rules_can_require_both_request_and_response_headers() {
    let rule: MatchRule =
        serde_json::from_value(serde_json::json!({"direction":"request","headers":[
            {"direction":"request","name":"x-requesttype","test":{"op":"equals","value":"Execute"}},
            {"direction":"response","name":"x-responsecode","test":{"op":"equals","value":"0"}}
        ]}))
        .unwrap();
    let request = [header("X-RequestType", b"Execute", 0)];
    let response = [header("X-ResponseCode", b"0", 0)];
    let message = |direction, headers| MessageMetadata {
        direction,
        headers,
        method: None,
        path: None,
        status: None,
    };
    let pending = ExchangeMetadata {
        request: Some(message(Direction::Request, &request)),
        response: None,
    };
    assert!(!matches(&rule, pending, Direction::Request));
    assert!(matches(
        &rule,
        ExchangeMetadata {
            response: Some(message(Direction::Response, &response)),
            ..pending
        },
        Direction::Request
    ));
    let response = [Header {
        redacted: true,
        ..response[0].clone()
    }];
    assert!(!matches(
        &rule,
        ExchangeMetadata {
            response: Some(message(Direction::Response, &response)),
            ..pending
        },
        Direction::Request
    ));
}

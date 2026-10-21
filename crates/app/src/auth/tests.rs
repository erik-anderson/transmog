use super::*;
use crate::AppConfig;
use transmog_core::{
    ClientIdentity, ConnectionId, HeaderField, HttpLegVersion, RequestHead, SessionId,
    SessionMetadata, StreamId, Target,
    intercept::{ExchangeId, ExchangeMetadata},
    observe::{ExchangeBoundary, ObserverEvent, ObserverEventKind},
};
fn application_fixture() -> Application {
    let app = Application::new(AppConfig::default()).unwrap();
    let target = Target {
        scheme: "https".into(),
        authority: "example.invalid".into(),
        host: "example.invalid".into(),
        port: 443,
        path: "/token".into(),
        query: Some("client_id=example&response_type=code&state=private-state".into()),
    };
    let metadata = Arc::new(ExchangeMetadata::from_session(
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
    let mut fields: Vec<_> = (0..520)
        .map(|_| HeaderField::try_new("x-ignored", "value").unwrap())
        .collect();
    fields.push(HeaderField::try_new("Authorization", "Basic dXNlcjpwYXNzOm1vcmU=").unwrap());
    for (sequence, boundary, headers) in [
        (
            2,
            ExchangeBoundary::ClientRequest,
            HeaderBlock::from_fields(fields),
        ),
        (3, ExchangeBoundary::UpstreamRequest, HeaderBlock::default()),
    ] {
        app.service.catalog().apply(ObserverEvent {
            exchange_id: ExchangeId(1),
            sequence,
            kind: ObserverEventKind::RequestHeadObserved {
                boundary,
                head: RequestHead {
                    method: "GET".into(),
                    target: target.clone(),
                    headers,
                    source_version: HttpLegVersion::Http1,
                },
            },
        });
    }
    app
}
fn request(app: &Application, generation: &str) -> AuthRequest {
    AuthRequest {
        slot: "request-auth".into(),
        generation: generation.into(),
        page_id: app.visualizer_page(),
        session_id: format!("{:032x}", 1),
        boundary: "client-request".into(),
        scan_body: false,
    }
}
#[tokio::test]
async fn scoped_values_stages_source_and_retention() {
    let app = application_fixture();
    let view = app.inspect_auth(request(&app, "1")).await.unwrap();
    assert_eq!(view.inspection.findings[0].source.occurrence, 520);
    assert!(
        !serde_json::to_string(&view)
            .unwrap()
            .contains("private-state")
    );
    let field = view.inspection.findings[0]
        .fields
        .iter()
        .find(|f| f.name == "Password")
        .unwrap();
    assert_eq!(
        app.auth_value(&view.handle, &field.id).unwrap(),
        "pass:more"
    );
    assert_eq!(
        app.auth_source(&view.handle, "f0")
            .unwrap()
            .source
            .occurrence,
        520
    );
    let other = application_fixture();
    let _ = other.inspect_auth(request(&other, "1")).await.unwrap();
    assert!(other.auth_value(&view.handle, &field.id).is_err());
    let mut next = request(&app, "2");
    next.boundary = "upstream-request".into();
    let next = app.inspect_auth(next).await.unwrap();
    assert!(app.auth_value(&view.handle, &field.id).is_err());
    assert!(next.inspection.findings.iter().all(|f| f.format != "Basic"));
    app.cancel_auth("request-auth", "3", &app.visualizer_page());
    assert!(app.auth_value(&next.handle, "v0").is_err());
    assert!(app.inspect_auth(request(&app, "3")).await.is_err());
}
#[tokio::test]
async fn document_and_visibility_invalidate_access() {
    let app = application_fixture();
    let view = app.inspect_auth(request(&app, "1")).await.unwrap();
    app.set_visualizers_visible(false);
    assert!(app.auth_value(&view.handle, "v0").is_err());
    assert!(app.inspect_auth(request(&app, "2")).await.is_err());
    app.set_visualizers_visible(true);
    let view = app.inspect_auth(request(&app, "2")).await.unwrap();
    app.reset_visualizer_document();
    assert!(app.auth_value(&view.handle, "v0").is_err());
    assert!(app.inspect_auth(request(&app, "1")).await.is_ok());
}
#[test]
fn late_cancel_cannot_close_newer_generation() {
    let service = Service::default();
    let application = application_fixture();
    let one = service.begin(&request(&application, "1")).unwrap();
    let two = service.begin(&request(&application, "2")).unwrap();
    assert!(one.load(Ordering::Acquire));
    service.cancel("request-auth", "1");
    assert!(!two.load(Ordering::Acquire));
    service.cancel("request-auth", "3");
    assert!(two.load(Ordering::Acquire));
    assert!(service.begin(&request(&application, "2")).is_err());
}

#[tokio::test]
async fn removed_and_redacted_evidence_revokes_cached_values() {
    let app = application_fixture();
    let view = app.inspect_auth(request(&app, "1")).await.unwrap();
    let snapshot = app.service.catalog().get(ExchangeId(1)).unwrap();
    let mut head = snapshot.request_heads[0].head.clone();
    let mut redacted = HeaderField::try_new("Authorization", "Basic dXNlcjpwYXNz").unwrap();
    redacted.redact_value();
    head.headers = HeaderBlock::from_fields(vec![redacted]);
    app.service.catalog().apply(ObserverEvent {
        exchange_id: ExchangeId(1),
        sequence: 4,
        kind: ObserverEventKind::RequestHeadObserved {
            boundary: ExchangeBoundary::ClientRequest,
            head,
        },
    });
    assert!(app.auth_value(&view.handle, "v1").is_err());
    let view = app.inspect_auth(request(&app, "2")).await.unwrap();
    assert_eq!(view.inspection.findings[0].status, "redacted");
    app.service.catalog().dismiss(&[ExchangeId(1)], &[]);
    assert!(app.auth_source(&view.handle, "f0").is_err());
}
#[tokio::test]
async fn imported_har_uses_the_same_lazy_body_analysis_and_missing_connection_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("auth.har");
    std::fs::write(&path,serde_json::to_vec(&serde_json::json!({"log":{"version":"1.2","creator":{"name":"synthetic","version":"1"},"entries":[{"startedDateTime":"2023-11-14T22:13:20Z","time":1,"request":{"method":"GET","url":"https://example.invalid/token","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":0},"response":{"status":200,"statusText":"OK","httpVersion":"HTTP/1.1","headers":[{"name":"Content-Type","value":"application/json"}],"cookies":[],"content":{"size":63,"mimeType":"application/json","text":"{\"access_token\":\"opaque-private-token\",\"token_type\":\"Bearer\"}"},"redirectURL":"","headersSize":-1,"bodySize":63},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}]}})).unwrap()).unwrap();
    let app = Application::new(AppConfig {
        body_store: Some(crate::BodyStoreConfig::product_default(
            directory.path().join("bodies"),
        )),
        ..AppConfig::default()
    })
    .unwrap();
    app.import_trace(
        crate::TraceImportRequest {
            password: None,
            path,
            operation_id: "auth-import".into(),
            max_file_bytes: 1024 * 1024,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    let id = app.service.catalog().retained_ids()[0];
    let snapshot = app.service.catalog().get(id).unwrap();
    assert!(snapshot.imported);
    let boundary = inspector::boundary(snapshot.response_heads.last().unwrap().boundary);
    let mut selection = request(&app, "1");
    selection.session_id = format!("{:032x}", id.0);
    selection.boundary = boundary;
    let discovery = app.inspect_auth(selection.clone()).await.unwrap();
    assert!(discovery.inspection.findings.is_empty());
    assert_eq!(
        discovery.inspection.coverage.last().unwrap().status,
        "skipped"
    );
    selection.generation = "2".into();
    selection.scan_body = true;
    let view = app.inspect_auth(selection).await.unwrap();
    assert_eq!(view.inspection.findings[0].format, "OAuth / OpenID Connect");
    assert!(
        !serde_json::to_string(&view)
            .unwrap()
            .contains("opaque-private-token")
    );
    let related = app.related_auth(&view.handle).unwrap();
    assert!(related.detail.contains("connection metadata is missing"));
}

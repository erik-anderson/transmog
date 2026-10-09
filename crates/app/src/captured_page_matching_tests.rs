//! Exercise preference selection using real native imports and recorded process metadata.
use super::*;
use crate::{AppConfig, BodyStoreConfig, TraceImportRequest};
use std::{
    fs::File,
    sync::{Arc, atomic::AtomicBool},
};
use transmog_capture::{
    CaptureLimits, CaptureRecord, CaptureRecordKind, CaptureWriter, CapturedHeader,
};

struct Row {
    id: u128,
    path: &'static str,
    at: u64,
    pid: u32,
    headers: Option<Vec<(&'static str, &'static str)>>,
    body: &'static str,
    status: u16,
    response_at: Option<u64>,
}
fn row(
    id: u128,
    path: &'static str,
    at: u64,
    pid: u32,
    headers: Option<Vec<(&'static str, &'static str)>>,
    body: &'static str,
) -> Row {
    Row {
        id,
        path,
        at,
        pid,
        headers,
        body,
        status: 200,
        response_at: None,
    }
}
fn response_timing(at: u64) -> CaptureRecordKind {
    CaptureRecordKind::Performance(transmog_core::performance::PerformanceEvidence {
        points: vec![transmog_core::performance::TimingPoint {
            milestone: transmog_core::performance::Milestone::ClientResponseBegin,
            unix_millis: 1_800_000_000_000 + at,
            offset_micros: 0,
        }],
        ..Default::default()
    })
}
fn header(name: &str, value: &str) -> CapturedHeader {
    CapturedHeader {
        name: name.as_bytes().to_vec(),
        value: Some(value.as_bytes().to_vec()),
        original_value_bytes: None,
    }
}
fn fixture(path: &std::path::Path, rows: Vec<Row>) {
    let mut writer =
        CaptureWriter::new(File::create(path).unwrap(), CaptureLimits::default()).unwrap();
    for row in rows {
        let mut headers = std::collections::BTreeMap::new();
        if let Some(extra) = row.headers {
            headers.extend([
                ("User-Agent", "Placeholder/1"),
                ("Origin", "https://placeholder.invalid"),
                ("Referer", "https://placeholder.invalid/page"),
                ("Sec-Fetch-Dest", "script"),
            ]);
            headers.extend(extra);
        }
        let mut request_headers = headers
            .into_iter()
            .map(|(name, value)| header(name, value))
            .collect::<Vec<_>>();
        request_headers.push(header("Content-Length", "0"));
        let kinds = [
            CaptureRecordKind::ExchangeStarted {
                client_addr: "127.0.0.1:1234".into(),
                client_identity: transmog_core::ClientIdentity::LocalProcess {
                    pid: row.pid,
                    name: Some("placeholder.exe".into()),
                },
                listener_addr: "127.0.0.1:8888".into(),
                authority: "placeholder.invalid".into(),
                started_unix_nanos: 1_800_000_000_000_000_000 + u128::from(row.at) * 1_000_000,
            },
            CaptureRecordKind::RequestHead {
                boundary: ExchangeBoundary::ClientRequest,
                method: "GET".into(),
                target: format!("https://placeholder.invalid{}", row.path),
                headers: request_headers,
            },
            CaptureRecordKind::ResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                status: row.status,
                headers: vec![
                    header(
                        "Content-Type",
                        if row.path == "/page" {
                            "text/html"
                        } else {
                            "text/plain"
                        },
                    ),
                    header("Content-Length", &row.body.len().to_string()),
                ],
            },
            CaptureRecordKind::BodySegment {
                boundary: ExchangeBoundary::ClientResponse,
                byte_count: row.body.len(),
                bytes: Some(row.body.as_bytes().to_vec()),
                truncated: false,
            },
            CaptureRecordKind::Completed,
        ];
        let kinds = kinds
            .into_iter()
            .take(4)
            .chain(row.response_at.map(response_timing))
            .chain(std::iter::once(CaptureRecordKind::Completed));
        for (sequence, kind) in kinds.enumerate() {
            writer
                .append(&CaptureRecord {
                    sequence: sequence as u64 + 1,
                    exchange_id: row.id,
                    kind,
                })
                .unwrap();
        }
    }
    writer.seal().unwrap();
}
fn response(scene: &CapturedPage, path: &str, headers: &[(String, String)]) -> String {
    let body = format!("{:x}", sha2::Sha256::digest([]));
    let resource = scene
        .resolve(
            "GET",
            &format!("https://placeholder.invalid{path}"),
            Some(headers),
            Some(&body),
            false,
        )
        .unwrap();
    std::fs::read_to_string(resource.body_path).unwrap()
}
use sha2::Digest as _;

#[allow(clippy::too_many_lines)] // One declarative matrix covers independent context preferences.
fn preference_rows() -> Vec<Row> {
    let mut rows = vec![
        row(
            1,
            "/page",
            100,
            10,
            Some(vec![("Sec-Fetch-Dest", "document")]),
            "<h1>Placeholder</h1>",
        ),
        row(
            2,
            "/ua",
            101,
            20,
            Some(vec![("User-Agent", "Placeholder/2")]),
            "other UA",
        ),
        row(3, "/ua", 120, 20, Some(vec![]), "matching UA"),
        row(4, "/process", 101, 20, Some(vec![]), "other process"),
        row(5, "/process", 120, 10, Some(vec![]), "matching process"),
        row(
            6,
            "/reference",
            101,
            10,
            Some(vec![
                ("Origin", "https://other.invalid"),
                ("Referer", "https://other.invalid/page"),
            ]),
            "other page",
        ),
        row(7, "/reference", 120, 10, Some(vec![]), "matching page"),
        row(
            8,
            "/dest",
            101,
            10,
            Some(vec![("Sec-Fetch-Dest", "image")]),
            "image",
        ),
        row(9, "/dest", 120, 10, Some(vec![]), "script"),
        row(
            10,
            "/sec",
            101,
            10,
            Some(vec![
                ("Sec-Fetch-Mode", "cors"),
                ("Sec-Fetch-Site", "cross-site"),
            ]),
            "other fetch context",
        ),
        row(
            11,
            "/sec",
            120,
            10,
            Some(vec![
                ("Sec-Fetch-Mode", "same-origin"),
                ("Sec-Fetch-Site", "same-origin"),
            ]),
            "matching fetch context",
        ),
        row(12, "/fallback", 101, 20, None, "available fallback"),
        row(13, "/time", 99, 10, Some(vec![]), "before navigation"),
        row(14, "/time", 105, 10, Some(vec![]), "first after navigation"),
        row(15, "/time", 110, 10, Some(vec![]), "next response"),
    ];
    rows.push(row(
        16,
        "/conditional",
        99,
        10,
        Some(vec![]),
        "cached representation",
    ));
    let mut conditional = row(17, "/conditional", 105, 10, Some(vec![]), "");
    conditional.status = 304;
    rows.push(conditional);
    let mut early_request = row(19, "/response-time", 90, 10, Some(vec![]), "later response");
    early_request.response_at = Some(115);
    rows.push(early_request);
    let mut later_request = row(
        20,
        "/response-time",
        100,
        10,
        Some(vec![]),
        "first response",
    );
    later_request.response_at = Some(105);
    rows.push(later_request);
    rows.push(row(
        21,
        "/tied-time",
        105,
        10,
        Some(vec![]),
        "first response",
    ));
    rows.push(row(
        22,
        "/tied-time",
        105,
        10,
        Some(vec![]),
        "next response",
    ));
    rows
}

async fn preference_scene(root: &std::path::Path) -> CapturedPage {
    let path = root.join("placeholder.tmcap");
    fixture(&path, preference_rows());
    let app = Application::new(AppConfig {
        body_store: Some(BodyStoreConfig::product_default(root.join("cache"))),
        ..AppConfig::default()
    })
    .unwrap();
    app.import_trace(
        TraceImportRequest {
            path,
            operation_id: "preferences".into(),
            password: None,
            max_file_bytes: 1024 * 1024,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    let additional = root.join("additional.tmcap");
    fixture(
        &additional,
        vec![row(
            18,
            "/process",
            101,
            10,
            Some(vec![]),
            "PID from another capture",
        )],
    );
    app.import_trace(
        TraceImportRequest {
            path: additional,
            operation_id: "additional".into(),
            password: None,
            max_file_bytes: 1024 * 1024,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();

    let id = app
        .query_sessions(crate::SessionQueryInput::default())
        .unwrap()
        .sessions
        .into_iter()
        .find(|s| s.path == "/page")
        .unwrap()
        .id;
    app.prepare_captured_page_with_options(
        id,
        CapturedPageOptions {
            scope: CapturedPageScope::AllLoaded,
        },
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn preview_prefers_client_navigation_and_sec_context_then_advances_and_reloads() {
    let root = tempfile::tempdir().unwrap();
    let scene = preference_scene(root.path()).await;
    assert_eq!(scene.user_agent.as_deref(), Some("Placeholder/1"));
    let headers = [
        ("User-Agent", "Placeholder/1"),
        ("Origin", "https://placeholder.invalid"),
        ("Referer", "https://placeholder.invalid/page"),
        ("Sec-Fetch-Dest", "script"),
        ("Sec-Fetch-Mode", "same-origin"),
        ("Sec-Fetch-Site", "same-origin"),
    ]
    .into_iter()
    .map(|(name, value)| (name.into(), value.into()))
    .collect::<Vec<_>>();
    assert_eq!(response(&scene, "/ua", &headers), "matching UA");
    assert_eq!(response(&scene, "/process", &headers), "matching process");
    assert_eq!(
        response(&scene, "/conditional", &headers),
        "cached representation"
    );
    assert_eq!(response(&scene, "/reference", &headers), "matching page");
    assert_eq!(response(&scene, "/dest", &headers), "script");
    assert_eq!(response(&scene, "/sec", &headers), "matching fetch context");
    assert_eq!(response(&scene, "/tied-time", &headers), "first response");
    assert_eq!(response(&scene, "/tied-time", &headers), "next response");
    assert_eq!(response(&scene, "/tied-time", &headers), "next response");
    assert_eq!(
        response(&scene, "/response-time", &headers),
        "first response"
    );
    assert!(
        scene
            .resolve(
                "GET",
                "https://placeholder.invalid/fallback",
                None,
                None,
                false
            )
            .is_some()
    );
    assert_eq!(
        response(&scene, "/time", &headers),
        "first after navigation"
    );
    assert_eq!(response(&scene, "/time", &headers), "next response");
    assert_eq!(response(&scene, "/time", &headers), "next response");
    assert!(
        scene
            .resolve("GET", "https://placeholder.invalid/page", None, None, true)
            .is_some()
    );
    assert_eq!(
        response(&scene, "/time", &headers),
        "first after navigation"
    );
    assert!(
        scene
            .resolve(
                "POST",
                "https://placeholder.invalid/time",
                None,
                None,
                false
            )
            .is_none()
    );
    assert!(
        scene
            .resolve(
                "GET",
                "https://placeholder.invalid/missing",
                None,
                None,
                false
            )
            .is_none()
    );
}

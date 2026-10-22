use super::*;
use crate::{CapturedPageScope, CapturedResourceDecision, PreviewRuleSnapshot};
use serde_json::json;

fn active(registry: &SavedDemoRegistry, owner: &str, incarnation: &str) {
    registry.admit_owner(owner, incarnation).unwrap();
}
fn scene(owner: &str, incarnation: &str, body: &[u8], skipped: usize) -> CapturedPage {
    let html = b"<html><link rel=stylesheet href=/theme.css><body>frozen</body></html>";
    let html_hash = hash(html);
    let style_hash = hash(body);
    let rule = transmog_automation::Rule {
        id: "frozen-style".into(),
        display_name: Some("Frozen style".into()),
        enabled: true,
        revision: 7,
        priority: 0,
        matcher: transmog_automation::RuleMatcher {
            url: Some(transmog_automation::UrlCondition::Exact(
                "https://demo.invalid/theme.css".into(),
            )),
            ..Default::default()
        },
        request: transmog_automation::RequestActions {
            response_asset: Some("deleted-or-linked-asset@4".into()),
            ..Default::default()
        },
        response: transmog_automation::ResponseActions::default(),
    };
    let selection = ExperimentSelection {
        experiment_id: owner.into(),
        incarnation: incarnation.into(),
        variant_id: String::new(),
        revision: 3,
    };
    let report = CapturedPageReport {
        url: "https://demo.invalid/".into(),
        scope: CapturedPageScope::OriginalTrace,
        source: "capture.saz".into(),
        source_trace_id: Some("deleted-trace".into()),
        scripts_enabled: Some(true),
        selected_entry_id: "00000000000000000000000000000001".into(),
        runtime: "old runtime".into(),
        bytes_available: true,
        rules: vec![PreviewRuleSnapshot {
            id: "frozen-style".into(),
            name: "Frozen style".into(),
            revision: 7,
            asset: "deleted-or-linked-asset@4".into(),
            sha256: style_hash.clone(),
            bytes: body.len() as u64,
            shadowed_by: None,
        }],
        variant: Some(selection),
        case_origin: None,
        saved_demo: None,
        available: 1,
        skipped,
        bytes: html.len() as u64 + body.len() as u64,
        hits: 12,
        misses: 5,
        failures: 2,
        resources: vec![CapturedResourceDecision {
            entry_id: "missing".into(),
            source: "capture.saz".into(),
            source_available: true,
            method: "GET".into(),
            url: "https://demo.invalid/missing.png".into(),
            unix_millis: Some(1),
            bytes: None,
            sha256: None,
            decision: "304 has no representation body".into(),
        }],
        requests: Vec::new(),
    };
    let durable: DurableScene = serde_json::from_value(json!({
        "url":"https://demo.invalid/", "userAgent":"Recorded browser", "resources":1, "skipped":skipped, "navigationTime":123,
        "variants":[{"method":"GET", "url":"https://demo.invalid/",
            "resource":{"sha256":html_hash,"bytes":html.len(),"status":200,"reason":"OK","headers":"Content-Type: text/html\r\n"},
            "entryId":"00000000000000000000000000000001", "bodySha256":null,"vary":[],"primary":true,
            "userAgent":"Recorded browser","sameProcess":true,"origin":null,"referer":null,"secHeaders":[],"time":123,"order":"1"}],
        "overlay":[{"rule":rule,
            "resource":{"sha256":style_hash,"bytes":body.len(),"status":200,"reason":"OK","headers":"Content-Type: text/css\r\n"}}],
        "report": report
    })).unwrap();
    let root = Arc::new(tempfile::tempdir().unwrap());
    durable
        .hydrate(root.clone(), &mut |resource| {
            let bytes = if resource.sha256 == html_hash {
                html.as_slice()
            } else {
                body
            };
            let path = root.path().join(&resource.sha256);
            fs::write(&path, bytes).unwrap();
            Ok(CapturedResource {
                sha256: resource.sha256.clone(),
                status: resource.status,
                reason: resource.reason.clone(),
                headers: resource.headers.clone(),
                body_path: path,
            })
        })
        .unwrap()
}
fn save(
    registry: &SavedDemoRegistry,
    owner: &str,
    incarnation: &str,
    name: &str,
    body: &[u8],
) -> SavedDemo {
    let prepared = registry
        .stage(
            owner,
            incarnation,
            name,
            None,
            scene(owner, incarnation, body, 0),
        )
        .unwrap();
    registry
        .commit(&prepared.token, false, &AtomicBool::new(false))
        .unwrap()
}
fn revision(saved: &SavedDemo) -> SavedDemoRevision {
    SavedDemoRevision {
        id: saved.id.clone(),
        revision: saved.revision,
    }
}
fn body(page: &CapturedPage) -> Vec<u8> {
    fs::read(
        page.resolve(
            "GET",
            "https://demo.invalid/theme.css",
            Some(&[]),
            None,
            false,
        )
        .unwrap()
        .body_path,
    )
    .unwrap()
}
fn bodies(root: &Path) -> usize {
    fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("body-"))
        .count()
}

#[test]
fn restart_uses_exact_frozen_overlay_and_fresh_playback_counters() {
    let root = tempfile::tempdir().unwrap();
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    active(&registry, "owner", "creation-a");
    let saved = save(
        &registry,
        "owner",
        "creation-a",
        "A durable demo",
        b"body{color:red}",
    );
    drop(registry);
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    registry
        .reconcile_owners(&HashMap::from([("owner".into(), "creation-a".into())]))
        .unwrap();
    let page = registry
        .open(
            "owner",
            "creation-a",
            &revision(&saved),
            &AtomicBool::new(false),
        )
        .unwrap();
    let report = page.diagnostics().snapshot();
    assert_eq!(
        (
            report.hits,
            report.misses,
            report.failures,
            report.scripts_enabled
        ),
        (0, 0, 0, None)
    );
    assert!(report.requests.is_empty());
    assert_eq!(body(&page), b"body{color:red}");
    assert!(
        page.resolve("GET", "https://uncaptured.invalid/", Some(&[]), None, false)
            .is_none()
    );
    assert_eq!(
        (
            page.diagnostics().snapshot().hits,
            page.diagnostics().snapshot().misses
        ),
        (1, 1)
    );
    let another = registry
        .open(
            "owner",
            "creation-a",
            &revision(&saved),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(another.diagnostics().snapshot().hits, 0);
    assert_eq!(body(&another), b"body{color:red}");
}

#[test]
fn replacement_and_delete_require_reviewed_revision_and_open_run_owns_its_bytes() {
    let root = tempfile::tempdir().unwrap();
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    active(&registry, "owner", "creation-a");
    let old = save(&registry, "owner", "creation-a", "Old", b"red");
    let running = registry
        .open(
            "owner",
            "creation-a",
            &revision(&old),
            &AtomicBool::new(false),
        )
        .unwrap();
    let first = registry
        .stage(
            "owner",
            "creation-a",
            "New",
            Some(revision(&old)),
            scene("owner", "creation-a", b"blue", 0),
        )
        .unwrap();
    let stale = registry
        .stage(
            "owner",
            "creation-a",
            "Stale",
            Some(revision(&old)),
            scene("owner", "creation-a", b"green", 0),
        )
        .unwrap();
    let new = registry
        .commit(&first.token, false, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(new.id, old.id);
    assert_eq!(new.revision, old.revision + 1);
    assert!(
        registry
            .commit(&stale.token, false, &AtomicBool::new(false))
            .is_err()
    );
    assert!(
        registry
            .delete("owner", "creation-a", &revision(&old))
            .is_err()
    );
    assert_eq!(body(&running), b"red");
    registry
        .delete("owner", "creation-a", &revision(&new))
        .unwrap();
    assert_eq!(body(&running), b"red");
    assert!(registry.list("owner", "creation-a").is_empty());
    assert_eq!(bodies(root.path()), 0);
}

#[test]
fn shared_objects_survive_other_owner_deletion_and_only_registry_files_are_collected() {
    let root = tempfile::tempdir().unwrap();
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    active(&registry, "one", "a");
    active(&registry, "two", "b");
    let first = save(&registry, "one", "a", "One", b"shared-css");
    let second = save(&registry, "two", "b", "Two", b"shared-css");
    fs::write(
        root.path().join("response-assets-must-stay.bin"),
        b"source evidence",
    )
    .unwrap();
    assert_eq!(bodies(root.path()), 2);
    registry.delete("one", "a", &revision(&first)).unwrap();
    assert_eq!(bodies(root.path()), 2);
    assert_eq!(
        body(
            &registry
                .open("two", "b", &revision(&second), &AtomicBool::new(false))
                .unwrap()
        ),
        b"shared-css"
    );
    registry.invalidate_owner("two");
    assert_eq!(bodies(root.path()), 0);
    assert!(root.path().join("response-assets-must-stay.bin").exists());
}

#[test]
fn deleted_owner_cannot_be_resurrected_by_reviewed_token_or_reused_owner_id() {
    let root = tempfile::tempdir().unwrap();
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    active(&registry, "owner", "old-creation");
    let old = save(&registry, "owner", "old-creation", "Old", b"old");
    let prepared = registry
        .stage(
            "owner",
            "old-creation",
            "Pending",
            None,
            scene("owner", "old-creation", b"pending", 0),
        )
        .unwrap();
    registry.invalidate_owner("owner");
    active(&registry, "owner", "new-creation");
    assert!(
        registry
            .commit(&prepared.token, false, &AtomicBool::new(false))
            .is_err()
    );
    assert!(
        registry
            .open(
                "owner",
                "new-creation",
                &revision(&old),
                &AtomicBool::new(false)
            )
            .is_err()
    );
    assert!(registry.list("owner", "new-creation").is_empty());
    save(&registry, "owner", "new-creation", "New", b"new");
    drop(registry);
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    registry.reconcile_owners(&HashMap::new()).unwrap();
    assert_eq!(bodies(root.path()), 0);
}

#[test]
fn cancellation_missing_review_and_corruption_never_publish_partial_demo() {
    let root = tempfile::tempdir().unwrap();
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    active(&registry, "owner", "a");
    let prepared = registry
        .stage(
            "owner",
            "a",
            "Review",
            None,
            scene("owner", "a", b"exact reviewed bytes", 1),
        )
        .unwrap();
    assert!(prepared.report.resources[0].decision.contains("304"));
    assert!(
        registry
            .commit(&prepared.token, false, &AtomicBool::new(false))
            .is_err()
    );
    assert!(
        registry
            .commit(&prepared.token, true, &AtomicBool::new(true))
            .is_err()
    );
    assert!(registry.list("owner", "a").is_empty());
    let saved = registry
        .commit(&prepared.token, true, &AtomicBool::new(false))
        .unwrap();
    let stored = registry.lock().catalog.demos[&saved.id].clone();
    let scene = registry.read_scene(&stored.manifest).unwrap();
    let digest = scene.objects().into_keys().next().unwrap();
    fs::write(root.path().join(format!("body-{digest}.bin")), b"corrupted").unwrap();
    assert!(
        registry
            .open("owner", "a", &revision(&saved), &AtomicBool::new(false))
            .is_err()
    );
    let canceled = registry
        .stage("owner", "a", "Cancel", None, scene_for_cancel())
        .unwrap();
    registry.cancel(&canceled.token);
    assert!(
        registry
            .commit(&canceled.token, true, &AtomicBool::new(false))
            .is_err()
    );
}
fn scene_for_cancel() -> CapturedPage {
    scene("owner", "a", b"cancel", 0)
}

#[test]
fn failed_catalog_publication_keeps_previous_demo_and_prepared_retry() {
    let root = tempfile::tempdir().unwrap();
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    active(&registry, "owner", "a");
    let old = save(&registry, "owner", "a", "Old", b"old");
    let prepared = registry
        .stage(
            "owner",
            "a",
            "New",
            Some(revision(&old)),
            scene("owner", "a", b"new", 0),
        )
        .unwrap();
    let generation = registry.lock().catalog.generation + 1;
    let blocked = root.path().join(format!("catalog-{generation:020}.json"));
    fs::create_dir(&blocked).unwrap();
    assert!(
        registry
            .commit(&prepared.token, false, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(registry.list("owner", "a")[0].revision, old.revision);
    assert_eq!(
        body(
            &registry
                .open("owner", "a", &revision(&old), &AtomicBool::new(false))
                .unwrap()
        ),
        b"old"
    );
    fs::remove_dir(blocked).unwrap();
    assert_eq!(
        registry
            .commit(&prepared.token, false, &AtomicBool::new(false))
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn catalog_checksum_rejects_valid_json_corruption_without_resurrecting_an_old_catalog() {
    let root = tempfile::tempdir().unwrap();
    let registry = SavedDemoRegistry::load(Some(root.path().into())).unwrap();
    active(&registry, "owner", "a");
    save(&registry, "owner", "a", "Original", b"red");
    let generation = registry.lock().catalog.generation;
    drop(registry);
    let path = root.path().join(format!("catalog-{generation:020}.json"));
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let id = document["catalog"]["demos"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    document["catalog"]["demos"][&id]["metadata"]["name"] = json!("Tampered");
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(SavedDemoRegistry::load(Some(root.path().into())).is_err());
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // End-to-end restart qualification keeps its state transitions together.
async fn application_review_commits_linked_snapshot_after_edit_and_restarts_without_traffic() {
    use crate::{
        AppConfig, Application, BodyStoreConfig, ImportResponseAsset, ResponseBodyFileMode,
        ResponseSetChange, SessionQueryInput, TraceImportRequest,
    };
    use transmog_automation::{RequestActions, ResponseActions, Rule, RuleMatcher, UrlCondition};
    let root = tempfile::tempdir().unwrap();
    let config = || AppConfig {
        automation_path: Some(root.path().join("automation")),
        response_asset_root: Some(root.path().join("assets")),
        saved_demo_root: Some(root.path().join("saved-demos-v1")),
        body_store: Some(BodyStoreConfig::product_default(root.path().join("cache"))),
        ..AppConfig::default()
    };
    let app = Application::new(config()).unwrap();
    let path = root.path().join("site.saz");
    capture_fixture(&path);
    app.import_trace(
        TraceImportRequest {
            path: path.clone(),
            password: None,
            operation_id: "fixture".into(),
            max_file_bytes: 1024 * 1024,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    let external = root.path().join("linked.css");
    fs::write(&external, b"body{color:red}").unwrap();
    let asset = app
        .import_response_asset(ImportResponseAsset {
            id: "linked".into(),
            revision: 1,
            status: 200,
            headers: transmog_core::HeaderBlock::new(),
            body_path: external.clone(),
            body_file_mode: ResponseBodyFileMode::Linked,
            media_type: Some("text/css".into()),
        })
        .unwrap();
    let rule = Rule {
        id: "style".into(),
        display_name: Some("Linked style".into()),
        enabled: true,
        revision: 1,
        priority: 0,
        matcher: RuleMatcher {
            url: Some(UrlCondition::Exact("https://demo.invalid/theme.css".into())),
            ..Default::default()
        },
        request: RequestActions {
            response_asset: Some(asset.asset_ref()),
            ..Default::default()
        },
        response: ResponseActions::default(),
    };
    let status = app
        .change_response_set(
            0,
            ResponseSetChange::Create {
                id: "owner".into(),
                name: "Task".into(),
                notes: String::new(),
                rules: vec![rule],
            },
        )
        .unwrap();
    let owner = &status.response_sets[0];
    let entry_id = app
        .query_sessions(SessionQueryInput::default())
        .unwrap()
        .sessions
        .into_iter()
        .find(|row| row.path == "/")
        .unwrap()
        .id;
    let prepared = app
        .prepare_demo_save(
            SaveDemoInput {
                owner_id: owner.id.clone(),
                name: "Frozen red".into(),
                entry_id,
                options: CapturedPageOptions {
                    variant: Some(owner.current_selection()),
                    ..Default::default()
                },
                replace: None,
            },
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    fs::write(&external, b"body{color:blue}").unwrap();
    app.change_response_set(
        status.generation,
        ResponseSetChange::SaveCurrent {
            id: owner.id.clone(),
            expected_revision: owner.revision,
            name: owner.name.clone(),
            notes: "Changed after review".into(),
            rules: Vec::new(),
        },
    )
    .unwrap();
    let saved = app
        .commit_demo_save(prepared.token, true, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();
    drop(app);
    fs::remove_file(external).unwrap();
    fs::remove_file(path).unwrap();
    let restarted = Application::new(config()).unwrap();
    assert_eq!(
        restarted
            .query_sessions(SessionQueryInput::default())
            .unwrap()
            .sessions,
        [] as [crate::SessionSummary; 0]
    );
    let page = restarted
        .prepare_saved_demo(
            "owner".into(),
            revision(&saved),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    assert_eq!(body(&page), b"body{color:red}");
    assert_eq!(
        page.diagnostics()
            .snapshot()
            .saved_demo
            .as_ref()
            .unwrap()
            .name,
        "Frozen red"
    );
    let status = restarted.automation_status();
    restarted
        .change_response_set(
            status.generation,
            ResponseSetChange::Delete { id: "owner".into() },
        )
        .unwrap();
    assert!(restarted.saved_demos("owner").is_err());
    assert_eq!(body(&page), b"body{color:red}");
}

fn capture_fixture(path: &Path) {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (id, url, mime, body) in [
        (
            1,
            "https://demo.invalid/",
            "text/html",
            "<html><link rel=stylesheet href=/theme.css></html>",
        ),
        (
            2,
            "https://demo.invalid/theme.css",
            "text/css",
            "body{color:black}",
        ),
    ] {
        for (name, text) in [
            (
                format!("raw/{id}_c.txt"),
                format!("GET {url} HTTP/1.1\r\nHost: demo.invalid\r\n\r\n"),
            ),
            (
                format!("raw/{id}_s.txt"),
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                ),
            ),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
    }
    fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
}

fn persistent_app_config(root: &Path) -> crate::AppConfig {
    crate::AppConfig {
        automation_path: Some(root.join("automation")),
        saved_demo_root: Some(root.join("demos")),
        body_store: Some(crate::BodyStoreConfig::product_default(root.join("cache"))),
        ..crate::AppConfig::default()
    }
}

fn create_demo_owner(app: &crate::Application, id: &str) -> crate::ResponseSet {
    let status = app
        .change_response_set(
            app.automation_status().generation,
            crate::ResponseSetChange::Create {
                id: id.into(),
                name: id.into(),
                notes: String::new(),
                rules: Vec::new(),
            },
        )
        .unwrap();
    let mut owner = status
        .response_sets
        .iter()
        .find(|set| set.id == id)
        .unwrap()
        .clone();
    for _ in 0..2 {
        let status = app
            .change_response_set(
                app.automation_status().generation,
                crate::ResponseSetChange::SaveCurrent {
                    id: id.into(),
                    expected_revision: owner.revision,
                    name: owner.name.clone(),
                    notes: owner.notes.clone(),
                    rules: owner.rules.clone(),
                },
            )
            .unwrap();
        owner = status
            .response_sets
            .iter()
            .find(|set| set.id == id)
            .unwrap()
            .clone();
    }
    owner
}

#[tokio::test]
async fn damaged_scene_restart_keeps_traffic_and_healthy_demos_usable_and_deletes_safely() {
    for missing in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let app = crate::Application::new(persistent_app_config(root.path())).unwrap();
        let (bad, good) = shared_owner_demos(&app);
        let manifest = app.saved_demos.lock().catalog.demos[&bad.id]
            .manifest
            .clone();
        let path = root
            .path()
            .join("demos")
            .join(format!("scene-{manifest}.json"));
        if missing {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, b"damaged JSON").unwrap();
        }
        drop(app);
        let restarted = crate::Application::new(persistent_app_config(root.path())).unwrap();
        assert!(
            restarted.saved_demos("damaged").unwrap()[0]
                .availability_error
                .is_some()
        );
        assert!(
            restarted.saved_demos("healthy").unwrap()[0]
                .availability_error
                .is_none()
        );
        assert!(
            restarted
                .prepare_saved_demo(
                    "damaged".into(),
                    revision(&bad),
                    Arc::new(AtomicBool::new(false))
                )
                .await
                .is_err()
        );
        let healthy = restarted
            .prepare_saved_demo(
                "healthy".into(),
                revision(&good),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
        assert_eq!(body(&healthy), b"shared-css");
        let capture = root.path().join("usable-traffic.saz");
        capture_fixture(&capture);
        restarted
            .import_trace(
                crate::TraceImportRequest {
                    path: capture.clone(),
                    password: None,
                    operation_id: "after-corruption".into(),
                    max_file_bytes: 1024 * 1024,
                },
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(
            restarted
                .query_sessions(crate::SessionQueryInput::default())
                .unwrap()
                .sessions
                .len(),
            2
        );
        create_demo_owner(&restarted, "new-work");
        restarted
            .delete_saved_demo("damaged", &revision(&bad))
            .unwrap();
        assert_eq!(
            bodies(&root.path().join("demos")),
            2,
            "shared referenced bodies were deleted"
        );
        assert_eq!(body(&healthy), b"shared-css");
        restarted
            .delete_saved_demo("healthy", &revision(&good))
            .unwrap();
        assert_eq!(bodies(&root.path().join("demos")), 0);
        assert!(capture.exists());
    }
}

fn shared_owner_demos(app: &crate::Application) -> (SavedDemo, SavedDemo) {
    let make = |id: &str, name: &str| {
        let owner = create_demo_owner(app, id);
        app.saved_demos(id).unwrap();
        save(
            &app.saved_demos,
            id,
            &owner.incarnation,
            name,
            b"shared-css",
        )
    };
    (make("damaged", "Damaged"), make("healthy", "Healthy"))
}

#[test]
fn damaged_catalog_startup_preserves_data_and_owner_delete_and_strict_repair_work() {
    let root = tempfile::tempdir().unwrap();
    let app = crate::Application::new(persistent_app_config(root.path())).unwrap();
    let owner = create_demo_owner(&app, "owner");
    app.saved_demos("owner").unwrap();
    save(
        &app.saved_demos,
        "owner",
        &owner.incarnation,
        "Older",
        b"old",
    );
    let older_generation = app.saved_demos.lock().catalog.generation;
    let older_path = root
        .path()
        .join("demos")
        .join(format!("catalog-{older_generation:020}.json"));
    let older_bytes = fs::read(&older_path).unwrap();
    save(
        &app.saved_demos,
        "owner",
        &owner.incarnation,
        "Latest",
        b"latest",
    );
    let generation = app.saved_demos.lock().catalog.generation;
    let current_path = root
        .path()
        .join("demos")
        .join(format!("catalog-{generation:020}.json"));
    let current_bytes = fs::read(&current_path).unwrap();
    fs::write(&current_path, b"damaged catalog").unwrap();
    drop(app);
    let restarted = crate::Application::new(persistent_app_config(root.path())).unwrap();
    assert!(
        restarted
            .saved_demos("owner")
            .err()
            .unwrap()
            .message
            .contains("preserved")
    );
    assert_eq!(fs::read(&current_path).unwrap(), b"damaged catalog");
    assert_eq!(
        restarted
            .query_sessions(crate::SessionQueryInput::default())
            .unwrap()
            .sessions,
        [] as [crate::SessionSummary; 0]
    );
    create_demo_owner(&restarted, "unrelated-work");
    fs::write(&older_path, &older_bytes).unwrap();
    fs::remove_file(&current_path).unwrap();
    assert!(
        restarted.saved_demos("owner").is_err(),
        "damaged newest catalog silently fell back to an older generation"
    );
    fs::write(&current_path, &current_bytes).unwrap();
    assert_eq!(
        restarted.saved_demos("owner").unwrap().len(),
        2,
        "strict catalog repair did not recover existing metadata"
    );
    fs::write(&current_path, b"damaged again").unwrap();
    drop(restarted);
    let restarted = crate::Application::new(persistent_app_config(root.path())).unwrap();
    restarted
        .change_response_set(
            restarted.automation_status().generation,
            crate::ResponseSetChange::Delete { id: "owner".into() },
        )
        .unwrap();
    assert!(
        !restarted
            .automation_status()
            .response_sets
            .iter()
            .any(|set| set.id == "owner")
    );
    assert_eq!(fs::read(&current_path).unwrap(), b"damaged again");
    assert!(
        bodies(&root.path().join("demos")) > 0,
        "unknown catalog references were guessed during owner cleanup"
    );
}

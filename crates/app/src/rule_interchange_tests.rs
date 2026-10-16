use crate::{
    AppConfig, Application, AuthoredResponseAsset, AutomationRuleSet, ErrorCategory,
    ExperimentChange, ExperimentSelection, ImportResponseAsset, ResponseBodyFileMode,
    RuleExportRequest,
};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use transmog_automation::{
    AutomationResponse, HeaderOperation, RequestActions, ResponseActions, ResponseAssetResolver,
    Rule, RuleMatcher,
};
use transmog_core::{BodyFrame, HeaderBlock};

fn config(root: &Path) -> AppConfig {
    AppConfig {
        automation_path: Some(root.join("automation")),
        response_asset_root: Some(root.join("assets")),
        ..Default::default()
    }
}
fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../verification/fixtures/portable")
}
fn source() -> transmog_portable::Collection {
    transmog_portable::Collection::parse(include_bytes!(
        "../../../verification/fixtures/portable/manifests/rules-only.json"
    ))
    .unwrap()
}
fn prepare(
    app: &Application,
    value: &transmog_portable::Collection,
    stage: &Path,
) -> crate::PreparedRuleImport {
    app.prepare_rule_import(
        &value.encode().unwrap(),
        stage,
        |sha| {
            Ok(Box::new(File::open(
                corpus().join("objects/sha256").join(sha),
            )?))
        },
        &AtomicBool::new(false),
    )
    .unwrap()
}
fn ordinary() -> Rule {
    Rule {
        id: "existing-header".into(),
        revision: 1,
        display_name: Some("Existing".into()),
        enabled: true,
        priority: 100,
        matcher: RuleMatcher {
            host: Some("other.invalid".into()),
            ..Default::default()
        },
        request: RequestActions {
            headers: vec![HeaderOperation::set("X-Existing", "preserved").unwrap()],
            ..Default::default()
        },
        response: ResponseActions::default(),
    }
}
fn install_existing(app: &Application) {
    let candidate = app
        .validate_automation(AutomationRuleSet {
            rules: vec![ordinary()],
            autoresponses_enabled: false,
            ..Default::default()
        })
        .unwrap();
    app.activate_automation(&candidate.candidate_id).unwrap();
}
fn registrations(app: &Application) -> Vec<String> {
    app.automation
        .active
        .read()
        .unwrap()
        .compiled
        .registrations()
        .iter()
        .map(|r| format!("{r:?}"))
        .collect()
}
fn bytes(app: &Application, reference: &str) -> Vec<u8> {
    let AutomationResponse::Buffered(response) =
        ResponseAssetResolver::resolve(&app.response_assets, reference, &AtomicBool::new(false))
            .unwrap()
    else {
        panic!("Small fixture must be buffered")
    };
    response
        .body
        .into_iter()
        .filter_map(|frame| {
            if let BodyFrame::Data(bytes) = frame {
                Some(bytes)
            } else {
                None
            }
        })
        .flatten()
        .collect()
}

#[test]
fn reviewed_import_is_disabled_atomic_and_survives_restart_with_exact_asset_bytes() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    install_existing(&app);
    let before = app.automation_status();
    let hooks = registrations(&app);
    let candidate = app
        .validate_automation(AutomationRuleSet {
            generation: before.generation,
            rules: before.rules.clone(),
            autoresponses_enabled: false,
            ..Default::default()
        })
        .unwrap();
    let import = prepare(&app, &source(), stage.path());
    assert_eq!(import.review().activation, "disabled");
    assert!(import.review().source.experiments[0].variants[1].revisions[0].rules[0].enabled_intent);
    assert_eq!(app.automation_status().generation, before.generation);
    assert!(app.response_assets().is_empty());
    let status = app
        .import_rule_collection(import, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(status.rules, before.rules);
    assert!(!status.autoresponses_enabled);
    assert_eq!(status.candidate_count, 1);
    assert!(!candidate.candidate_id.is_empty());
    assert_eq!(registrations(&app), hooks);
    assert_eq!(status.experiments.len(), 1);
    assert!(status.experiments[0].paused);
    assert!(status.experiments[0].active.is_none());
    let reference = app.response_assets()[0].asset_ref();
    assert_eq!(bytes(&app, &reference), b"h1 { color: blue; }\n");
    assert_eq!(
        app.response_assets()[0]
            .headers
            .values("x-duplicate")
            .collect::<Vec<_>>(),
        [b"one".as_slice(), b"two".as_slice()]
    );
    assert_eq!(
        app.response_assets()[0]
            .headers
            .values("x-bytes")
            .next()
            .unwrap(),
        [0x80, 0xff]
    );
    drop(app);
    let reopened = Application::new(config(root.path())).unwrap();
    assert_eq!(reopened.automation_status().rules, before.rules);
    assert!(reopened.automation_status().experiments[0].active.is_none());
    assert_eq!(bytes(&reopened, &reference), b"h1 { color: blue; }\n");
    assert_eq!(registrations(&reopened), hooks);
}

#[test]
fn identical_and_conflicting_source_definitions_get_reviewed_fresh_mappings() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    app.import_rule_collection(
        prepare(&app, &source(), stage.path()),
        &AtomicBool::new(false),
    )
    .unwrap();
    let identical = prepare(&app, &source(), stage.path());
    assert!(
        identical
            .review()
            .items
            .iter()
            .all(|i| i.state == "identical-source-definition")
    );
    let old = app.response_assets()[0].asset_ref();
    app.import_rule_collection(identical, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(app.response_assets().len(), 2);
    let mut changed = source();
    changed.assets[0].headers.push(transmog_portable::Header {
        name: "X-Changed".into(),
        value_base64: "eWVz".into(),
    });
    let review = prepare(&app, &changed, stage.path());
    assert_eq!(
        review
            .review()
            .items
            .iter()
            .find(|i| i.kind == "asset")
            .unwrap()
            .state,
        "conflicting"
    );
    app.import_rule_collection(review, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(app.response_assets().len(), 3);
    assert_eq!(bytes(&app, &old), b"h1 { color: blue; }\n");
    assert!(
        app.automation_status()
            .experiments
            .iter()
            .all(|e| e.active.is_none())
    );
}

#[test]
fn stale_canceled_and_failed_persistence_imports_preserve_live_and_saved_state() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    install_existing(&app);
    let stale_review = prepare(&app, &source(), stage.path());
    app.set_autoresponses_enabled(true, app.automation_status().generation)
        .unwrap();
    assert_eq!(
        app.import_rule_collection(stale_review, &AtomicBool::new(false))
            .unwrap_err()
            .category,
        ErrorCategory::Conflict
    );
    let before = app.automation_status();
    let hooks = registrations(&app);
    let canceled = prepare(&app, &source(), stage.path());
    assert_eq!(
        app.import_rule_collection(canceled, &AtomicBool::new(true))
            .unwrap_err()
            .category,
        ErrorCategory::Canceled
    );
    let target = root
        .path()
        .join(format!("automation.{}.json", (before.generation + 1) % 2));
    if target.exists() {
        std::fs::remove_file(&target).unwrap();
    }
    std::fs::create_dir(&target).unwrap();
    let import = prepare(&app, &source(), stage.path());
    assert!(
        app.import_rule_collection(import, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(app.automation_status().generation, before.generation);
    assert_eq!(app.automation_status().rules, before.rules);
    assert!(app.response_assets().is_empty());
    assert_eq!(registrations(&app), hooks);
    drop(app);
    let reopened = Application::new(config(root.path())).unwrap();
    assert_eq!(reopened.automation_status().rules, before.rules);
    assert!(reopened.response_assets().is_empty());
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
}

#[test]
fn source_changes_after_review_do_not_change_imported_bytes_and_dot_ids_remap() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    let mut value = source();
    value.experiments[0].variants[1].id = "with.dot".into();
    let object = &value.objects[0];
    let path = root.path().join("source.body");
    std::fs::copy(corpus().join("objects/sha256").join(&object.sha256), &path).unwrap();
    let review = app
        .prepare_rule_import(
            &value.encode().unwrap(),
            stage.path(),
            |_| Ok(Box::new(File::open(&path)?)),
            &AtomicBool::new(false),
        )
        .unwrap();
    std::fs::write(path, b"changed after review").unwrap();
    let status = app
        .import_rule_collection(review, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        bytes(&app, &app.response_assets()[0].asset_ref()),
        b"h1 { color: blue; }\n"
    );
    assert_ne!(status.experiments[0].variants[1].id, "with.dot");
}

#[test]
fn import_flat_header_and_literal_rules_keeps_authored_intent_but_never_installs_hooks() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    let mut value = source();
    value.assets.clear();
    value.objects.clear();
    value.experiments.clear();
    let header = ordinary();
    value
        .rules
        .push(transmog_portable::from_native(&header).unwrap());
    let mut literal = header;
    literal.id = "fresh-text".into();
    literal.request.headers.clear();
    literal.priority += 1;
    literal.response.text_transform = Some(transmog_automation::LiteralTransform {
        find: "seed".into(),
        replacement: "edited".into(),
        occurrences: transmog_automation::Occurrences::First,
        expected_matches: None,
        media_type: "text/plain".into(),
        encoding: "utf-8".into(),
        max_bytes: 1024,
    });
    value
        .rules
        .push(transmog_portable::from_native(&literal).unwrap());
    let review = prepare(&app, &value, stage.path());
    assert!(
        review
            .review()
            .source
            .rules
            .iter()
            .all(|r| r.enabled_intent)
    );
    let status = app
        .import_rule_collection(review, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(status.rules.len(), 2);
    assert!(status.rules.iter().all(|r| !r.enabled));
    assert!(registrations(&app).is_empty());
    let document = app.automation.active.read().unwrap();
    assert!(
        document.document.portable_imports[0]
            .source
            .rules
            .iter()
            .all(|r| r.enabled_intent)
    );
}

#[test]
fn selected_export_is_passive_preserves_bytes_and_freezes_linked_files_without_paths() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    let path = root.path().join("author-private-name.css");
    std::fs::write(&path, b"h1 { color: green; }\n").unwrap();
    let asset = app
        .import_response_asset(ImportResponseAsset {
            id: "linked".into(),
            revision: 1,
            status: 200,
            headers: HeaderBlock::new(),
            body_path: path.clone(),
            body_file_mode: ResponseBodyFileMode::Linked,
            media_type: Some("text/css".into()),
        })
        .unwrap();
    let mut rule = ordinary();
    rule.id = "linked-rule".into();
    rule.request.headers.clear();
    rule.request.response_asset = Some(asset.asset_ref());
    app.change_experiment(
        0,
        ExperimentChange::Create {
            id: "named".into(),
            name: "Named".into(),
            variant_id: "edited".into(),
            variant_name: "Edited".into(),
            rules: vec![rule],
        },
    )
    .unwrap();
    let before = app.automation_status();
    let export = app
        .export_rule_collection(
            &RuleExportRequest {
                id: "export".into(),
                revision: "1".into(),
                title: "Selected".into(),
                rule_ids: vec![],
                selections: vec![ExperimentSelection {
                    experiment_id: "named".into(),
                    variant_id: "edited".into(),
                    revision: 1,
                }],
            },
            stage.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
    std::fs::write(&path, b"changed later").unwrap();
    let metadata = export.manifest();
    let mut body = Vec::new();
    export
        .open_object(&metadata.objects[0].sha256)
        .unwrap()
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, b"h1 { color: green; }\n");
    assert_eq!(metadata.assets[0].provenance.kind, "export-snapshot");
    assert!(
        !String::from_utf8(metadata.encode().unwrap())
            .unwrap()
            .contains("author-private-name")
    );
    assert_eq!(app.automation_status().generation, before.generation);
    assert!(registrations(&app).is_empty());
    std::fs::remove_file(path).unwrap();
    assert!(
        app.export_rule_collection(
            &RuleExportRequest {
                id: "export".into(),
                revision: "1".into(),
                title: "Missing".into(),
                rule_ids: vec![],
                selections: vec![ExperimentSelection {
                    experiment_id: "named".into(),
                    variant_id: "edited".into(),
                    revision: 1
                }]
            },
            stage.path(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

#[test]
fn unrelated_asset_writes_do_not_duplicate_import_catalog_and_export_omits_other_origins() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    app.import_rule_collection(
        prepare(&app, &source(), stage.path()),
        &AtomicBool::new(false),
    )
    .unwrap();
    app.create_response_asset(AuthoredResponseAsset {
        id: "other".into(),
        revision: 1,
        status: 200,
        headers: HeaderBlock::new(),
        body: b"other".to_vec(),
        media_type: None,
    })
    .unwrap();
    let index = std::fs::read_to_string(root.path().join("assets/index.1.json")).unwrap();
    assert!(!index.contains("import-asset-"));
    let request = RuleExportRequest {
        id: "empty".into(),
        revision: "1".into(),
        title: "No selection".into(),
        rule_ids: vec![],
        selections: vec![],
    };
    let export = app
        .export_rule_collection(&request, stage.path(), &AtomicBool::new(false))
        .unwrap();
    assert!(export.manifest().extensions.is_empty());
    assert!(export.manifest().objects.is_empty());
    drop(app);
    let reopened = Application::new(config(root.path())).unwrap();
    assert_eq!(reopened.response_assets().len(), 2);
}

#[test]
fn authored_active_archived_and_large_revision_intent_is_retained_without_activation() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    let mut value = source();
    let experiment = &mut value.experiments[0];
    experiment.paused_intent = false;
    experiment.archived_intent = true;
    experiment.variants[1].archived_intent = true;
    experiment.variants[1].revisions[0].revision = "9007199254740993".into();
    experiment.active_intent = Some(transmog_portable::ActiveSelection {
        variant_id: "edited".into(),
        revision: "9007199254740993".into(),
    });
    let review = prepare(&app, &value, stage.path());
    app.import_rule_collection(review, &AtomicBool::new(false))
        .unwrap();
    let active_document = app.automation.active.read().unwrap();
    let source = &active_document.document.portable_imports[0].source;
    assert_eq!(source, &value);
    let imported = &active_document.document.experiments[0];
    assert!(imported.paused);
    assert!(imported.active.is_none());
    assert_eq!(imported.variants[1].revisions[0].revision, 1);
    assert_eq!(
        imported.variants[1].revisions[0].rules[0].revision,
        9_007_199_254_740_993
    );
    assert!(registrations(&app).is_empty());
}

#[test]
fn large_linked_export_streams_beyond_the_text_editor_limit() {
    use std::io::{Seek, SeekFrom};
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    let path = root.path().join("large.body");
    let length = 17 * 1024 * 1024;
    File::create(&path).unwrap().set_len(length).unwrap();
    let asset = app
        .import_response_asset(ImportResponseAsset {
            id: "large".into(),
            revision: 1,
            status: 200,
            headers: HeaderBlock::new(),
            body_path: path,
            body_file_mode: ResponseBodyFileMode::Linked,
            media_type: None,
        })
        .unwrap();
    let mut rule = ordinary();
    rule.id = "large-rule".into();
    rule.request.headers.clear();
    rule.request.response_asset = Some(asset.asset_ref());
    let candidate = app
        .validate_automation(AutomationRuleSet {
            rules: vec![rule],
            ..Default::default()
        })
        .unwrap();
    app.activate_automation(&candidate.candidate_id).unwrap();
    let export = app
        .export_rule_collection(
            &RuleExportRequest {
                id: "large-export".into(),
                revision: "1".into(),
                title: "Large".into(),
                rule_ids: vec!["large-rule".into()],
                selections: vec![],
            },
            stage.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(export.manifest().objects[0].bytes, length.to_string());
    let mut file = export
        .open_object(&export.manifest().objects[0].sha256)
        .unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    let mut last = [1];
    file.read_exact(&mut last).unwrap();
    assert_eq!(last, [0]);
}

#[test]
fn unpublished_asset_metadata_is_hidden_until_the_registry_generation_changes() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    app.import_rule_collection(
        prepare(&app, &source(), stage.path()),
        &AtomicBool::new(false),
    )
    .unwrap();
    let original = app.response_assets();
    let mut next = original.clone();
    let mut staged = original[0].clone();
    staged.id = "not-published".into();
    next.push(staged);
    app.response_assets.publish_imports(&next);
    assert_eq!(app.response_assets(), original);
    app.response_assets.publish_imports(&original);
    let resolver = app.response_assets.clone();
    let reference = original[0].asset_ref();
    drop(app);
    assert!(ResponseAssetResolver::resolve(&resolver, &reference, &AtomicBool::new(false)).is_ok());
}

#[test]
fn review_is_bound_to_its_workspace_even_when_generations_match() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let source_app = Application::new(config(first.path())).unwrap();
    let target = Application::new(config(second.path())).unwrap();
    let review = prepare(&source_app, &source(), stage.path());
    assert!(
        target
            .import_rule_collection(review, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(target.automation_status().generation, 0);
    assert!(target.response_assets().is_empty());
}

#[test]
fn unsupported_and_canceled_empty_imports_have_distinct_categories_and_no_staging() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    let unsupported = br#"{"format":"transmog-investigation","schemaVersion":99,"kind":"rules"}"#;
    assert_eq!(
        app.prepare_rule_import(
            unsupported,
            stage.path(),
            |_| unreachable!(),
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .category,
        ErrorCategory::Unsupported
    );
    let mut empty = source();
    empty.assets.clear();
    empty.objects.clear();
    empty.experiments.clear();
    assert_eq!(
        app.prepare_rule_import(
            &empty.encode().unwrap(),
            stage.path(),
            |_| unreachable!(),
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .category,
        ErrorCategory::Canceled
    );
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
    assert_eq!(app.automation_status().generation, 0);
}

#[test]
fn in_memory_workspaces_enforce_the_same_import_capacity_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let mut options = config(root.path());
    options.automation_path = None;
    let app = Application::new(options).unwrap();
    let mut value = source();
    value
        .extensions
        .insert("test.large".into(), "x".repeat(4 * 1024 * 1024).into());
    let review = prepare(&app, &value, stage.path());
    assert_eq!(
        app.import_rule_collection(review, &AtomicBool::new(false))
            .unwrap_err()
            .category,
        ErrorCategory::Limit
    );
    assert_eq!(app.automation_status().generation, 0);
    assert!(app.response_assets().is_empty());
    assert!(app.automation_status().experiments.is_empty());
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
}

#[test]
fn corrupted_import_catalog_falls_back_as_a_whole_to_the_previous_commit() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    install_existing(&app);
    app.import_rule_collection(
        prepare(&app, &source(), stage.path()),
        &AtomicBool::new(false),
    )
    .unwrap();
    let generation = app.automation_status().generation;
    let latest = root
        .path()
        .join(format!("automation.{}.json", generation % 2));
    drop(app);
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&latest).unwrap()).unwrap();
    stored["portableImports"][0]["assets"][0]["status"] = 503.into();
    std::fs::write(latest, serde_json::to_vec(&stored).unwrap()).unwrap();
    let reopened = Application::new(config(root.path())).unwrap();
    assert_eq!(reopened.automation_status().generation, generation - 1);
    assert_eq!(reopened.automation_status().rules, vec![ordinary()]);
    assert!(reopened.response_assets().is_empty());
    assert!(reopened.automation_status().experiments.is_empty());
}

#[cfg(windows)]
#[test]
fn blocked_atomic_publication_preserves_both_existing_recovery_slots() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    install_existing(&app);
    app.set_autoresponses_enabled(true, app.automation_status().generation)
        .unwrap();
    let generation = app.automation_status().generation;
    let destination = root
        .path()
        .join(format!("automation.{}.json", (generation + 1) % 2));
    let original = std::fs::read(&destination).unwrap();
    // Allow reads/writes but deny delete/replacement: in-place truncation would still succeed.
    let held = File::options()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&destination)
        .unwrap();
    let review = prepare(&app, &source(), stage.path());
    assert_eq!(
        app.import_rule_collection(review, &AtomicBool::new(false))
            .unwrap_err()
            .category,
        ErrorCategory::Unavailable
    );
    assert_eq!(std::fs::read(&destination).unwrap(), original);
    assert_eq!(app.automation_status().generation, generation);
    assert!(app.response_assets().is_empty());
    drop(held);
    drop(app);
    let reopened = Application::new(config(root.path())).unwrap();
    assert_eq!(reopened.automation_status().generation, generation);
    assert_eq!(reopened.automation_status().rules, vec![ordinary()]);
    assert!(reopened.response_assets().is_empty());
    assert!(std::fs::read_dir(root.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".automation-")
    }));
}

#[test]
fn imported_selected_revision_reexports_a_verified_closure_with_scoped_origins() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    app.import_rule_collection(
        prepare(&app, &source(), stage.path()),
        &AtomicBool::new(false),
    )
    .unwrap();
    let status = app.automation_status();
    let experiment = &status.experiments[0];
    let variant = experiment
        .variants
        .iter()
        .find(|v| v.id != "baseline")
        .unwrap();
    let export = app
        .export_rule_collection(
            &RuleExportRequest {
                id: "reexport".into(),
                revision: "1".into(),
                title: "Selected imported revision".into(),
                rule_ids: vec![],
                selections: vec![ExperimentSelection {
                    experiment_id: experiment.id.clone(),
                    variant_id: variant.id.clone(),
                    revision: 1,
                }],
            },
            stage.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(export.manifest().assets.len(), 1);
    let origins = &export.manifest().extensions["transmog.rule-origins"];
    assert_eq!(origins.as_object().unwrap().len(), 1);
    assert_eq!(
        origins.as_object().unwrap().values().next().unwrap()["originalRuleId"],
        source().experiments[0].variants[1].revisions[0].rules[0].id
    );
    let mut body = Vec::new();
    export
        .open_object(&export.manifest().objects[0].sha256)
        .unwrap()
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, b"h1 { color: blue; }\n");
    assert!(export.manifest().encode().is_ok());
}

#[tokio::test]
async fn linked_compressed_export_retains_encoded_entity_and_matching_content_coding() {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    let app = Application::new(config(root.path())).unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../verification/fixtures/portable/manifests/coded-duplicate-headers.json"
    ))
    .unwrap();
    let original = std::fs::read(
        corpus()
            .join("objects/sha256")
            .join(metadata["assets"][1]["body"]["sha256"].as_str().unwrap()),
    )
    .unwrap();
    let headers = HeaderBlock::from_fields(vec![
        transmog_core::HeaderField::try_new("Content-Encoding", "gzip").unwrap(),
    ]);
    let source = app
        .create_response_asset(AuthoredResponseAsset {
            id: "gzip-source".into(),
            revision: 1,
            status: 200,
            headers,
            body: original,
            media_type: Some("text/javascript".into()),
        })
        .unwrap();
    let path = root.path().join("edited.js");
    std::fs::write(&path, b"globalThis.edited = true;\n").unwrap();
    let asset = app
        .edit_response_asset(crate::ResponseAssetEdit {
            asset_reference: source.asset_ref(),
            status: 200,
            headers: source.headers,
            media_type: Some("text/javascript".into()),
            decoded_body: None,
            body_path: Some(path),
            body_file_mode: ResponseBodyFileMode::Linked,
            preserve_content_encoding: true,
        })
        .await
        .unwrap();
    let mut rule = ordinary();
    rule.id = "gzip-rule".into();
    rule.request.headers.clear();
    rule.request.response_asset = Some(asset.asset_ref());
    let candidate = app
        .validate_automation(AutomationRuleSet {
            rules: vec![rule],
            ..Default::default()
        })
        .unwrap();
    app.activate_automation(&candidate.candidate_id).unwrap();
    let export = app
        .export_rule_collection(
            &RuleExportRequest {
                id: "gzip-export".into(),
                revision: "1".into(),
                title: "Gzip".into(),
                rule_ids: vec!["gzip-rule".into()],
                selections: vec![],
            },
            stage.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
    let mut encoded = Vec::new();
    export
        .open_object(&export.manifest().objects[0].sha256)
        .unwrap()
        .read_to_end(&mut encoded)
        .unwrap();
    assert_eq!(encoded[..2], [0x1f, 0x8b]);
    assert!(
        export.manifest().assets[0]
            .headers
            .iter()
            .any(
                |h| h.name.eq_ignore_ascii_case("content-encoding") && h.value_base64 == "Z3ppcA=="
            )
    );
    assert_eq!(
        crate::inspector::decode_content(&["gzip".into()], encoded)
            .await
            .unwrap(),
        b"globalThis.edited = true;\n"
    );
}

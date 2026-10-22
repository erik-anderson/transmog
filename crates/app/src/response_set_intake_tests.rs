use super::*;
use crate::{AppConfig, AuthoredResponseAsset, BodyStoreConfig, ResponseSetChange};
use std::sync::Arc;
use transmog_core::HeaderBlock;

fn configured(root: &std::path::Path) -> Application {
    Application::new(AppConfig {
        automation_path: Some(root.join("automation")),
        response_asset_root: Some(root.join("assets")),
        body_store: Some(BodyStoreConfig::product_default(root.join("bodies"))),
        ..AppConfig::default()
    })
    .unwrap()
}

fn rule(app: &Application, id: &str, priority: i32, enabled: bool) -> Rule {
    let asset = app
        .create_response_asset(AuthoredResponseAsset {
            id: id.into(),
            revision: 1,
            status: 201,
            headers: HeaderBlock::new(),
            body: vec![0, 255, 13, 10, 65],
            media_type: Some("application/octet-stream".into()),
        })
        .unwrap();
    Rule {
        id: id.into(),
        display_name: Some(format!("Original {id}")),
        enabled,
        revision: 7,
        priority,
        matcher: RuleMatcher {
            method: Some("POST".into()),
            url: Some(UrlCondition::Exact(format!("https://intake.invalid/{id}"))),
            request_headers: vec![transmog_automation::HeaderPredicate {
                name: "X-Original".into(),
                condition: transmog_automation::HeaderCondition::Exact(b"keep".to_vec()),
            }],
            ..RuleMatcher::default()
        },
        request: RequestActions {
            response_asset: Some(asset.asset_ref()),
            ..RequestActions::default()
        },
        response: ResponseActions::default(),
    }
}

fn install(app: &Application, rules: Vec<Rule>) {
    let candidate = app
        .validate_automation(AutomationRuleSet {
            generation: app.automation_status().generation,
            rules,
            ..AutomationRuleSet::default()
        })
        .unwrap();
    app.activate_automation(&candidate.candidate_id).unwrap();
}

fn new_input(
    app: &Application,
    ids: &[&str],
    mode: ResponseSetIntakeMode,
) -> ResponseSetIntakeInput {
    ResponseSetIntakeInput {
        generation: app.automation_status().generation,
        source: ResponseSetIntakeSource::Rules {
            ids: ids.iter().map(|id| (*id).into()).collect(),
        },
        destination: ResponseSetIntakeDestination::New {
            id: "destination".into(),
            name: "Debugging task".into(),
            notes: "Keep the task context".into(),
        },
        mode,
        acknowledge_overlaps: false,
    }
}

fn existing_input(
    app: &Application,
    ids: &[&str],
    mode: ResponseSetIntakeMode,
) -> ResponseSetIntakeInput {
    let mut input = new_input(app, ids, mode);
    let set = app.automation_status().response_sets.remove(0);
    input.destination = ResponseSetIntakeDestination::Existing {
        id: set.id,
        incarnation: set.incarnation,
        revision: set.revision,
    };
    input
}

fn assert_definition_preserved(actual: &Rule, original: &Rule) {
    let mut comparable = actual.clone();
    assert_ne!(
        actual.id, original.id,
        "A new owner must receive a new identity"
    );
    comparable.id.clone_from(&original.id);
    comparable.priority = original.priority;
    comparable.revision = original.revision;
    assert_eq!(&comparable, original);
}

#[tokio::test]
async fn copying_preserves_full_definitions_bytes_inclusion_and_original_order() {
    let root = tempfile::tempdir().unwrap();
    let app = configured(root.path());
    let first = rule(&app, "first", -20, false);
    let second = rule(&app, "second", 80, true);
    install(&app, vec![first.clone(), second.clone()]);
    let before = app.automation_status();
    let input = new_input(&app, &["second", "first"], ResponseSetIntakeMode::Copy);
    let review = app.review_response_set_intake(&input).unwrap();
    assert_eq!(
        review
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(review.active_count, 1);
    let result = app.collect_response_set(input).await.unwrap();
    assert_eq!(result.status.rules, before.rules);
    assert_eq!(result.added_count, 2);
    let set = &result.status.response_sets[0];
    assert!(set.applied.is_none());
    assert_eq!(set.versions, [] as [crate::ResponseSetVersion; 0]);
    assert_eq!(set.notes, "Keep the task context");
    assert_definition_preserved(&set.rules[0], &first);
    assert_definition_preserved(&set.rules[1], &second);
    let copied = app
        .inspect_response_asset(set.rules[0].request.response_asset.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(copied.bytes_base64.as_deref(), Some("AP8NCkE="));
    let restarted = configured(root.path()).automation_status();
    assert_eq!(restarted.rules, before.rules);
    assert_eq!(restarted.response_sets, result.status.response_sets);
}

#[tokio::test]
async fn move_preserves_other_rules_and_applied_snapshot_and_obeys_global_pause() {
    let root = tempfile::tempdir().unwrap();
    let app = configured(root.path());
    let first = rule(&app, "first", 10, true);
    let second = rule(&app, "second", 20, false);
    let owned = rule(&app, "owned", i32::MAX, true);
    install(&app, vec![first.clone(), second.clone()]);
    let created = app
        .change_response_set(
            app.automation_status().generation,
            ResponseSetChange::Create {
                id: "destination".into(),
                name: "Existing task".into(),
                notes: "notes".into(),
                rules: vec![owned.clone()],
            },
        )
        .unwrap();
    let applied = app
        .change_response_set(
            created.generation,
            ResponseSetChange::Apply {
                id: "destination".into(),
                expected_revision: 1,
                acknowledge_overlaps: true,
                resume: false,
            },
        )
        .unwrap();
    app.set_autoresponses_enabled(false, applied.generation)
        .unwrap();
    let input = existing_input(&app, &["first"], ResponseSetIntakeMode::Move);
    assert_eq!(
        app.review_response_set_intake(&input).unwrap().active_count,
        0
    );
    let result = app.collect_response_set(input).await.unwrap();
    assert!(!result.status.autoresponses_enabled);
    assert_eq!(result.status.rules, vec![second, owned]);
    assert_eq!(
        result.status.response_sets[0].applied,
        applied.response_sets[0].applied
    );
    assert_eq!(result.status.response_sets[0].rules.len(), 2);
    assert_eq!(
        result.status.response_sets[0].rules[0].revision, 8,
        "Reordering the current snapshot needs a distinct revision"
    );
    assert_definition_preserved(&result.status.response_sets[0].rules[1], &first);
    assert!(result.status.response_sets[0].undo_apply.is_none());
}

#[tokio::test]
async fn failed_move_write_keeps_originals_and_destination_in_memory_and_on_disk() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured(root.path());
    let original = rule(&app, "original", 10, true);
    install(&app, vec![original]);
    let before = app.automation_status();
    let blocked = root.path().join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    app.automation.path = Some(Arc::new(blocked.join("automation")));
    let input = new_input(&app, &["original"], ResponseSetIntakeMode::Move);
    assert!(app.collect_response_set(input).await.is_err());
    let after = app.automation_status();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.rules, before.rules);
    assert_eq!(after.response_sets, before.response_sets);
    let restarted = configured(root.path()).automation_status();
    assert_eq!(restarted.rules, before.rules);
    assert_eq!(restarted.response_sets, [] as [crate::ResponseSet; 0]);
}

#[tokio::test]
async fn stale_destination_and_owned_or_nonresponse_sources_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let app = configured(root.path());
    let ordinary = rule(&app, "ordinary", 10, true);
    let owned = rule(&app, "owned", 20, true);
    let mut other = rule(&app, "headers", 30, true);
    other.request.response_asset = None;
    other
        .request
        .headers
        .push(transmog_automation::HeaderOperation::set("X-Other", "keep").unwrap());
    install(&app, vec![ordinary, other]);
    app.change_response_set(
        app.automation_status().generation,
        ResponseSetChange::Create {
            id: "destination".into(),
            name: "Destination".into(),
            notes: String::new(),
            rules: vec![owned],
        },
    )
    .unwrap();
    app.change_response_set(
        app.automation_status().generation,
        ResponseSetChange::Apply {
            id: "destination".into(),
            expected_revision: 1,
            acknowledge_overlaps: true,
            resume: false,
        },
    )
    .unwrap();
    for id in ["owned", "headers", "missing"] {
        let input = existing_input(&app, &[id], ResponseSetIntakeMode::Move);
        assert!(app.review_response_set_intake(&input).is_err());
        assert!(app.collect_response_set(input).await.is_err());
    }
    let mut stale = existing_input(&app, &["ordinary"], ResponseSetIntakeMode::Move);
    if let ResponseSetIntakeDestination::Existing { revision, .. } = &mut stale.destination {
        *revision += 1;
    }
    assert!(app.collect_response_set(stale).await.is_err());
    let before = app.automation_status();
    let mut stale = existing_input(&app, &["ordinary"], ResponseSetIntakeMode::Move);
    if let ResponseSetIntakeDestination::Existing { incarnation, .. } = &mut stale.destination {
        *incarnation = "old-owner".into();
    }
    assert!(app.collect_response_set(stale).await.is_err());
    assert_eq!(app.automation_status().rules, before.rules);
    assert_eq!(app.automation_status().response_sets, before.response_sets);
}

#[tokio::test]
async fn overlapping_responses_require_explicit_review_and_never_replace() {
    let root = tempfile::tempdir().unwrap();
    let app = configured(root.path());
    let first = rule(&app, "first", 10, true);
    let mut second = rule(&app, "second", 20, true);
    second.matcher.clone_from(&first.matcher);
    install(&app, vec![first, second]);
    let input = new_input(&app, &["first", "second"], ResponseSetIntakeMode::Copy);
    let review = app.review_response_set_intake(&input).unwrap();
    assert_eq!(review.warnings.len(), 1);
    assert!(app.collect_response_set(input.clone()).await.is_err());
    assert_eq!(
        app.automation_status().response_sets,
        [] as [crate::ResponseSet; 0]
    );
    let result = app
        .collect_response_set(ResponseSetIntakeInput {
            acknowledge_overlaps: true,
            ..input
        })
        .await
        .unwrap();
    assert_eq!(result.status.response_sets[0].rules.len(), 2);
    assert_eq!(result.status.rules.len(), 2);
}

#[tokio::test]
async fn linked_and_copied_file_sources_remain_explicit_and_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let app = configured(root.path());
    let file = root.path().join("response.bin");
    std::fs::write(&file, [0, 255, 42]).unwrap();
    let mut rules = Vec::new();
    for (index, mode) in [ResponseBodyFileMode::Copy, ResponseBodyFileMode::Linked]
        .into_iter()
        .enumerate()
    {
        let id = format!("file-{index}");
        let mut source = rule(&app, &id, i32::try_from(index).unwrap(), index == 0);
        let asset = app
            .import_response_asset(crate::ImportResponseAsset {
                id: format!("file-asset-{index}"),
                revision: 1,
                status: 200,
                headers: HeaderBlock::new(),
                body_path: file.clone(),
                body_file_mode: mode,
                media_type: None,
            })
            .unwrap();
        source.request.response_asset = Some(asset.asset_ref());
        rules.push(source);
    }
    install(&app, rules.clone());
    let input = new_input(&app, &["file-1", "file-0"], ResponseSetIntakeMode::Move);
    let review = app.review_response_set_intake(&input).unwrap();
    assert!(review.rows[0].body_source.starts_with("Copied file:"));
    assert!(review.rows[1].body_source.starts_with("Linked file:"));
    let result = app.collect_response_set(input).await.unwrap();
    assert_eq!(result.status.rules, [] as [transmog_automation::Rule; 0]);
    for (actual, original) in result.status.response_sets[0].rules.iter().zip(&rules) {
        assert_definition_preserved(actual, original);
    }
}

async fn import_binary(app: &Application, root: &std::path::Path) -> String {
    let path = root.join("captured.har");
    let bytes = serde_json::to_vec(&serde_json::json!({"log":{"version":"1.2","creator":{"name":"Intake test","version":"1"},"entries":[{
        "startedDateTime":"2026-10-18T18:00:00Z","time":1,
        "request":{"method":"POST","url":"https://intake.invalid/captured","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":0},
        "response":{"status":201,"statusText":"Created","httpVersion":"HTTP/1.1","headers":[{"name":"X-Duplicate","value":"one"},{"name":"X-Duplicate","value":"two"}],"cookies":[],"content":{"size":5,"mimeType":"application/octet-stream","text":"AP8NCkE=","encoding":"base64"},"redirectURL":"","headersSize":-1,"bodySize":5},
        "cache":{},"timings":{"send":0,"wait":0,"receive":1}
    }]}})).unwrap();
    std::fs::write(&path, bytes).unwrap();
    app.import_trace(
        crate::TraceImportRequest {
            path,
            password: None,
            operation_id: "intake".into(),
            max_file_bytes: u64::MAX,
        },
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    app.query_sessions(crate::SessionQueryInput::default())
        .unwrap()
        .sessions
        .remove(0)
        .id
}

#[tokio::test]
async fn traffic_copies_complete_binary_bytes_and_cleans_assets_after_failed_commit() {
    let root = tempfile::tempdir().unwrap();
    let mut app = configured(root.path());
    let id = import_binary(&app, root.path()).await;
    let mut input = new_input(&app, &[], ResponseSetIntakeMode::Copy);
    input.source = ResponseSetIntakeSource::Traffic { ids: vec![id] };
    assert_eq!(
        app.review_response_set_intake(&input).unwrap().active_count,
        0
    );
    let original_path = app.automation.path.clone();
    let blocked = root.path().join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    app.automation.path = Some(Arc::new(blocked.join("automation")));
    assert!(app.collect_response_set(input.clone()).await.is_err());
    assert!(
        app.response_assets().is_empty(),
        "Failed admission leaked asset references"
    );
    assert_eq!(
        app.automation_status().response_sets,
        [] as [crate::ResponseSet; 0]
    );
    app.automation.path = original_path;
    let result = app.collect_response_set(input).await.unwrap();
    assert_eq!(result.status.rules, [] as [transmog_automation::Rule; 0]);
    let rule = &result.status.response_sets[0].rules[0];
    assert_eq!(rule.matcher.method.as_deref(), Some("POST"));
    let asset = app
        .inspect_response_asset(rule.request.response_asset.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(asset.bytes_base64.as_deref(), Some("AP8NCkE="));
    assert_eq!(asset.asset.status, 201);
    assert_eq!(asset.asset.body_bytes, 5);
    assert_eq!(asset.asset.headers.values("x-duplicate").count(), 2);
}

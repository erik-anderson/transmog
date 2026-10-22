use super::*;
use std::sync::Arc;
use transmog_automation::{RequestActions, ResponseActions, RuleMatcher};

fn setup(path: Option<std::path::PathBuf>) -> (AutomationRegistry, Rule, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let assets =
        crate::response_assets::ResponseAssetStore::load(Some(root.path().join("assets"))).unwrap();
    let asset = assets
        .create_authored(crate::AuthoredResponseAsset {
            id: "body".into(),
            revision: 1,
            status: 200,
            headers: transmog_core::HeaderBlock::new(),
            body: b"original".to_vec(),
            media_type: None,
        })
        .unwrap();
    let rule = Rule {
        id: "owned".into(),
        display_name: Some("Edited response".into()),
        enabled: true,
        revision: 1,
        priority: -100,
        matcher: RuleMatcher {
            host: Some("example.test".into()),
            ..RuleMatcher::default()
        },
        request: RequestActions {
            response_asset: Some(asset.asset_ref()),
            ..RequestActions::default()
        },
        response: ResponseActions::default(),
    };
    (
        AutomationRegistry::load(path, Arc::new(assets)).unwrap(),
        rule,
        root,
    )
}

#[test]
fn collection_accepts_one_thousand_responses_and_rejects_one_more_atomically() {
    let (registry, template, _root) = setup(None);
    let rules = (0..1_000)
        .map(|index| {
            let mut rule = template.clone();
            rule.id = format!("response-{index}");
            rule.priority = index;
            rule
        })
        .collect::<Vec<_>>();
    create(&registry, rules.clone());
    version(&registry, "milestone");
    assert_eq!(
        registry.status().response_sets[0].versions[0].rules.len(),
        1_000
    );
    let before = registry.status();
    let set = &before.response_sets[0];
    let mut too_many = rules;
    let mut extra = template;
    extra.id = "one-too-many".into();
    extra.priority = 1_000;
    too_many.push(extra);
    let error = registry
        .change_response_set(
            before.generation,
            ResponseSetChange::SaveCurrent {
                id: set.id.clone(),
                expected_revision: set.revision,
                name: set.name.clone(),
                notes: set.notes.clone(),
                rules: too_many,
            },
        )
        .unwrap_err();
    assert!(error.message.contains("1,000"));
    assert_eq!(registry.status(), before);
}

fn change(registry: &AutomationRegistry, change: ResponseSetChange) -> AutomationStatus {
    registry
        .change_response_set(registry.status().generation, change)
        .unwrap()
}
fn create(registry: &AutomationRegistry, rules: Vec<Rule>) -> AutomationStatus {
    change(
        registry,
        ResponseSetChange::Create {
            id: "set".into(),
            name: "Workspace".into(),
            notes: "Investigation notes".into(),
            rules,
        },
    )
}
fn save(registry: &AutomationRegistry, rules: Vec<Rule>) -> AutomationStatus {
    let set = registry.status().response_sets.remove(0);
    change(
        registry,
        ResponseSetChange::SaveCurrent {
            id: set.id,
            expected_revision: set.revision,
            name: set.name,
            notes: set.notes,
            rules,
        },
    )
}
fn apply(registry: &AutomationRegistry) -> AutomationStatus {
    change(
        registry,
        ResponseSetChange::Apply {
            id: "set".into(),
            expected_revision: registry.status().response_sets[0].revision,
            acknowledge_overlaps: true,
            resume: false,
        },
    )
}
fn version(registry: &AutomationRegistry, id: &str) -> AutomationStatus {
    change(
        registry,
        ResponseSetChange::SaveVersion {
            id: "set".into(),
            expected_revision: registry.status().response_sets[0].revision,
            version_id: id.into(),
            name: format!("Version {id}"),
        },
    )
}
fn delete_version(registry: &AutomationRegistry, id: &str) -> AutomationStatus {
    change(
        registry,
        ResponseSetChange::DeleteVersion {
            id: "set".into(),
            version_id: id.into(),
        },
    )
}

#[test]
fn forty_ordinary_saves_replace_current_without_history_or_live_changes() {
    let root = tempfile::tempdir().unwrap();
    let (registry, mut rule, _assets) = setup(Some(root.path().join("automation")));
    create(&registry, vec![rule.clone()]);
    let live = apply(&registry);
    let admitted = registry.active.read().unwrap().clone();
    registry
        .set_autoresponses_enabled(false, live.generation)
        .unwrap();
    for revision in 2..=41 {
        rule.revision = revision;
        rule.display_name = Some(format!("Edit {revision}"));
        let status = save(&registry, vec![rule.clone()]);
        assert_eq!(status.rules, live.rules);
        assert!(!status.autoresponses_enabled);
        assert_eq!(
            status.response_sets[0].versions,
            [] as [crate::ResponseSetVersion; 0]
        );
        assert_eq!(status.response_sets[0].rules, vec![rule.clone()]);
    }
    assert_eq!(admitted.compiled.registrations().len(), 1);
    let reopened =
        AutomationRegistry::load(registry.path.as_deref().cloned(), registry.assets.clone())
            .unwrap();
    assert_eq!(
        reopened.status().response_sets,
        registry.status().response_sets
    );
    assert_eq!(reopened.status().rules, live.rules);
    assert!(!reopened.status().autoresponses_enabled);
}

#[test]
fn versions_restore_and_delete_independently_from_applied_snapshot() {
    let (registry, mut rule, _assets) = setup(None);
    create(&registry, vec![rule.clone()]);
    version(&registry, "first");
    let first = apply(&registry);
    rule.revision = 2;
    rule.display_name = Some("Second".into());
    save(&registry, vec![rule.clone()]);
    version(&registry, "second");
    let second = apply(&registry);
    change(
        &registry,
        ResponseSetChange::RestoreVersion {
            id: "set".into(),
            expected_revision: second.response_sets[0].revision,
            version_id: "first".into(),
        },
    );
    assert_eq!(
        registry.status().response_sets[0].rules,
        first.response_sets[0].rules
    );
    assert_eq!(registry.status().rules, second.rules);
    // An unchanged historical definition can be saved again (for example after editing notes).
    save(&registry, first.response_sets[0].rules.clone());
    delete_version(&registry, "second");
    delete_version(&registry, "first");
    assert_eq!(registry.status().rules, second.rules);
    assert_eq!(
        registry.status().response_sets[0].applied,
        second.response_sets[0].applied
    );
    assert_eq!(
        registry.status().response_sets[0].versions,
        [] as [crate::ResponseSetVersion; 0]
    );
}

#[test]
fn empty_sets_and_versions_work_without_baseline_and_stale_selectors_fail() {
    let (registry, rule, _assets) = setup(None);
    let empty = create(&registry, Vec::new());
    assert_eq!(
        empty.response_sets[0].versions,
        [] as [crate::ResponseSetVersion; 0]
    );
    let selection = empty.response_sets[0].current_selection();
    version(&registry, "empty");
    save(&registry, vec![rule]);
    assert!(
        registry.status().response_sets[0]
            .selected_rules(&selection)
            .is_none()
    );
    assert!(
        registry
            .response_set_overlaps("set", selection.revision)
            .is_err()
    );
    change(
        &registry,
        ResponseSetChange::RestoreVersion {
            id: "set".into(),
            expected_revision: registry.status().response_sets[0].revision,
            version_id: "empty".into(),
        },
    );
    assert_eq!(
        registry.status().response_sets[0].rules,
        [] as [transmog_automation::Rule; 0]
    );
    assert_eq!(apply(&registry).rules, [] as [transmog_automation::Rule; 0]);
}

#[test]
fn deletion_stops_only_owned_rules_and_keeps_shared_assets_and_ordinary_copies() {
    let (registry, rule, _assets) = setup(None);
    let mut ordinary = rule.clone();
    ordinary.id = "ordinary-copy".into();
    ordinary.priority = 100;
    let candidate = registry
        .validate(AutomationRuleSet {
            rules: vec![ordinary.clone()],
            ..AutomationRuleSet::default()
        })
        .unwrap();
    registry.activate(&candidate.candidate_id).unwrap();
    create(&registry, vec![rule]);
    version(&registry, "retained");
    apply(&registry);
    let previous = registry.active.read().unwrap().clone();
    let mut other_rule = registry.status().response_sets[0].rules[0].clone();
    other_rule.id = "other-set-rule".into();
    other_rule.priority = 200;
    change(
        &registry,
        ResponseSetChange::Create {
            id: "other".into(),
            name: "Other set".into(),
            notes: String::new(),
            rules: vec![other_rule.clone()],
        },
    );
    change(
        &registry,
        ResponseSetChange::Apply {
            id: "other".into(),
            expected_revision: 1,
            acknowledge_overlaps: true,
            resume: false,
        },
    );
    change(&registry, ResponseSetChange::Delete { id: "set".into() });
    assert_eq!(registry.status().rules, vec![ordinary, other_rule]);
    assert_eq!(registry.status().response_sets.len(), 1);
    assert_eq!(registry.status().response_sets[0].id, "other");
    assert!(
        registry
            .assets
            .list()
            .iter()
            .any(|asset| asset.asset_ref() == "body@1")
    );
    assert_eq!(previous.compiled.registrations().len(), 2);
}

#[test]
fn applying_and_resuming_preserve_global_pause_and_owned_enabled_choices() {
    let (registry, rule, _assets) = setup(None);
    create(&registry, vec![rule.clone()]);
    change(
        &registry,
        ResponseSetChange::Pause {
            id: "set".into(),
            paused: true,
        },
    );
    registry
        .set_autoresponses_enabled(false, registry.status().generation)
        .unwrap();
    let applied = apply(&registry);
    assert!(applied.response_sets[0].paused);
    assert!(!applied.autoresponses_enabled);
    assert_eq!(applied.rules, vec![rule]);
    assert!(
        registry
            .active
            .read()
            .unwrap()
            .compiled
            .registrations()
            .is_empty()
    );
    change(
        &registry,
        ResponseSetChange::Pause {
            id: "set".into(),
            paused: false,
        },
    );
    assert!(!registry.status().autoresponses_enabled);
    assert!(
        registry
            .active
            .read()
            .unwrap()
            .compiled
            .registrations()
            .is_empty()
    );
    registry
        .set_autoresponses_enabled(true, registry.status().generation)
        .unwrap();
    assert_eq!(
        registry
            .active
            .read()
            .unwrap()
            .compiled
            .registrations()
            .len(),
        1
    );
}

#[test]
fn apply_and_resume_undo_restores_exact_applied_content_pause_and_page_metadata() {
    let (registry, mut rule, _assets) = setup(None);
    create(&registry, vec![rule.clone()]);
    version(&registry, "first");
    let first = apply(&registry);
    delete_version(&registry, "first");
    change(
        &registry,
        ResponseSetChange::Pause {
            id: "set".into(),
            paused: true,
        },
    );
    rule.revision = 2;
    rule.display_name = Some("New current".into());
    save(&registry, vec![rule.clone()]);
    registry
        .set_autoresponses_enabled(false, registry.status().generation)
        .unwrap();
    let second = change(
        &registry,
        ResponseSetChange::Apply {
            id: "set".into(),
            expected_revision: registry.status().response_sets[0].revision,
            acknowledge_overlaps: true,
            resume: true,
        },
    );
    assert!(!second.response_sets[0].paused);
    assert!(!second.autoresponses_enabled);
    assert_eq!(
        second.response_sets[0].undo_apply.as_ref().unwrap().applied,
        first.response_sets[0].applied
    );
    assert!(second.response_sets[0].undo_apply.as_ref().unwrap().paused);
    let page = crate::ExperimentPreviewPage {
        capture: "c".repeat(32),
        entry_id: format!("{:032x}", 7),
        request_number: "7".into(),
        trace_id: None,
        url: "https://example.test/page".into(),
        method: "GET".into(),
        started_at: 1,
        source: "Live traffic".into(),
    };
    registry
        .set_experiment_preview_page("set", 0, page.clone())
        .unwrap();
    assert_eq!(registry.status().generation, second.generation);
    assert_eq!(
        registry.status().response_sets[0].undo_apply,
        second.response_sets[0].undo_apply
    );
    let undone = change(&registry, ResponseSetChange::UndoApply { id: "set".into() });
    assert_eq!(undone.rules, first.rules);
    assert_eq!(undone.response_sets[0].rules, vec![rule]);
    assert!(undone.response_sets[0].paused);
    assert!(!undone.autoresponses_enabled);
    assert_eq!(undone.response_sets[0].preview_page, Some(page));
    assert!(undone.response_sets[0].undo_apply.is_none());
    assert!(
        registry
            .change_response_set(
                undone.generation,
                ResponseSetChange::UndoApply { id: "set".into() }
            )
            .is_err()
    );
    let stopped = change(&registry, ResponseSetChange::Stop { id: "set".into() });
    assert_eq!(stopped.rules, [] as [transmog_automation::Rule; 0]);
    assert!(stopped.response_sets[0].undo_apply.is_some());
    change(
        &registry,
        ResponseSetChange::Pause {
            id: "set".into(),
            paused: false,
        },
    );
    assert!(registry.status().response_sets[0].undo_apply.is_none());
    assert!(
        registry
            .change_response_set(
                registry.status().generation,
                ResponseSetChange::UndoApply { id: "set".into() }
            )
            .is_err()
    );
}

#[test]
fn stale_writes_compile_errors_and_persistence_failure_are_atomic() {
    let root = tempfile::tempdir().unwrap();
    let (registry, rule, _assets) = setup(Some(root.path().join("folder/automation")));
    create(&registry, vec![rule.clone()]);
    apply(&registry);
    let before = registry.status();
    assert!(
        registry
            .change_response_set(
                before.generation - 1,
                ResponseSetChange::Delete { id: "set".into() }
            )
            .is_err()
    );
    assert!(
        registry
            .change_response_set(
                before.generation,
                ResponseSetChange::SaveCurrent {
                    id: "set".into(),
                    expected_revision: 99,
                    name: "Changed".into(),
                    notes: String::new(),
                    rules: vec![rule.clone()]
                }
            )
            .is_err()
    );
    let mut missing = rule;
    missing.revision = 2;
    missing.request.response_asset = Some("missing@1".into());
    assert!(
        registry
            .change_response_set(
                before.generation,
                ResponseSetChange::SaveCurrent {
                    id: "set".into(),
                    expected_revision: 1,
                    name: "Changed".into(),
                    notes: String::new(),
                    rules: vec![missing]
                }
            )
            .is_err()
    );
    // A path with a file as parent makes the durable transaction fail before publication.
    let blocked = root.path().join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    let mut blocked_registry = registry.clone();
    blocked_registry.path = Some(Arc::new(blocked.join("automation")));
    assert!(
        blocked_registry
            .change_response_set(
                before.generation,
                ResponseSetChange::Delete { id: "set".into() }
            )
            .is_err()
    );
    assert_eq!(registry.status().rules, before.rules);
    assert_eq!(registry.status().response_sets, before.response_sets);
    assert_eq!(registry.status().generation, before.generation);
}

#[test]
fn named_and_current_selectors_cannot_rebind_after_replacement_or_owner_recreation() {
    let (registry, mut rule, _assets) = setup(None);
    let initial = create(&registry, vec![rule.clone()]);
    let current = initial.response_sets[0].current_selection();
    let saved = version(&registry, "milestone");
    let named = ExperimentSelection {
        variant_id: "milestone".into(),
        ..current.clone()
    };
    let frozen = saved.response_sets[0]
        .selected_rules(&named)
        .unwrap()
        .to_vec();
    delete_version(&registry, "milestone");
    rule.revision = 2;
    rule.display_name = Some("Later current".into());
    save(&registry, vec![rule.clone()]);
    let replaced = version(&registry, "milestone");
    assert!(replaced.response_sets[0].selected_rules(&named).is_none());
    assert!(replaced.response_sets[0].selected_rules(&current).is_none());
    assert_eq!(frozen, initial.response_sets[0].rules);
    change(&registry, ResponseSetChange::Delete { id: "set".into() });
    let recreated = create(&registry, vec![rule]);
    assert_eq!(recreated.response_sets[0].revision, 1);
    assert_ne!(recreated.response_sets[0].incarnation, current.incarnation);
    assert!(
        recreated.response_sets[0]
            .selected_rules(&current)
            .is_none()
    );
}

#[test]
fn page_choices_preserve_candidates_and_exact_snapshot_undo() {
    let root = tempfile::tempdir().unwrap();
    let (registry, rule, _assets) = setup(Some(root.path().join("automation")));
    create(&registry, vec![rule]);
    let live = apply(&registry);
    let candidate = registry
        .validate(registry.active.read().unwrap().document.clone())
        .unwrap();
    let page = crate::ExperimentPreviewPage {
        capture: "d".repeat(32),
        entry_id: format!("{:032x}", 1),
        request_number: "1".into(),
        trace_id: None,
        url: "https://example.test/page".into(),
        method: "GET".into(),
        started_at: 1,
        source: "Live traffic".into(),
    };
    registry
        .set_experiment_preview_page("set", 0, page.clone())
        .unwrap();
    assert_eq!(
        registry.status().response_sets[0].undo_apply,
        live.response_sets[0].undo_apply
    );
    assert_eq!(registry.status().generation, live.generation);
    assert_eq!(registry.status().candidate_count, 1);
    registry.activate(&candidate.candidate_id).unwrap();
    assert_eq!(registry.status().response_sets[0].preview_page, Some(page));
    assert_eq!(registry.status().response_sets[0].preview_page_revision, 1);
}

#[test]
fn concurrent_current_saves_accept_one_complete_collection() {
    let (registry, rule, _assets) = setup(None);
    let initial = create(&registry, vec![rule]);
    let generation = initial.generation;
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let handles = (0..2)
        .map(|index| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                registry.change_response_set(
                    generation,
                    ResponseSetChange::SaveCurrent {
                        id: "set".into(),
                        expected_revision: 1,
                        name: format!("Writer {index}"),
                        notes: format!("Complete notes {index}"),
                        rules: Vec::new(),
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let saved = registry.status().response_sets.remove(0);
    assert_eq!(saved.revision, 2);
    assert_eq!(
        saved.name.replace("Writer ", ""),
        saved.notes.replace("Complete notes ", "")
    );
    assert_eq!(saved.rules, [] as [transmog_automation::Rule; 0]);
    assert_eq!(saved.versions, [] as [crate::ResponseSetVersion; 0]);
}

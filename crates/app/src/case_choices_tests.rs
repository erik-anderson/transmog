use super::*;
use crate::{AppConfig, AutomationRuleSet, BodyStoreConfig, SessionQueryInput, TraceImportRequest};
use std::{fs::File, sync::Arc};
use transmog_automation::{HeaderOperation, RequestActions, ResponseActions, RuleMatcher};
use transmog_capture::{CaptureLimits, CaptureRecord, CaptureRecordKind, CaptureWriter};
use transmog_core::{HeaderField, HttpLegVersion};

fn rule(id: &str, path: &str) -> Rule {
    Rule {
        id: id.into(),
        display_name: Some(id.into()),
        enabled: false,
        revision: 1,
        priority: 0,
        matcher: RuleMatcher {
            path_prefix: Some(path.into()),
            ..Default::default()
        },
        request: RequestActions {
            headers: vec![HeaderOperation::set("X-Picker-Test", "must-not-run").unwrap()],
            ..Default::default()
        },
        response: ResponseActions::default(),
    }
}
fn samples(request: RequestHead, response: Option<ResponseHead>) -> Samples {
    Samples {
        heads: vec![(Some(request), response)],
        complete: true,
        remaining: 1_000_000,
        deadline: Instant::now() + Duration::from_secs(10),
    }
}
#[test]
fn assessment_distinguishes_response_predicates_redaction_missing_heads_and_budget() {
    let mut rule = rule("response", "/");
    rule.matcher.response_status = Some(201);
    rule.matcher.response_headers = vec![HeaderPredicate {
        name: "X-Binary".into(),
        condition: HeaderCondition::Exact(vec![0xff]),
    }];
    let request =
        transmog_automation::request_for_test("GET", "https://case.invalid/", &[]).unwrap();
    let response = ResponseHead {
        status: 201,
        headers: HeaderBlock::from_fields(vec![
            HeaderField::try_new("X-Binary", vec![0xff]).unwrap(),
        ]),
        source_version: HttpLegVersion::Http1,
    };
    assert_eq!(
        samples(request.clone(), Some(response.clone())).relevance(&[rule.clone()]),
        "matched"
    );
    let mut other = response.clone();
    other.status = 200;
    assert_eq!(
        samples(request.clone(), Some(other)).relevance(&[rule.clone()]),
        "unmatched"
    );
    assert_eq!(
        samples(request.clone(), None).relevance(&[rule.clone()]),
        "unknown"
    );
    let mut hidden = response;
    hidden.headers = HeaderBlock::from_fields(vec![
        HeaderField::from_redacted(b"X-Binary".to_vec(), Some(1)).unwrap(),
    ]);
    assert_eq!(
        samples(request.clone(), Some(hidden)).relevance(&[rule.clone()]),
        "unknown"
    );
    let mut budget = samples(request, None);
    budget.remaining = 0;
    assert_eq!(budget.relevance(&[rule]), "unknown");
    budget.heads = vec![(None, None)];
    budget.remaining = 100;
    assert_eq!(budget.relevance(&[self::rule("missing", "/")]), "unknown");
}
#[tokio::test]
#[allow(clippy::too_many_lines)] // One integration scenario qualifies full scopes, activation and review lifetimes together.
async fn suggestions_match_the_entire_filtered_or_source_scope_without_activation_or_export_reviews()
 {
    let root = tempfile::tempdir().unwrap();
    let app = Application::new(AppConfig {
        body_store: Some(BodyStoreConfig::product_default(root.path().join("bodies"))),
        automation_path: Some(root.path().join("automation")),
        ..Default::default()
    })
    .unwrap();
    let input = root.path().join("many.tmcap");
    let mut writer =
        CaptureWriter::new(File::create(&input).unwrap(), CaptureLimits::default()).unwrap();
    let mut sequence = 0;
    for id in 1..=99u128 {
        for kind in [
            CaptureRecordKind::RequestHead {
                boundary: ExchangeBoundary::ClientRequest,
                method: "GET".into(),
                target: format!("https://case.invalid/item/{id}"),
                headers: vec![],
            },
            CaptureRecordKind::Completed,
        ] {
            sequence += 1;
            writer
                .append(&CaptureRecord {
                    sequence,
                    exchange_id: id,
                    kind,
                })
                .unwrap();
        }
    }
    writer.seal().unwrap();
    let imported = app
        .import_trace(
            TraceImportRequest {
                path: input,
                operation_id: "picker-import".into(),
                password: None,
                max_file_bytes: u64::MAX,
            },
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    let candidate = app
        .validate_automation(AutomationRuleSet {
            rules: vec![
                rule("last-request", "/item/99"),
                rule("missing-resource", "/not-captured"),
            ],
            ..Default::default()
        })
        .unwrap();
    app.activate_automation(&candidate.candidate_id).unwrap();
    let before = serde_json::to_value(app.automation_status()).unwrap();
    let review = app.review_traffic_export(None).unwrap();
    let query = SessionQueryInput {
        limit: Some(1),
        ..Default::default()
    };
    assert_eq!(app.query_sessions(query.clone()).unwrap().sessions.len(), 1);
    for scope in [
        ComparisonScope::Filtered {
            query: Box::new(query),
        },
        ComparisonScope::Source {
            trace_id: Some(imported.trace.id),
        },
    ] {
        for _ in 0..4 {
            app.case_export_choices(Some(&scope)).unwrap();
        }
        let choices = app.case_export_choices(Some(&scope)).unwrap();
        assert_eq!(choices.request_count, 99);
        assert_eq!(
            choices
                .choices
                .iter()
                .find(|c| c.rule_id.as_deref() == Some("last-request"))
                .unwrap()
                .relevance,
            "matched"
        );
        assert_eq!(
            choices
                .choices
                .iter()
                .find(|c| c.rule_id.as_deref() == Some("missing-resource"))
                .unwrap()
                .relevance,
            "unmatched"
        );
    }
    let all = app.case_export_choices(None).unwrap();
    assert!(!all.evidence_selected);
    assert!(all.choices.iter().all(|c| c.relevance == "not-assessed"));
    assert!(app.export_scope(Some(&review.id)).is_ok());
    let rows = app
        .query_sessions(SessionQueryInput::default())
        .unwrap()
        .sessions;
    let subject = |id: String| crate::comparison::ComparisonSubject::Exchange {
        id,
        request_boundary: "client-request".into(),
        response_boundary: "client-response".into(),
    };
    let comparison = app
        .compare_evidence(
            crate::comparison::ComparisonRequest {
                operation_id: "scope-union".into(),
                left: subject(rows[0].id.clone()),
                right: subject(rows[1].id.clone()),
                options: crate::comparison::ComparisonOptions::default(),
            },
            [None, None],
        )
        .await
        .unwrap();
    let scope = app
        .case_export_scope(
            Some(ComparisonScope::Selected {
                ids: vec![rows[0].id.clone(), rows[2].id.clone()],
            }),
            std::slice::from_ref(&comparison.id),
        )
        .unwrap()
        .unwrap();
    let ids = crate::export_review::scope_ids(&app, &scope).unwrap();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&rows[1].id));
    let required = app
        .case_export_scope(None, std::slice::from_ref(&comparison.id))
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::export_review::scope_ids(&app, &required)
            .unwrap()
            .len(),
        2
    );
    assert!(
        app.case_export_scope(None, &["expired-comparison".into()])
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(app.automation_status()).unwrap(),
        before
    );
}

use super::*;
use transmog_extensions::{ContextExchange, ContextPage, Header, Manifest, MessageInput};
fn declaration() -> Visualizer {
    serde_json::from_value::<Manifest>(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.context","version":"1.0.0","name":"Context",
        "visualizers":[{"id":"view","name":"View","entry":"main.js","surfaces":["headers"],
            "inputs":{"request":{"headers":["x-conversation"],"body":"none"},"response":{"headers":[],"body":"none"}},
            "matches":[{"direction":"request","pathPrefixes":["/protocol"]}],
            "context":{"headers":[{"direction":"request","name":"x-conversation","test":{"op":"present"}}],"maxExchanges":1}}],
        "files":[{"path":"main.js","bytes":0}]
    })).unwrap().visualizers.remove(0)
}
fn step() -> VisualizationStep {
    VisualizationStep::NeedContext {
        token: "context".into(),
        request: ContextRequest {
            position: ContextPosition::Before,
            cursor: None,
            limit: 1,
        },
    }
}
fn page() -> ContextPage {
    ContextPage {
        anchor_order: "2".into(),
        next_cursor: None,
        status: ContextStatus::Complete,
        exchanges: vec![ContextExchange {
            order: "1".into(),
            input: VisualizationInput {
                instance_id: "worker".into(),
                generation: "1".into(),
                direction: Direction::Request,
                response: None,
                request: Some(MessageInput {
                    evidence_id: "related".into(),
                    headers: vec![Header {
                        source_id: None,
                        index: 0,
                        name: "Cookie".into(),
                        value: Some(b"not declared".to_vec()),
                        redacted: false,
                        original_bytes: None,
                    }],
                    body: Some(transmog_extensions::BodyReference {
                        source_id: "undeclared".into(),
                        length: "4".into(),
                        availability: "complete".into(),
                    }),
                    method: Some("POST".into()),
                    path: Some("/protocol".into()),
                    status: None,
                }),
            },
        }],
    }
}
#[test]
fn context_tokens_order_grants_lookahead_and_lifetime_totals_are_enforced() {
    let visualizer = declaration();
    let mut pending = Pending::default();
    let mut grants = BTreeMap::new();
    let mut remaining = 4096;
    let mut continuations = 0;
    let mut undeclared = visualizer.clone();
    undeclared.context = None;
    assert!(validate_step(&step(), &undeclared, &grants, &mut pending, remaining).is_err());
    let mut after = step();
    if let VisualizationStep::NeedContext { request, .. } = &mut after {
        request.position = ContextPosition::After;
    }
    assert!(validate_step(&after, &visualizer, &grants, &mut pending, remaining).is_err());
    for mutate in [
        (|page: &mut ContextPage| page.exchanges[0].order = "2".into()) as fn(&mut ContextPage),
        |page| page.anchor_order = "02".into(),
        |page| page.status = ContextStatus::More,
        |page| page.next_cursor = Some("forged".into()),
        |page| page.exchanges.push(page.exchanges[0].clone()),
    ] {
        let mut pending = Pending::default();
        validate_step(&step(), &visualizer, &grants, &mut pending, remaining).unwrap();
        let mut page = page();
        mutate(&mut page);
        assert!(
            prepare(
                Invocation::ResumeContext {
                    token: "context".into(),
                    page
                },
                &visualizer,
                &mut grants,
                &mut pending,
                &mut remaining,
                &mut continuations
            )
            .is_err()
        );
    }
    validate_step(&step(), &visualizer, &grants, &mut pending, remaining).unwrap();
    assert!(
        prepare(
            Invocation::Resume {
                token: "context".into(),
                pages: Vec::new()
            },
            &visualizer,
            &mut grants,
            &mut pending,
            &mut remaining,
            &mut continuations
        )
        .is_err()
    );
    assert!(
        prepare(
            Invocation::ResumeContext {
                token: "other".into(),
                page: page()
            },
            &visualizer,
            &mut grants,
            &mut pending,
            &mut remaining,
            &mut continuations
        )
        .is_err()
    );
    let output = prepare(
        Invocation::ResumeContext {
            token: "context".into(),
            page: page(),
        },
        &visualizer,
        &mut grants,
        &mut pending,
        &mut remaining,
        &mut continuations,
    )
    .unwrap();
    let Invocation::ResumeContext { page, .. } = output else {
        panic!("context expected")
    };
    let message = page.exchanges[0].input.request.as_ref().unwrap();
    assert!(message.headers.is_empty() && message.body.is_none() && grants.is_empty());
    validate_step(&step(), &visualizer, &grants, &mut pending, remaining).unwrap();
    assert!(
        prepare(
            Invocation::ResumeContext {
                token: "context".into(),
                page: self::page()
            },
            &visualizer,
            &mut grants,
            &mut pending,
            &mut remaining,
            &mut continuations
        )
        .is_err()
    );
}

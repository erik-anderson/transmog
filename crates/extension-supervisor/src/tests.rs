use super::*;
use transmog_extensions::{
    ContextExchange, ContextPage, ContextPosition, ContextStatus, Header, Manifest, MessageInput,
};
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

mod decoding;

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
                exchange_id: "exchange-one".into(),
                request_number: "1".into(),
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

#[test]
fn live_delivery_requires_declared_interests_and_strict_native_sequences() {
    use transmog_extensions::{LiveUpdates, TrafficEvent, TrafficPhase};
    let mut visualizer = declaration();
    visualizer.live_updates = Some(LiveUpdates {
        events: vec![Direction::Request],
        body_progress: false,
    });
    let event = TrafficEvent {
        sequence: "1".into(),
        exchange_id: "exchange-one".into(),
        direction: Direction::Request,
        phase: TrafficPhase::Head,
        revision: "1".into(),
        input: page().exchanges.remove(0).input,
    };
    for change in [
        (|event: &mut TrafficEvent| event.sequence = "0".into()) as fn(&mut TrafficEvent),
        |event| event.sequence = "01".into(),
        |event| event.exchange_id = "another-exchange".into(),
        |event| event.direction = Direction::Response,
        |event| event.phase = TrafficPhase::Body,
        |event| event.phase = TrafficPhase::BodyCompleted,
        |event| event.revision = "01".into(),
    ] {
        let mut forged = event.clone();
        change(&mut forged);
        assert!(
            prepare(
                Invocation::Traffic {
                    events: vec![forged],
                    missed_hints: "0".into()
                },
                &visualizer,
                &mut BTreeMap::new(),
                &mut Pending::default(),
                &mut 4096,
                &mut 0
            )
            .is_err()
        );
    }
    let mut pending = Pending::default();
    let mut grants = BTreeMap::new();
    let mut remaining = 4096;
    let mut continuations = 0;
    let invocation = prepare(
        Invocation::Traffic {
            events: vec![event.clone()],
            missed_hints: "0".into(),
        },
        &visualizer,
        &mut grants,
        &mut pending,
        &mut remaining,
        &mut continuations,
    )
    .unwrap();
    let Invocation::Traffic { events, .. } = invocation else {
        panic!("traffic expected")
    };
    assert!(events[0].input.request.as_ref().unwrap().headers.is_empty());
    assert!(events[0].input.request.as_ref().unwrap().body.is_none());
    assert!(
        prepare(
            Invocation::Traffic {
                events: vec![event],
                missed_hints: "0".into()
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

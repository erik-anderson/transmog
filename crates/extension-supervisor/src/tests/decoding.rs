use super::*;
use transmog_extensions::{BodyReference, DecodedBody, HttpDecodeResult, Span};
struct Case {
    visualizer: Visualizer,
    pending: Pending,
    grants: BTreeMap<String, u64>,
    remaining: usize,
    continuations: usize,
}
impl Case {
    fn new() -> Self {
        let mut visualizer = declaration();
        visualizer.inputs.request.body = transmog_extensions::BodyAccess::Optional;
        visualizer
            .inputs
            .request
            .headers
            .push("content-encoding".into());
        visualizer.inputs.request.decode_http_content = true;
        visualizer.inputs.response.body = transmog_extensions::BodyAccess::Optional;
        visualizer.limits.input_bytes = 8;
        let mut input = page().exchanges.remove(0).input;
        let request = input.request.as_mut().unwrap();
        request.headers.clear();
        request.body.as_mut().unwrap().source_id = "encoded-body".into();
        input.response = Some(MessageInput {
            evidence_id: "other".into(),
            headers: vec![],
            body: Some(BodyReference {
                source_id: "other-body".into(),
                length: "4".into(),
                availability: "complete".into(),
            }),
            method: None,
            path: None,
            status: Some(200),
        });
        let mut case = Self {
            visualizer,
            pending: Pending::default(),
            grants: BTreeMap::new(),
            remaining: 0,
            continuations: 0,
        };
        case.invoke(Invocation::Visualize {
            input: Box::new(input),
        })
        .unwrap();
        case
    }
    fn invoke(&mut self, invocation: Invocation) -> Result<Invocation, Failure> {
        prepare(
            invocation,
            &self.visualizer,
            &mut self.grants,
            &mut self.pending,
            &mut self.remaining,
            &mut self.continuations,
        )
    }
    fn step(&mut self, step: &VisualizationStep) -> Result<(), Failure> {
        validate_step(
            step,
            &self.visualizer,
            &self.grants,
            &mut self.pending,
            self.remaining,
        )
    }
}
fn need(source: &str) -> VisualizationStep {
    VisualizationStep::NeedDecoded {
        source_id: source.into(),
        token: "decode".into(),
    }
}
fn result(decoded: DecodedBody) -> Invocation {
    Invocation::ResumeDecoded {
        token: "decode".into(),
        result: HttpDecodeResult::Ready { decoded },
    }
}
fn decoded() -> DecodedBody {
    DecodedBody {
        body: BodyReference {
            source_id: "decoded-result".into(),
            length: "3".into(),
            availability: "complete".into(),
        },
        parent: Span {
            source_id: "encoded-body".into(),
            offset: "0".into(),
            length: "4".into(),
        },
        codings: vec!["gzip".into()],
    }
}

#[test]
fn decode_grants_and_parent_binding_reject_other_sides_sources_and_forged_results() {
    let mut case = Case::new();
    for source in ["other-body", "unknown", "header-fake"] {
        assert!(case.step(&need(source)).is_err());
    }
    for mutate in [
        (|value: &mut DecodedBody| value.parent.source_id = "other-body".into())
            as fn(&mut DecodedBody),
        |value| value.parent.offset = "1".into(),
        |value| value.parent.length = "5".into(),
        |value| value.body.length = "03".into(),
        |value| value.body.source_id = "header-decoded".into(),
        |value| value.codings = vec!["unknown".into()],
    ] {
        let mut case = Case::new();
        case.step(&need("encoded-body")).unwrap();
        let mut bad = decoded();
        mutate(&mut bad);
        assert!(case.invoke(result(bad)).is_err());
    }
    case.step(&need("encoded-body")).unwrap();
    assert!(
        case.invoke(Invocation::Resume {
            token: "decode".into(),
            pages: vec![]
        })
        .is_err()
    );
}
#[test]
fn encoded_work_and_decoded_pages_share_one_input_budget_without_recursive_decoding() {
    let mut case = Case::new();
    case.step(&need("encoded-body")).unwrap();
    case.invoke(result(decoded())).unwrap();
    assert_eq!(case.remaining, 4);
    assert!(case.step(&need("decoded-result")).is_err());
    case.step(&VisualizationStep::NeedInput {
        reads: vec![ReadRequest {
            source_id: "decoded-result".into(),
            offset: "0".into(),
            max_bytes: 3,
        }],
        token: "bytes".into(),
    })
    .unwrap();
    case.invoke(Invocation::Resume {
        token: "bytes".into(),
        pages: vec![BytePage {
            source_id: "decoded-result".into(),
            offset: "0".into(),
            bytes: vec![1, 2, 3],
        }],
    })
    .unwrap();
    assert_eq!(case.remaining, 1);
    assert!(case.step(&need("encoded-body")).is_err());
}

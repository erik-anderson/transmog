//! Production MAPI contributions with ordinary authenticated host continuations.
#![cfg(windows)]
mod support;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use transmog_extension_supervisor::{HostConfig, Runner};
use transmog_extensions::{
    BodyReference, BytePage, ContextExchange, ContextPage, ContextStatus, DecodeFailure,
    DecodedBody, Direction, HttpDecodeResult, Invocation, Manifest, Node, Snapshot, Span,
    TrafficEvent, TrafficPhase, Value, VisualizationDocument, VisualizationInput,
    VisualizationStep,
};

fn snapshot() -> Snapshot {
    static BUILD: std::sync::Once = std::sync::Once::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/mapi");
    BUILD.call_once(|| {
        let output = std::process::Command::new("node")
            .arg(root.join("build.mjs"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    });
    let manifest = Manifest::parse(
        &std::fs::read(root.join("dist/package-manifest.json")).unwrap(),
        false,
    )
    .unwrap();
    let files = manifest
        .files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                Arc::<[u8]>::from(std::fs::read(root.join(&file.path)).unwrap()),
            )
        })
        .collect();
    Snapshot::new(manifest, files).unwrap()
}

#[tokio::test]
async fn mapi_loose_folder_package_roundtrip_keeps_only_declared_runtime_files() {
    use transmog_extension_registry::Registry;
    let original = snapshot();
    for path in ["dist/runtime/body.js", "dist/runtime/headers.js"] {
        let source = std::str::from_utf8(&original.files[path]).unwrap();
        assert!(!source.contains("__toCommonJS"), "CommonJS wrapper: {path}");
    }
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("source");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, bytes) in &original.files {
        let path = folder.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut manifest = original.manifest.clone();
    manifest.id = "test.mapi.roundtrip".into();
    std::fs::write(
        folder.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        folder.join("author.ts"),
        "This unlisted source must not be packaged",
    )
    .unwrap();
    let registry = Registry::load(temporary.path().join("registry"), None);
    registry.register_directory(&folder, false).unwrap();
    let archive = temporary.path().join("mapi.tmext");
    registry
        .package_directory("test.mapi.roundtrip", &archive)
        .unwrap();
    for packaged in [false, true] {
        if packaged {
            registry.uninstall("test.mapi.roundtrip").unwrap();
            registry.install(&archive).unwrap();
        }
        let snapshot = registry.snapshot("test.mapi.roundtrip").unwrap();
        assert_eq!(snapshot.files.len(), 4);
        assert!(!snapshot.files.contains_key("author.ts"));
        let runner = Runner::start(
            &HostConfig {
                executable: support::host_executable().into(),
            },
            snapshot,
            "body-request",
        )
        .unwrap();
        let document = interpret(&runner, &fixture("connect-request-nonzero"), true).await;
        assert!(matches!(
            document.status,
            transmog_extensions::ParseStatus::Complete
        ));
        runner.stop();
    }
    registry.uninstall("test.mapi.roundtrip").unwrap();
    assert!(folder.join("author.ts").exists());
    assert!(folder.join("dist/runtime/body.js").exists());
}
fn start(id: &str) -> Arc<Runner> {
    Runner::start(
        &HostConfig {
            executable: support::host_executable().into(),
        },
        snapshot(),
        id,
    )
    .unwrap()
}
fn fixtures() -> Vec<serde_json::Value> {
    serde_json::from_str(include_str!(
        "../../../extensions/mapi/test/reference-cases.json"
    ))
    .unwrap()
}
fn fixture(name: &str) -> serde_json::Value {
    fixtures()
        .into_iter()
        .find(|value| value["fixture"]["name"] == name)
        .unwrap()["fixture"]
        .take()
}
fn input(
    message: &serde_json::Value,
    index: usize,
    direction: &str,
    sources: &mut BTreeMap<String, Vec<u8>>,
) -> VisualizationInput {
    use base64::Engine as _;
    let order = message["order"].as_u64().unwrap_or(index as u64 + 1);
    let exact_order = 9_007_199_254_740_993_u64 + order;
    let operation = message["requestType"].as_str().unwrap();
    let mut fields = serde_json::json!({
        "instanceId":"test","exchangeId":format!("exchange.{order}"),"requestNumber":exact_order.to_string(),"generation":"1","direction":direction,
        "request":{"evidenceId":format!("request.{order}"),"headers":[],"method":"POST","path":"/mapi/emsmdb"},
        "response":{"evidenceId":format!("response.{order}"),"headers":[],"status":200}
    });
    for side in ["request", "response"] {
        let names = if side == "request" {
            vec![
                ("content-type", "application/mapi-http"),
                ("x-requesttype", operation),
                ("x-clientinfo", "synthetic-client"),
            ]
        } else {
            vec![
                ("content-type", "application/mapi-http"),
                ("x-responsecode", "0"),
            ]
        };
        fields[side]["headers"] = serde_json::json!(names.into_iter().enumerate().map(|(index,(name,value))|serde_json::json!({"index":index,"name":name,"value":value.as_bytes(),"redacted":false})).collect::<Vec<_>>());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(message[side].as_str().unwrap_or_default())
            .unwrap();
        if !bytes.is_empty() {
            let id = format!("body.{order}.{side}");
            fields[side]["body"] = serde_json::json!({"sourceId":id,"length":bytes.len().to_string(),"availability":"complete"});
            sources.insert(id, bytes);
        }
    }
    serde_json::from_value(fields).unwrap()
}
async fn interpret(
    runner: &Runner,
    fixture: &serde_json::Value,
    expand: bool,
) -> VisualizationDocument {
    let direction = fixture["message"]["direction"].as_str().unwrap();
    let mut sources = BTreeMap::new();
    let contexts = fixture["context"].as_array().cloned().unwrap_or_default();
    let mut related: Vec<_> = contexts
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let input = input(message, index, direction, &mut sources);
            ContextExchange {
                order: input.request_number.clone(),
                input,
            }
        })
        .collect();
    related.sort_by_key(|exchange| exchange.order.parse::<u64>().unwrap());
    let selected = input(&fixture["message"], contexts.len(), direction, &mut sources);
    let anchor = selected.request_number.clone();
    let mut step = runner
        .invoke(Invocation::Visualize {
            input: Box::new(selected),
        })
        .await
        .unwrap();
    for _ in 0..512 {
        let invocation = match step {
            VisualizationStep::NeedInput { token, reads } => Invocation::Resume {
                token,
                pages: reads
                    .into_iter()
                    .map(|read| {
                        let bytes = &sources[&read.source_id];
                        let offset = read.offset.parse::<usize>().unwrap();
                        BytePage {
                            source_id: read.source_id,
                            offset: read.offset,
                            bytes: bytes[offset..(offset + read.max_bytes).min(bytes.len())]
                                .to_vec(),
                        }
                    })
                    .collect(),
            },
            VisualizationStep::NeedContext { token, request } => {
                assert_eq!(
                    request.position,
                    transmog_extensions::ContextPosition::Before
                );
                assert!(request.limit <= 32);
                Invocation::ResumeContext {
                    token,
                    page: ContextPage {
                        anchor_order: anchor.clone(),
                        exchanges: related.clone(),
                        next_cursor: None,
                        status: ContextStatus::Complete,
                    },
                }
            }
            VisualizationStep::Document { document } => {
                if expand && let Some(token) = expansion(&document.roots) {
                    Invocation::Expand { token }
                } else {
                    return document;
                }
            }
            other @ VisualizationStep::NeedDecoded { .. } => {
                panic!("Unexpected MAPI step {other:?}")
            }
        };
        step = runner.invoke(invocation).await.unwrap();
    }
    panic!("MAPI continuation did not finish")
}
fn expansion(nodes: &[Node]) -> Option<String> {
    nodes.iter().find_map(|node| {
        node.children_token
            .clone()
            .or_else(|| expansion(&node.children))
    })
}
fn values(nodes: &[Node]) -> Vec<&Value> {
    nodes
        .iter()
        .flat_map(|node| node.value.iter().chain(values(&node.children)))
        .collect()
}

#[tokio::test]
async fn mapi_production_body_visualizers_decode_and_expand_retained_inputs() {
    for (name, expected) in [
        ("connect-request-nonzero", Value::Integer("1252".into())),
        (
            "nspi-QueryRows-response",
            Value::String("Synthetic person café Ω".into()),
        ),
        (
            "success-table-retained-columns",
            Value::Integer("9007199254740993".into()),
        ),
        (
            "success-RopGetPropertiesAll-response",
            Value::Integer("132537600000000000".into()),
        ),
        (
            "ft-out-of-order-RopFastTransferDestinationPutBufferExtended",
            Value::String("Synthetic transfer café Ω".into()),
        ),
    ] {
        let fixture = fixture(name);
        let id = format!("body-{}", fixture["message"]["direction"].as_str().unwrap());
        let runner = start(&id);
        let document = interpret(&runner, &fixture, true).await;
        assert!(values(&document.roots).contains(&&expected), "{name}");
        assert!(
            matches!(document.status, transmog_extensions::ParseStatus::Complete),
            "{name}: {:?}",
            document.diagnostics
        );
        runner.stop();
    }
}

#[tokio::test]
async fn mapi_header_contribution_uses_no_body_capability() {
    let runner = start("headers");
    let mut fixture = fixture("connect-request-nonzero");
    fixture["message"]["request"] = serde_json::json!("");
    let document = interpret(&runner, &fixture, false).await;
    assert!(values(&document.roots).contains(&&Value::String("Connect".into())));
    assert_eq!(
        document.sources,
        [] as [transmog_extensions::DerivedSource; 0]
    );
}

#[tokio::test]
async fn mapi_production_http_decode_keeps_encoded_and_decoded_capabilities_separate() {
    for ready in [true, false] {
        let runner = start("body-request");
        let fixture = fixture("connect-request-nonzero");
        let mut sources = BTreeMap::new();
        let mut input = input(&fixture["message"], 0, "request", &mut sources);
        let message = input.request.as_mut().unwrap();
        message.headers.push(
            serde_json::from_value(serde_json::json!({
                "index":3,"name":"content-encoding","value":b"gzip","redacted":false
            }))
            .unwrap(),
        );
        let original = message.body.clone().unwrap();
        let bytes = sources[&original.source_id].clone();
        let VisualizationStep::NeedDecoded { token, source_id } = runner
            .invoke(Invocation::Visualize {
                input: Box::new(input),
            })
            .await
            .unwrap()
        else {
            panic!("HTTP decoding must precede MAPI byte reads")
        };
        assert_eq!(source_id, original.source_id);
        let result = if ready {
            HttpDecodeResult::Ready {
                decoded: DecodedBody {
                    body: BodyReference {
                        source_id: "decoded-http".into(),
                        length: bytes.len().to_string(),
                        availability: "complete".into(),
                    },
                    parent: Span {
                        source_id: original.source_id,
                        offset: "0".into(),
                        length: original.length,
                    },
                    codings: vec!["gzip".into()],
                },
            }
        } else {
            HttpDecodeResult::Unavailable {
                reason: DecodeFailure::Unsupported,
            }
        };
        let step = runner
            .invoke(Invocation::ResumeDecoded { token, result })
            .await
            .unwrap();
        let document = if ready {
            let VisualizationStep::NeedInput { token, reads } = step else {
                panic!("Decoded pages required")
            };
            assert_eq!(reads.len(), 1);
            assert_eq!(reads[0].source_id, "decoded-http");
            let VisualizationStep::Document { document } = runner
                .invoke(Invocation::Resume {
                    token,
                    pages: vec![BytePage {
                        source_id: "decoded-http".into(),
                        offset: "0".into(),
                        bytes,
                    }],
                })
                .await
                .unwrap()
            else {
                panic!("Decoded MAPI document required")
            };
            assert!(values(&document.roots).contains(&&Value::Integer("1252".into())));
            document
        } else {
            let VisualizationStep::Document { document } = step else {
                panic!("Failed HTTP decoding must not read encoded pages")
            };
            document
        };
        assert_eq!(
            document.status,
            if ready {
                transmog_extensions::ParseStatus::Complete
            } else {
                transmog_extensions::ParseStatus::Unavailable
            }
        );
        runner.stop();
    }
}

#[tokio::test]
async fn mapi_production_live_completion_replaces_an_incomplete_selected_body() {
    let runner = start("body-request");
    let fixture = fixture("connect-request-nonzero");
    let mut sources = BTreeMap::new();
    let complete = input(&fixture["message"], 0, "request", &mut sources);
    let mut incomplete = complete.clone();
    incomplete.request.as_mut().unwrap().body = Some(BodyReference {
        source_id: "capturing".into(),
        length: "0".into(),
        availability: "capturing".into(),
    });
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Visualize {
            input: Box::new(incomplete),
        })
        .await
        .unwrap()
    else {
        panic!("Incomplete snapshot required")
    };
    assert_eq!(
        document.status,
        transmog_extensions::ParseStatus::Unavailable
    );
    let event = TrafficEvent {
        sequence: "1".into(),
        exchange_id: complete.exchange_id.clone(),
        direction: Direction::Request,
        phase: TrafficPhase::BodyCompleted,
        revision: "1".into(),
        input: complete,
    };
    let VisualizationStep::NeedInput { token, reads } = runner
        .invoke(Invocation::Traffic {
            events: vec![event],
            missed_hints: "0".into(),
        })
        .await
        .unwrap()
    else {
        panic!("Updated selected body must be read")
    };
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Resume {
            token,
            pages: vec![BytePage {
                source_id: reads[0].source_id.clone(),
                offset: "0".into(),
                bytes: sources[&reads[0].source_id].clone(),
            }],
        })
        .await
        .unwrap()
    else {
        panic!("Completed snapshot required")
    };
    assert_eq!(document.status, transmog_extensions::ParseStatus::Complete);
    assert!(values(&document.roots).contains(&&Value::Integer("1252".into())));
}

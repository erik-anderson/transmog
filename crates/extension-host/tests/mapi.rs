//! The TypeScript decoder runs in the ordinary capability-free native worker.
#![cfg(windows)]
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Once},
};
use transmog_extension_supervisor::{HostConfig, Runner};
use transmog_extensions::{
    BytePage, Invocation, Manifest, Snapshot, Value, VisualizationInput, VisualizationStep,
};

fn prepare() -> PathBuf {
    static BUILD: Once = Once::new();
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
    root
}

fn start(entry: &str) -> Arc<Runner> {
    let core = std::fs::read(prepare().join("dist/testing.bundle.js")).unwrap();
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"test.mapi.typescript","version":"1.0.0","name":"MAPI TypeScript compatibility",
        "visualizers":[{"id":"test","name":"Test","entry":"main.js","surfaces":["headers"],
            "inputs":{"request":{"headers":["x-requesttype"],"body":"optional"},"response":{"headers":[],"body":"none"}},
            "matches":[{"direction":"request","headers":[{"direction":"request","name":"x-requesttype","test":{"op":"present"}}]}]}],
        "files":[{"path":"main.js","bytes":entry.len()},{"path":"testing.js","bytes":core.len()}]
    })).unwrap();
    let snapshot = Snapshot::new(
        manifest,
        BTreeMap::from([
            ("main.js".into(), Arc::<[u8]>::from(entry.as_bytes())),
            ("testing.js".into(), Arc::<[u8]>::from(core)),
        ]),
    )
    .unwrap();
    Runner::start(
        &HostConfig {
            executable: env!("CARGO_BIN_EXE_transmog-extension-host").into(),
        },
        snapshot,
        "test",
    )
    .unwrap()
}

fn input() -> VisualizationInput {
    serde_json::from_value(serde_json::json!({
        "instanceId":"test","exchangeId":"test","requestNumber":"1","generation":"1","direction":"request",
        "request":{"evidenceId":"test","headers":[],"method":"POST","path":"/mapi/emsmdb"}
    })).unwrap()
}

#[tokio::test]
async fn mapi_typescript_preserves_exact_values_without_runtime_capabilities() {
    let runner = start(
        r"
import { decodeFixture } from './testing.js';
function fields(node) { return [node, ...node.children.flatMap(fields)]; }
function property(type, bytes) {
  return decodeFixture([{id:1,requestType:'Execute',request:{sourceId:'synthetic',bytes}}],1,'request',{type}).root;
}
export function visualize() {
  for (const name of ['Function','fetch','Deno','process','require','WebSocket','XMLHttpRequest']) {
    if (globalThis[name] !== undefined) throw new Error('Capability restored: ' + name);
  }
  let denied = false;
  try { (function(){}).constructor('return 1')(); } catch { denied = true; }
  if (!denied) throw new Error('String compilation permitted');
  const wide = fields(property('PtypInteger64',[1,0,0,0,0,0,32,0])).find(node=>node.value?.kind==='integer');
  const guid = fields(property('PtypGuid',[51,34,17,0,85,68,119,102,136,153,170,187,204,221,238,255])).find(node=>node.value?.kind==='guid');
  if (wide.value.value !== '9007199254740993' || guid.value.value !== '00112233-4455-6677-8899-aabbccddeeff') throw new Error('Primitive data changed');
  return {kind:'document',document:{title:'TypeScript MAPI compatibility',status:'complete',presentation:'tree',roots:[
    {id:'wide',name:'Exact integer',value:{kind:'integer',value:wide.value.value}},
    {id:'guid',name:'GUID',value:{kind:'guid',value:guid.value.value}}
  ]}};
}
",
    );
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap()
    else {
        panic!("Expected parsed values")
    };
    assert_eq!(
        document.roots[0].value,
        Some(Value::Integer("9007199254740993".into()))
    );
    assert_eq!(
        document.roots[1].value,
        Some(Value::Guid("00112233-4455-6677-8899-aabbccddeeff".into()))
    );
}

#[tokio::test]
async fn mapi_typescript_keeps_authenticated_encoded_and_decoded_ranges_separate() {
    let runner = start(
        r"
import { decodeFixture, base64 } from './testing.js';
function fields(node) { return [node, ...node.children.flatMap(fields)]; }
export function visualize(input) {
  return {kind:'needInput',token:'body',reads:[{sourceId:input.request.body.sourceId,offset:'0',maxBytes:20}]};
}
export function resume(token,pages) {
  const decoded=decodeFixture([{id:1,requestType:'Connect',request:{sourceId:pages[0].sourceId,bytes:pages[0].bytes}}],1,'request',{type:'ExtendedBuffer'});
  const field=fields(decoded.root).find(node=>node.type==='EndpointCapabilityFlag');
  if (!field || field.value.value !== '1') throw new Error('Auxiliary value changed');
  const span=range=>({sourceId:range.sourceId,offset:String(range.offset),length:String(range.length)});
  return {kind:'document',document:{title:'MAPI decoded evidence',status:'complete',presentation:'tree',
    sources:decoded.derivedSources.map(source=>({id:source.id,name:'Decoded buffer',bytesBase64:base64(source.bytes),parents:source.parents.map(span),transform:source.transform,exactMapping:source.exactMapping})),
    roots:[{id:'value',name:'Endpoint capabilities',value:{kind:'integer',value:field.value.value},spans:[span(field.range)]}]}};
}
",
    );
    let mut input = input();
    input.request.as_mut().unwrap().body = Some(transmog_extensions::BodyReference {
        source_id: "encoded-body".into(),
        length: "20".into(),
        availability: "complete".into(),
    });
    assert!(matches!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input)
            })
            .await
            .unwrap(),
        VisualizationStep::NeedInput { .. }
    ));
    let encoded = [0_u8, 0, 0, 0, 8, 0, 1, 0x48, 1, 0, 0, 0].map(|byte| byte ^ 0xa5);
    let mut bytes = vec![0, 0, 7, 0, 12, 0, 8, 0];
    bytes.extend(encoded);
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Resume {
            token: "body".into(),
            pages: vec![BytePage {
                source_id: "encoded-body".into(),
                offset: "0".into(),
                bytes,
            }],
        })
        .await
        .unwrap()
    else {
        panic!("Expected decoded source")
    };
    assert_eq!(document.roots[0].spans[0].source_id, "mapi-derived.0");
    assert_eq!(document.roots[0].spans[0].offset, "4");
    assert_eq!(document.roots[0].spans[0].length, "4");
    assert_eq!(document.sources[0].parents[0].source_id, "encoded-body");
    assert_eq!(document.sources[0].parents[0].offset, "8");
    assert_eq!(document.sources[0].parents[0].length, "12");
    assert!(!document.sources[0].exact_mapping);
}

fn native_numbers(node: &serde_json::Value, values: &mut Vec<serde_json::Value>) {
    if node["raw"].is_string() && node["size"].as_str() != Some("0") {
        values.push(serde_json::json!([
            node["offset"],
            node["size"],
            node["raw"]
        ]));
    }
    for child in node["children"].as_array().into_iter().flatten() {
        native_numbers(child, values);
    }
}

#[tokio::test]
async fn mapi_typescript_representative_native_reference_values_run_in_sandbox() {
    use base64::Engine as _;
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../extensions/mapi/test/reference-cases.json"
    ))
    .unwrap();
    let names = [
        "nspi-QueryRows-response",
        "nspi-GetMatches-request",
        "nspi-ModLinkAtt-request",
        "nspi-ResolveNames-response",
        "nspi-UpdateStat-response",
        "nspi-props-without-values-response",
        "success-table-retained-columns",
        "dispatch-RopWritePerUserInformationRequest",
        "ft-RopFastTransferDestinationPutBufferExtended-markers-named-multivalue-request",
        "success-RopGetPropertiesAll-response",
        "ft-partial-get-unicode",
        "ft-out-of-order-RopFastTransferDestinationPutBufferExtended",
    ];
    let mut selected = Vec::new();
    for item in cases
        .into_iter()
        .filter(|item| names.contains(&item["fixture"]["name"].as_str().unwrap()))
    {
        let fixture = &item["fixture"];
        let messages:Vec<_>=fixture["context"].as_array().into_iter().flatten().chain(std::iter::once(&fixture["message"])).enumerate().map(|(index,message)| {
            let id=message["order"].as_u64().unwrap_or(index as u64+1);
            serde_json::json!({"id":id,"requestType":message["requestType"],"request":{"sourceId":format!("body.{id}.request"),"bytes":base64::engine::general_purpose::STANDARD.decode(message["request"].as_str().unwrap_or_default()).unwrap()},"response":{"sourceId":format!("body.{id}.response"),"bytes":base64::engine::general_purpose::STANDARD.decode(message["response"].as_str().unwrap_or_default()).unwrap()}})
        }).collect();
        let mut expected = Vec::new();
        if fixture["family"] != "fast-transfer-partial" {
            native_numbers(&item["tree"], &mut expected);
        }
        selected.push(serde_json::json!({"messages":messages,"direction":fixture["message"]["direction"],"partial":fixture["family"]=="fast-transfer-partial","expected":expected}));
    }
    assert_eq!(selected.len(), names.len());
    let entry=r"
import { decodeFixture } from './testing.js';
const cases=__CASES__;
function fields(node) { return [node,...node.children.flatMap(fields)]; }
export function visualize() {
  for (const item of cases) {
    const result=decodeFixture(item.messages,item.messages.at(-1).id,item.direction,{});
    const nodes=fields(result.root);
    if (nodes.some(node=>node.issue)) throw new Error('Incomplete native decode');
    for (const [offset,size,raw] of item.expected) {
      if (!nodes.some(node=>String(node.range.offset)===offset && String(node.range.length)===size && String(node.value?.value).toLowerCase()===raw.toLowerCase())) throw new Error('Native reference value changed');
    }
    if (item.partial && !nodes.some(node=>node.value?.value==='Synthetic transfer café Ω')) throw new Error('Continuation lost its complete value');
  }
  return {kind:'document',document:{title:'Native TypeScript reference values',status:'complete',presentation:'tree',roots:[{id:'count',name:'Cases passed',value:{kind:'integer',value:String(cases.length)}}]}};
}
".replace("__CASES__",&serde_json::to_string(&selected).unwrap());
    let runner = start(&entry);
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Visualize {
            input: Box::new(input()),
        })
        .await
        .unwrap()
    else {
        panic!("Expected reference result")
    };
    assert_eq!(document.roots[0].value, Some(Value::Integer("12".into())));
}

#[tokio::test]
async fn mapi_typescript_corrupt_buffers_recover_and_preserve_empty_upload_boundaries() {
    let runner = start(
        r"
import { decodeFixture } from './testing.js';
const u16=n=>[n&255,(n>>>8)&255],u32=n=>[n&255,(n>>>8)&255,(n>>>16)&255,(n>>>24)&255];
function parse(bytes){return decodeFixture([{id:1,requestType:'Execute',request:{sourceId:'synthetic',bytes}}],1,'request',{}).root;}
function fields(node){return [node,...node.children.flatMap(fields)];}
export function visualize(){
  for(const bytes of [[...u32(0),...u32(0xffffffff),...Array(16).fill(0)],[...u32(0),...u32(8),0,0,4,0,255,255,255,255,...u32(0),...u32(0)],[...u32(0),...u32(16),...Array(8).fill(0)]]) {
    if(!fields(parse(bytes)).some(node=>node.issue))throw new Error('Malformed buffer accepted');
  }
  const rops=[0x54,7,0,0,0,1,7,0],payload=[...u16(2+rops.length),...rops,255,255,255,255];
  const rpc=[0,0,4,0,...u16(payload.length),...u16(payload.length),...payload];
  const nodes=fields(parse([...u32(0),...u32(rpc.length),...rpc,...u32(65536),...u32(0)]));
  if(nodes.some(node=>node.issue)||!nodes.some(node=>node.name==='RopRelease request'))throw new Error('Empty upload consumed Release');
  return {kind:'document',document:{title:'MAPI recovery',status:'complete',presentation:'tree',roots:[]}};
}
",
    );
    assert!(matches!(
        runner
            .invoke(Invocation::Visualize {
                input: Box::new(input())
            })
            .await
            .unwrap(),
        VisualizationStep::Document { .. }
    ));
}

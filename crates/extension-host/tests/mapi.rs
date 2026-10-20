//! The generated static parser must work inside the ordinary network-free OS sandbox.
#![cfg(windows)]
use std::{collections::BTreeMap, sync::Arc};
use transmog_extension_supervisor::{HostConfig, Runner};
use transmog_extensions::{
    BytePage, Invocation, Manifest, Snapshot, Value, VisualizationInput, VisualizationStep,
};

fn start(entry: &str) -> Arc<Runner> {
    let parser = include_bytes!("../../../extensions/mapi/generated/parser.js");
    let core = include_bytes!("../../../extensions/mapi/src/protocol.js");
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"test.mapi.compiler","version":"1.0.0","name":"MAPI compiler compatibility",
        "visualizers":[{"id":"test","name":"Test","entry":"main.js","surfaces":["headers"],
            "inputs":{"request":{"headers":["x-requesttype"],"body":"optional"},"response":{"headers":[],"body":"none"}},
            "matches":[{"direction":"request","headers":[{"direction":"request","name":"x-requesttype","test":{"op":"present"}}]}]}],
        "files":[{"path":"main.js","bytes":entry.len()},{"path":"generated/parser.js","bytes":parser.len()},{"path":"src/protocol.js","bytes":core.len()}]
    })).unwrap();
    let snapshot = Snapshot::new(
        manifest,
        BTreeMap::from([
            ("main.js".into(), Arc::<[u8]>::from(entry.as_bytes())),
            (
                "generated/parser.js".into(),
                Arc::<[u8]>::from(parser.as_slice()),
            ),
            ("src/protocol.js".into(), Arc::<[u8]>::from(core.as_slice())),
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

#[tokio::test]
async fn generated_mapi_parser_preserves_exact_values_without_restoring_runtime_capabilities() {
    let entry = r"
import './generated/parser.js';
export function visualize() {
  if(typeof globalThis.Function!=='undefined'||typeof fetch!=='undefined'||typeof Deno!=='undefined')throw new Error('Capability restored');
  let blocked=false;try{(function(){}).constructor('return 1')();}catch{blocked=true;}
  if(!blocked)throw new Error('String compilation permitted');
  const bytes=System.Array.init([47,111,61,69,120,97,109,112,108,101,0,4,3,2,1,228,4,0,0,9,4,0,0,17,4,0,0,0,0,0,0],System.Byte);
  const buffer=new BlockParser.BinaryParser.$ctor1(bytes);
  const result=BlockParser.Block.Parse(MAPIInspector.Parsers.ConnectRequestBody,buffer);
  if(result.Size.toString()!=='31'||!result.FullString().includes('DefaultCodePage: 1252'))throw new Error('Transport field mismatch');
  const wide=BlockParser.PortablePrimitives.Read(System.UInt64,System.Array.init([1,0,0,0,0,0,32,0],System.Byte));
  const guid=BlockParser.PortablePrimitives.Read(System.Guid,System.Array.init([51,34,17,0,85,68,119,102,136,153,170,187,204,221,238,255],System.Byte));
  const rpc=BlockParser.Block.Parse(MAPIInspector.Parsers.RPC_HEADER_EXT,new BlockParser.BinaryParser.$ctor1(System.Array.init([0,0,5,0,8,0,8,0],System.Byte)));
  if(!rpc.FullString().includes('Compressed'))throw new Error('Enum mismatch');
  const error=BlockParser.Block.Parse(BlockParser.BlockT$1(MAPIInspector.Parsers.ErrorCodes),new BlockParser.BinaryParser.$ctor1(System.Array.init([15,1,4,128],System.Byte)));
  if(!error.Parsed||error.Data!==2147746063||MAPIInspector.Parsers.HelpMethod.FormatErrorCode(error.Data)!=='NotFound = 0x8004010F')throw new Error('High-bit enum mismatch');
  const unknown=BlockParser.PortablePrimitives.Read(MAPIInspector.Parsers.ErrorCodes,System.Array.init([255,255,255,255],System.Byte));
  if(unknown!==4294967295)throw new Error('Unknown enum bits lost');
  const bounded=new BlockParser.BinaryParser.$ctor1(System.Array.init([1,2,3],System.Byte));
  bounded.PushStrictCap(0);if(!bounded.Empty)throw new Error('Empty protocol buffer widened');bounded.PopCap();
  let rejected=false;try{bounded.PushStrictCap(4);}catch{rejected=true;}if(!rejected||bounded.CheckSize(-1))throw new Error('Invalid protocol bounds accepted');
  return {kind:'document',document:{title:'MAPI compatibility',status:'complete',presentation:'tree',roots:[
    {id:'wide',name:'Exact integer',value:{kind:'integer',value:wide.toString()}},
    {id:'guid',name:'GUID',value:{kind:'guid',value:guid.toString()}},
    {id:'connect',name:'Connect',value:{kind:'string',value:result.FullString()}}
  ]}};
}
";
    let runner = start(entry);
    let input: VisualizationInput = serde_json::from_value(serde_json::json!({
        "instanceId":"test","exchangeId":"test","requestNumber":"1","generation":"1","direction":"request",
        "request":{"evidenceId":"test","headers":[],"method":"POST","path":"/mapi/emsmdb"}
    })).unwrap();
    let VisualizationStep::Document { document } = runner
        .invoke(Invocation::Visualize {
            input: Box::new(input),
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
async fn mapi_decoded_fields_keep_separate_sources_and_authenticated_encoded_provenance() {
    let runner = start(
        r"
import {decode} from './src/protocol.js';
export function visualize(input) {return {kind:'needInput',token:'body',reads:[{sourceId:input.request.body.sourceId,offset:'0',maxBytes:20}]};}
export function resume(token,pages) {
  const decoded=decode([{id:1,requestType:'Connect',request:{sourceId:pages[0].sourceId,bytes:pages[0].bytes}}],1,'request',{type:'MAPIInspector.Parsers.ExtendedBuffer'});
  function field(node){if(node.valueType==='MAPIInspector.Parsers.EndpointCapabilityFlag')return node;for(const child of node.children){const found=field(child);if(found)return found;}}
  const value=field(decoded.tree);if(!value)throw new Error('Missing auxiliary structure');
  return {kind:'document',document:{title:'Decoded MAPI auxiliary buffer',status:'complete',presentation:'tree',
    sources:decoded.derived.map(source=>({id:source.id,name:'Decoded MAPI buffer',bytesBase64:System.Convert.toBase64String(System.Array.init(source.bytes,System.Byte)),parents:source.parents,transform:source.transform,exactMapping:source.exactMapping})),
    roots:[{id:'capabilities',name:'Endpoint capabilities',value:{kind:'integer',value:value.raw},spans:[{sourceId:value.sourceId,offset:value.offset,length:value.size}]}]}};
}
",
    );
    let input: VisualizationInput = serde_json::from_value(serde_json::json!({
        "instanceId":"test","exchangeId":"test","requestNumber":"1","generation":"1","direction":"request",
        "request":{"evidenceId":"test","headers":[],"body":{"sourceId":"encoded-body","length":"20","availability":"complete"},"method":"POST","path":"/mapi/emsmdb"}
    })).unwrap();
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
    assert_eq!(document.roots[0].value, Some(Value::Integer("1".into())));
    assert_eq!(document.roots[0].spans[0].offset, "4");
    assert_eq!(document.roots[0].spans[0].length, "4");
    assert_eq!(document.roots[0].spans[0].source_id, "mapi-derived.0");
    assert_eq!(document.sources[0].parents[0].source_id, "encoded-body");
    assert_eq!(document.sources[0].parents[0].offset, "8");
    assert_eq!(document.sources[0].parents[0].length, "12");
    assert!(!document.sources[0].exact_mapping);
}

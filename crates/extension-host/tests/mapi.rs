//! The generated static parser must work inside the ordinary network-free OS sandbox.
#![cfg(windows)]
use std::{collections::BTreeMap, sync::Arc};
use transmog_extension_supervisor::{HostConfig, Runner};
use transmog_extensions::{
    Invocation, Manifest, Snapshot, Value, VisualizationInput, VisualizationStep,
};

#[tokio::test]
async fn generated_mapi_parser_preserves_exact_values_without_restoring_runtime_capabilities() {
    let parser = include_bytes!("../../../extensions/mapi/generated/parser.js");
    let entry = r"
import './parser.js';
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
  return {kind:'document',document:{title:'MAPI compatibility',status:'complete',presentation:'tree',roots:[
    {id:'wide',name:'Exact integer',value:{kind:'integer',value:wide.toString()}},
    {id:'guid',name:'GUID',value:{kind:'guid',value:guid.toString()}},
    {id:'connect',name:'Connect',value:{kind:'string',value:result.FullString()}}
  ]}};
}
";
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"test.mapi.compiler","version":"1.0.0","name":"MAPI compiler compatibility",
        "visualizers":[{"id":"test","name":"Test","entry":"main.js","surfaces":["headers"],
            "inputs":{"request":{"headers":["x-requesttype"],"body":"none"},"response":{"headers":[],"body":"none"}},
            "matches":[{"direction":"request","headers":[{"direction":"request","name":"x-requesttype","test":{"op":"present"}}]}]}],
        "files":[{"path":"main.js","bytes":entry.len()},{"path":"parser.js","bytes":parser.len()}]
    })).unwrap();
    let snapshot = Snapshot::new(
        manifest,
        BTreeMap::from([
            ("main.js".into(), Arc::<[u8]>::from(entry.as_bytes())),
            ("parser.js".into(), Arc::<[u8]>::from(parser.as_slice())),
        ]),
    )
    .unwrap();
    let runner = Runner::start(
        &HostConfig {
            executable: env!("CARGO_BIN_EXE_transmog-extension-host").into(),
        },
        snapshot,
        "test",
    )
    .unwrap();
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

// This module runs only inside an explicitly activated visualizer worker.
import '../generated/parser.js';

const MAX_INPUT=8*1024*1024,MAX_BLOCKS=50000,MAX_DEPTH=48;
let active;
const typeName=type=>Transpose.Reflection.getTypeFullName(type);
const originalEnsure=BlockParser.Block.prototype.EnsureParsed;
BlockParser.Block.prototype.EnsureParsed=function(){
  if(active&&this.parser){
    if(++active.parseSteps>100000)throw new Error('MAPI parser work limit');
    let origin=active.parsers.get(this.parser);
    if(!origin){origin=active.root;active.parsers.set(this.parser,origin);}
    active.blocks.set(this,origin);
  }
  return originalEnsure.call(this);
};
for(const method of ['ParseHTTPPayload','ParseHTTPExecuteResponsePayload']){
  const original=MapiInspector.MAPIParser[method];
  MapiInspector.MAPIParser[method]=function(session,direction,...rest){
    const previous=active?.root;
    if(active)active.root=active.messages.get(session.id)?.[direction===0?'request':'response'];
    try{return original.call(this,session,direction,...rest);}
    finally{if(active)active.root=previous;}
  };
}
BlockParser.PortableEvidence.OnPayload=(parser,encoded,bytes,transformed,flags)=>{
  if(!active)return;
  const parent=active.blocks.get(encoded)??active.root;
  if(!parent)throw new Error('Missing payload evidence');
  if(!transformed){active.parsers.set(parser,parent);return;}
  if(active.derived.length>=128)throw new Error('Derived source limit');
  const id='mapi-derived.'+active.derived.length;
  const transforms=[];if(flags&2)transforms.push('MAPI XOR 0xA5');if(flags&1)transforms.push('MAPI LZ77');
  const source={id,bytes:Array.from(bytes),parents:[{sourceId:parent.id,offset:encoded.Offset.toString(),length:encoded.Size.toString()}],
    transform:transforms.join(' then '),exactMapping:!(flags&1)};
  active.derived.push(source);
  active.parsers.set(parser,{id,length:bytes.length});
};

function clear(collection){if(typeof collection?.clear==='function')collection.clear();else collection?.Clear?.();}
function reset(){
  const parser=MapiInspector.MAPIParser;
  const partial=MAPIInspector.Parsers.Partial;
  partial.ResetPartialParameters();partial.ResetPartialContextInformation();
  for(const side of ['Get','Put','PutExtend']){
    for(const field of ['Session','ServerUrl','ProcessName','ClientInfo'])partial['Partial'+side+field]=null;
    partial['IsOneMore'+side+'ByteToRead']=false;partial['OneMore'+side+'Byte']=0;
  }
  parser.ResetHandleInformation();
  for(const key of ['TargetHandle','ContextInformationCollection','AllRopsList','BuffersIsCompressed','requestBytesForHexview','responseBytesForHexview'])clear(parser[key]);
  parser.IsLooperCall=false;parser.NeedToParseCROPSLayer=false;parser.IsOnlyGetServerHandle=false;
  parser.InputPayLoadCompressedXOR=null;parser.OutputPayLoadCompressedXOR=null;parser.AuxPayLoadCompressedXOR=null;parser.BaseHeaders=null;
  parser.ParsingSession=null;
  new MAPIInspector.Parsers.DecodingContext();
}
function children(block){
  const result=[],iterator=TransposeR.getEnumerator(block.Children);
  try{while(iterator.moveNext())result.push(iterator.current);}finally{TransposeR.dispose(iterator);}
  return result;
}
function scalar(block){
  let current=block;
  for(let depth=0;depth<4&&current&&!Transpose.is(current,BlockParser.IBlockT);depth++)current=current.Value??current.value;
  if(!current?.Parsed||!Transpose.is(current,BlockParser.IBlockT))return {};
  const type=Transpose.getType(current).$typeArguments[0],data=Transpose.unbox(current.Data),name=typeName(type);
  let raw=name==='System.Boolean'?(data?'True':'False'):name==='System.Char'?String.fromCharCode(Transpose.unbox(data)):data.toString();
  if(Transpose.Reflection.isEnum(type))return {valueType:name,raw,enumName:System.Enum.toString(type,data),flags:Transpose.Reflection.isFlags(type)};
  return {valueType:name,raw};
}
function filetime(raw){
  const ticks=BigInt(raw);
  if(ticks>2650467743999999999n)return null;
  const relative=ticks-116444736000000000n;
  const millis=relative/10000n-(relative%10000n<0n?1n:0n);
  return new Date(Number(millis)).toISOString().replace(/\.\d{3}Z$/,'.'+String(ticks%10000000n).padStart(7,'0')+'Z');
}
function sourceOf(block,depth=0){
  if(++active.sourceLookups>MAX_BLOCKS*4)throw new Error('Parsed structure limit');
  const known=active.blocks.get(block);if(known)return known;
  if(depth>MAX_DEPTH)throw new Error('Parsed structure limit');
  let source;
  for(const child of children(block)){
    const next=sourceOf(child,depth+1);if(!next)continue;
    if(source&&source.id!==next.id){source={id:null,length:0};break;}
    source=next;
  }
  if(source)active.blocks.set(block,source);
  return source;
}
function project(block,inherited,depth=0){
  if(!block)return null;
  if(++active.count>MAX_BLOCKS||depth>MAX_DEPTH)throw new Error('Parsed structure limit');
  const actual=Transpose.getType(block),type=typeName(actual.$genericTypeDefinition??actual),origin=sourceOf(block)??inherited;
  const value=scalar(block),absent=(type==='MAPIInspector.Parsers.PtypString'||type==='MAPIInspector.Parsers.PtypString8')&&block.HasValue?.Data===false;
  const result={type,text:absent?'No value':block.Text??null,parsed:block.Parsed,offset:block.Offset.toString(),size:block.Size.toString(),
    valueType:null,raw:null,...value,sourceId:origin?.id??null,
    children:(absent?[block.HasValue]:children(block)).map(child=>project(child,origin,depth+1))};
  if(absent)result.children[0].text='HasValue: False';
  if(type==='MAPIInspector.Parsers.PtypTime'&&value.raw!==undefined)result.timestamp=filetime(value.raw);
  let string=block;
  for(let level=0;level<4&&string&&!Transpose.is(string,BlockParser.BlockString);level++)string=string.Value??string.value;
  if(string&&Transpose.is(string,BlockParser.BlockString)){
    result.stringValue=string.Data;result.charset=Transpose.is(string,BlockParser.BlockStringW)?'utf-16le':'us-ascii';
  }
  return result;
}
function session(message){
  const result=new Fiddler.Session();result.id=message.id;result.LocalProcess='bounded-conversation';
  result.RequestHeaders.RequestPath=message.path??'/mapi/emsmdb';
  result.RequestHeaders.setItem('Content-Type','application/mapi-http');
  result.RequestHeaders.setItem('X-RequestType',message.requestType);
  result.RequestHeaders.setItem('X-ClientInfo',message.clientInfo??'synthetic-client');
  result.ResponseHeaders.setItem('Content-Type',message.responseType??'application/mapi-http');
  result.ResponseHeaders.setItem('X-RequestType',message.requestType);
  result.ResponseHeaders.setItem('X-ResponseCode',message.responseCode??'0');
  result.requestBodyBytes=System.Array.init(Array.from(message.request?.bytes??[]),System.Byte);
  result.responseBodyBytes=System.Array.init(Array.from(message.response?.bytes??[]),System.Byte);
  return result;
}

/** Decode only host-granted, already transport-decoded data. No capture enumeration. */
export function decode(messages,selectedId,direction,options={}){
  if(active)throw new Error('Reentrant MAPI invocation');
  if(direction!=='request'&&direction!=='response')throw new Error('Invalid message direction');
  if(!messages.length||messages.length>257)throw new Error('Context limit');
  const identities=new Set(),origins=new Map();let bytes=0;
  for(const message of messages){
    if(!Number.isSafeInteger(message.id)||message.id<1||identities.has(message.id))throw new Error('Invalid message identity');
    identities.add(message.id);
    const sides={};
    for(const side of ['request','response'])if(message[side]){
      const input=message[side];bytes+=input.bytes.length;
      if(bytes>MAX_INPUT||!input.sourceId)throw new Error('Input limit');
      sides[side]={id:input.sourceId,length:input.bytes.length};
    }
    origins.set(message.id,sides);
  }
  if(!identities.has(selectedId))throw new Error('Missing selected exchange');
  reset();
  active={parsers:new WeakMap(),blocks:new WeakMap(),root:origins.get(selectedId)[direction],messages:origins,derived:[],count:0,sourceLookups:0,parseSteps:0};
  try{
    const sessions=messages.map(session),selected=sessions.find(item=>item.id===selectedId);
    MapiInspector.SessionExtensions.AllSessionsNavigator.Init(System.Array.init(sessions,Fiddler.Session));
    MapiInspector.MAPIParser.ParsingSession=selected;
    let result;
    if(options.type){
      if(options.prepareRequest)MapiInspector.MAPIParser.ParseRequestMessage(selected,{v:null});
      MapiInspector.MAPIParser.ParsingSession=selected;
      const type=options.type.split('.').reduce((value,key)=>value?.[key],globalThis);
      if(!type)throw new Error('Unknown parser type');
      result=Transpose.createInstance(type,options.arguments??[]);
      const parser=new BlockParser.BinaryParser.$ctor1(direction==='request'?selected.requestBodyBytes:selected.responseBodyBytes);
      parser.Offset=options.offset??0;
      if(options.count)parser.PushCap(options.count);
      result.Parse(parser);
    }else{
      const output={v:null};
      result=direction==='request'?MapiInspector.MAPIParser.ParseRequestMessage(selected,output):MapiInspector.MAPIParser.ParseResponseMessage(selected,output);
    }
    return {tree:project(result,active.root),derived:active.derived};
  }finally{active=null;reset();MapiInspector.SessionExtensions.AllSessionsNavigator.Init(System.Array.init([],Fiddler.Session));}
}

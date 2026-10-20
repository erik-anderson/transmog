import {decode} from './protocol.js';
import {header,notice} from './shared.js';
import {view} from './view.js';

const MAX_BYTES=8*1024*1024,MAX_CONTEXT=64;
let state,epoch=0;
const order=input=>BigInt(input.requestNumber);
const title=()=> 'MAPI/HTTP '+(state.operation??'body');
function warning(code,message){if(!state.diagnostics.some(item=>item.code===code))state.diagnostics.push({code,message});}
function problem(source,code,message){source.skipped=true;source.issue={code,message};}
function diagnostics(){
  const result=[...state.diagnostics];
  for(const record of state.records.values())for(const source of Object.values(record.sides)){
    const issue=source?.issue??(source?.bytes&&source.reference.availability!=='complete'?{code:'body-incomplete',message:'Some retained body data is incomplete.'}:null);
    if(issue&&!result.some(item=>item.code===issue.code))result.push(issue);
  }
  return result;
}
function upsert(input){
  let record=state.records.get(input.exchangeId);
  if(!record){if(state.records.size>=MAX_CONTEXT+1)return;record={input,sides:{}};state.records.set(input.exchangeId,record);}
  for(const side of ['request','response']){
    const body=input[side]?.body,old=record.sides[side];
    const metadata=message=>JSON.stringify((message?.headers??[]).filter(field=>['content-type','content-encoding'].includes(field.name.toLowerCase())));
    if(old&&old.original.sourceId===body?.sourceId&&old.original.length===body.length&&old.original.availability===body.availability&&metadata(record.input[side])===metadata(input[side]))continue;
    if(old)state.bytes-=old.offset;
    record.sides[side]=body?{original:body,reference:body,decoded:false,offset:0,chunks:[],bytes:null,skipped:false}:null;
  }
  record.input=input;
}
function token(kind,target){const value='m.'+epoch+'.'+(++state.sequence);state.pending={token:value,kind,target};return value;}
function checked(value,kind){if(state.pending?.token!==value||state.pending.kind!==kind)throw new Error('Expired MAPI continuation');const pending=state.pending;state.pending=null;return pending.target;}
function records(){return [...state.records.values()].sort((left,right)=>order(left.input)<order(right.input)?-1:1);}
function task(){
  const selected=state.records.get(state.selected),others=records().filter(record=>record!==selected);
  for(const record of [selected,...others])for(const side of [state.direction,state.direction==='request'?'response':'request']){
    if(record===selected&&side!==state.direction&&state.operation!=='Execute')continue;
    const source=record.sides[side];
    if(source&&!source.skipped&&!source.bytes)return {record,side,source};
  }
  return null;
}
function pump(){
  for(let next=task();next;next=task()){
    const {record,side,source}=next,reference=source.reference;
    if(header(record.input[side],'content-type')?.split(';',1)[0].trim().toLowerCase()!=='application/mapi-http'){
      problem(source,'content-type-unavailable','Some paired or related content is not declared as MAPI/HTTP.');continue;
    }
    if(!source.decoded){
      const encoding=header(record.input[side],'content-encoding');
      if(encoding&&encoding.trim().toLowerCase()!=='identity')return {kind:'needDecoded',sourceId:reference.sourceId,token:token('decoded',next)};
      // A redacted coding must not be mistaken for identity encoding.
      if((record.input[side]?.headers??[]).some(field=>field.name.toLowerCase()==='content-encoding'&&(field.redacted||!field.value))){
        problem(source,'http-coding-unavailable','A content-encoding header is unavailable or redacted. Encoded bytes were not interpreted as MAPI.');continue;
      }
      source.decoded=true;
    }
    const length=BigInt(reference.length);
    if(length-BigInt(source.offset)>BigInt(MAX_BYTES-state.bytes)){
      problem(source,'input-limit','Retained input exceeds the 8 MiB MAPI snapshot limit.');continue;
    }
    if(length===0n){source.bytes=[];continue;}
    if(!['complete','capturing','truncated'].includes(reference.availability)){
      problem(source,'body-unavailable','Some requested body data is unavailable.');continue;
    }
    return {kind:'needInput',token:token('bytes',next),reads:[{sourceId:reference.sourceId,offset:String(source.offset),maxBytes:Math.min(65536,Number(length)-source.offset)}]};
  }
  if(state.historyRequested&&!state.historyDone){
    return {kind:'needContext',token:token('context',null),request:{position:'before',cursor:state.cursor,limit:Math.min(32,MAX_CONTEXT-(state.records.size-1))}};
  }
  return render();
}
function contextNeeded(tree){
  if(!tree)return true;
  return tree.type==='BlockParser.BlockException'||/Rop(?:FastTransfer|QueryRows|FindRow|ExpandRow|GetPropertiesSpecific|WritePerUserInformation|SetMessageReadFlag)/.test(tree.type)
    ||tree.children.some(contextNeeded);
}
function render(){
  const selected=state.records.get(state.selected),body=selected.sides[state.direction];
  if(!body?.bytes)return notice(title(),body?.issue?.code==='input-limit'?'limited':'unavailable',body?.issue?.message??'The selected body is unavailable within the visualizer input limit.');
  if(body.bytes.length===0)return notice(title(),body.reference.availability==='complete'?'partial':'unavailable','The selected body has no retained bytes yet. Live updates can follow it while this visualizer remains active.');
  const ordered=records(),messages=ordered.map((record,index)=>({id:index+1,requestType:header(record.input.request,'x-requesttype')??'',
    path:record.input.request?.path??'/mapi/emsmdb',clientInfo:header(record.input.request,'x-clientinfo')??'',
    responseCode:header(record.input.response,'x-responsecode')??'',responseType:header(record.input.response,'content-type')?.split(';',1)[0]??'',
    ...Object.fromEntries(['request','response'].filter(side=>record.sides[side]?.bytes).map(side=>[side,{sourceId:record.sides[side].reference.sourceId,bytes:record.sides[side].bytes}]))}));
  const selectedId=ordered.indexOf(selected)+1;
  let decoded;
  try{decoded=decode(messages,selectedId,state.direction);}
  catch{
    if(state.operation==='Execute'&&!state.historyRequested){state.historyRequested=true;return pump();}
    return notice(title(),'partial','MAPI decoding could not complete with the bounded retained input. Inspect the raw body or open a fresh visualizer after more traffic is retained.');
  }
  if(state.operation==='Execute'&&!state.historyRequested&&contextNeeded(decoded.tree)){
    state.historyRequested=true;return pump();
  }
  const origins=[];
  for(const record of ordered)for(const side of ['request','response']){
    const source=record.sides[side];if(source?.bytes)origins.push([source.reference.sourceId,source.bytes.length]);
  }
  state.presentation=view(decoded,origins,title(),diagnostics(),state.presentation);
  return state.presentation.step;
}

export function visualize(input){
  epoch++;
  state={selected:input.exchangeId,direction:input.direction,operation:header(input.request,'x-requesttype'),records:new Map(),revisions:new Map(),
    sequence:0,pending:null,bytes:0,diagnostics:[],historyRequested:false,historyDone:false,cursor:null,presentation:null};
  upsert(input);
  if(!state.operation)return notice('MAPI/HTTP','unavailable','The request operation is unavailable or redacted.');
  const responseCode=header(input.response,'x-responsecode');
  if(input.direction==='response'&&!responseCode)return notice(title(),'unavailable','The MAPI response status is unavailable or redacted. Inspect the retained headers and raw body.');
  if(input.direction==='response'&&responseCode&&responseCode!=='0')return notice(title(),'partial','The server returned X-ResponseCode '+responseCode.slice(0,64)+'. Inspect the retained headers and raw body for details.');
  return pump();
}
export function resume(value,pages){
  const {source}=checked(value,'bytes');
  if(pages.length!==1||pages[0].sourceId!==source.reference.sourceId||pages[0].offset!==String(source.offset)||!pages[0].bytes.length)throw new Error('Incomplete MAPI byte page');
  const bytes=Array.from(pages[0].bytes);
  if(bytes.length>Math.min(65536,Number(source.reference.length)-source.offset))throw new Error('Invalid MAPI byte page');
  source.chunks.push(bytes);source.offset+=bytes.length;state.bytes+=bytes.length;
  if(source.offset===Number(source.reference.length)){
    source.bytes=[];for(const chunk of source.chunks)for(const byte of chunk)source.bytes.push(byte);source.chunks=[];
  }
  return pump();
}
export function resumeDecoded(value,result){
  const {source}=checked(value,'decoded');source.decoded=true;
  if(result.kind==='ready')source.reference=result.decoded.body;
  else problem(source,'http-decoding-'+result.reason,'HTTP content decoding is '+result.reason+'. Encoded bytes were not interpreted as MAPI.');
  return pump();
}
export function resumeContext(value,page){
  checked(value,'context');
  const anchor=order(state.records.get(state.selected).input);
  for(const exchange of page.exchanges)if(order(exchange.input)<anchor&&header(exchange.input.request,'x-requesttype')==='Execute')upsert(exchange.input);
  state.cursor=page.nextCursor??null;
  state.historyDone=page.status!=='more'||!state.cursor||state.records.size>=MAX_CONTEXT+1;
  if(page.status==='unavailable'||page.status==='limited'||page.status==='more'&&state.records.size>=MAX_CONTEXT+1)warning('context-'+page.status,'Some earlier conversation data is unavailable within the retained context limit.');
  return pump();
}
export function expand(value){if(!state?.presentation)throw new Error('No MAPI structure');return state.presentation.expand(value);}
export function onTraffic(batch,info){
  if(!state)throw new Error('No MAPI instance');
  if(info.missedHints!=='0')warning('missed-live-data','Some live updates were coalesced or missed. Open a fresh visualizer to rebuild retained context.');
  const anchor=order(state.records.get(state.selected).input);let changed=info.missedHints!=='0';
  for(const event of batch){
    if(order(event.input)>anchor||header(event.input.request,'x-requesttype')!=='Execute'&&event.exchangeId!==state.selected)continue;
    const revision=BigInt(event.revision),previous=state.revisions.get(event.exchangeId);
    if(previous!==undefined&&revision<=previous)continue;
    state.revisions.set(event.exchangeId,revision);upsert(event.input);changed=true;
  }
  return changed?pump():state.presentation?.step??pump();
}

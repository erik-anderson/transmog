// Synthetic native acceptance fixture. Executed only in the production sandbox.
let parsed;
const document = (roots, sources=[]) => ({kind:'document',document:{title:'Synthetic protocol',status:'complete',presentation:'tree',summary:'Native sandbox and retained-evidence acceptance fixture.',roots,sources,diagnostics:[]}});
export function visualize(input) {
  if(typeof fetch!=='undefined'||typeof WebSocket!=='undefined'||typeof Deno!=='undefined'||typeof process!=='undefined')throw new Error('Unexpected ambient capability');
  const body=input.response?.body;
  if(body&&BigInt(body.length)>0n)return {kind:'needInput',token:'body-page',reads:[{sourceId:body.sourceId,offset:'0',maxBytes:Number(BigInt(body.length)<64n?BigInt(body.length):64n)}]};
  const headers=input.direction==='request'?input.request?.headers:input.response?.headers;
  return document((headers??[]).map(header=>({id:'header-'+header.index,name:header.name,value:{kind:'string',value:header.redacted?'[redacted]':String.fromCharCode(...header.value)},format:{charset:'us-ascii'},children:[],spans:header.sourceId?[{sourceId:header.sourceId,offset:'0',length:String(header.value.length)}]:[]})));
}
export function resume(token,pages) {
  if(token!=='body-page')throw new Error('Unknown continuation');
  const page=pages[0],span={sourceId:page.sourceId,offset:page.offset,length:String(page.bytes.length)};
  parsed=document([{id:'body-text',name:'Body text',value:{kind:'string',value:String.fromCharCode(...page.bytes)},format:{charset:'us-ascii',protocol:'Synthetic protocol'},spans:[span],children:[],childrenToken:'fields'}]);
  if(parsed.document.roots[0].value.value==='Native final response')parsed.document.updates='final';
  return parsed;
}
export function expand(token) {
  if(token!=='fields'||!parsed)throw new Error('Unknown expansion');
  const node=parsed.document.roots[0];node.childrenToken=null;
  // Omission in later documents must preserve the host's terminal update policy.
  delete parsed.document.updates;
  node.children=[{id:'wide-integer',name:'Exact integer',value:{kind:'integer',value:'9007199254740993'},format:{byteOrder:'little-endian',radix:16},spans:[],children:[]}];
  return parsed;
}
export function onTraffic(events) {
  return events.length?visualize(events[events.length-1].input):(parsed??document([]));
}

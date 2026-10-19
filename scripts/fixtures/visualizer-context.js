// Synthetic native acceptance: all other origins use the same correlation header and process.
const document=()=>({kind:'document',document:{title:'Related context',status:'complete',presentation:'tree',roots:[{id:'count',name:'Same-origin exchanges',value:{kind:'integer',value:'1'}}]}});
export function visualize() {return {kind:'needContext',token:'related',request:{position:'after',cursor:null,limit:8}};}
export function resumeContext(token,page) {
  if(token!=='related'||page.exchanges.length!==1||page.status!=='complete')throw new Error('Context crossed a native scope');
  const input=page.exchanges[0].input;
  if(input.request.headers.some(header=>header.name.toLowerCase()!=='x-example')||input.request.body)throw new Error('Context widened input grants');
  return {kind:'needInput',token:'related-body',reads:[{sourceId:input.response.body.sourceId,offset:'0',maxBytes:1}]};
}
export function resume(token,pages) {
  if(token!=='related-body'||pages[0].bytes[0]!==65)throw new Error('Context returned other-origin body bytes');
  return document();
}

// Small dependency-free helpers shared with the header-only contribution.
export function header(message,name){
  const fields=(message?.headers??[]).filter(field=>field.name.toLowerCase()===name);
  if(!fields.length||fields.some(field=>field.redacted||!field.value))return null;
  return fields.map(field=>latin1(field.value)).join(', ');
}
export function latin1(bytes){
  let result='';for(let offset=0;offset<bytes.length;offset+=4096)result+=String.fromCharCode(...bytes.slice(offset,offset+4096));return result;
}
export function base64(bytes){
  const alphabet='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/',parts=[];
  let part='';
  for(let offset=0;offset<bytes.length;offset+=3){
    const first=bytes[offset],second=bytes[offset+1],third=bytes[offset+2];
    part+=alphabet[first>>>2]+alphabet[((first&3)<<4)|((second??0)>>>4)]+(second===undefined?'=':alphabet[((second&15)<<2)|((third??0)>>>6)])+(third===undefined?'=':alphabet[third&63]);
    if(part.length>=4096){parts.push(part);part='';}
  }
  parts.push(part);return parts.join('');
}
export function notice(title,status,message){return {kind:'document',document:{title,status,presentation:'tree',summary:message,roots:[],diagnostics:[{code:status,message}]}};}

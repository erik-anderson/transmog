// The same sandboxed parser consumes a separately granted HTTP representation.
import {visualize as parse, resume as parseResume, expand as parseExpand} from './parser.js';
let original;
export function visualize(input) {
  original=input;
  return {kind:'needDecoded',sourceId:input.response.body.sourceId,token:'http-body'};
}
export function resumeDecoded(token,result) {
  if(token!=='http-body')throw new Error('Unexpected HTTP continuation');
  if(!Object.isFrozen(result)||!Object.isFrozen(result.decoded))throw new Error('Mutable decoded metadata');
  if(result.kind!=='ready')return {kind:'document',document:{title:'HTTP decoding unavailable',status:'partial',presentation:'tree',roots:[],sources:[],diagnostics:[{code:result.reason,message:'The encoded source remains available.'}]}};
  return parse({...original,response:{...original.response,body:result.decoded.body}});
}
export function resume(token,pages) {return parseResume(token,pages);}
export function expand(token) {return parseExpand(token);}

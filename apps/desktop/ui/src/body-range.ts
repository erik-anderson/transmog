import {invoke} from '@tauri-apps/api/core';
import type {BodyBytePage,StoredBodyMetadata} from './models.js';

const MAX_METADATA_BYTES=256*1024;
const MAX_BODY_READ_BYTES=16*1024*1024;
const U64_MAX=(1n<<64n)-1n;
const exactOffset=(value:unknown):value is string=>typeof value==='string'&&/^(0|[1-9][0-9]{0,19})$/.test(value)&&BigInt(value)<=U64_MAX;
const record=(value:unknown):value is Record<string,unknown>=>typeof value==='object'&&value!==null&&!Array.isArray(value);
const count=(value:unknown):value is number=>typeof value==='number'&&Number.isInteger(value)&&value>=0;
const optionalText=(value:unknown):boolean=>value===null||typeof value==='string';
const metadata=(value:unknown):value is StoredBodyMetadata=>record(value)&&
  typeof value.lengthKnown==='boolean'&&typeof value.exchangeId==='string'&&typeof value.boundary==='string'&&
  count(value.observedBytes)&&count(value.retainedBytes)&&(value.wireBodyBytes===null||count(value.wireBodyBytes))&&
  typeof value.availability==='string'&&['unavailable','capturing','complete','truncated','lost','evicted','disabled','quota-omitted'].includes(value.availability)&&
  ['mediaType','charset','sha256','reason'].every(key=>optionalText(value[key]))&&
  Array.isArray(value.contentCodings)&&value.contentCodings.every(coding=>typeof coding==='string');

/** Decode the Tauri-only frame; bytes borrow the ArrayBuffer without a base64 step or copy. */
export function decodeBodyPage(buffer:ArrayBuffer,offset:string,maxBytes:number):BodyBytePage {
  const invalid=():never=>{throw new Error('The returned body byte page is invalid');};
  if(!(buffer instanceof ArrayBuffer)||buffer.byteLength<4||buffer.byteLength>4+MAX_METADATA_BYTES+MAX_BODY_READ_BYTES)return invalid();
  const size=new DataView(buffer).getUint32(0,true);
  if(size>MAX_METADATA_BYTES||size>buffer.byteLength-4)return invalid();
  let header:unknown;
  try {header=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(new Uint8Array(buffer,4,size)));}catch{return invalid();}
  if(!record(header)||!metadata(header.metadata)||!exactOffset(header.offset)||!exactOffset(header.totalBytes)||header.offset!==offset||
    !count(header.readLimit)||header.readLimit<1||header.readLimit>MAX_BODY_READ_BYTES)return invalid();
  const bytes=new Uint8Array(buffer,4+size),start=BigInt(header.offset),total=BigInt(header.totalBytes);
  if(!Number.isInteger(maxBytes)||maxBytes<1||maxBytes>MAX_BODY_READ_BYTES||bytes.length>maxBytes||bytes.length>header.readLimit||
    start>total||BigInt(bytes.length)>total-start||Number(total)!==header.metadata.retainedBytes||(!bytes.length&&start<total))return invalid();
  return {metadata:header.metadata,totalBytes:header.totalBytes,offset:header.offset,readLimit:header.readLimit,bytes};
}

export async function readBodyRange(sessionId:string,boundary:string,offset:string,maxBytes:number):Promise<BodyBytePage> {
  const buffer=await invoke<ArrayBuffer>('read_body_range',{request:{sessionId,boundary,offset,maxBytes}});
  const page=decodeBodyPage(buffer,offset,maxBytes);
  if(page.metadata.exchangeId!==sessionId||page.metadata.boundary!==boundary)throw new Error('The body byte page did not match its requested source');
  return page;
}

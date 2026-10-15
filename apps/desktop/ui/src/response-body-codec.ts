import {encodeEditedText} from './utilities.js';

export type BodyMode='text'|'hex';
export const MAX_RESPONSE_BODY_BYTES=16*1024*1024;

/** Hex contains bytes only: offsets and ASCII columns are deliberately not editable. */
export function bytesToHex(bytes:Uint8Array|readonly number[]):string {
  const rows:string[]=[];
  for(let offset=0;offset<bytes.length;offset+=16)rows.push(Array.from(bytes.slice(offset,offset+16),byte=>byte.toString(16).padStart(2,'0')).join(' '));
  return rows.join('\n');
}

export function parseHexBody(value:string):number[] {
  if(!/^[\da-f\s]*$/i.test(value))throw new Error('Enter hexadecimal byte pairs (00–FF), separated by spaces or new lines.');
  const compact=value.replace(/\s/g,'');
  if(compact.length%2)throw new Error('Each hexadecimal byte needs two digits. Complete the final byte before applying.');
  if(compact.length/2>MAX_RESPONSE_BODY_BYTES)throw new Error('Body exceeds the 16 MiB editor limit. Serve it from a file instead.');
  const bytes:number[]=[];
  for(let index=0;index<compact.length;index+=2)bytes.push(Number.parseInt(compact.slice(index,index+2),16));
  return bytes;
}

export function encodeBody(value:string,mode:BodyMode,encoding:string|null):number[] {
  if(mode==='text'&&encoding===null)throw new Error('This body has an unsupported text encoding. Continue editing in Hex.');
  // TextEncoder silently replaces lone surrogates, which is never appropriate for an editor.
  if(mode==='text'&&!value.isWellFormed())throw new Error('The body contains an unpaired Unicode surrogate.');
  const bytes=mode==='hex'?parseHexBody(value):encodeEditedText(value,encoding!);
  if(bytes.length>MAX_RESPONSE_BODY_BYTES)throw new Error('Body exceeds the 16 MiB editor limit. Serve it from a file instead.');
  return bytes;
}

export function bodyByteLength(value:string,mode:BodyMode,encoding:string|null):number {return encodeBody(value,mode,encoding).length;}

export function equalBodyBytes(left:readonly number[],right:readonly number[]):boolean {
  return left.length===right.length&&left.every((byte,index)=>byte===right[index]);
}

export function decodeBodyBase64(value:string):number[] {return Array.from(atob(value),character=>character.charCodeAt(0));}

/** Only offer Text when decoding and re-encoding preserve every byte. */
export function renderBodyBytes(bytes:readonly number[],mode:BodyMode,encoding:string|null):string {
  if(mode==='hex')return bytesToHex(bytes);
  if(encoding===null)throw new Error('This body has an unsupported text encoding. Continue editing in Hex.');
  const codec=encoding,hasBom=codec.endsWith('-bom');
  let text:string;
  try {
    if(codec.startsWith('utf-32')) {
      const start=hasBom?4:0;
      if((bytes.length-start)%4!==0)throw new Error();
      const view=new DataView(Uint8Array.from(bytes).buffer),parts:string[]=[];
      for(let offset=start;offset<bytes.length;offset+=4){const point=view.getUint32(offset,codec.includes('le'));if(point>0x10ffff||(point>=0xd800&&point<=0xdfff))throw new Error();parts.push(String.fromCodePoint(point));}
      text=parts.join('');
    } else {
      const name=codec==='us-ascii'?'utf-8':codec.replace(/-bom$/,'');
      text=new TextDecoder(name,{fatal:true,ignoreBOM:!hasBom}).decode(Uint8Array.from(bytes));
    }
    if(/[\u0000-\u0008\u000b\u000e-\u001f\u007f-\u009f]/u.test(text)||!equalBodyBytes(encodeBody(text,'text',codec),bytes))throw new Error();
  }catch{throw new Error('These bytes cannot be shown as lossless Unicode text. Continue editing in Hex.');}
  return text;
}

export function changeBodyMode(value:string,from:BodyMode,to:BodyMode,encoding:string|null):string {
  return from===to?value:renderBodyBytes(encodeBody(value,from,encoding),to,encoding);
}

import {invoke} from '@tauri-apps/api/core';
import {decodeBytes} from './hex-formats.js';
import type {HexPreview} from './hex-viewer.js';
import type {BodyBytePage} from '../models.js';
import {describeError} from '../utilities.js';

const MAX_PAGES=8;
export const HEX_COPY_BYTES=16*1024*1024;
class ObsoleteRange extends Error {}

/** Keeps visible byte pages bounded; logical offsets stay exact for the whole body. */
export class HexByteSource {
  length:bigint;
  readonly pageBytes:number;
  error='';
  loading=false;
  private inline:Uint8Array|null;
  private pages=new Map<string,Uint8Array>();
  private desired:{first:bigint;last:bigint}|null=null;
  private pumping=false;
  private live=true;
  private serial:Promise<unknown>=Promise.resolve();
  private revision=0;

  constructor(private preview:HexPreview,private changed:()=>void) {
    const bytes=decodeBytes(preview.bytesBase64);
    this.inline=preview.source?null:bytes;
    this.length=preview.source?BigInt(preview.source.totalBytes):BigInt(bytes.length);
    this.pageBytes=Math.min(65536,preview.source?.readLimit??65536);
    if(preview.source)this.pages.set('0',bytes.subarray(0,this.pageBytes));
  }
  update(preview:HexPreview):void {
    this.revision++;this.error='';
    this.preview=preview;
    if(!preview.source){this.inline=decodeBytes(preview.bytesBase64);this.length=BigInt(this.inline.length);return;}
    const previous=this.length;this.length=BigInt(preview.source.totalBytes);
    if(this.length>previous)for(const [offset,page]of this.pages)if(page.length<this.pageBytes)this.pages.delete(offset);
    this.pages.set('0',decodeBytes(preview.bytesBase64).subarray(0,this.pageBytes));
    while(this.pages.size>MAX_PAGES)this.pages.delete(this.pages.keys().next().value!);
  }
  byte(index:bigint):number|undefined {
    if(index<0n||index>=this.length)return undefined;
    if(this.inline)return this.inline[Number(index)];
    const start=index/BigInt(this.pageBytes)*BigInt(this.pageBytes);
    return this.pages.get(start.toString())?.[Number(index-start)];
  }
  cached(first:bigint,length:bigint):Uint8Array|null {
    if(length<0n||length>BigInt(HEX_COPY_BYTES)||first<0n||first+length>this.length)return null;
    if(this.inline)return this.inline.subarray(Number(first),Number(first+length));
    const segments:Uint8Array[]=[];let position=first,left=Number(length);
    while(left){const start=position/BigInt(this.pageBytes)*BigInt(this.pageBytes),page=this.pages.get(start.toString());const offset=Number(position-start);if(!page||offset>=page.length)return null;const count=Math.min(left,page.length-offset);segments.push(page.subarray(offset,offset+count));left-=count;position+=BigInt(count);}
    const result=new Uint8Array(Number(length));let offset=0;for(const segment of segments){result.set(segment,offset);offset+=segment.length;}return result;
  }
  ensure(first:bigint,last:bigint):void {
    if(this.inline||!this.live||this.error)return;
    const size=BigInt(this.pageBytes);let missing=false;
    for(let offset=(first<0n?0n:first)/size*size;offset<last&&offset<this.length;offset+=size)if(!this.pages.has(offset.toString())){missing=true;break;}
    if(!missing)return;
    this.desired={first:first<0n?0n:first,last:last>this.length?this.length:last};
    if(!this.pumping)void this.pump();
  }
  private async pump():Promise<void> {
    this.pumping=true;
    try {
      while(this.live&&this.desired){
        const desired=this.desired;this.desired=null;
        const size=BigInt(this.pageBytes);
        for(let offset=desired.first/size*size;offset<desired.last;offset+=size){
          if(this.desired)break;
          if(this.pages.has(offset.toString()))continue;
          this.loading=true;this.changed();
          const page=await this.fetch(offset,this.pageBytes);
          if(!this.live)return;
          this.pages.set(offset.toString(),page);
          while(this.pages.size>MAX_PAGES)this.pages.delete(this.pages.keys().next().value!);
          this.error='';this.changed();
        }
      }
    } catch(error:unknown){if(this.live&&!(error instanceof ObsoleteRange)){this.error=describeError(error);this.desired=null;}}
    finally {this.pumping=false;this.loading=false;if(this.live)this.changed();}
  }
  private fetch(offset:bigint,length:number):Promise<Uint8Array> {
    const source=this.preview.source!;
    const revision=this.revision;
    const operation=this.serial.then(async()=>{
      if(!this.live)throw new Error('Byte view closed');
      if(revision!==this.revision)throw new ObsoleteRange();
      const page=await invoke<BodyBytePage>('read_body_range',{request:{sessionId:source.sessionId,boundary:source.boundary,offset:offset.toString(),maxBytes:length}});
      if(!this.live)throw new Error('Byte view closed');
      if(revision!==this.revision)throw new ObsoleteRange();
      if(BigInt(page.offset)!==offset)throw new Error('The byte range did not match its requested offset');
      this.length=BigInt(page.totalBytes);
      const bytes=decodeBytes(page.bytesBase64);
      if(bytes.length>length)throw new Error('The returned byte page exceeded its requested budget');
      if(!bytes.length&&offset<this.length)throw new Error('The requested byte range is unavailable');
      return bytes;
    });
    this.serial=operation.catch(()=>{});return operation;
  }
  async read(first:bigint,length:bigint,current:()=>boolean):Promise<Uint8Array> {
    if(first<0n||length<0n||first+length>this.length)throw new Error('Selected bytes are unavailable');
    const cached=this.cached(first,length);if(cached)return cached;
    if(length>BigInt(HEX_COPY_BYTES))throw new Error('This selection is too large for the clipboard. Use Save as to save the complete body.');
    const result=new Uint8Array(Number(length));let position=first,written=0;
    while(written<result.length){if(!current()||!this.live)throw new Error('Byte selection changed');const part=await this.fetch(position,Math.min(result.length-written,this.preview.source!.readLimit));const take=Math.min(part.length,result.length-written);if(!take)throw new Error('Selected bytes are unavailable');result.set(part.subarray(0,take),written);written+=take;position+=BigInt(take);}
    return result;
  }
  dispose():void {this.live=false;this.desired=null;this.pages.clear();this.inline=null;}
  retry():void {this.error='';this.changed();}
}

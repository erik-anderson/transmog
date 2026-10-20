import {invoke} from '@tauri-apps/api/core';
import {listen,type UnlistenFn} from '@tauri-apps/api/event';
import type {ExtensionTrafficColumn,TrafficClassification} from './models.js';

/** Only viewport metadata is requested. Values are fixed manifest labels. */
export class TrafficClassifications {
  choices:ExtensionTrafficColumn[]=[];
  error='';
  private generation=0;
  private connected=false;
  private lifetime=0;
  private loading=false;
  private unlisten:UnlistenFn|undefined;
  private cache=new Map<string,TrafficClassification>();
  private pending=new Set<string>();
  constructor(private selected:()=>string[],private changed:()=>void) {}
  get columns():ExtensionTrafficColumn[] {return this.selected().flatMap(key=>this.choices.filter(column=>column.key===key));}
  async connect():Promise<void> {
    if(this.connected)return;
    this.connected=true;
    const lifetime=++this.lifetime;
    try {const unlisten=await listen('extensions-changed',()=>{void this.refresh();});if(this.connected&&lifetime===this.lifetime)this.unlisten=unlisten;else unlisten();}
    catch { /* Refresh remains available in View. */ }
    if(this.connected&&lifetime===this.lifetime)await this.refresh();
  }
  disconnect():void {this.connected=false;this.lifetime++;this.generation++;this.unlisten?.();this.unlisten=undefined;this.cache.clear();this.pending.clear();}
  invalidate(ids?:string[]):void {this.generation++;this.pending.clear();if(ids){for(const id of ids)this.cache.delete(id);}else this.cache.clear();}
  async refresh():Promise<void> {
    this.invalidate();const generation=this.generation;this.choices=[];this.error='';this.changed();
    try {const choices=await invoke<ExtensionTrafficColumn[]>('extension_traffic_columns');if(this.connected&&generation===this.generation){this.choices=choices;this.changed();}}
    catch {if(this.connected&&generation===this.generation){this.error='Extension columns could not be loaded. Refresh to retry.';this.changed();}}
  }
  cells(id:string):Array<{key:string;text:string;title:string}> {
    const row=this.cache.get(id);
    return this.columns.map(column=>{const text=row?.limited?'Limited':row?(row.values[column.key]??[]).join(' · ')||'—':'…';return {key:column.key,text,title:row?.limited?'Header classification exceeded its metadata limit.':column.name+' · '+column.extensionName+' · '+text+(row?' · Fixed labels matched from retained headers':' · Loading header classifications')};});
  }
  async load(ids:string[]):Promise<void> {
    const keys=this.columns.map(column=>column.key);
    if(!this.connected||!keys.length||this.loading)return;
    const missing=ids.filter(id=>!this.cache.has(id)&&!this.pending.has(id)).slice(0,100);
    if(!missing.length)return;
    const generation=this.generation;
    this.loading=true;
    for(const id of missing)this.pending.add(id);
    try {
      const rows=await invoke<TrafficClassification[]>('classify_traffic_rows',{ids:missing,columns:keys});
      if(!this.connected||generation!==this.generation)return;
      const returned=new Map(rows.map(row=>[row.id,row]));
      for(const id of missing){this.cache.delete(id);this.cache.set(id,returned.get(id)??{id,values:{},limited:false});}
      while(this.cache.size>1024)this.cache.delete(this.cache.keys().next().value!);
      this.error='';
    } catch {if(this.connected&&generation===this.generation){this.error='Header classifications could not be loaded. Refresh to retry.';for(const id of missing)this.cache.set(id,{id,values:{},limited:true});}}
    finally {this.loading=false;if(generation===this.generation)for(const id of missing)this.pending.delete(id);if(this.connected)this.changed();}
  }
}

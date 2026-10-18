import { observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { SessionDetail, StreamMessage, StreamPage, StreamRow, StreamSummary } from '../models.js';
import type { HexPreview } from '../hex-viewer/hex-viewer.js';
import { describeError } from '../utilities.js';
import {formatDuration,lifetimeDescription, formatBytes } from '../table-model.js';

interface Row extends StreamRow { time:string; size:string; label:string; selected:string; tabIndex:number; }
export class StreamInspector extends WorkspaceElement {
  @observable detail:SessionDetail|null=null;
  @observable streamRows:Row[]=[];
  @observable streamStatus='';
  @observable streamNotice='';
  @observable streamLifetimeText='';
  @observable streamHttpLabel='Handshake';
  @observable streamHttpTiming='';
  @observable streamTimingNote='';
  @observable streamError='';
  @observable streamTotal=0;
  @observable streamFollow=true;
  @observable streamControls=true;
  @observable streamSender='';
  @observable streamSearch='';
  @observable streamSelected=0;
  @observable streamReading=false;
  showMessages():void {this.streamReading=false;this.$flushUpdates();this.streamPanelScroller.scrollTop=0;this.focusSelected();}
  @observable streamView='text';
  @observable streamText='Select a message to inspect its payload.';
  @observable streamFacts='';
  @observable streamPayload:StreamMessage|null=null;
  @observable streamHex:HexPreview|null=null;
  @observable streamTop='0px';
  @observable streamBottom='0px';
  @observable streamBusy=false;
  streamScroller!:HTMLDivElement;
  streamPanelScroller!:HTMLElement;
  streamPayloadPane!:HTMLDivElement;
  streamSearchInput!:HTMLInputElement;
  private sessionId='';
  private generation=0;
  private payloadGeneration=0;
  private offset=0;
  private timer:number|undefined;
  private loading=false;
  private again=false;
  private anchor:number|null=null;
  private programmatic=false;
  private keyTarget:number|null=null;
  private focusSelected():void {(this.getRootNode() as ShadowRoot).getElementById('stream-item-'+this.streamSelected)?.focus();}
  detailChanged():void {
    if(this.sessionId!==(this.detail?.id??'')) {
      this.sessionId=this.detail?.id??'';this.generation++;this.payloadGeneration++;this.offset=0;this.anchor=null;
      this.streamSelected=0;this.streamReading=false;this.streamPayload=null;this.streamRows=[];this.streamTotal=0;this.streamFollow=true;
      this.streamText='Select a message to inspect its payload.';this.streamFacts='';this.streamHex=null;this.streamNotice='';this.streamError='';
    }
    this.describe(this.detail?.stream??null);this.schedule();
  }
  private describe(summary:StreamSummary|null):void {
    if(!summary){this.streamStatus='';return;}
    this.streamLifetimeText=summary.lifetime?.durationMs==null?'Unavailable':(summary.lifetime.live?'Live · ':summary.lifetime.boundary==='open-at-capture'?'Open at capture · ':summary.lifetime.boundary==='incomplete'?'Incomplete · ':'Final · ')+formatDuration(summary.lifetime.durationMs);
    this.streamHttpLabel=summary.state.protocol==='sse'?'Response headers':'Handshake';
    this.streamHttpTiming=formatDuration(summary.state.protocol==='sse'?summary.responseHeadersMs:summary.httpDurationMs);
    this.streamTimingNote=lifetimeDescription(summary.lifetime)+' '+(summary.state.protocol==='sse'?'Response headers measures request headers to upstream response headers.':'Handshake uses the original HTTP request interval.')+' HTTP timing stays separate from stream lifetime. Sent and received totals count decoded message bytes, including observed controls/events; they exclude HTTP handshake bodies, framing and transport overhead. Recorded totals can be incomplete when observations are missing; imported traffic keeps its saved totals.';
    const date=(value:number|null|undefined)=>value==null?'Unavailable':Number.isFinite(new Date(value).valueOf())?new Date(value).toISOString():'Outside supported calendar range';
    if(summary.lifetime)this.streamTimingNote+=' Established: '+date(summary.lifetime.establishedAt)+'. Ended: '+date(summary.lifetime.endedAt)+'. Observed until: '+date(summary.lifetime.observedUntil)+'. Times use the recorded source clock.';
    const state=summary.imported&&['connecting','open','closing'].includes(summary.state.status)?'Open at capture':summary.state.status;
    this.streamStatus=(summary.state.protocol==='sse'?'SSE':'WebSocket')+' · '+state+' · '+summary.observed.toLocaleString()+' items · Sent '+(summary.clientBytes==null?'Unavailable':formatBytes(summary.clientBytes))+' · Received '+(summary.serverBytes==null?'Unavailable':formatBytes(summary.serverBytes));
    this.streamNotice=(summary.evicted?summary.evicted.toLocaleString()+' older items evicted. ':'')+(summary.sequenceLoss?summary.sequenceLoss+' observation records missing. ':'')+(summary.state.reason??'');
  }
  private schedule():void {if(this.detail?.stream&&this.sessionId&&this.timer===undefined)this.timer=window.setTimeout(()=>{this.timer=undefined;void this.load();},100);}
  private async load():Promise<void> {
    if(!this.detail?.stream||!this.sessionId)return;
    if(this.loading){this.again=true;return;}
    this.loading=true;const generation=this.generation,id=this.sessionId;
    try {
      const page=await invoke<StreamPage>('stream_page',{query:{sessionId:id,offset:this.offset,limit:100,tail:this.streamFollow,anchor:this.anchor,sender:this.streamSender||null,controls:this.streamControls,search:this.streamSearch}});
      if(!this.isConnected||generation!==this.generation||id!==this.sessionId)return;
      this.offset=page.offset;this.streamTotal=page.total;this.anchor=page.rows[0]?.sequence??null;
      this.streamRows=page.rows.map(row=>({...row,selected:String(row.sequence===this.streamSelected),tabIndex:row.sequence===this.streamSelected||(!this.streamSelected&&row===page.rows[0])?0:-1,time:new Date(row.message.atMillis).toLocaleTimeString(undefined,{hour12:false})+'.'+String(row.message.atMillis%1000).padStart(3,'0'),size:formatBytes(row.message.byteCount),label:row.message.event??row.message.kind}));
      this.streamTop=page.offset*32+'px';this.streamBottom=Math.max(0,page.total-page.offset-page.rows.length)*32+'px';
      this.describe(page.summary);this.streamError='';this.$flushUpdates();
      if(this.keyTarget!==null){const row=this.streamRows[this.keyTarget-this.offset];this.keyTarget=null;if(row){void this.select(row);this.$flushUpdates();this.focusSelected();}}
      if(this.streamFollow&&this.streamScroller){this.programmatic=true;this.streamScroller.scrollTop=this.streamScroller.scrollHeight;requestAnimationFrame(()=>this.programmatic=false);}
    } catch(error:unknown){if(generation===this.generation)this.streamError=describeError(error);}
    finally {this.loading=false;if(this.again){this.again=false;this.schedule();}}
  }
  scrolled():void {
    if(this.programmatic||!this.streamScroller)return;
    const desired=Math.max(0,Math.floor(this.streamScroller.scrollTop/32)-8);
    if(this.streamScroller.scrollTop+this.streamScroller.clientHeight<this.streamScroller.scrollHeight-8)this.streamFollow=false;
    if(Math.abs(desired-this.offset)>20){this.offset=desired;this.anchor=null;this.schedule();}
  }
  follow():void {this.streamFollow=true;this.anchor=null;this.schedule();}
  filter(event:Event,field:string):void {
    if(field==='sender')this.streamSender=(event.currentTarget as HTMLSelectElement).value;
    else this.streamControls=(event.currentTarget as HTMLInputElement).checked;
    this.changedFilter();
  }
  search(event:Event):void {event.preventDefault();this.streamSearch=this.streamSearchInput.value;this.changedFilter();}
  private changedFilter():void {this.generation++;this.offset=0;this.anchor=null;if(this.streamScroller){this.programmatic=true;this.streamScroller.scrollTop=0;requestAnimationFrame(()=>this.programmatic=false);}this.schedule();}
  async select(row:StreamRow):Promise<void> {
    this.streamReading=true;this.streamSelected=row.sequence;this.streamRows=this.streamRows.map(item=>({...item,selected:String(item.sequence===row.sequence),tabIndex:item.sequence===row.sequence?0:-1}));this.streamFollow=false;this.streamPayload=null;this.streamHex=null;this.streamText='Loading message…';this.streamFacts='';
    this.$flushUpdates();if(this.streamScroller.clientHeight===0)this.streamPayloadPane.focus({preventScroll:true});
    const id=this.sessionId,generation=++this.payloadGeneration;
    try {
      const message=await invoke<StreamMessage>('stream_message',{id,sequence:row.sequence});
      if(generation!==this.payloadGeneration||id!==this.sessionId||!this.isConnected)return;
      this.streamPayload=message;
      this.streamView=message.kind==='binary'?'hex':'text';this.renderPayload();this.$flushUpdates();this.streamPayloadPane.scrollIntoView({block:'nearest'});
    } catch(error:unknown){if(generation===this.payloadGeneration)this.streamText=describeError(error);}
  }
  key(event:KeyboardEvent,row:StreamRow):void {
    if(!['ArrowDown','ArrowUp','Home','End'].includes(event.key))return;event.preventDefault();
    const index=this.offset+this.streamRows.findIndex(item=>item.sequence===row.sequence);
    const target=event.key==='Home'?0:event.key==='End'?this.streamTotal-1:Math.max(0,Math.min(this.streamTotal-1,index+(event.key==='ArrowDown'?1:-1)));
    const next=this.streamRows[target-this.offset];
    if(next){void this.select(next);this.$flushUpdates();this.focusSelected();}
    else{this.keyTarget=target;this.streamFollow=false;this.offset=Math.max(0,target-8);this.anchor=null;this.schedule();}
  }

  viewer(event:Event):void {this.streamView=(event.currentTarget as HTMLSelectElement).value;this.renderPayload();}
  private renderPayload():void {
    const message=this.streamPayload;if(!message)return;
    const bytes=new Uint8Array(message.payload);let text=new TextDecoder().decode(bytes);this.streamHex=null;
    if(this.streamView==='raw')text=message.raw??text;
    if(this.streamView==='json'){try{text=JSON.stringify(JSON.parse(text),null,2);}catch{text='Invalid or incomplete JSON.\n\n'+text;}}
    if(this.streamView==='hex'){let binary='';for(const byte of bytes)binary+=String.fromCharCode(byte);this.streamHex={key:this.sessionId+':'+this.streamSelected,bytesBase64:btoa(binary),offset:0,truncated:message.truncated};}
    this.streamText=text;
    this.streamFacts=message.sender+' · '+message.kind+' · '+formatBytes(message.byteCount)+(message.truncated?' · Retained prefix only':'')+(message.compressed?' · permessage-deflate':'')+(message.eventId!==null?' · ID: '+message.eventId:'')+(message.retry!==null?' · Retry: '+message.retry+' ms':'')+(message.closeCode!==null?' · Close: '+message.closeCode:'');
  }
  async copy():Promise<void> {try{await navigator.clipboard.writeText(this.streamText);this.diagnostic='Message text copied.';}catch{this.diagnostic='Select the message text and use Copy.';}}
  async save():Promise<void> {
    if(!this.streamPayload||this.streamBusy)return;this.streamBusy=true;
    try {if(await invoke<boolean>('save_stream_message',{id:this.sessionId,sequence:this.streamSelected,raw:this.streamView==='raw'}))this.diagnostic='Retained message saved.';}
    catch(error:unknown){await this.showError('Message save failed',describeError(error));}finally{this.streamBusy=false;}
  }
  disconnectedCallback():void {this.generation++;this.payloadGeneration++;window.clearTimeout(this.timer);super.disconnectedCallback();}
}
StreamInspector.define('stream-inspector');

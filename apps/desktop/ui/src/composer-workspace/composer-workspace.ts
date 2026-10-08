import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { BodyInspection, ComposerResult, SessionDetail } from '../models.js';
import { describeError, parseHeaderLines, shortUrl } from '../utilities.js';

export class ComposerWorkspace extends WorkspaceElement {
 @attr view='traffic';
 @observable composerText=initialState.composerText;
 @observable composerError='';
 @observable composerBusy=false;
 @observable composerDirty=false;
 @observable composerPane='request';
 @observable composerResult:ComposerResult|null=null;
 @observable resultMode='body';
 @observable resultHeaders:Array<{id:string;name:string;value:string}>=[];
 @observable resultRequestLabel='';
 @observable resultDetails='';
 @observable resultTone='';
 @observable needsNonIdempotent=false;
 @observable needsCredentials=false;
 @observable bodyHexMode=false;
 @observable composerSourceId='';
 @observable composerSourceLabel='';
 @observable composerSourceAvailable=false;
 composerForm!:HTMLFormElement;
 replaceComposerDialog!:HTMLDialogElement;
 private draftGeneration=0;
 private bodyRevision=0;
 private editVersion=0;
 private leaveResolve:((value:boolean)=>void)|null=null;
 protected hydratedCallback():void {this.updateRisk();}
 viewChanged():void {if(this.view==='composer' && this.composerSourceId)void this.checkSource();}
 private input(name:string):HTMLInputElement {return this.composerForm.elements.namedItem(name) as HTMLInputElement;}
 markComposerDirty(event:Event):void {this.composerDirty=true;this.editVersion++;const name=(event.target as HTMLInputElement).name;if(name==='body'||name==='bodyHex')this.bodyRevision++;if(name==='method')this.input('nonIdempotent').checked=false;if(name==='headers')this.input('credentials').checked=false;this.updateRisk();}
 private updateRisk():void {this.needsNonIdempotent=!['GET','HEAD','PUT','DELETE','OPTIONS','TRACE'].includes(this.input('method').value.trim());this.needsCredentials=/^\s*(authorization|proxy-authorization|cookie)\s*:/im.test(this.input('headers').value);this.bodyHexMode=this.input('bodyHex').checked;}
 showComposerPane(pane:string):void {this.composerPane=pane;}
 showResult(mode:string):void {this.resultMode=mode;}
 returnToTraffic():void {this.activateView('traffic');}
 private async checkSource():Promise<void> {const id=this.composerSourceId;try {await invoke('session_detail',{id});if(this.composerSourceId===id)this.composerSourceAvailable=true;}catch {if(this.composerSourceId===id)this.composerSourceAvailable=false;}}
 async showComposerSource():Promise<void> {await this.checkSource();if(this.composerSourceAvailable)this.$emit('source-traffic-request',this.composerSourceId);}
 resolveComposerReplacement(replace:boolean):void {this.replaceComposerDialog.close();this.leaveResolve?.(replace);this.leaveResolve=null;}
 replacementCancelled(event:Event):void {event.preventDefault();this.resolveComposerReplacement(false);}
 async populateRequest(detail:SessionDetail):Promise<void> {
  const generation=++this.draftGeneration;
  if(this.composerDirty){if(this.leaveResolve)return;const replace=await new Promise<boolean>(resolve=>{this.leaveResolve=resolve;this.replaceComposerDialog.showModal();});if(!replace||generation!==this.draftGeneration)return;}
  const request=detail.requests.find(head=>head.boundary==='client-request')??detail.requests[0];if(!request){this.composerError='The captured request is no longer available.';return;}
  this.input('method').value=request.method??'GET';this.input('url').value=request.target??'';
  this.input('headers').value=request.headers.filter(header=>!header.sensitive&&!['host','content-length','transfer-encoding','connection','content-encoding'].includes(header.name.toLowerCase())).map(header=>header.name+': '+header.value).join('\n');
  this.input('body').value='';this.input('bodyHex').checked=false;this.input('nonIdempotent').checked=false;this.input('credentials').checked=false;
  this.composerSourceId=detail.id;this.composerSourceLabel=(request.method??'Request')+' '+shortUrl(request.target??'');this.composerSourceAvailable=true;this.composerDirty=false;this.editVersion++;this.composerPane='request';this.composerError='';this.updateRisk();this.input('url').focus();
  this.composerText='Captured request loaded. Credential headers were omitted.';
  const body=detail.storedBodies.find(item=>item.boundary===request.boundary),revision=++this.bodyRevision;
  if(body?.observedBytes){if(body.availability!=='complete'||body.retainedBytes>1024*1024){this.composerText+=' The complete request body is unavailable; supply it before sending.';}
   else {try {const inspection=await invoke<BodyInspection>('inspect_body',{request:{sessionId:detail.id,boundary:request.boundary,representation:'bytes',decodeContent:true,offset:0,maxBytes:1024*1024}});if(generation!==this.draftGeneration||revision!==this.bodyRevision||!this.isConnected)return;if(inspection.truncated||inspection.nextOffset!==null)throw new Error('request body exceeds the replay limit');if(inspection.representation!=='bytes')throw new Error('request bytes are unavailable');this.input('body').value=inspection.display.split('\n').filter(Boolean).map(line=>line.slice(10).split('|')[0]!.trim().replace(/\s+/g,'')).join('');this.input('bodyHex').checked=true;this.updateRisk();}catch(error:unknown){if(generation===this.draftGeneration)this.composerText+=' Request body could not be loaded: '+describeError(error);}}
  }
 }
 async executeComposer(event:Event):Promise<void> {
  event.preventDefault();if(this.composerBusy)return;this.composerBusy=true;this.composerError='';this.composerText='Sending request…';const data=new FormData(this.composerForm);
  const version=this.editVersion;const label=String(data.get('method'))+' '+String(data.get('url'));
  try {const result=await invoke<ComposerResult>('execute_composer',{request:{method:String(data.get('method')??''),url:String(data.get('url')??''),headers:parseHeaderLines(String(data.get('headers')??'')),body:String(data.get('body')??''),bodyIsHex:data.get('bodyHex')==='on',acknowledgeNonIdempotent:data.get('nonIdempotent')==='on',acknowledgeCredentials:data.get('credentials')==='on'}});
   this.composerResult=result;this.resultHeaders=result.headers.map((header,index)=>({...header,id:String(index)}));this.resultRequestLabel=label;this.resultDetails=JSON.stringify({id:result.id,attribution:result.attribution,bodyIsHex:result.bodyIsHex,truncated:result.truncated},null,2);this.resultTone=result.status>=400?'failed':result.status>=300?'redirect':'success';this.composerText='Response received: HTTP '+result.status+(result.truncated?' · preview truncated':'');if(version===this.editVersion)this.composerPane='response';
  }catch(error:unknown){this.composerError='Send failed: '+describeError(error);this.composerText='The request draft is ready to retry.';}finally{this.composerBusy=false;}
 }
 disconnectedCallback():void {this.draftGeneration++;this.leaveResolve?.(false);this.leaveResolve=null;super.disconnectedCallback();}
}
ComposerWorkspace.define('composer-workspace');

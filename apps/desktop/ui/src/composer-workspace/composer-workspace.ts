import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { ComposerSource, ComposerResult, SessionDetail } from '../models.js';
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
 @observable composerBodyMissing=false;
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
 markComposerDirty(event:Event):void {this.composerDirty=true;this.editVersion++;const name=(event.target as HTMLInputElement).name;if(name==='body'||name==='bodyHex'){this.bodyRevision++;this.composerBodyMissing=false;}if(name==='method')this.input('nonIdempotent').checked=false;if(name==='headers')this.input('credentials').checked=false;this.updateRisk();}
 private updateRisk():void {this.needsNonIdempotent=!['GET','HEAD','PUT','DELETE','OPTIONS','TRACE'].includes(this.input('method').value.trim());this.needsCredentials=/^\s*(authorization|proxy-authorization|cookie)\s*:/im.test(this.input('headers').value);this.bodyHexMode=this.input('bodyHex').checked;}
 showComposerPane(pane:string):void {this.composerPane=pane;}
 useEmptyBody():void {this.composerBodyMissing=false;this.input('body').value='';this.composerDirty=true;this.editVersion++;this.bodyRevision++;}
 showResult(mode:string):void {this.resultMode=mode;}
 returnToTraffic():void {this.activateView('traffic');}
 private async checkSource():Promise<void> {const id=this.composerSourceId;try {await invoke('session_detail',{id});if(this.composerSourceId===id)this.composerSourceAvailable=true;}catch {if(this.composerSourceId===id)this.composerSourceAvailable=false;}}
 async showComposerSource():Promise<void> {await this.checkSource();if(this.composerSourceAvailable)this.$emit('source-traffic-request',this.composerSourceId);}
 resolveComposerReplacement(replace:boolean):void {this.replaceComposerDialog.close();this.leaveResolve?.(replace);this.leaveResolve=null;}
 replacementCancelled(event:Event):void {event.preventDefault();this.resolveComposerReplacement(false);}
 async populateRequest(detail:SessionDetail):Promise<void> {
  const generation=++this.draftGeneration;
  if(this.composerDirty){if(this.leaveResolve)return;const replace=await new Promise<boolean>(resolve=>{this.leaveResolve=resolve;this.replaceComposerDialog.showModal();});if(!replace||generation!==this.draftGeneration)return;}
  this.composerText='Loading the complete captured request…';this.composerError='';
  const revision=++this.bodyRevision,editVersion=this.editVersion;
  try {const source=await invoke<ComposerSource>('composer_source',{id:detail.id});
   if(generation!==this.draftGeneration||revision!==this.bodyRevision||editVersion!==this.editVersion||!this.isConnected)return;
   this.input('method').value=source.method;this.input('url').value=source.url;
   this.input('headers').value=source.headers.map(header=>header.name+': '+header.value).join('\n');
   this.composerBodyMissing=!source.bodyAvailable;this.input('body').value=source.body;this.input('bodyHex').checked=true;
   this.input('nonIdempotent').checked=false;this.input('credentials').checked=false;
   this.composerSourceId=detail.id;this.composerSourceLabel=source.method+' '+shortUrl(source.url);this.composerSourceAvailable=true;this.composerDirty=false;this.editVersion++;this.composerPane='request';this.updateRisk();this.input('url').focus();
   this.composerText='Captured request loaded. Review it before sending.'+(source.notices.length?' '+source.notices.join(' '):'');
  }catch(error:unknown){if(generation===this.draftGeneration){this.composerError='Captured request could not be loaded: '+describeError(error);this.composerText='Your previous draft is still available.';}}

 }
 async executeComposer(event:Event):Promise<void> {
  event.preventDefault();if(this.composerBusy||this.composerBodyMissing)return;this.draftGeneration++;this.composerBusy=true;this.composerError='';this.composerText='Sending request…';const data=new FormData(this.composerForm);
  const version=this.editVersion;const label=String(data.get('method'))+' '+String(data.get('url'));
  try {const result=await invoke<ComposerResult>('execute_composer',{request:{method:String(data.get('method')??''),url:String(data.get('url')??''),headers:parseHeaderLines(String(data.get('headers')??'')),body:String(data.get('body')??''),bodyIsHex:data.get('bodyHex')==='on',acknowledgeNonIdempotent:data.get('nonIdempotent')==='on',acknowledgeCredentials:data.get('credentials')==='on'}});
   this.composerResult=result;this.resultHeaders=result.headers.map((header,index)=>({...header,id:String(index)}));this.resultRequestLabel=label;this.resultDetails=JSON.stringify({id:result.id,attribution:result.attribution,bodyIsHex:result.bodyIsHex,truncated:result.truncated},null,2);this.resultTone=result.status>=400?'failed':result.status>=300?'redirect':'success';this.composerText='Response received: HTTP '+result.status+(result.truncated?' · preview truncated':'');if(version===this.editVersion)this.composerPane='response';
  }catch(error:unknown){this.composerError='Send failed: '+describeError(error);this.composerText='The request draft is ready to retry.';}finally{this.composerBusy=false;}
 }
 disconnectedCallback():void {this.draftGeneration++;this.leaveResolve?.(false);this.leaveResolve=null;super.disconnectedCallback();}
}
ComposerWorkspace.define('composer-workspace');

import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { CaptureReadModel, TraceInspection, TraceImportIntent, CaptureExportResult, ProductState, AppStatus } from '../models.js';
import { describeError } from '../utilities.js';

export const storageSize=(bytes:number):string=>{const units=['B','KiB','MiB','GiB'];let value=bytes,index=0;while(value>=1024&&index<units.length-1){value/=1024;index++;}return value.toLocaleString(undefined,{maximumFractionDigits:1})+' '+units[index];};
export class CaptureWorkspace extends WorkspaceElement {
 @attr view='traffic';
 @attr lifecycle='stopped';
 @observable captureTask='record';
 @observable recordingContent='headers';
 @observable captureWaiting=false;
 private recordingCanceled=false;
 cancelRecordingStart():void {this.recordingCanceled=true;this.captureText='Recording canceled. Proxy routing remains off.';}
 @observable exportCanEncrypt=true;
 @observable incidentMarkBusy=false;
 @observable incidentMarkStatus='';
 incidentNote!:HTMLTextAreaElement;
 async markIncident():Promise<void>{
  if(this.incidentMarkBusy||!this.captureActive)return;
  const path=this.capturePathText,note=this.incidentNote.value;
  this.incidentMarkBusy=true;this.incidentMarkStatus='Recording incident marker…';
  try{
   const marker=await invoke<{id:string;unixMillis:number}>('mark_incident',{note});
   if(!this.isConnected||this.capturePathText!==path)return;
   this.incidentMarkStatus='Issue marker recorded at '+new Date(marker.unixMillis).toISOString()+'. It will appear when this file is imported into Traffic.';
   if(this.incidentNote.value===note)this.incidentNote.value='';
  }catch(error:unknown){if(this.isConnected&&this.capturePathText===path)this.incidentMarkStatus='Marker was not recorded: '+describeError(error);}
  finally{this.incidentMarkBusy=false;}
 }

 @observable captureText=initialState.captureText;
 @observable captureError='';
 @observable captureBusy=false;
 @observable captureActive=false;
 @observable captureStateText='Not recording';
 @observable capturePathText='';
 @observable captureDetails='';
 @observable captureFacts:Array<{label:string;value:string}>=[];
 @observable captureMetadataExpanded=false;
 @observable canExportInspected=false;
 captureForm!:HTMLFormElement;
 inspectionForm!:HTMLFormElement;
 exportForm!:HTMLFormElement;
 private bodyChoiceTouched=false;
 private defaultRevision=0;
 recordBodiesChanged(event:Event):void {this.bodyChoiceTouched=true;this.recordingContent=(event.currentTarget as HTMLSelectElement).value;}
 private async loadRecordingDefaults():Promise<void> {
  const revision=++this.defaultRevision;
  try {const state=await invoke<ProductState>('product_state');if(this.isConnected&&revision===this.defaultRevision&&!this.bodyChoiceTouched&&!this.captureActive&&!this.captureBusy)this.recordingContent=state.privacy.retainBodySamples?'bodies':'headers';}catch { /* The form remains usable if preferences cannot be read. */ }
 }
 private timer:number|undefined;
 private refreshPending=false;
 private generation=0;
 private exportExtension='tmcap';
 private inspectedPath='';
 captureMetadataToggled(event:Event):void {this.captureMetadataExpanded=(event.target as HTMLDetailsElement).open;}
 exportFormatChanged():void {const format=String(new FormData(this.exportForm).get('format'));this.exportCanEncrypt=format!=='json-lines';const extension=format==='native'?'tmcap':format==='json-lines'?'jsonl':'saz';const input=this.exportForm.elements.namedItem('destination') as HTMLInputElement;if(input.value.toLowerCase().endsWith('.'+this.exportExtension))input.value=input.value.slice(0,-this.exportExtension.length)+extension;this.exportExtension=extension;}
 protected hydratedCallback():void {
  // Lifecycle can change while this workspace is still waiting to hydrate.
  this.$update('lifecycle');
  void this.refreshCapture();void this.loadRecordingDefaults();
 }
 viewChanged():void {if(this.view==='captures'){void this.refreshCapture();void this.loadRecordingDefaults();}else window.clearTimeout(this.timer);}
 showCaptureTask(task:string):void {this.captureTask=task;this.captureError='';}
 private renderCapture(status:CaptureReadModel):void {this.captureActive=status.state==='active';if(this.captureActive&&this.captureText==='Capture is idle.')this.captureText='Recording directly to file. Import the saved file to inspect its traffic.';this.capturePathText=status.path??'';this.captureStateText=status.state==='active'?'Recording · '+storageSize(status.bytesWritten??0)+' written':status.state==='sealed'?'Saved · '+storageSize(status.bytesWritten??0):status.state==='failed'?'Recording stopped: '+(status.message??'capture failed'):status.state==='shutdown'?'Capture service stopped':'Not recording';}
 private async runCapture(title:string,operation:()=>Promise<void>):Promise<void> {if(this.captureBusy)return;this.captureBusy=true;this.captureError='';this.generation++;try{await operation();}catch(error:unknown){this.captureError=describeError(error);this.captureBusy=false;await this.showError(title,this.captureError);}finally{this.captureBusy=false;if(this.view==='captures')void this.refreshCapture();}}
 async chooseCapturePath(kind:string):Promise<void> {await this.runCapture('Could not choose capture file',async()=>{const format=String(new FormData(this.exportForm).get('format')??'native');const path=await invoke<string|null>('pick_capture_path',{kind,format});if(path===null)return;const form=kind==='source'?this.inspectionForm:this.exportForm;const name=kind==='source'?'source':'destination';const input=form.elements.namedItem(name) as HTMLInputElement;input.value=path;input.focus();});}
 async chooseExportSource():Promise<void> {await this.runCapture('Could not open export source',async()=>{const path=await invoke<string|null>('pick_capture_path',{kind:'source',format:null});if(path!==null){const input=this.exportForm.elements.namedItem('source') as HTMLInputElement;input.value=path;input.focus();}});}
 async startCapture(event:Event):Promise<void> {event.preventDefault();const data=new FormData(this.captureForm);await this.runCapture('Recording could not start',async()=>{const path=await invoke<string|null>('pick_capture_path',{kind:'record',format:'native'});if(path===null){this.captureText='Recording canceled.';return;}const password=data.get('encrypt')==='on'?await this.promptTracePassword('Encrypt recording',true):null;if(data.get('encrypt')==='on'&&password===null){this.captureText='Recording canceled.';return;}if(!await this.prepareRecording())return;const status=await invoke<CaptureReadModel>('start_capture',{request:{password,path,retainBodySamples:data.get('bodies')==='bodies',redactSensitiveHeaders:data.get('redactHeaders')==='on',includeNetworkContext:data.get('networkContext')==='on'}});this.renderCapture(status);this.captureText='Recording to '+(status.path??path)+'. New traffic is written to this file.';this.$emit('status-changed',await invoke('app_status'));});}
 private async prepareRecording():Promise<boolean> {
  this.recordingCanceled=false;
  let status=await invoke<AppStatus>('app_status');
  if(status.lifecycle==='stopped')return true;
  if(status.lifecycle!=='running'&&status.lifecycle!=='draining')throw new Error('Wait for the current proxy operation to finish, then try recording again.');
  this.captureText='Finishing active requests before recording. Proxy routing is off.';this.captureWaiting=true;
  try {
   if(status.lifecycle==='running')status=await invoke<AppStatus>('stop_application');
   this.$emit('status-changed',status);
   while(status.lifecycle==='draining'&&!this.recordingCanceled&&this.isConnected){
    await new Promise(resolve=>window.setTimeout(resolve,250));status=await invoke<AppStatus>('app_status');
   }
   if(this.recordingCanceled||!this.isConnected)return false;
   if(status.lifecycle!=='stopped')throw new Error('The proxy did not stop. Recording has not started.');
   return true;
  } finally {this.captureWaiting=false;}
 }
 async stopCapture():Promise<void> {await this.runCapture('Recording could not stop',async()=>{const status=await invoke<CaptureReadModel>('stop_capture');this.renderCapture(status);this.captureText='Saved recording: '+(status.path??'')+'.';this.$emit('status-changed',await invoke('app_status'));});}
 async refreshCapture():Promise<void> {if(this.refreshPending||this.captureBusy)return;this.refreshPending=true;const generation=this.generation;try{const status=await invoke<CaptureReadModel>('capture_status');if(this.isConnected&&generation===this.generation)this.renderCapture(status);}catch(error:unknown){if(this.isConnected&&generation===this.generation)this.captureError='Capture status unavailable: '+describeError(error);}finally{this.refreshPending=false;window.clearTimeout(this.timer);if(this.isConnected&&this.view==='captures')this.timer=window.setTimeout(()=>void this.refreshCapture(),1000);}}
 async inspectCapture(event:Event):Promise<void> {
  event.preventDefault();await this.runCapture('Trace metadata inspection failed',async()=>{
   const path=String(new FormData(this.inspectionForm).get('source')??'');this.canExportInspected=false;this.inspectedPath='';
   const metadata=await this.withTracePassword('Inspect trace metadata',password=>invoke<TraceInspection>('inspect_trace_metadata',{request:{path,password}}));
   if(metadata===null){this.captureText='Inspection canceled.';return;}
   this.captureFacts=[{label:'Full path',value:metadata.path},{label:'Format',value:metadata.format.toUpperCase()},{label:'Exchanges',value:metadata.sessions.toLocaleString()},{label:'File size',value:storageSize(metadata.bytes)}];
   this.captureDetails=JSON.stringify({context:metadata.context,sources:metadata.sources,notes:metadata.notes},null,2);
   this.captureMetadataExpanded=true;
   this.inspectedPath=metadata.path;this.canExportInspected=metadata.format==='native';this.captureText='Trace metadata inspected. Traffic is unchanged.';
  });
 }
 async chooseInspectionPath():Promise<void> {await this.runCapture('Could not choose trace',async()=>{const path=await invoke<string|null>('pick_trace_path');if(path){const input=this.inspectionForm.elements.namedItem('source') as HTMLInputElement;input.value=path;input.focus();}});}
 importInspectionTrace():void {if(this.captureBusy)return;const input=this.inspectionForm.elements.namedItem('source') as HTMLInputElement;if(input.reportValidity())this.$emit('trace-import-request',{paths:[input.value]} satisfies TraceImportIntent);}
 useInspectedForExport():void {if(!this.canExportInspected)return;const source=this.inspectedPath;(this.exportForm.elements.namedItem('source') as HTMLInputElement).value=source;(this.exportForm.elements.namedItem('destination') as HTMLInputElement).value=source.replace(/\.tmcap$/i,'')+'.exported.tmcap';(this.exportForm.elements.namedItem('format') as HTMLSelectElement).value='native';this.exportExtension='tmcap';this.exportCanEncrypt=true;this.captureTask='export';}
 async exportCapture(event:Event):Promise<void> {event.preventDefault();await this.runCapture('Capture export failed',async()=>{const data=new FormData(this.exportForm);const password=data.get('encrypt')==='on'?await this.promptTracePassword('Encrypt export',true):null;if(data.get('encrypt')==='on'&&password===null){this.captureText='Export canceled.';return;}const report=await this.withTracePassword('Open source capture',sourcePassword=>invoke<CaptureExportResult>('export_capture',{request:{password,sourcePassword,source:String(data.get('source')??''),destination:String(data.get('destination')??''),format:String(data.get('format')??'native'),redactSensitiveHeaders:data.get('redactHeaders')==='on'}}));if(report===null){this.captureText='Export canceled.';return;}this.captureFacts=[{label:'Saved to',value:report.destination},{label:'Output size',value:storageSize(report.bytes)},{label:'Records / exchanges',value:report.records.toLocaleString()},{label:'Format fidelity',value:report.fidelity},{label:'Source state',value:report.sourceSealed?'Sealed':report.sourceTruncatedTail?'Recovered valid prefix':'Unsealed'}];this.captureDetails=JSON.stringify(report,null,2);this.captureText='Export saved to a new file.';});}
 dismissCaptureError():void {this.captureError='';const form=this.captureTask==='record'?this.captureForm:this.captureTask==='inspect'?this.inspectionForm:this.exportForm;(form.elements.namedItem('source') as HTMLInputElement|null)?.focus();}
 disconnectedCallback():void {this.recordingCanceled=true;this.generation++;window.clearTimeout(this.timer);super.disconnectedCallback();}
}
CaptureWorkspace.define('capture-workspace');

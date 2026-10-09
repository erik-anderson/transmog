import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { CaptureReadModel, CaptureSummaryView, CaptureExportResult, ProductState } from '../models.js';
import { describeError } from '../utilities.js';

export const storageSize=(bytes:number):string=>{const units=['B','KiB','MiB','GiB'];let value=bytes,index=0;while(value>=1024&&index<units.length-1){value/=1024;index++;}return value.toLocaleString(undefined,{maximumFractionDigits:1})+' '+units[index];};
export class CaptureWorkspace extends WorkspaceElement {
 @attr view='traffic';
 @observable captureTask='record';
 @observable exportCanEncrypt=true;
 @observable captureText=initialState.captureText;
 @observable captureError='';
 @observable captureBusy=false;
 @observable captureActive=false;
 @observable captureStateText='Not recording';
 @observable capturePathText='';
 @observable captureDetails='';
 @observable captureFacts:Array<{label:string;value:string}>=[];
 @observable canExportInspected=false;
 captureForm!:HTMLFormElement;
 inspectionForm!:HTMLFormElement;
 exportForm!:HTMLFormElement;
 private bodyChoiceTouched=false;
 private defaultRevision=0;
 recordBodiesChanged():void {this.bodyChoiceTouched=true;}
 private async loadRecordingDefaults():Promise<void> {
  const revision=++this.defaultRevision;
  try {const state=await invoke<ProductState>('product_state');if(this.isConnected&&revision===this.defaultRevision&&!this.bodyChoiceTouched&&!this.captureActive&&!this.captureBusy)(this.captureForm.elements.namedItem('bodies') as HTMLInputElement).checked=state.privacy.retainBodySamples;}catch { /* The form remains usable if preferences cannot be read. */ }
 }
 private timer:number|undefined;
 private refreshPending=false;
 private generation=0;
 private exportExtension='tmcap';
 exportFormatChanged():void {const format=String(new FormData(this.exportForm).get('format'));this.exportCanEncrypt=format!=='json-lines';const extension=format==='native'?'tmcap':format==='json-lines'?'jsonl':'saz';const input=this.exportForm.elements.namedItem('destination') as HTMLInputElement;if(input.value.toLowerCase().endsWith('.'+this.exportExtension))input.value=input.value.slice(0,-this.exportExtension.length)+extension;this.exportExtension=extension;}
 protected hydratedCallback():void {void this.refreshCapture();void this.loadRecordingDefaults();}
 viewChanged():void {if(this.view==='captures'){void this.refreshCapture();void this.loadRecordingDefaults();}else window.clearTimeout(this.timer);}
 showCaptureTask(task:string):void {this.captureTask=task;this.captureError='';}
 private renderCapture(status:CaptureReadModel):void {this.captureActive=status.state==='active';this.capturePathText=status.path??'';this.captureStateText=status.state==='active'?'Recording · '+storageSize(status.bytesWritten??0)+' written':status.state==='sealed'?'Saved · '+storageSize(status.bytesWritten??0):status.state==='failed'?'Recording stopped: '+(status.message??'capture failed'):status.state==='shutdown'?'Capture service stopped':'Not recording';}
 private async runCapture(title:string,operation:()=>Promise<void>):Promise<void> {if(this.captureBusy)return;this.captureBusy=true;this.captureError='';this.generation++;try{await operation();}catch(error:unknown){this.captureError=describeError(error);this.captureBusy=false;await this.showError(title,this.captureError);}finally{this.captureBusy=false;if(this.view==='captures')void this.refreshCapture();}}
 async chooseCapturePath(kind:string):Promise<void> {await this.runCapture('Could not choose capture file',async()=>{const format=String(new FormData(this.exportForm).get('format')??'native');const path=await invoke<string|null>('pick_capture_path',{kind,format});if(path===null)return;const form=kind==='record'?this.captureForm:kind==='source'?this.inspectionForm:this.exportForm;const name=kind==='record'?'capturePath':kind==='source'?'source':'destination';const input=form.elements.namedItem(name) as HTMLInputElement;input.value=path;input.focus();});}
 async chooseExportSource():Promise<void> {await this.runCapture('Could not open export source',async()=>{const path=await invoke<string|null>('pick_capture_path',{kind:'source',format:null});if(path!==null){const input=this.exportForm.elements.namedItem('source') as HTMLInputElement;input.value=path;input.focus();}});}
 async startCapture(event:Event):Promise<void> {event.preventDefault();await this.runCapture('Recording could not start',async()=>{const data=new FormData(this.captureForm);const password=data.get('encrypt')==='on'?await this.promptTracePassword('Encrypt recording',true):null;if(data.get('encrypt')==='on'&&password===null){this.captureText='Recording canceled.';return;}const status=await invoke<CaptureReadModel>('start_capture',{request:{password,path:String(data.get('capturePath')??''),retainBodySamples:data.get('bodies')==='on',includeNetworkContext:data.get('networkContext')==='on'}});this.renderCapture(status);this.captureText='Recording started.';});}
 async stopCapture():Promise<void> {await this.runCapture('Recording could not stop',async()=>{const status=await invoke<CaptureReadModel>('stop_capture');this.renderCapture(status);this.captureText='Recording stopped and the file was sealed.';});}
 async refreshCapture():Promise<void> {if(this.refreshPending||this.captureBusy)return;this.refreshPending=true;const generation=this.generation;try{const status=await invoke<CaptureReadModel>('capture_status');if(generation===this.generation)this.renderCapture(status);}catch(error:unknown){this.captureError='Capture status unavailable: '+describeError(error);}finally{this.refreshPending=false;window.clearTimeout(this.timer);if(this.isConnected&&this.view==='captures')this.timer=window.setTimeout(()=>void this.refreshCapture(),1000);}}
 async inspectCapture(event:Event):Promise<void> {event.preventDefault();await this.runCapture('Capture inspection failed',async()=>{const path=String(new FormData(this.inspectionForm).get('source')??'');this.canExportInspected=false;const summary=await this.withTracePassword('Inspect '+(path.split(/[\\/]/).pop()??'capture'),password=>invoke<CaptureSummaryView>('import_capture',{request:{path,password}}));if(summary===null){this.captureText='Inspection canceled.';return;}this.captureFacts=[{label:'Exchanges',value:summary.exchanges.toLocaleString()},{label:'Records',value:summary.records.toLocaleString()},{label:'Retained body samples',value:storageSize(summary.retainedBodyBytes)},{label:'Missing-event markers',value:summary.lossMarkers.toLocaleString()},{label:'File state',value:summary.sealed?'Sealed':summary.truncatedTail?'Interrupted tail; valid prefix recovered':'Unsealed'}];this.captureDetails=JSON.stringify(summary,null,2);this.canExportInspected=true;this.captureText=summary.truncatedTail?'Inspection recovered the valid prefix. Export a TMCap copy to save it; the original file is unchanged.':'Capture inspected.';});}
 useInspectedForExport():void {const source=String(new FormData(this.inspectionForm).get('source')??'');(this.exportForm.elements.namedItem('source') as HTMLInputElement).value=source;(this.exportForm.elements.namedItem('destination') as HTMLInputElement).value=source.replace(/\.tmcap$/i,'')+'.recovered.tmcap';(this.exportForm.elements.namedItem('format') as HTMLSelectElement).value='native';this.exportExtension='tmcap';this.exportCanEncrypt=true;this.captureTask='export';}
 async exportCapture(event:Event):Promise<void> {event.preventDefault();await this.runCapture('Capture export failed',async()=>{const data=new FormData(this.exportForm);const password=data.get('encrypt')==='on'?await this.promptTracePassword('Encrypt export',true):null;if(data.get('encrypt')==='on'&&password===null){this.captureText='Export canceled.';return;}const report=await this.withTracePassword('Open source capture',sourcePassword=>invoke<CaptureExportResult>('export_capture',{request:{password,sourcePassword,source:String(data.get('source')??''),destination:String(data.get('destination')??''),format:String(data.get('format')??'native'),redactSensitiveHeaders:data.get('redactHeaders')==='on'}}));if(report===null){this.captureText='Export canceled.';return;}this.captureFacts=[{label:'Saved to',value:report.destination},{label:'Output size',value:storageSize(report.bytes)},{label:'Records / exchanges',value:report.records.toLocaleString()},{label:'Format fidelity',value:report.fidelity},{label:'Source state',value:report.sourceSealed?'Sealed':report.sourceTruncatedTail?'Recovered valid prefix':'Unsealed'}];this.captureDetails=JSON.stringify(report,null,2);this.captureText='Export saved to a new file.';});}
 dismissCaptureError():void {this.captureError='';const form=this.captureTask==='record'?this.captureForm:this.captureTask==='inspect'?this.inspectionForm:this.exportForm;(form.elements.namedItem(this.captureTask==='record'?'capturePath':'source') as HTMLInputElement).focus();}
 disconnectedCallback():void {this.generation++;window.clearTimeout(this.timer);super.disconnectedCallback();}
}
CaptureWorkspace.define('capture-workspace');

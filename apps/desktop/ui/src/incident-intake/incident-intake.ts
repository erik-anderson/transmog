import {attr,observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError} from '../utilities.js';
export type IncidentWindow={traceId:string;markerId:string;beforeSeconds:number;afterSeconds:number;relevantHostsOnly:boolean};
type Marker={id:string;runId:string;unixMillis:number;offsetMillis:number|null;origin:string;creationKind:string;note:string};
type Intake={traceId:string;source:string;profile:null|{title:string;instructions:string;steps:string[];expectedApplication:string|null;relevantHosts:Array<{host:string;includeSubdomains:boolean}>};markers:Marker[];expectedBehavior:string;actualBehavior:string;enteredClientVersion:string;collector:string;observedClients:string[];observedUserAgents:string[];entries:number;incompleteBodies:number;note:string};
export class IncidentIntake extends WorkspaceElement{
 @attr traceId='';
 @observable intakeTitle='Incident intake';
 @observable intakeInstructions='';
 @observable intakeSteps:Array<{id:string;text:string}>=[];
 @observable intakeMarkers:Array<Marker&{timeLabel:string;originLabel:string}>=[];
 @observable intakeHosts:Array<{id:string;text:string}>=[];
 @observable intakeFacts:Array<{label:string;value:string}>=[];
 @observable intakeNote='';
 @observable intakeStatus='';
 @observable intakeBusy=false;
 @observable incidentBefore=30;
 @observable incidentAfter=30;
 @observable incidentRelevantOnly=false;
 private revision=0;
 override connectedCallback():void{super.connectedCallback();void this.load();}
 traceIdChanged():void{if(this.isConnected)void this.load();}
 private async load():Promise<void>{
  const id=this.traceId,revision=++this.revision;this.intakeMarkers=[];this.intakeSteps=[];this.intakeFacts=[];this.intakeHosts=[];this.intakeInstructions='';this.intakeNote='';this.intakeTitle='Incident intake';this.incidentRelevantOnly=false;
  if(!id){this.intakeBusy=false;this.intakeStatus='Choose an imported source to review incident context.';return;}
  this.intakeBusy=true;this.intakeStatus='Loading original incident context…';
  try{const intake=await invoke<Intake>('incident_intake',{traceId:id});if(!this.isConnected||revision!==this.revision)return;
   this.intakeTitle=intake.profile?.title??'Incident intake';this.intakeInstructions=intake.profile?.instructions??'';
   this.intakeSteps=(intake.profile?.steps??[]).map((text,index)=>({id:String(index),text}));
   this.intakeHosts=(intake.profile?.relevantHosts??[]).map((host,index)=>({id:String(index),text:host.host+(host.includeSubdomains?' (and subdomains)':'')}));
   this.intakeMarkers=intake.markers.map(marker=>({...marker,timeLabel:new Date(marker.unixMillis).toISOString()+(marker.offsetMillis==null?'':' · '+marker.offsetMillis+' ms from recording start'),originLabel:marker.origin+' · '+marker.creationKind}));
   this.intakeFacts=[{label:'Retained source entries',value:String(intake.entries)},{label:'Incomplete body observations',value:String(intake.incompleteBodies)},{label:'Expected behavior',value:intake.expectedBehavior||'Not entered'},{label:'Actual behavior',value:intake.actualBehavior||'Not entered'},{label:'Expected application (profile)',value:intake.profile?.expectedApplication||'Not specified'},{label:'Client/version entered by customer',value:intake.enteredClientVersion||'Not entered'},{label:'Observed client identity (sample)',value:intake.observedClients.join(' · ')||'Unavailable'},{label:'Observed User-Agent (sample)',value:intake.observedUserAgents.join(' · ')||'Unavailable'},{label:'Collector version',value:intake.collector||'Unrecorded'}];
   this.intakeNote=intake.note;this.intakeStatus=intake.markers.length?`${intake.markers.length} incident markers recorded.`:'No issue markers were recorded. Review the source traffic and collection gaps.';
  }catch(error:unknown){if(revision===this.revision)this.intakeStatus=describeError(error);}finally{if(revision===this.revision)this.intakeBusy=false;}
 }
 setInterval(side:string,event:Event):void{const value=Number((event.currentTarget as HTMLInputElement).value);if(side==='before')this.incidentBefore=value;else this.incidentAfter=value;}
 setRelevant(event:Event):void{this.incidentRelevantOnly=(event.currentTarget as HTMLInputElement).checked;}
 showMarker(markerId:string):void{
  if(!Number.isInteger(this.incidentBefore)||!Number.isInteger(this.incidentAfter)||this.incidentBefore<0||this.incidentAfter<0||this.incidentBefore>86400||this.incidentAfter>86400){this.intakeStatus='Choose 0–86400 seconds before and after the marker.';return;}
  const window:IncidentWindow={traceId:this.traceId,markerId,beforeSeconds:this.incidentBefore,afterSeconds:this.incidentAfter,relevantHostsOnly:markerId?this.incidentRelevantOnly:false};
  const detail={window,label:this.intakeTitle+(markerId?` · ${this.incidentBefore}s before / ${this.incidentAfter}s after incident`:' · all source traffic')+(window.relevantHostsOnly?' · relevant-host hints applied':' · all hosts')};this.$emit('incident-window-request',detail);
 }
 override disconnectedCallback():void{this.revision++;super.disconnectedCallback();}
}
IncidentIntake.define('incident-intake');

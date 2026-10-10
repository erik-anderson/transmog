import {observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {WorkspaceElement} from '../workspace-element.js';
import type {AutomationCandidate,AutomationRule,AutomationStatus,UserAgentOverride} from '../models.js';
import {describeError} from '../utilities.js';
import {userAgentPresets,presetClientHints} from '../user-agent-presets.js';

const BASE=-500_000;
const hintNames=['Sec-CH-UA','Sec-CH-UA-Mobile','Sec-CH-UA-Platform','Sec-CH-UA-Platform-Version','Sec-CH-UA-Arch','Sec-CH-UA-Bitness','Sec-CH-UA-Model','Sec-CH-UA-Full-Version','Sec-CH-UA-Full-Version-List','Sec-CH-UA-WoW64'];
interface HintField {name:string;value:string;enabled:boolean;}
interface Row {id:string;name:string;scope:string;enabled:boolean;selected:boolean;first:boolean;last:boolean;dropBefore:boolean;dropAfter:boolean;}
export class UserAgentOverrides extends WorkspaceElement {
  @observable uaState:AutomationStatus|null=null;
  @observable uaRows:Row[]=[];
  @observable uaEditing=false;
  @observable uaDirty=false;
  @observable uaBusy=false;
  @observable uaStatus='';
  @observable uaSelectedId='';
  @observable uaName='';
  @observable uaHost='';
  @observable uaPath='';
  @observable uaValue='';
  @observable uaVersion='';
  @observable uaEnabled=true;
  @observable uaPreset='custom';
  @observable uaHintMode='preserve';
  @observable uaHintFields:HintField[]=[];
  @observable uaPresetGroups:Array<{name:string;options:Array<{id:string;title:string}>}>=[];
  @observable uaDuplicateName='';
  @observable uaDuplicateScope='';
  @observable uaUndoAvailable=false;
  uaNameInput!:HTMLInputElement;
  uaSaveButton!:HTMLButtonElement;
  uaDuplicateDialog!:HTMLDialogElement;
  private ready=false;
  private operation=0;
  private draggedId='';
  private dropId='';
  private dropAfter=false;
  private confirmResolve:((replace:boolean)=>void)|null=null;
  private undoRule:{rule:AutomationRule;nextId:string|null}|null=null;
  private loadedRevision=0;

  protected hydratedCallback():void {
    this.ready=true;
    this.uaPresetGroups=[...new Set(userAgentPresets.map(preset=>preset.group))].map(name=>({name,options:userAgentPresets.filter(preset=>preset.group===name).map(preset=>({id:preset.id,title:preset.title}))}));
    this.uaStateChanged();
  }
  private ordered(status=this.uaState):AutomationRule[] {return (status?.rules??[]).filter(rule=>rule.request.userAgent).sort((a,b)=>a.priority-b.priority||a.id.localeCompare(b.id));}
  uaStateChanged():void {
    const rules=this.ordered();this.uaRows=rules.map((rule,index)=>({id:rule.id,name:rule.displayName??'User-Agent override',scope:this.scope(rule.matcher.host,rule.matcher.pathPrefix),enabled:rule.enabled!==false,selected:rule.id===this.uaSelectedId,first:index===0,last:index===rules.length-1,dropBefore:rule.id===this.dropId&&!this.dropAfter,dropAfter:rule.id===this.dropId&&this.dropAfter}));
    if(this.ready&&!this.uaEditing&&rules[0])this.load(rules[0]);
    else if(this.ready&&!this.uaDirty&&this.uaSelectedId){const selected=rules.find(rule=>rule.id===this.uaSelectedId);if(selected&&selected.revision!==this.loadedRevision)this.load(selected);}
  }
  private scope(host:string|null,path:string|null):string {return (host||'All hosts')+' · '+(path?'Path starts with '+path:'All paths');}
  private hints(values:Record<string,string>):void {
    const normalized=new Map(Object.entries(values).map(([name,value])=>[name.toLowerCase(),value]));
    const names=[...hintNames,...Object.keys(values).filter(name=>!hintNames.some(known=>known.toLowerCase()===name.toLowerCase()))];
    this.uaHintFields=names.map(name=>({name,value:normalized.get(name.toLowerCase())??'',enabled:normalized.has(name.toLowerCase())}));
  }
  private load(rule:AutomationRule|null):void {
    this.loadedRevision=rule?.revision??0;
    const policy=rule?.request.userAgent;
    this.uaSelectedId=rule?.id??'';this.uaName=rule?.displayName??'';this.uaHost=rule?.matcher.host??'';this.uaPath=rule?.matcher.pathPrefix??'';
    this.uaValue=policy?.template??(rule?'':'Transmog/1.0');this.uaVersion=policy?.versionOverride??'';this.uaEnabled=rule?.enabled!==false;
    this.uaPreset=userAgentPresets.find(preset=>preset.value===this.uaValue)?.id??'custom';
    this.uaHintMode=policy?.stripClientHints?'strip':Object.keys(policy?.clientHints??{}).length?'override':'preserve';this.hints(policy?.clientHints??{});
    this.uaEditing=true;this.uaDirty=false;this.uaStatus='';this.uaStateChanged();
  }
  async select(id:string):Promise<void> {if(this.uaBusy||id===this.uaSelectedId)return;if(!this.leaveDraft())return;const rule=this.ordered().find(rule=>rule.id===id);if(rule)this.load(rule);}
  add():void {if(this.uaBusy||!this.leaveDraft())return;this.load(null);this.uaDirty=true;this.$flushUpdates();this.uaNameInput.focus();}
  private leaveDraft():boolean {return !this.uaDirty||window.confirm('Discard unsaved header override changes?');}
  edit(field:'uaName'|'uaHost'|'uaPath'|'uaValue'|'uaVersion',event:Event):void {this[field]=(event.currentTarget as HTMLInputElement).value;this.uaDirty=true;if(field==='uaValue')this.uaPreset='custom';}
  setEnabled(event:Event):void {this.uaEnabled=(event.currentTarget as HTMLInputElement).checked;this.uaDirty=true;}
  choosePreset(event:Event):void {
    this.uaPreset=(event.currentTarget as HTMLSelectElement).value;const preset=userAgentPresets.find(preset=>preset.id===this.uaPreset);
    if(preset){this.uaValue=preset.value;this.uaHintMode=preset.metadata?'override':'preserve';this.hints(presetClientHints(preset));if(!this.uaName)this.uaName=preset.title;}
    this.uaDirty=true;
  }
  chooseHintMode(event:Event):void {this.uaHintMode=(event.currentTarget as HTMLSelectElement).value;this.uaDirty=true;}
  editHint(name:string,event:Event):void {const value=(event.currentTarget as HTMLInputElement).value;this.uaHintFields=this.uaHintFields.map(field=>field.name===name?{...field,value}:field);this.uaDirty=true;}
  enableHint(name:string,event:Event):void {const enabled=(event.currentTarget as HTMLInputElement).checked;this.uaHintFields=this.uaHintFields.map(field=>field.name===name?{...field,enabled}:field);this.uaDirty=true;}
  private policy():UserAgentOverride {
    if(/[\r\n]/.test(this.uaValue))throw new Error('The User-Agent value must be a single header line.');
    if(this.uaVersion&&!/^\d{1,8}(?:\.\d{1,8}){0,3}$/.test(this.uaVersion))throw new Error('Enter a major version or up to four dotted numbers. Leave it blank to use the source browser’s major version.');
    const clientHints=this.uaHintMode==='override'?Object.fromEntries(this.uaHintFields.filter(field=>field.enabled).map(field=>[field.name,field.value])):{};
    if(!this.uaValue&&!Object.keys(clientHints).length&&this.uaHintMode!=='strip')throw new Error('Enter a User-Agent value, override an existing client hint, or choose Strip client hints.');
    return {template:this.uaValue||null,versionOverride:this.uaVersion||null,clientHints,stripClientHints:this.uaHintMode==='strip'};
  }
  private matcherKey(matcher:AutomationRule['matcher']):string {
    return JSON.stringify({method:matcher.method?.toUpperCase()??null,url:matcher.url??null,scheme:matcher.scheme?.toLowerCase()??null,host:matcher.host?.toLowerCase()??null,port:matcher.port,pathPrefix:matcher.pathPrefix,query:matcher.query,requestHeaders:matcher.requestHeaders,responseHeaders:matcher.responseHeaders,responseStatus:matcher.responseStatus,responseStatusClass:matcher.responseStatusClass});
  }
  private async activate(current:AutomationStatus,rules:AutomationRule[]):Promise<AutomationStatus> {
    const candidate=await invoke<AutomationCandidate>('validate_automation',{document:{schemaVersion:1,generation:current.generation,rules,autoresponsesEnabled:current.autoresponsesEnabled!==false}});
    return invoke<AutomationStatus>('activate_automation',{candidateId:candidate.candidateId});
  }
  private accept(status:AutomationStatus):void {this.uaState=status;this.$emit('automation-state-change',status);}
  async save(event:Event):Promise<void> {
    event.preventDefault();if(this.uaBusy)return;this.uaBusy=true;const operation=++this.operation;
    try {
      const policy=this.policy(),host=this.uaHost.trim().toLowerCase()||null,path=this.uaPath||null;
      if(path&&!path.startsWith('/'))throw new Error('A path-prefix filter must start with /.');
      const current=await invoke<AutomationStatus>('automation_status');if(operation!==this.operation)return;
      const ordered=this.ordered(current),existing=ordered.find(rule=>rule.id===this.uaSelectedId);
      const matcher=existing?{...existing.matcher,host,pathPrefix:path}:{method:null,url:null,scheme:null,host,port:null,pathPrefix:path,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null};
      const duplicate=ordered.find(rule=>rule.id!==this.uaSelectedId&&this.matcherKey(rule.matcher)===this.matcherKey(matcher));
      if(duplicate&&!await this.confirmDuplicate(duplicate)){this.uaStatus='Existing override kept. Your draft is still available.';return;}if(operation!==this.operation)return;
      const target=duplicate??existing;
      const rule:AutomationRule=target?structuredClone(target):{id:'desktop-user-agent-'+crypto.randomUUID(),revision:0,priority:BASE+ordered.length,displayName:null,enabled:true,matcher:{method:null,url:null,scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null},request:{headers:[],replaceBody:null,discardBody:false,abortReason:null,responseAsset:null,allowNonIdempotentBodyReplacement:false},response:{headers:[],replaceBody:null,discardBody:false,abortReason:null}};
      rule.revision++;rule.enabled=this.uaEnabled;rule.displayName=this.uaName.trim()||'User-Agent / client hints';rule.matcher.host=host;rule.matcher.pathPrefix=path;rule.request.userAgent=policy;
      const rules=current.rules.filter(candidate=>candidate.id!==rule.id&&(!duplicate||candidate.id!==this.uaSelectedId));rules.push(rule);
      const status=await this.activate(current,rules);if(operation!==this.operation)return;
      this.accept(status);this.load(status.rules.find(candidate=>candidate.id===rule.id)??rule);this.uaStatus='Override saved for new matching requests.';
    }catch(error){if(operation===this.operation)this.uaStatus='Override could not be saved: '+describeError(error);}
    finally{if(operation===this.operation){this.uaBusy=false;this.$flushUpdates();this.uaSaveButton?.focus({preventScroll:true});}}
  }
  private confirmDuplicate(rule:AutomationRule):Promise<boolean> {
    this.uaDuplicateName=rule.displayName??'User-Agent override';this.uaDuplicateScope=this.scope(rule.matcher.host,rule.matcher.pathPrefix);
    return new Promise(resolve=>{this.confirmResolve=resolve;this.uaDuplicateDialog.showModal();});
  }
  finishDuplicate(replace:boolean,event?:Event):void {event?.preventDefault();this.confirmResolve?.(replace);this.confirmResolve=null;this.uaDuplicateDialog.close();}
  duplicateClosed():void {if(this.uaDuplicateDialog.open)return;this.confirmResolve?.(false);this.confirmResolve=null;}
  async toggle(id:string,enabled:boolean):Promise<void> {const saved=await this.mutate(current=>current.rules.map(rule=>rule.id===id?{...rule,enabled,revision:rule.revision+1}:rule));if(saved&&this.uaSelectedId===id)this.uaEnabled=enabled;}
  async toggleRow(id:string):Promise<void> {const rule=this.ordered().find(rule=>rule.id===id);if(rule)await this.toggle(id,rule.enabled===false);}
  async remove():Promise<void> {
    if(!this.uaSelectedId||this.uaBusy||!this.leaveDraft())return;const id=this.uaSelectedId;let removed:{rule:AutomationRule;nextId:string|null}|null=null;
    const saved=await this.mutate(current=>{const ordered=this.ordered(current),index=ordered.findIndex(rule=>rule.id===id),rule=ordered[index];if(!rule)throw new Error('This override was removed while you were editing. Refresh the list.');removed={rule:structuredClone(rule),nextId:ordered[index+1]?.id??null};return current.rules.filter(rule=>rule.id!==id);});
    if(saved){this.undoRule=removed;this.uaUndoAvailable=true;this.uaEditing=false;this.uaSelectedId='';this.uaDirty=false;this.uaStateChanged();this.uaStatus='Override deleted. Undo is available.';}
  }
  async undoDelete():Promise<void> {
    const removed=this.undoRule;if(!removed||this.uaBusy||!this.leaveDraft())return;
    const saved=await this.mutate(current=>{if(current.rules.some(rule=>rule.id===removed.rule.id))throw new Error('The deleted override ID is already in use. Your current rules were kept.');
      if(this.ordered(current).some(rule=>this.matcherKey(rule.matcher)===this.matcherKey(removed.rule.matcher)))throw new Error('Another override now uses these filters. Edit that override to replace it.');
      const ordered=this.ordered(current),index=ordered.findIndex(rule=>rule.id===removed.nextId);ordered.splice(index<0?ordered.length:index,0,{...removed.rule,revision:removed.rule.revision+1});
      const ranked=ordered.map((rule,index)=>({...rule,priority:BASE+index,revision:rule.revision+(rule.priority!==BASE+index?1:0)}));return [...current.rules.filter(rule=>!rule.request.userAgent),...ranked];});
    if(saved){this.undoRule=null;this.uaUndoAvailable=false;const rule=this.ordered().find(rule=>rule.id===removed.rule.id);if(rule)this.load(rule);this.uaStatus='Deleted override restored.';}
  }
  async move(id:string,delta:number):Promise<void> {const ordered=this.ordered(),index=ordered.findIndex(rule=>rule.id===id),target=ordered[index+delta];if(index<0||!target)return;await this.reorder(id,target.id,delta>0);}
  private async reorder(id:string,target:string,after=false):Promise<void> {
    await this.mutate(current=>{const ordered=this.ordered(current),rule=ordered.find(rule=>rule.id===id);if(!rule)return current.rules;
      const others=ordered.filter(rule=>rule.id!==id),index=others.findIndex(rule=>rule.id===target);if(index<0)return current.rules;
      others.splice(index+(after?1:0),0,rule);const ranked=new Map(others.map((rule,index)=>[rule.id,{...rule,priority:BASE+index,revision:rule.revision+1}]));return current.rules.map(rule=>ranked.get(rule.id)??rule);});
  }
  private async mutate(change:(current:AutomationStatus)=>AutomationRule[]):Promise<boolean> {
    if(this.uaBusy)return false;this.uaBusy=true;const operation=++this.operation;
    try {const current=await invoke<AutomationStatus>('automation_status');if(operation!==this.operation)return false;const status=await this.activate(current,change(current));if(operation!==this.operation)return false;this.accept(status);this.uaStatus='Override list updated for new requests.';return true;}
    catch(error){if(operation===this.operation)this.uaStatus='Change failed: '+describeError(error);return false;}
    finally{if(operation===this.operation)this.uaBusy=false;}
  }
  dragStart(id:string,event:DragEvent):void {if(this.uaBusy){event.preventDefault();return;}this.draggedId=id;event.dataTransfer?.setData('text/plain',id);if(event.dataTransfer)event.dataTransfer.effectAllowed='move';}
  dragOver(id:string,event:DragEvent):void {if(!this.draggedId||id===this.draggedId||this.uaBusy)return;event.preventDefault();const bounds=(event.currentTarget as HTMLElement).getBoundingClientRect(),after=event.clientY>bounds.y+bounds.height/2;if(this.dropId!==id||this.dropAfter!==after){this.dropId=id;this.dropAfter=after;this.uaStateChanged();}}
  async drop(id:string,event:DragEvent):Promise<void> {event.preventDefault();const dragged=this.draggedId,after=this.dropAfter;this.dragEnd();if(dragged&&dragged!==id)await this.reorder(dragged,id,after);}
  dragEnd():void {this.draggedId='';this.dropId='';this.dropAfter=false;this.uaStateChanged();}
  disconnectedCallback():void {++this.operation;this.confirmResolve?.(false);this.confirmResolve=null;super.disconnectedCallback();}
}
UserAgentOverrides.define('user-agent-overrides');

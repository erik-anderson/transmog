import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import '../paused-exchange/paused-exchange.js';
import type { PausedExchangeElement } from '../paused-exchange/paused-exchange.js';
import type { BreakpointPhase, PausedExchange, BreakpointStatus } from '../models.js';
import { describeError, shortUrl } from '../utilities.js';
import { isTextEditing } from '../list-selection.js';

type PausedRow=PausedExchange & {name:string;phaseLabel:string;expirationText:string;selected:boolean;tabIndex:string};
const phaseLabels:Record<BreakpointPhase,string>={'request-head':'Request headers','request-body':'Request body','response-head':'Response headers','response-body':'Response body'};
export class BreakpointWorkspace extends WorkspaceElement {
  @attr view='traffic';
  @attr theme='system';
  @observable pausedExchanges:PausedRow[]=[];
  @observable breakpointText=initialState.breakpointText;
  @observable controllerEnabled=false;
  @observable selectedPaused:PausedExchange|null=null;
  @observable selectedExpired=false;
  @observable decisionBusy=false;
  @observable detailsOpen=false;
  @observable pausedSourceAvailable=false;
  @observable breakpointError='';
  breakpointForm!:HTMLFormElement;
  pausedQueue!:HTMLElement;
  pausedEditor!:PausedExchangeElement;
  private selectedId:number|null=null;
  private refreshPending=false;
  private timer:number|undefined;
  private expiryTimer:number|undefined;
  private generation=0;
  protected hydratedCallback():void {void this.refreshBreakpoints();}
  viewChanged():void {if(this.view==='breakpoints')void this.refreshBreakpoints();}
  private schedule():void {window.clearTimeout(this.timer);if(this.isConnected && (this.view==='breakpoints'||this.controllerEnabled))this.timer=window.setTimeout(()=>void this.refreshBreakpoints(),this.view==='breakpoints'?500:1000);}
  private renderBreakpoints(status:BreakpointStatus):void {
    this.controllerEnabled=status.enabled;
    const rows=status.paused.filter(item=>!item.expiresAtUnixMs || item.expiresAtUnixMs>Date.now());
    if(!rows.some(item=>item.decisionId===this.selectedId))this.selectedId=rows[0]?.decisionId??null;
    const previous=this.selectedPaused?.decisionId;
    this.selectedPaused=rows.find(item=>item.decisionId===this.selectedId)??null;
    this.selectedExpired=Boolean(this.selectedPaused?.expiresAtUnixMs && this.selectedPaused.expiresAtUnixMs<=Date.now());
    window.clearTimeout(this.expiryTimer);const expires=this.selectedPaused?.expiresAtUnixMs;if(expires)this.expiryTimer=window.setTimeout(()=>{this.selectedExpired=true;void this.refreshBreakpoints();},Math.max(0,expires-Date.now()));
    this.pausedExchanges=rows.map(item=>({ ...item,name:this.requestLabel(item),phaseLabel:phaseLabels[item.phase],expirationText:item.expiresAtUnixMs?'Expires in '+Math.max(0,Math.ceil((item.expiresAtUnixMs-Date.now())/1000))+'s':'Paused',selected:item.decisionId===this.selectedId,tabIndex:item.decisionId===this.selectedId?'0':'-1'}));
    this.breakpointText=rows.length?rows.length+' paused request'+(rows.length===1?'':'s'):status.enabled?'Waiting for matching traffic.':'Enable interception to pause matching requests.';
    this.$emit('breakpoint-state-change',{enabled:status.enabled,count:rows.length});
    if(previous!==this.selectedPaused?.decisionId){this.pausedSourceAvailable=false;if(this.selectedPaused)void this.checkSource(this.selectedPaused);}
    this.schedule();
  }
  private requestLabel(item:PausedExchange):string {const target=item.requestHead?.target;return typeof target==='string'?(String(item.requestHead?.method??'Request')+' '+shortUrl(target)):'Request '+(item.exchangeId.replace(/^0+/,'')||'0');}
  private async checkSource(item:PausedExchange):Promise<void> {try {await invoke('session_detail',{id:item.exchangeId});if(this.selectedPaused?.decisionId===item.decisionId)this.pausedSourceAvailable=true;}catch {if(this.selectedPaused?.decisionId===item.decisionId)this.pausedSourceAvailable=false;}}
  showSource():void {if(this.selectedPaused && this.pausedSourceAvailable)this.$emit('source-traffic-request',this.selectedPaused.exchangeId);}
  choosePaused(id:number):void {this.selectedId=id;this.detailsOpen=true;this.renderBreakpoints({enabled:this.controllerEnabled,paused:this.pausedExchanges});}
  backToQueue():void {this.detailsOpen=false;this.$flushUpdates();this.pausedQueue.querySelector<HTMLElement>('[data-decision-id="'+this.selectedId+'"]')?.focus();}
  queueKeyboard(id:number,event:KeyboardEvent):void {
    if(isTextEditing(event))return;
    if(event.key==='Enter'||event.key===' '){event.preventDefault();this.choosePaused(id);this.$flushUpdates();this.pausedEditor?.focusEditor();return;}
    const index=this.pausedExchanges.findIndex(row=>row.decisionId===id);let next=index;
    if(event.key==='ArrowDown')next=Math.min(index+1,this.pausedExchanges.length-1);else if(event.key==='ArrowUp')next=Math.max(0,index-1);else if(event.key==='Home')next=0;else if(event.key==='End')next=this.pausedExchanges.length-1;else return;
    event.preventDefault();this.selectedId=this.pausedExchanges[next]?.decisionId??null;this.renderBreakpoints({enabled:this.controllerEnabled,paused:this.pausedExchanges});this.$flushUpdates();this.pausedQueue.querySelector<HTMLElement>('[data-decision-id="'+this.selectedId+'"]')?.focus();
  }
  onDecision(event:CustomEvent<{paused:PausedExchange;action:object}>):void {void this.submitBreakpoint(event.detail.paused,event.detail.action);}
  async enableBreakpoints(event?:Event):Promise<void> {
    event?.preventDefault();if(this.decisionBusy)return;const data=new FormData(this.breakpointForm),phases:BreakpointPhase[]=[];
    if(data.get('requestHead')==='on')phases.push('request-head');if(data.get('requestBody')==='on')phases.push('request-body');if(data.get('responseHead')==='on')phases.push('response-head');if(data.get('responseBody')==='on')phases.push('response-body');
    this.decisionBusy=true;this.generation++;this.breakpointError='';
    try {this.renderBreakpoints(await invoke<BreakpointStatus>('enable_breakpoints',{settings:{phases,bodyLimit:4*1024*1024,maxPending:64,timeoutMs:30_000}}));}
    catch(error:unknown){this.breakpointError=describeError(error);}finally{this.decisionBusy=false;this.schedule();}
  }
  async disableBreakpoints():Promise<void> {if(this.decisionBusy)return;this.decisionBusy=true;this.generation++;try {this.renderBreakpoints(await invoke<BreakpointStatus>('disable_breakpoints'));this.breakpointError='';}catch(error:unknown){this.breakpointError=describeError(error);}finally{this.decisionBusy=false;this.schedule();}}
  async toggleBreakpoints():Promise<void> {if(this.controllerEnabled)await this.disableBreakpoints();else await this.enableBreakpoints();}
  async refreshBreakpoints():Promise<void> {if(this.refreshPending||this.decisionBusy){this.schedule();return;}this.refreshPending=true;const generation=this.generation;try {const status=await invoke<BreakpointStatus>('breakpoint_status');if(generation===this.generation)this.renderBreakpoints(status);}catch(error:unknown){this.breakpointError='Queue refresh failed: '+describeError(error);}finally{this.refreshPending=false;this.schedule();}}
  async submitBreakpoint(paused:PausedExchange,action:object):Promise<void> {if(this.decisionBusy||this.selectedExpired)return;this.decisionBusy=true;this.generation++;this.breakpointError='';try {this.renderBreakpoints(await invoke<BreakpointStatus>('decide_breakpoint',{decision:{decisionId:paused.decisionId,exchangeId:paused.exchangeId,action}}));}catch(error:unknown){this.breakpointError='Decision failed: '+describeError(error);}finally{this.decisionBusy=false;await this.refreshBreakpoints();}}
  disconnectedCallback():void {window.clearTimeout(this.timer);window.clearTimeout(this.expiryTimer);this.generation++;super.disconnectedCallback();}
}
BreakpointWorkspace.define('breakpoint-workspace');

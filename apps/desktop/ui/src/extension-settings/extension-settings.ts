import initialState from '../initial-state.json';
import {attr,observable} from '@microsoft/webui-framework';
import {invoke} from '@tauri-apps/api/core';
import {listen,type UnlistenFn} from '@tauri-apps/api/event';
import {WorkspaceElement} from '../workspace-element.js';
import {describeError} from '../utilities.js';
import type {ContributionRow,ExtensionContribution,ExtensionEntry,ExtensionRow,ExtensionStatus,ExtensionRule} from './models.js';

const bodyLabel={none:'No body access',optional:'Optional',required:'Required'};
const headersLabel=(headers:string[]):string=>headers.length?headers.join(', '):'No headers';
const bodyAccess=(access:ExtensionContribution['inputs']['request']):string=>bodyLabel[access.body]+(access.decodeHttpContent?' · HTTP decoding on demand':'');
const ruleLabel=(rule:ExtensionRule):string=>[rule.direction==='request'?'Request':'Response',...(rule.contentTypes??[]),...(rule.methods??[]),...(rule.pathPrefixes??[]),...(rule.statuses??[]).map(String),...(rule.headers??[]).map(header=>`${header.direction} ${header.name}: ${header.test.op}${header.test.value===undefined?'':` ${header.test.value}`}`)].join(' · ');
function contribution(value:ExtensionContribution):ContributionRow {
  return {id:value.id,name:value.name,surfaces:value.surfaces.map(side=>side==='headers'?'Headers':'Body').join(' and '),
    requestHeaders:headersLabel(value.inputs.request.headers),responseHeaders:headersLabel(value.inputs.response.headers),requestBody:bodyAccess(value.inputs.request),responseBody:bodyAccess(value.inputs.response),
    matches:value.matches.map(ruleLabel),
    live:value.liveUpdates?`Optional ${value.liveUpdates.events.join('/')} updates${value.liveUpdates.bodyProgress?', including body progress':''}`:'Not supported',
    context:value.context?`Up to ${value.context.maxExchanges} ${value.context.lookAhead?'earlier or later':'earlier'} exchanges from the same capture, origin, and conversation; ${value.context.headers.map(header=>`${header.direction} ${header.name}`).join(', ')}`:'Selected exchange only'};
}
function row(entry:ExtensionEntry):ExtensionRow {
  const {manifest,enabled,origin,digest}=entry.catalog;
  return {id:manifest.id,name:manifest.name,version:manifest.version,enabled,origin:origin==='bundled'?'Included with Transmog':origin==='development'?'Unpacked development folder':'Installed',bundled:origin==='bundled',development:origin==='development',directory:entry.directory??'',hotReload:entry.hotReload,runtime:manifest.runtime,revision:digest,visualizers:manifest.visualizers.map(contribution),columns:(manifest.trafficColumns??[]).map(column=>({id:column.id,name:column.name,requestHeaders:headersLabel(column.inputs.request.headers),responseHeaders:headersLabel(column.inputs.response.headers),values:column.values.map(value=>({value:value.value,matches:value.matches.map(ruleLabel)}))}))};
}
export class ExtensionSettings extends WorkspaceElement {
  @attr({mode:'boolean'}) active=false;
  @observable extensionsEntries:ExtensionRow[]=[];
  @observable extensionsBusy=initialState.extensionsBusy;
  @observable extensionsLoaded=initialState.extensionsLoaded;
  @observable extensionsText=initialState.extensionsText;
  @observable extensionsError=initialState.extensionsError;
  @observable extensionsWarning=initialState.extensionsWarning;
  private unlisten:UnlistenFn|undefined;
  private observing=false;
  private listenerGeneration=0;
  private refreshAgain=false;
  private generation=0;
  activeChanged():void {if(this.active&&this.isConnected){this.watch();if(this.extensionsBusy)this.refreshAgain=true;else void this.refresh();}else this.unwatch();}
  protected hydratedCallback():void {
    if(this.active){this.watch();void this.refresh();}
  }
  private watch():void {
    if(this.observing)return;this.observing=true;const generation=++this.listenerGeneration;
    void listen('extensions-changed',()=>{if(this.active&&!this.extensionsBusy)void this.refresh();}).then(unlisten=>{if(generation===this.listenerGeneration&&this.isConnected&&this.active)this.unlisten=unlisten;else void Promise.resolve(unlisten()).catch(()=>undefined);}).catch(()=>{if(generation===this.listenerGeneration){this.observing=false;this.extensionsError='Extension changes could not be observed. Use Refresh to check the current state.';}});
  }
  private unwatch():void {this.listenerGeneration++;this.observing=false;const unlisten=this.unlisten;this.unlisten=undefined;if(unlisten)void Promise.resolve(unlisten()).catch(()=>undefined);}
  disconnectedCallback():void {this.generation++;this.unwatch();super.disconnectedCallback();}
  private refreshQueued():void {if(this.refreshAgain&&this.active){this.refreshAgain=false;void this.refresh();}}
  private accept(status:ExtensionStatus):void {this.extensionsEntries=status.entries.map(row);this.extensionsWarning=status.warning??'';this.extensionsLoaded=true;}
  async refresh():Promise<void> {
    if(this.extensionsBusy)return;
    const generation=++this.generation;this.extensionsBusy=true;this.extensionsError='';this.extensionsText='';
    try {const status=await invoke<ExtensionStatus>('extension_status');if(generation===this.generation&&this.isConnected&&!this.refreshAgain)this.accept(status);}
    catch(error:unknown){if(generation===this.generation&&!this.refreshAgain)this.extensionsError=describeError(error);}
    finally {if(generation===this.generation){this.extensionsBusy=false;this.refreshQueued();}}
  }
  private async change(command:string,args:Record<string,unknown>,progress:string,complete:string,rollback?:ExtensionRow[]):Promise<void> {
    if(this.extensionsBusy)return;
    const generation=++this.generation;this.extensionsBusy=true;this.extensionsError='';this.extensionsText=progress;
    try {
      const status=await invoke<ExtensionStatus|null>(command,args);
      if(generation!==this.generation||!this.isConnected)return;
      if(status){this.accept(status);this.extensionsText=complete;}else this.extensionsText='';
    } catch(error:unknown) {
      if(generation!==this.generation||!this.isConnected)return;
      this.extensionsError=describeError(error);this.extensionsText='';
      if(rollback)this.extensionsEntries=rollback;
      // A file cleanup error can follow a successful catalog change. Read the
      // authoritative state instead of restoring an obsolete checkbox/list.
      try {const status=await invoke<ExtensionStatus>('extension_status');if(generation===this.generation&&this.isConnected)this.accept(status);}catch { /* Keep the last valid inventory and the original actionable error. */ }
      if(this.active)await this.showError('Extension change could not finish',this.extensionsError);
    } finally {if(generation===this.generation){this.extensionsBusy=false;this.refreshQueued();}}
  }
  install():void {void this.change('install_extension',{},'Installing extension…','Extension installed. Choose it from a matching message to visualize.');}
  unpacked():void {void this.change('load_unpacked_extension',{},'Choosing an unpacked extension…','Unpacked extension registered. Its source files stay in their folder.');}
  enabled(id:string,event:Event):void {if(this.extensionsBusy)return;const enabled=(event.target as HTMLInputElement).checked;const previous=this.extensionsEntries;this.extensionsEntries=previous.map(entry=>entry.id===id?{...entry,enabled}:entry);void this.change('enable_extension',{id,enabled},'Saving extension settings…',enabled?'Extension enabled.':'Extension disabled. Its active visualizers have stopped in every window.',previous);}
  hotReload(id:string,event:Event):void {if(this.extensionsBusy)return;const enabled=(event.target as HTMLInputElement).checked;const previous=this.extensionsEntries;this.extensionsEntries=previous.map(entry=>entry.id===id?{...entry,hotReload:enabled}:entry);void this.change('extension_hot_reload',{id,enabled},'Saving development settings…',enabled?'Hot reload enabled for active development visualizers.':'Hot reload disabled.',previous);}
  async reload(id:string):Promise<void> {
    if(this.extensionsBusy)return;
    const generation=++this.generation;this.extensionsBusy=true;this.extensionsError='';this.extensionsText='Requesting reload…';
    try {
      const result=await invoke<{activeInstances:number}>('reload_extension',{id});
      if(generation!==this.generation||!this.isConnected)return;
      this.extensionsText=result.activeInstances?'Reload requested for active visualizers in every window. Each keeps its last good result if replacement fails.':'No active visualizers to reload. Open one from a matching message to use the current source files.';
    }catch(error:unknown){if(generation===this.generation&&this.isConnected){this.extensionsError=describeError(error);this.extensionsText='';if(this.active)await this.showError('Extension reload could not be requested',this.extensionsError);}}
    finally {if(generation===this.generation){this.extensionsBusy=false;this.refreshQueued();}}
  }
  uninstall(id:string):void {void this.change('uninstall_extension',{id},'Removing extension…','Extension removed from Transmog. Unpacked source files are unchanged.');}
  dismiss():void {this.extensionsText='';this.extensionsError='';}
}
ExtensionSettings.define('extension-settings');
